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

/// Largest accepted image side, in pixels (Photoshop's PSB limit).
pub const MAX_SIDE: u64 = 300_000;

/// File formats [`import_file`] recognises by their first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sniffed {
	Tiff,
	Png,
	Jpeg,
}

/// Recognise a file from its first bytes (at least 8 are needed for PNG).
pub fn sniff(header: &[u8]) -> Option<Sniffed> {
	match header {
		[b'I', b'I', 42, 0, ..] | [b'M', b'M', 0, 42, ..] | [b'I', b'I', 43, 0, ..] | [b'M', b'M', 0, 43, ..] => Some(Sniffed::Tiff),
		[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some(Sniffed::Png),
		[0xFF, 0xD8, 0xFF, ..] => Some(Sniffed::Jpeg),
		_ => None,
	}
}

/// Import by sniffing the file header (not the extension).
pub fn import_file(path: &Path, store: &TileStore, progress: Progress<'_>) -> Result<ImportedImage, IoError> {
	let mut header = [0u8; 8];
	let read = {
		use std::io::Read;
		let mut file = std::fs::File::open(path)?;
		let mut n = 0;
		while n < header.len() {
			match file.read(&mut header[n..])? {
				0 => break,
				k => n += k,
			}
		}
		n
	};
	match sniff(&header[..read]) {
		Some(Sniffed::Tiff) => tiff::import(path, store, progress),
		Some(Sniffed::Png) => png::import(path, store, progress),
		Some(Sniffed::Jpeg) => jpeg::import(path, store, progress),
		None => Err(IoError::UnsupportedFormat),
	}
}

mod band;
mod jpeg;
mod png;
#[cfg(test)]
mod png_jpeg_tests;
mod tiff;
#[cfg(test)]
mod tiff_tests;
