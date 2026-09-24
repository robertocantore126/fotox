use std::sync::Arc;

use fx_tiles::PixelFormat;
use serde::{Deserialize, Serialize};

/// Bits per channel of a document. 32-bit float is intentionally absent for
/// now (see docs/DECISIONS.md, D-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BitDepth {
	U8,
	U16,
}

impl BitDepth {
	pub fn rgba_format(self) -> PixelFormat {
		match self {
			BitDepth::U8 => PixelFormat::Rgba8,
			BitDepth::U16 => PixelFormat::Rgba16,
		}
	}

	pub fn gray_format(self) -> PixelFormat {
		match self {
			BitDepth::U8 => PixelFormat::Gray8,
			BitDepth::U16 => PixelFormat::Gray16,
		}
	}
}

/// Working colour space of a document. Pixels are stored *encoded* in this
/// space (not linearised) and blend modes operate on encoded values, exactly
/// like Photoshop's default. See docs/ARCHITECTURE.md §6.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorProfile {
	Srgb,
	AdobeRgb1998,
	DisplayP3,
	ProPhotoRgb,
	/// Embedded ICC profile bytes (from an imported file).
	Icc(Arc<[u8]>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentColor {
	pub depth: BitDepth,
	pub profile: ColorProfile,
}
