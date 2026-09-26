//! M6-T01 tests: the resampling core against whole-image references.

use std::sync::Arc;

use fx_core::{BezierPatch, Filter, Mapping};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileError};

use super::{SourceInfo, resample};
use crate::neighbourhood::{LevelSource, TileRef};

/// A source image given by a pixel function at level 0; coarser levels are
/// exact 2 × 2 block averages (premultiplied), like the real mip pyramid.
struct TestImage {
	w: i64,
	h: i64,
	format: PixelFormat,
	px: Vec<[u16; 4]>,
}

impl TestImage {
	fn new(w: i64, h: i64, f: impl Fn(i64, i64) -> [u16; 4]) -> Self {
		Self::with_format(w, h, PixelFormat::Rgba16, f)
	}

	fn with_format(w: i64, h: i64, format: PixelFormat, f: impl Fn(i64, i64) -> [u16; 4]) -> Self {
		let mut px = Vec::with_capacity((w * h) as usize);
		for y in 0..h {
			for x in 0..w {
				px.push(f(x, y));
			}
		}
		Self { w, h, format, px }
	}

	fn from_pixels(w: i64, h: i64, px: Vec<[u16; 4]>) -> Self {
		assert_eq!(px.len(), (w * h) as usize);
		Self {
			w,
			h,
			format: PixelFormat::Rgba16,
			px,
		}
	}

	fn at(&self, x: i64, y: i64) -> [u16; 4] {
		self.px[(y * self.w + x) as usize]
	}

	fn source_info(&self) -> SourceInfo {
		let (mut w, mut h) = (self.w as u32, self.h as u32);
		let mut levels = 1;
		while w > TILE_SIZE || h > TILE_SIZE {
			w = w.div_ceil(2);
			h = h.div_ceil(2);
			levels += 1;
		}
		SourceInfo {
			size: (self.w as u32, self.h as u32),
			levels,
		}
	}

	fn level_pixel(&self, level: usize, x: i64, y: i64) -> [u16; 4] {
		let s = 1i64 << level;
		let mut acc = [0.0f64; 4];
		let mut n = 0.0;
		for yy in y * s..(y + 1) * s {
			for xx in x * s..(x + 1) * s {
				let p = if xx < self.w && yy < self.h { self.at(xx, yy) } else { [0; 4] };
				let a = f64::from(p[3]) / 65535.0;
				for c in 0..3 {
					acc[c] += f64::from(p[c]) / 65535.0 * a;
				}
				acc[3] += a;
				n += 1.0;
			}
		}
		let a = acc[3] / n;
		if a <= 0.0 {
			return [0; 4];
		}
		let q = |v: f64| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
		[q(acc[0] / n / a), q(acc[1] / n / a), q(acc[2] / n / a), q(a)]
	}
}

impl LevelSource for TestImage {
	fn format(&self) -> PixelFormat {
		self.format
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		let tile = TILE_SIZE as i64;
		let s = 1i64 << level;
		let (lw, lh) = ((self.w + s - 1) / s, (self.h + s - 1) / s);
		if tx < 0 || ty < 0 || tx * tile >= lw || ty * tile >= lh {
			return Ok(None);
		}
		let mut buffer = TileBuffer::zeroed(self.format);
		for y in 0..tile {
			for x in 0..tile {
				let (gx, gy) = (tx * tile + x, ty * tile + y);
				if gx >= lw || gy >= lh {
					continue;
				}
				let p = self.level_pixel(level, gx, gy);
				let i = ((y * tile + x) * 4) as usize;
				match self.format {
					PixelFormat::Rgba16 => buffer.as_u16_mut()[i..i + 4].copy_from_slice(&p),
					PixelFormat::Rgba8 => {
						for (c, value) in p.iter().enumerate() {
							buffer.bytes_mut()[i + c] = (*value / 257) as u8;
						}
					}
					PixelFormat::Gray16 => buffer.as_u16_mut()[i / 4] = p[0],
					PixelFormat::Gray8 => buffer.bytes_mut()[i / 4] = (p[0] / 257) as u8,
				}
			}
		}
		Ok(Some(TileRef::Data(Arc::new(buffer))))
	}
}

/// The destination tiles of a `dw × dh` image at `level`.
fn dst_tiles(dw: u32, dh: u32, level: usize) -> Vec<(u32, u32)> {
	let d = 1u32 << level;
	let cols = (dw.div_ceil(d)).div_ceil(TILE_SIZE).max(1);
	let rows = (dh.div_ceil(d)).div_ceil(TILE_SIZE).max(1);
	(0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect()
}

/// Assemble the level-`level` result as straight RGBA16 (8-bit scaled up).
fn assemble(dw: u32, dh: u32, level: usize, tiles: &[((u32, u32), TileBuffer)]) -> Vec<[u16; 4]> {
	let d = 1u32 << level;
	let (w, h) = ((dw.div_ceil(d)) as i64, (dh.div_ceil(d)) as i64);
	let mut out = vec![[0u16; 4]; (w * h) as usize];
	let tile = TILE_SIZE as i64;
	for ((tx, ty), buffer) in tiles {
		for y in 0..tile {
			for x in 0..tile {
				let (gx, gy) = (i64::from(*tx) * tile + x, i64::from(*ty) * tile + y);
				if gx >= w || gy >= h {
					continue;
				}
				let i = ((y * tile + x) * 4) as usize;
				let p = match buffer.format() {
					PixelFormat::Rgba16 => [buffer.as_u16()[i], buffer.as_u16()[i + 1], buffer.as_u16()[i + 2], buffer.as_u16()[i + 3]],
					PixelFormat::Rgba8 => [
						u16::from(buffer.bytes()[i]) * 257,
						u16::from(buffer.bytes()[i + 1]) * 257,
						u16::from(buffer.bytes()[i + 2]) * 257,
						u16::from(buffer.bytes()[i + 3]) * 257,
					],
					_ => unreachable!(),
				};
				out[(gy * w + gx) as usize] = p;
			}
		}
	}
	out
}

/// Resample a whole destination image at level 0 and assemble it.
fn resample_image(src: &TestImage, mapping: Mapping, filter: Filter, dw: u32, dh: u32) -> Vec<[u16; 4]> {
	let tiles = dst_tiles(dw, dh, 0);
	let out = resample(src, src.source_info(), mapping, filter, 0, &tiles).expect("resample");
	assemble(dw, dh, 0, &out)
}

#[test]
fn a_gray_mask_keeps_its_value_at_the_border() {
	// A mask of one value everywhere: any fade at the image border would show.
	// Gray images have no transparency, so the taps outside the image must see
	// the edge pixel (D-036), not transparent black.
	let src = TestImage::with_format(600, 400, PixelFormat::Gray16, |_, _| [40_000, 40_000, 40_000, 65_535]);
	let tiles = dst_tiles(300, 200, 0);
	let out = resample(&src, src.source_info(), Mapping::scale(0.5, 0.5), Filter::Bicubic, 0, &tiles).expect("resample");
	let mut checked = 0;
	for ((tx, ty), buffer) in &out {
		assert_eq!(buffer.format(), PixelFormat::Gray16);
		let gray = buffer.as_u16();
		for y in 0..TILE_SIZE as i64 {
			for x in 0..TILE_SIZE as i64 {
				let (gx, gy) = (i64::from(*tx) * 256 + x, i64::from(*ty) * 256 + y);
				if gx >= 300 || gy >= 200 {
					continue;
				}
				let value = i32::from(gray[(y * 256 + x) as usize]);
				assert!((value - 40_000).abs() <= 1, "({gx},{gy}) = {value}");
				checked += 1;
			}
		}
	}
	assert_eq!(checked, 300 * 200);
}

fn close(a: [u16; 4], b: [u16; 4], tolerance: i32) -> bool {
	(0..4).all(|c| (i32::from(a[c]) - i32::from(b[c])).abs() <= tolerance)
}

#[test]
fn identity_is_an_exact_copy() {
	// 600 × 400 spans 3 × 2 tiles: the seams are part of the test.
	let src = TestImage::new(600, 400, |x, y| {
		[((x * 977 + y * 13) % 65536) as u16, ((y * 613 + x) % 65536) as u16, 12_345, 65_535]
	});
	for filter in [Filter::Nearest, Filter::Bilinear, Filter::Bicubic, Filter::BicubicSmoother, Filter::Lanczos3] {
		let out = resample_image(&src, Mapping::identity(), filter, 600, 400);
		for y in 0..400 {
			for x in 0..600 {
				assert!(close(out[(y * 600 + x) as usize], src.at(x, y), 1), "{filter:?} at ({x},{y})");
			}
		}
	}
}

#[test]
fn whole_pixel_translation_is_exact() {
	let src = TestImage::new(300, 200, |x, y| [((x * 331) % 65536) as u16, ((y * 173) % 65536) as u16, 9000, 65_535]);
	// `dst = src + (20, -10)`, so source `(x - 20, y + 10)` lands at destination `(x, y)`.
	let out = resample_image(&src, Mapping::translation(20.0, -10.0), Filter::Bicubic, 300, 200);
	for y in 0..190 {
		for x in 20..300 {
			assert!(close(out[(y * 300 + x) as usize], src.at(x - 20, y + 10), 1), "({x},{y})");
		}
	}
}

#[test]
fn a_half_pixel_translation_averages_neighbours() {
	let src = TestImage::new(300, 16, |x, y| [(x * 1000) as u16, (y * 1000) as u16, 0, 65_535]);
	let out = resample_image(&src, Mapping::translation(0.5, 0.0), Filter::Bilinear, 300, 16);
	for y in 0..16 {
		for x in 1..300 {
			let (a, b) = (src.at(x - 1, y), src.at(x, y));
			let expected = std::array::from_fn(|c| ((u32::from(a[c]) + u32::from(b[c])) / 2) as u16);
			assert!(
				close(out[(y * 300 + x) as usize], expected, 1),
				"({x},{y}): {:?} vs {expected:?}",
				out[(y * 300 + x) as usize]
			);
		}
	}
}

#[test]
fn a_quarter_reduction_of_a_checkerboard_is_flat_grey() {
	// 1-px checkerboard, reduced 4×: reading the level-2 mip (exact 4 × 4 box
	// averages) must give a flat 50 % grey with no moiré.
	let src = TestImage::new(1024, 1024, |x, y| if (x + y) % 2 == 0 { [65_535; 4] } else { [0, 0, 0, 65_535] });
	for filter in [Filter::Bicubic, Filter::BicubicAutomatic, Filter::Lanczos3] {
		let out = resample_image(&src, Mapping::scale(0.25, 0.25), filter, 256, 256);
		for (i, p) in out.iter().enumerate() {
			assert!(p[3] > 65_000, "{filter:?} alpha at {i}: {p:?}");
			for c in 0..3 {
				assert!((i32::from(p[c]) - 32_768).abs() <= 300, "{filter:?} channel {c} at {i}: {p:?}");
			}
		}
	}
}

#[test]
fn no_dark_fringe_around_half_transparent_red() {
	let src = TestImage::new(300, 64, |x, _| if (100..200).contains(&x) { [65_535, 0, 0, 32_768] } else { [0; 4] });
	// Bicubic has negative lobes: without the premultiplied clamp this is where
	// a dark grey fringe appears.
	let out = resample_image(&src, Mapping::translation(0.5, 0.0), Filter::Bicubic, 300, 64);
	let mut seen = 0;
	for (i, p) in out.iter().enumerate() {
		if p[3] == 0 {
			continue;
		}
		seen += 1;
		assert!(p[0] >= 65_534, "not pure red at {i}: {p:?}");
		assert!(p[1] == 0 && p[2] == 0, "grey fringe at {i}: {p:?}");
	}
	// Bicubic rings at the edge (alpha may overshoot), which Photoshop shows
	// too; the point of the test is the absence of a *dark fringe*.
	assert!(seen > 100, "the red band survived the resample");
}

#[test]
fn identity_warp_is_an_exact_copy() {
	let src = TestImage::new(300, 200, |x, y| [((x * 7 + y * 3) % 65536) as u16, ((x * y) % 65536) as u16, 40_000, 65_535]);
	let out = resample_image(&src, Mapping::Warp(BezierPatch::identity(300, 200)), Filter::Bicubic, 300, 200);
	for y in 0..200 {
		for x in 0..300 {
			assert!(close(out[(y * 300 + x) as usize], src.at(x, y), 1), "({x},{y})");
		}
	}
}

#[test]
fn a_magnifying_warp_covers_every_destination_pixel() {
	// Source [0,100]² mapped onto a 200 × 200 destination rectangle: 2× up.
	let src = TestImage::new(100, 100, |x, y| [(x * 600) as u16, (y * 600) as u16, 1000, 65_535]);
	let patch = BezierPatch::rect([0.0, 0.0, 200.0, 200.0], [0.0, 0.0, 100.0, 100.0]);
	let out = resample_image(&src, Mapping::Warp(patch), Filter::Bilinear, 200, 200);
	// Interior: fully covered, no holes. The border pixels blend with the
	// transparent area outside the layer, which is correct.
	for y in 8..192 {
		for x in 8..192 {
			let p = out[(y * 200 + x) as usize];
			assert!(p[3] > 65_000, "hole at ({x},{y}): {p:?}");
			// Destination (x, y) samples source `(x + 0.5)/2`; source pixel `i`
			// (value 600 · i) has its centre at `i + 0.5`, so the ramp value there
			// is 600 · (source − 0.5) = 300 · x − 150. Bilinear is exact on a
			// linear ramp.
			let expected = (x * 300 - 150) as u16;
			assert!((i32::from(p[0]) - i32::from(expected)).abs() <= 2, "x={x}: {p:?} vs {expected}");
		}
	}
}

#[test]
fn rotate_thirty_then_minus_thirty_keeps_a_smooth_image() {
	// A smooth ramp, rotated by +30° and back by −30°: away from the (transparent)
	// corners the two bicubic passes must land within 2/255 of the original.
	let src = TestImage::new(400, 400, |x, y| [(x * 160) as u16, (y * 160) as u16, ((x + y) * 80) as u16, 65_535]);
	let centre = (200.0, 200.0);
	let first = resample_image(&src, Mapping::rotation_about(30f64.to_radians(), centre.0, centre.1), Filter::Bicubic, 400, 400);
	let middle = TestImage::from_pixels(400, 400, first);
	let back = resample_image(
		&middle,
		Mapping::rotation_about((-30f64).to_radians(), centre.0, centre.1),
		Filter::Bicubic,
		400,
		400,
	);
	for y in 100..300 {
		for x in 100..300 {
			let got = back[(y * 400 + x) as usize];
			let want = src.at(x, y);
			assert!(close(got, want, 2 * 257), "({x},{y}): {got:?} vs {want:?}");
		}
	}
}

#[test]
fn eight_bit_identity_is_exact() {
	let src = TestImage::with_format(300, 200, PixelFormat::Rgba8, |x, y| {
		[u16::from((x % 256) as u8) * 257, u16::from((y % 256) as u8) * 257, u16::from(7u8) * 257, 65_535]
	});
	let out = resample_image(&src, Mapping::identity(), Filter::Bicubic, 300, 200);
	for y in 0..200 {
		for x in 0..300 {
			assert_eq!(out[(y * 300 + x) as usize], src.at(x, y), "({x},{y})");
		}
	}
}

#[test]
fn nearest_uses_one_source_pixel() {
	let src = TestImage::new(64, 8, |x, _| [(x * 1000) as u16, 0, 0, 65_535]);
	// 2× up: each source pixel becomes a 2 × 2 block of itself under nearest.
	let out = resample_image(&src, Mapping::scale(2.0, 1.0), Filter::Nearest, 128, 8);
	for x in 0..128 {
		assert_eq!(out[x as usize], src.at(x / 2, 0), "x={x}");
	}
}
