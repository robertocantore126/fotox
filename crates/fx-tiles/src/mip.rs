//! CPU mip generation. (A GPU version may come later; this one is the
//! reference and must stay bit-exact with its tests.)

use crate::PixelFormat;
use crate::format::{PixelValue, TILE_SIZE};
use crate::store::TileBuffer;

/// Pixels of one child tile, already resolved from its `TileSlot`.
#[derive(Clone, Copy, Debug)]
pub enum ChildPixels<'a> {
	/// Transparent / zero, also used for children outside the grid.
	Empty,
	Solid(PixelValue),
	Data(&'a TileBuffer),
}

/// Build one tile of level `n + 1` from its four children at level `n`.
///
/// `children` order: `[top_left, top_right, bottom_left, bottom_right]`.
/// Output pixel `(x, y)` is the 2×2 box average of child
/// `(x / 128, y / 128)` at pixels `(2·(x % 128) + {0,1}, 2·(y % 128) + {0,1})`.
///
/// RGBA formats average **premultiplied** (straight alpha would create dark
/// fringes around transparent areas):
/// ```text
/// A   = Σa
/// out_a = round(A / 4)
/// out_c = if A == 0 { 0 } else { round(Σ(c·a) / A) }     for c in r, g, b
/// ```
/// Gray formats: `out = round(Σv / 4)`. `round` is round-half-up in integer
/// arithmetic (`(2·num + den) / (2·den)`), computed in `u64` for 16-bit.
///
/// Performance target (M1-T05): ≥ 1.5 GB/s of *input* per core for Rgba16 in
/// a release build. Process rows, avoid per-pixel branches on format.
pub fn downsample_2x2(format: PixelFormat, children: [ChildPixels<'_>; 4]) -> TileBuffer {
	// Four identical uniform children: the parent is that value, no pixel work.
	if let Some(value) = uniform_parent(format, &children) {
		return TileBuffer::filled(format, value);
	}
	let mut out = TileBuffer::zeroed(format);
	for (quadrant, child) in children.iter().enumerate() {
		let (qx, qy) = ((quadrant % 2) * HALF, (quadrant / 2) * HALF);
		match child {
			// `zeroed` already holds the result of an empty child.
			ChildPixels::Empty => {}
			ChildPixels::Solid(value) => fill_quadrant(&mut out, qx, qy, solid_average(format, *value)),
			ChildPixels::Data(buffer) => {
				assert_eq!(buffer.format(), format, "child tile format does not match");
				match format {
					PixelFormat::Rgba16 => rgba16_quadrant(buffer.as_u16(), out.as_u16_mut(), qx, qy),
					PixelFormat::Rgba8 => rgba8_quadrant(buffer.bytes(), out.bytes_mut(), qx, qy),
					PixelFormat::Gray16 => gray16_quadrant(buffer.as_u16(), out.as_u16_mut(), qx, qy),
					PixelFormat::Gray8 => gray8_quadrant(buffer.bytes(), out.bytes_mut(), qx, qy),
				}
			}
		}
	}
	out
}

const SIZE: usize = TILE_SIZE as usize;
/// Output pixels per child along one axis.
const HALF: usize = SIZE / 2;

/// The value four identical 2×2 pixels of `value` average to: itself, except
/// that premultiplied RGBA drops the colour of a fully transparent pixel.
fn solid_average(format: PixelFormat, value: PixelValue) -> PixelValue {
	if format.has_alpha() && value.0[3] == 0 {
		PixelValue::TRANSPARENT
	} else {
		value
	}
}

/// `Some(value)` when all four children are the same `Empty`/`Solid` value.
fn uniform_parent(format: PixelFormat, children: &[ChildPixels<'_>; 4]) -> Option<PixelValue> {
	let value_of = |child: &ChildPixels<'_>| match child {
		ChildPixels::Empty => Some(PixelValue::TRANSPARENT),
		ChildPixels::Solid(value) => Some(solid_average(format, *value)),
		ChildPixels::Data(_) => None,
	};
	let first = value_of(&children[0])?;
	children[1..].iter().all(|c| value_of(c) == Some(first)).then_some(first)
}

fn fill_quadrant(out: &mut TileBuffer, qx: usize, qy: usize, value: PixelValue) {
	let format = out.format();
	let bpp = format.bytes_per_pixel();
	let pixel = TileBuffer::filled(format, value);
	let px = &pixel.bytes()[..bpp];
	let bytes = out.bytes_mut();
	for y in qy..qy + HALF {
		let row = &mut bytes[(y * SIZE + qx) * bpp..(y * SIZE + qx + HALF) * bpp];
		for chunk in row.chunks_exact_mut(bpp) {
			chunk.copy_from_slice(px);
		}
	}
}

/// Round-half-up integer division.
#[inline(always)]
fn div_round(num: u64, den: u64) -> u64 {
	(2 * num + den) / (2 * den)
}

fn rgba16_quadrant(src: &[u16], out: &mut [u16], qx: usize, qy: usize) {
	for y in 0..HALF {
		let r0 = &src[(2 * y) * SIZE * 4..(2 * y + 1) * SIZE * 4];
		let r1 = &src[(2 * y + 1) * SIZE * 4..(2 * y + 2) * SIZE * 4];
		let dst = &mut out[((qy + y) * SIZE + qx) * 4..((qy + y) * SIZE + qx + HALF) * 4];
		for (x, o) in dst.chunks_exact_mut(4).enumerate() {
			let i = x * 8;
			let p = [&r0[i..i + 4], &r0[i + 4..i + 8], &r1[i..i + 4], &r1[i + 4..i + 8]];
			let a: u64 = p.iter().map(|q| u64::from(q[3])).sum();
			o[3] = div_round(a, 4) as u16;
			if a == 0 {
				o[..3].fill(0);
				continue;
			}
			for c in 0..3 {
				let sum: u64 = p.iter().map(|q| u64::from(q[c]) * u64::from(q[3])).sum();
				o[c] = div_round(sum, a) as u16;
			}
		}
	}
}

fn rgba8_quadrant(src: &[u8], out: &mut [u8], qx: usize, qy: usize) {
	for y in 0..HALF {
		let r0 = &src[(2 * y) * SIZE * 4..(2 * y + 1) * SIZE * 4];
		let r1 = &src[(2 * y + 1) * SIZE * 4..(2 * y + 2) * SIZE * 4];
		let dst = &mut out[((qy + y) * SIZE + qx) * 4..((qy + y) * SIZE + qx + HALF) * 4];
		for (x, o) in dst.chunks_exact_mut(4).enumerate() {
			let i = x * 8;
			let p = [&r0[i..i + 4], &r0[i + 4..i + 8], &r1[i..i + 4], &r1[i + 4..i + 8]];
			let a: u64 = p.iter().map(|q| u64::from(q[3])).sum();
			o[3] = div_round(a, 4) as u8;
			if a == 0 {
				o[..3].fill(0);
				continue;
			}
			for c in 0..3 {
				let sum: u64 = p.iter().map(|q| u64::from(q[c]) * u64::from(q[3])).sum();
				o[c] = div_round(sum, a) as u8;
			}
		}
	}
}

fn gray16_quadrant(src: &[u16], out: &mut [u16], qx: usize, qy: usize) {
	for y in 0..HALF {
		let r0 = &src[(2 * y) * SIZE..(2 * y + 1) * SIZE];
		let r1 = &src[(2 * y + 1) * SIZE..(2 * y + 2) * SIZE];
		let dst = &mut out[(qy + y) * SIZE + qx..(qy + y) * SIZE + qx + HALF];
		for (x, o) in dst.iter_mut().enumerate() {
			let sum = u64::from(r0[2 * x]) + u64::from(r0[2 * x + 1]) + u64::from(r1[2 * x]) + u64::from(r1[2 * x + 1]);
			*o = div_round(sum, 4) as u16;
		}
	}
}

fn gray8_quadrant(src: &[u8], out: &mut [u8], qx: usize, qy: usize) {
	for y in 0..HALF {
		let r0 = &src[(2 * y) * SIZE..(2 * y + 1) * SIZE];
		let r1 = &src[(2 * y + 1) * SIZE..(2 * y + 2) * SIZE];
		let dst = &mut out[(qy + y) * SIZE + qx..(qy + y) * SIZE + qx + HALF];
		for (x, o) in dst.iter_mut().enumerate() {
			let sum = u64::from(r0[2 * x]) + u64::from(r0[2 * x + 1]) + u64::from(r1[2 * x]) + u64::from(r1[2 * x + 1]);
			*o = div_round(sum, 4) as u8;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::format::TILE_SIZE;

	fn set_rgba16(buf: &mut TileBuffer, x: u32, y: u32, v: [u16; 4]) {
		let i = ((y * TILE_SIZE + x) * 8) as usize;
		for (c, value) in v.iter().enumerate() {
			buf.bytes_mut()[i + c * 2..i + c * 2 + 2].copy_from_slice(&value.to_ne_bytes());
		}
	}

	fn get_rgba16(buf: &TileBuffer, x: u32, y: u32) -> [u16; 4] {
		let i = ((y * TILE_SIZE + x) * 4) as usize;
		let s = buf.as_u16();
		[s[i], s[i + 1], s[i + 2], s[i + 3]]
	}

	#[test]
	fn solid_children_stay_solid() {
		let v = PixelValue::rgba16(1000, 2000, 3000, 65535);
		let out = downsample_2x2(PixelFormat::Rgba16, [ChildPixels::Solid(v); 4]);
		assert_eq!(out.uniform_value(), Some(v));
	}

	#[test]
	fn empty_children_are_transparent() {
		let out = downsample_2x2(PixelFormat::Rgba8, [ChildPixels::Empty; 4]);
		assert_eq!(out.uniform_value(), Some(PixelValue::TRANSPARENT));
	}

	#[test]
	fn premultiplied_average_has_no_dark_fringe() {
		// One opaque red pixel next to three transparent black ones.
		let mut child = TileBuffer::zeroed(PixelFormat::Rgba16);
		set_rgba16(&mut child, 0, 0, [65535, 0, 0, 65535]);
		let out = downsample_2x2(
			PixelFormat::Rgba16,
			[ChildPixels::Data(&child), ChildPixels::Empty, ChildPixels::Empty, ChildPixels::Empty],
		);
		let px = get_rgba16(&out, 0, 0);
		assert_eq!(px, [65535, 0, 0, 16384], "colour stays pure red, alpha = 1/4 (rounded half up)");
	}

	#[test]
	fn gray_averages_round_half_up() {
		let mut child = TileBuffer::zeroed(PixelFormat::Gray8);
		// 1 + 2 + 0 + 0 = 3 → 3/4 = 0.75 → 1
		child.bytes_mut()[0] = 1;
		child.bytes_mut()[1] = 2;
		let out = downsample_2x2(
			PixelFormat::Gray8,
			[ChildPixels::Data(&child), ChildPixels::Empty, ChildPixels::Empty, ChildPixels::Empty],
		);
		assert_eq!(out.bytes()[0], 1);
		assert_eq!(out.bytes()[1], 0);

		let mut child16 = TileBuffer::zeroed(PixelFormat::Gray16);
		child16.as_u16_mut()[..2].copy_from_slice(&[65535, 65535]);
		child16.as_u16_mut()[256..258].copy_from_slice(&[65535, 0]);
		let out = downsample_2x2(
			PixelFormat::Gray16,
			[ChildPixels::Empty, ChildPixels::Data(&child16), ChildPixels::Empty, ChildPixels::Empty],
		);
		// top-right quadrant starts at x = 128: (3 × 65535) / 4 = 49151.25 → 49151
		assert_eq!(out.as_u16()[128], 49151);
	}

	#[test]
	fn rgba8_premultiplied_average() {
		let mut child = TileBuffer::zeroed(PixelFormat::Rgba8);
		// two opaque pixels (200, 100, 0) and (0, 100, 200), two transparent
		child.bytes_mut()[..8].copy_from_slice(&[200, 100, 0, 255, 0, 100, 200, 255]);
		let out = downsample_2x2(
			PixelFormat::Rgba8,
			[ChildPixels::Data(&child), ChildPixels::Empty, ChildPixels::Empty, ChildPixels::Empty],
		);
		// alpha 510/4 = 127.5 → 128; colours are the opaque pixels' mean
		assert_eq!(&out.bytes()[..4], &[100, 100, 100, 128]);
	}

	#[test]
	fn transparent_solid_loses_its_colour() {
		let clear_red = PixelValue::rgba16(65535, 0, 0, 0);
		let out = downsample_2x2(PixelFormat::Rgba16, [ChildPixels::Solid(clear_red); 4]);
		assert_eq!(out.uniform_value(), Some(PixelValue::TRANSPARENT));
		let mixed = downsample_2x2(
			PixelFormat::Rgba16,
			[ChildPixels::Solid(clear_red), ChildPixels::Empty, ChildPixels::Empty, ChildPixels::Empty],
		);
		assert_eq!(
			mixed.uniform_value(),
			Some(PixelValue::TRANSPARENT),
			"a transparent solid equals an empty child"
		);
	}

	#[test]
	fn quadrants_map_to_children() {
		let colours = [[65535, 0, 0, 65535], [0, 65535, 0, 65535], [0, 0, 65535, 65535], [7, 7, 7, 65535]];
		let out = downsample_2x2(PixelFormat::Rgba16, colours.map(|c| ChildPixels::Solid(PixelValue(c))));
		assert_eq!(get_rgba16(&out, 0, 0), colours[0]);
		assert_eq!(get_rgba16(&out, 255, 0), colours[1]);
		assert_eq!(get_rgba16(&out, 0, 255), colours[2]);
		assert_eq!(get_rgba16(&out, 255, 255), colours[3]);
	}
}
