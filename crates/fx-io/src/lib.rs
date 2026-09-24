//! # fx-io — getting pixels in and out
//!
//! **Streaming is mandatory.** No importer or exporter may hold the whole
//! image in memory. Decode one band of 256 rows at a time
//! (`width × 256 × bytes_per_pixel`: 61 MB for 30 000 px RGBA16), cut it
//! into tiles, insert them into the [`TileStore`], drop the band. Peak
//! memory of an import is therefore independent of the image height.
//!
//! Formats and milestones:
//! * TIFF (strip and tiled, 8/16-bit, RGB/RGBA/Gray, uncompressed/LZW/Deflate,
//!   BigTIFF) — import M1-T02, export M3.
//! * PNG (8/16-bit), JPEG (8-bit) — import M1-T09, export M3.
//! * Native `.fxd` — M3, spec in docs/FILE_FORMAT.md.
//! * PSD/PSB import — M7; PSD export later.

use std::path::Path;

use fx_core::{BitDepth, ColorProfile};
use fx_tiles::{TileStore, TiledImage};

#[derive(Debug, thiserror::Error)]
pub enum IoError {
	#[error("unsupported file format")]
	UnsupportedFormat,
	#[error("unsupported variant: {0}")]
	Unsupported(String),
	#[error("image too large: {width}×{height} (max 300 000 px per side)")]
	TooLarge { width: u64, height: u64 },
	#[error("decode error: {0}")]
	Decode(String),
	#[error(transparent)]
	Io(#[from] std::io::Error),
	#[error(transparent)]
	Tiles(#[from] fx_tiles::TileError),
	#[error("cancelled")]
	Cancelled,
}

/// Result of importing a flat image file: one pixel layer.
pub struct ImportedImage {
	pub width: u32,
	pub height: u32,
	pub depth: BitDepth,
	pub profile: ColorProfile,
	pub ppi: f32,
	/// Always RGBA of `depth` (gray and RGB sources are expanded).
	pub image: TiledImage,
}

/// Progress callback: `fraction` in 0..=1. Return `false` to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(f32) -> bool;

/// Import by sniffing the file header (not the extension).
pub fn import_file(path: &Path, store: &TileStore, progress: Progress<'_>) -> Result<ImportedImage, IoError> {
	let _ = (path, store, progress);
	todo!("M1-T02 (TIFF) / M1-T09 (PNG, JPEG)")
}
