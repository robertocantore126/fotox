use serde::{Deserialize, Serialize};

/// Edge length of a tile in pixels. Fixed for the whole application.
/// Changing it invalidates the native file format.
pub const TILE_SIZE: u32 = 256;

/// Pixels in one tile.
pub const TILE_PIXELS: usize = (TILE_SIZE * TILE_SIZE) as usize;

/// Memory layout of the pixels inside a tile.
///
/// * RGBA formats hold **straight (non-premultiplied) alpha**, channels in
///   R, G, B, A order, row-major, no padding. Straight alpha is what editing
///   tools and PSD expect; the renderer premultiplies on upload.
/// * Gray formats hold a single channel and are used for layer masks,
///   selections and alpha channels. They have no alpha.
/// * 16-bit values use the full `0..=65535` range (not Photoshop's 15-bit
///   `0..=32768`); PSD import/export converts.
/// * 16-bit samples are stored in native (little-endian) byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PixelFormat {
	Rgba8,
	Rgba16,
	Gray8,
	Gray16,
}

impl PixelFormat {
	pub const fn channels(self) -> usize {
		match self {
			PixelFormat::Rgba8 | PixelFormat::Rgba16 => 4,
			PixelFormat::Gray8 | PixelFormat::Gray16 => 1,
		}
	}

	pub const fn bytes_per_channel(self) -> usize {
		match self {
			PixelFormat::Rgba8 | PixelFormat::Gray8 => 1,
			PixelFormat::Rgba16 | PixelFormat::Gray16 => 2,
		}
	}

	pub const fn bytes_per_pixel(self) -> usize {
		self.channels() * self.bytes_per_channel()
	}

	/// Size in bytes of one uncompressed tile of this format.
	pub const fn tile_bytes(self) -> usize {
		self.bytes_per_pixel() * TILE_PIXELS
	}

	pub const fn has_alpha(self) -> bool {
		self.channels() == 4
	}
}

/// One pixel value, independent of the storage format.
///
/// Always expressed on a 16-bit scale: an 8-bit value `v` is stored as
/// `v * 257` so that `255 → 65535`. Gray formats use only `.0[0]`; the other
/// entries must be 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PixelValue(pub [u16; 4]);

impl PixelValue {
	pub const TRANSPARENT: PixelValue = PixelValue([0, 0, 0, 0]);

	pub const fn rgba16(r: u16, g: u16, b: u16, a: u16) -> Self {
		PixelValue([r, g, b, a])
	}

	pub const fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
		PixelValue([r as u16 * 257, g as u16 * 257, b as u16 * 257, a as u16 * 257])
	}

	pub const fn gray16(v: u16) -> Self {
		PixelValue([v, 0, 0, 0])
	}

	/// True if this value is fully transparent in `format`.
	/// Gray formats have no alpha and are never transparent.
	pub const fn is_transparent(self, format: PixelFormat) -> bool {
		format.has_alpha() && self.0[3] == 0
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tile_sizes() {
		assert_eq!(PixelFormat::Rgba8.tile_bytes(), 256 * 256 * 4);
		assert_eq!(PixelFormat::Rgba16.tile_bytes(), 256 * 256 * 8);
		assert_eq!(PixelFormat::Gray8.tile_bytes(), 256 * 256);
		assert_eq!(PixelFormat::Gray16.tile_bytes(), 256 * 256 * 2);
	}

	#[test]
	fn eight_bit_scales_to_full_range() {
		assert_eq!(PixelValue::rgba8(255, 0, 128, 255), PixelValue([65535, 0, 128 * 257, 65535]));
	}
}
