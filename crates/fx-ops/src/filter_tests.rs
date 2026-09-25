//! Filter driver tests: tiled results against whole-image references, seams,
//! canvas edges, transparency and the coarse-level path for large radii.

use std::sync::Arc;

use fx_core::FilterParams;
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileError};

use crate::filter::{Geometry, extra_levels, filter_tile};
use crate::gaussian;
use crate::neighbourhood::{LevelSource, Rect, TileRef, gather, pixel, premul16, unpremul};

const T: i64 = TILE_SIZE as i64;

type PixelFn = Box<dyn Fn(i64, i64) -> [u16; 4] + Sync>;

/// A layer given by a pixel function over a `w × h` image at level 0; higher
/// levels are exact block averages (premultiplied), like the real mips.
struct FnSource {
	w: i64,
	h: i64,
	f: PixelFn,
}

impl FnSource {
	fn level_pixel(&self, level: usize, x: i64, y: i64) -> [u16; 4] {
		let s = 1i64 << level;
		let mut acc = [0.0f64; 4];
		for yy in y * s..(y + 1) * s {
			for xx in x * s..(x + 1) * s {
				let p = if xx < self.w && yy < self.h { (self.f)(xx, yy) } else { [0; 4] };
				let a = f64::from(p[3]) / 65535.0;
				for c in 0..3 {
					acc[c] += f64::from(p[c]) / 65535.0 * a;
				}
				acc[3] += a;
			}
		}
		let n = (s * s) as f64;
		let a = acc[3] / n;
		if a <= 0.0 {
			return [0; 4];
		}
		let q = |v: f64| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
		[q(acc[0] / n / a), q(acc[1] / n / a), q(acc[2] / n / a), q(a)]
	}
}

impl LevelSource for FnSource {
	fn format(&self) -> PixelFormat {
		PixelFormat::Rgba16
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		let s = 1i64 << level;
		let (lw, lh) = ((self.w + s - 1) / s, (self.h + s - 1) / s);
		if tx < 0 || ty < 0 || tx * T >= lw || ty * T >= lh {
			return Ok(None);
		}
		let mut tile = TileBuffer::zeroed(PixelFormat::Rgba16);
		let out = tile.as_u16_mut();
		for y in 0..T {
			for x in 0..T {
				let (gx, gy) = (tx * T + x, ty * T + y);
				if gx < lw && gy < lh {
					let p = self.level_pixel(level, gx, gy);
					out[((y * T + x) * 4) as usize..][..4].copy_from_slice(&p);
				}
			}
		}
		Ok(Some(TileRef::Data(Arc::new(tile))))
	}
}

fn geometry(w: i64, h: i64) -> Geometry {
	Geometry {
		offset: (0, 0),
		canvas: (w as u32, h as u32),
		image: (w as u32, h as u32),
	}
}

/// Pixel (x, y) of the level-0 output, from the tile the driver produced.
fn output_pixel(src: &FnSource, geo: &Geometry, params: &FilterParams, x: i64, y: i64) -> [u16; 4] {
	let tile = filter_tile(src, geo, params, 0, (x / T) as u32, (y / T) as u32).unwrap();
	pixel(&tile, PixelFormat::Rgba16, (x % T) as usize, (y % T) as usize)
}

fn close(a: [u16; 4], b: [u16; 4], tolerance: i32) -> bool {
	(0..4).all(|c| (i32::from(a[c]) - i32::from(b[c])).abs() <= tolerance)
}

#[test]
fn tiled_blur_equals_a_whole_image_reference_across_seams() {
	// 3 × 3 tiles of content crossing every seam.
	let (w, h) = (3 * T, 3 * T);
	let f = |x: i64, y: i64| [((x * 97 + y * 31) % 65536) as u16, ((x * y) % 65536) as u16, 20_000, 65535];
	let src = FnSource { w, h, f: Box::new(f) };
	let geo = geometry(w, h);
	let sigma = 3.5;
	let params = FilterParams::GaussianBlur { radius: sigma };
	// Reference: one blur over the whole canvas (edge replicate at the canvas).
	let r = gaussian::radius(sigma) as i64;
	let area = Rect {
		x0: -r,
		y0: -r,
		x1: w + r,
		y1: h + r,
	};
	let canvas = Rect { x0: 0, y0: 0, x1: w, y1: h };
	let whole = gaussian::blur(&gather(&src, 0, area, canvas).unwrap(), area.width(), area.height(), sigma);
	for (x, y) in [(255, 255), (256, 256), (255, 256), (0, 0), (511, 300), (767, 767), (400, 10)] {
		let expected = unpremul(whole[(y * w + x) as usize]).map(|v| (v * 65535.0 + 0.5) as u16);
		let got = output_pixel(&src, &geo, &params, x, y);
		assert!(close(got, expected, 1), "({x},{y}): {got:?} vs {expected:?}");
	}
}

#[test]
fn a_constant_opaque_image_stays_exact_at_seams_and_canvas_edges() {
	let (w, h) = (2 * T + 40, T + 7);
	let colour = [12_345, 40_000, 65_535, 65_535];
	let src = FnSource {
		w,
		h,
		f: Box::new(move |_, _| colour),
	};
	let geo = geometry(w, h);
	for params in [FilterParams::GaussianBlur { radius: 5.0 }, FilterParams::GaussianBlur { radius: 90.0 }] {
		for (x, y) in [(0, 0), (w - 1, h - 1), (T, 3), (T - 1, h - 1), (2 * T + 39, 0)] {
			let got = output_pixel(&src, &geo, &params, x, y);
			assert!(close(got, colour, 1), "{params:?} at ({x},{y}): {got:?}");
		}
	}
}

#[test]
fn no_dark_fringe_around_transparent_pixels() {
	// Opaque red square in the middle of a transparent layer.
	let src = FnSource {
		w: T,
		h: T,
		f: Box::new(|x, y| {
			if (100..156).contains(&x) && (100..156).contains(&y) {
				[65535, 0, 0, 65535]
			} else {
				[0; 4]
			}
		}),
	};
	let geo = geometry(T, T);
	let tile = filter_tile(&src, &geo, &FilterParams::GaussianBlur { radius: 6.0 }, 0, 0, 0).unwrap();
	for (x, y) in [(95, 128), (90, 128), (128, 160), (99, 99)] {
		let p = pixel(&tile, PixelFormat::Rgba16, x, y);
		assert!(p[3] > 0, "the blur spreads alpha to ({x},{y})");
		assert!(p[0] >= 65534 && p[1] == 0 && p[2] == 0, "still pure red at ({x},{y}): {p:?}");
	}
}

#[test]
fn outside_the_layer_image_the_tile_stays_transparent() {
	let (w, h) = (T + 10, 20);
	let src = FnSource {
		w,
		h,
		f: Box::new(|_, _| [30_000, 30_000, 30_000, 65535]),
	};
	let geo = geometry(w, h);
	let tile = filter_tile(&src, &geo, &FilterParams::GaussianBlur { radius: 4.0 }, 0, 1, 0).unwrap();
	assert!(pixel(&tile, PixelFormat::Rgba16, 5, 5)[3] > 0);
	assert_eq!(pixel(&tile, PixelFormat::Rgba16, 20, 5), [0; 4], "x beyond the image");
	assert_eq!(pixel(&tile, PixelFormat::Rgba16, 5, 30), [0; 4], "y beyond the image");
}

#[test]
fn a_large_radius_uses_a_coarse_level_and_stays_close_to_the_exact_blur() {
	let params = FilterParams::GaussianBlur { radius: 40.0 };
	assert_eq!(extra_levels(&params, 0), 1);
	assert_eq!(extra_levels(&FilterParams::GaussianBlur { radius: 1000.0 }, 0), 5);
	assert_eq!(extra_levels(&FilterParams::GaussianBlur { radius: 1000.0 }, 5), 0);
	// Gradients and a hard edge.
	let (w, h) = (2 * T, 2 * T);
	let f = |x: i64, y: i64| {
		let v = if x < 200 { 10_000 } else { 50_000 };
		[v, ((y * 200) % 65536) as u16, 30_000, 65535]
	};
	let src = FnSource { w, h, f: Box::new(f) };
	let geo = geometry(w, h);
	let sigma = 40.0;
	let r = gaussian::radius(sigma) as i64;
	let area = Rect {
		x0: -r,
		y0: -r,
		x1: w + r,
		y1: h + r,
	};
	let canvas = Rect { x0: 0, y0: 0, x1: w, y1: h };
	let exact = gaussian::blur(&gather(&src, 0, area, canvas).unwrap(), area.width(), area.height(), sigma);
	for (x, y) in [(200, 100), (180, 300), (256, 256), (10, 500), (400, 40)] {
		let expected = unpremul(exact[(y * w + x) as usize]);
		let got = output_pixel(&src, &geo, &params, x, y);
		for c in 0..3 {
			let got = f32::from(got[c]) / 65535.0;
			assert!((got - expected[c]).abs() < 0.01, "({x},{y}) channel {c}: {got} vs {}", expected[c]);
		}
	}
}

#[test]
fn unsharp_mask_with_threshold_255_is_the_identity() {
	let f = |x: i64, y: i64| [((x * 311) % 65536) as u16, ((y * 173) % 65536) as u16, 5000, 65535];
	let src = FnSource { w: T, h: T, f: Box::new(f) };
	let geo = geometry(T, T);
	let params = FilterParams::UnsharpMask {
		amount: 200.0,
		radius: 2.0,
		threshold: 255,
	};
	let tile = filter_tile(&src, &geo, &params, 0, 0, 0).unwrap();
	for (x, y) in [(0, 0), (17, 200), (255, 255)] {
		assert_eq!(pixel(&tile, PixelFormat::Rgba16, x as usize, y as usize), f(x, y));
	}
}

#[test]
fn unsharp_mask_increases_contrast_at_an_edge() {
	let f = |x: i64, _| {
		if x < 128 {
			[20_000, 20_000, 20_000, 65535]
		} else {
			[40_000, 40_000, 40_000, 65535]
		}
	};
	let src = FnSource { w: T, h: T, f: Box::new(f) };
	let geo = geometry(T, T);
	let params = FilterParams::UnsharpMask {
		amount: 100.0,
		radius: 2.0,
		threshold: 0,
	};
	let tile = filter_tile(&src, &geo, &params, 0, 0, 0).unwrap();
	assert!(pixel(&tile, PixelFormat::Rgba16, 126, 50)[0] < 20_000, "the dark side gets darker");
	assert!(pixel(&tile, PixelFormat::Rgba16, 129, 50)[0] > 40_000, "the bright side gets brighter");
	assert_eq!(pixel(&tile, PixelFormat::Rgba16, 20, 50)[0], 20_000, "flat areas do not change");
}

#[test]
fn premul_and_unpremul_round_trip() {
	let p = [30_000u16, 60_000, 1_000, 32_768];
	let back = unpremul(premul16(p)).map(|v| (v * 65535.0 + 0.5) as u16);
	assert!(close(back, p, 1), "{back:?}");
}
