//! CPU mip generation. (A GPU version may come later; this one is the
//! reference and must stay bit-exact with its tests.)

use crate::PixelFormat;
use crate::format::PixelValue;
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
	let _ = (format, children);
	todo!("M1-T05: implement as documented")
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
	#[ignore = "M1-T05"]
	fn solid_children_stay_solid() {
		let v = PixelValue::rgba16(1000, 2000, 3000, 65535);
		let out = downsample_2x2(PixelFormat::Rgba16, [ChildPixels::Solid(v); 4]);
		assert_eq!(out.uniform_value(), Some(v));
	}

	#[test]
	#[ignore = "M1-T05"]
	fn empty_children_are_transparent() {
		let out = downsample_2x2(PixelFormat::Rgba8, [ChildPixels::Empty; 4]);
		assert_eq!(out.uniform_value(), Some(PixelValue::TRANSPARENT));
	}

	#[test]
	#[ignore = "M1-T05"]
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
	#[ignore = "M1-T05"]
	fn quadrants_map_to_children() {
		let colours = [[65535, 0, 0, 65535], [0, 65535, 0, 65535], [0, 0, 65535, 65535], [7, 7, 7, 65535]];
		let out = downsample_2x2(PixelFormat::Rgba16, colours.map(|c| ChildPixels::Solid(PixelValue(c))));
		assert_eq!(get_rgba16(&out, 0, 0), colours[0]);
		assert_eq!(get_rgba16(&out, 255, 0), colours[1]);
		assert_eq!(get_rgba16(&out, 0, 255), colours[2]);
		assert_eq!(get_rgba16(&out, 255, 255), colours[3]);
	}
}
