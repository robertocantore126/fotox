//! JPEG import (M1-T09): baseline and progressive, 8-bit, ICC profile from
//! APP2, EXIF orientation applied, density → ppi.
//!
//! Unlike TIFF and PNG this is **not** streamed: `zune-jpeg` decodes a whole
//! image at once, and applying an EXIF rotation needs every row anyway. The
//! decoded RGBA8 image (`width × height × 4` bytes; JPEG caps sides at
//! 65 535 px) is then cut into bands like every other format. Recorded as a
//! deviation in the M1-T09 report.

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use fx_core::{BitDepth, ColorProfile};
use fx_tiles::{TileStore, TiledImage};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::band::{Band, RowRef};
use crate::{ImportedImage, IoError, Progress};

pub(crate) fn import(path: &Path, store: &TileStore, progress: Progress<'_>) -> Result<ImportedImage, IoError> {
	let file = BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
	let options = DecoderOptions::default()
		.jpeg_set_out_colorspace(ColorSpace::RGBA)
		.set_max_width(usize::from(u16::MAX))
		.set_max_height(usize::from(u16::MAX));
	let mut decoder = JpegDecoder::new_with_options(file, options);
	decoder.decode_headers().map_err(decode)?;
	let info = decoder.info().ok_or_else(|| IoError::Decode("JPEG without a frame header".into()))?;
	let (src_w, src_h) = (u32::from(info.width), u32::from(info.height));
	let ppi = match (info.pixel_density, info.x_density) {
		(1, d) if d > 0 => f32::from(d),
		(2, d) if d > 0 => f32::from(d) * 2.54,
		_ => 72.0,
	};
	let profile = match decoder.icc_profile() {
		Some(bytes) => ColorProfile::Icc(Arc::from(bytes)),
		None => ColorProfile::Srgb,
	};
	let orientation = decoder.exif().and_then(|exif| exif_orientation(exif)).unwrap_or(1);

	let pixels = decoder.decode().map_err(decode)?;
	if pixels.len() != src_w as usize * src_h as usize * 4 {
		return Err(IoError::Decode(format!(
			"JPEG decoded to {} bytes, expected {src_w} × {src_h} × 4",
			pixels.len()
		)));
	}
	if !progress(0.5) {
		return Err(IoError::Cancelled);
	}

	let (width, height) = if orientation >= 5 { (src_h, src_w) } else { (src_w, src_h) };
	let format = BitDepth::U8.rgba_format();
	let mut image = TiledImage::new(width, height, format);
	let mut band = Band::new(format, width as usize * 4);
	let mut row = vec![0u8; width as usize * 4];
	for y in 0..height {
		for x in 0..width {
			let (sx, sy) = source_position(orientation, x, y, src_w, src_h);
			let i = (sy as usize * src_w as usize + sx as usize) * 4;
			row[x as usize * 4..][..4].copy_from_slice(&pixels[i..i + 4]);
		}
		band.push_row(RowRef::U8(&row), &mut image, store, y as usize)?;
		if y % 256 == 255 && !progress(0.5 + 0.5 * (y + 1) as f32 / height as f32) {
			return Err(IoError::Cancelled);
		}
	}
	band.flush(&mut image, store)?;
	progress(1.0);

	Ok(ImportedImage {
		width,
		height,
		depth: BitDepth::U8,
		profile,
		ppi,
		image,
	})
}

fn decode(error: zune_jpeg::errors::DecodeErrors) -> IoError {
	IoError::Decode(format!("JPEG: {error}"))
}

/// Where output pixel `(x, y)` of the upright image comes from in the stored
/// `src_w × src_h` image, for EXIF orientation 1–8.
pub(crate) fn source_position(orientation: u16, x: u32, y: u32, src_w: u32, src_h: u32) -> (u32, u32) {
	match orientation {
		2 => (src_w - 1 - x, y),             // mirrored horizontally
		3 => (src_w - 1 - x, src_h - 1 - y), // rotated 180°
		4 => (x, src_h - 1 - y),             // mirrored vertically
		5 => (y, x),                         // transposed
		6 => (y, src_h - 1 - x),             // needs 90° clockwise
		7 => (src_w - 1 - y, src_h - 1 - x), // transverse
		8 => (src_w - 1 - y, x),             // needs 90° counter-clockwise
		_ => (x, y),
	}
}

/// The Orientation tag (0x0112) from an EXIF block (with or without its
/// `Exif\0\0` prefix). `None` if it is missing or the block is malformed.
pub(crate) fn exif_orientation(exif: &[u8]) -> Option<u16> {
	let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
	let le = match tiff.get(..2)? {
		b"II" => true,
		b"MM" => false,
		_ => return None,
	};
	let u16_at = |i: usize| -> Option<u16> {
		let b = tiff.get(i..i + 2)?;
		Some(if le {
			u16::from_le_bytes([b[0], b[1]])
		} else {
			u16::from_be_bytes([b[0], b[1]])
		})
	};
	let u32_at = |i: usize| -> Option<u32> {
		let b = tiff.get(i..i + 4)?;
		Some(if le {
			u32::from_le_bytes([b[0], b[1], b[2], b[3]])
		} else {
			u32::from_be_bytes([b[0], b[1], b[2], b[3]])
		})
	};
	if u16_at(2)? != 42 {
		return None;
	}
	let ifd = u32_at(4)? as usize;
	let count = u16_at(ifd)? as usize;
	for n in 0..count {
		let entry = ifd + 2 + n * 12;
		if u16_at(entry)? == 0x0112 {
			let value = u16_at(entry + 8)?;
			return (1..=8).contains(&value).then_some(value);
		}
	}
	None
}
