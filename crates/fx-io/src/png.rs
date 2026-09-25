//! PNG import (M1-T09): 8/16-bit; gray, gray+alpha, RGB, RGBA, palette
//! (with or without tRNS). Same band pipeline as TIFF: non-interlaced rows
//! stream straight into the band. Interlaced PNGs arrive pass by pass, so
//! they are decoded whole — capped at 16 384 px per side (they are never
//! huge in practice).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use ::png::{BitDepth as PngDepth, ColorType, Transformations, Unit};
use fx_core::{BitDepth, ColorProfile};
use fx_tiles::{TileStore, TiledImage};

use crate::band::{Band, RowRef};
use crate::{ImportedImage, IoError, MAX_SIDE, Progress};

/// Largest side of an interlaced PNG (it is decoded in one piece).
const MAX_INTERLACED_SIDE: u32 = 16_384;

pub(crate) fn import(path: &Path, store: &TileStore, progress: Progress<'_>) -> Result<ImportedImage, IoError> {
	let mut decoder = ::png::Decoder::new(BufReader::with_capacity(1 << 20, File::open(path)?));
	// Palette → RGB, tRNS → alpha, 1/2/4-bit gray → 8-bit; then add an opaque
	// alpha channel to anything without one. 16-bit stays 16-bit.
	decoder.set_transformations(Transformations::EXPAND | Transformations::ALPHA);
	let mut reader = decoder.read_info().map_err(decode)?;
	let (width, height, interlaced, profile, ppi) = {
		let info = reader.info();
		let profile = match &info.icc_profile {
			Some(bytes) => ColorProfile::Icc(Arc::from(bytes.as_ref())),
			None => ColorProfile::Srgb,
		};
		let ppi = match info.pixel_dims {
			Some(dims) if dims.unit == Unit::Meter && dims.xppu > 0 => dims.xppu as f32 * 0.0254,
			_ => 72.0,
		};
		(info.width, info.height, info.interlaced, profile, ppi)
	};
	if u64::from(width) > MAX_SIDE || u64::from(height) > MAX_SIDE {
		return Err(IoError::TooLarge {
			width: width.into(),
			height: height.into(),
		});
	}
	let (color, depth) = reader.output_color_type();
	let gray = match color {
		ColorType::GrayscaleAlpha => true,
		ColorType::Rgba => false,
		other => return Err(IoError::Decode(format!("PNG decoder produced {other:?} after expansion"))),
	};
	let sixteen = match depth {
		PngDepth::Eight => false,
		PngDepth::Sixteen => true,
		other => return Err(IoError::Unsupported(format!("PNG bit depth {other:?} after expansion"))),
	};
	let doc_depth = if sixteen { BitDepth::U16 } else { BitDepth::U8 };
	let mut image = TiledImage::new(width, height, doc_depth.rgba_format());
	let row_samples = width as usize * 4;
	let mut band = Band::new(doc_depth.rgba_format(), row_samples);
	let mut rgba8 = vec![0u8; if sixteen { 0 } else { row_samples }];
	let mut rgba16 = vec![0u16; if sixteen { row_samples } else { 0 }];

	let mut push = |y: usize, data: &[u8], band: &mut Band, image: &mut TiledImage| -> Result<(), IoError> {
		let row = if sixteen {
			to_rgba16(data, gray, &mut rgba16);
			RowRef::U16(&rgba16)
		} else {
			to_rgba8(data, gray, &mut rgba8);
			RowRef::U8(&rgba8)
		};
		band.push_row(row, image, store, y)
	};

	if interlaced {
		if width > MAX_INTERLACED_SIDE || height > MAX_INTERLACED_SIDE {
			return Err(IoError::Unsupported(format!(
				"interlaced PNG larger than {MAX_INTERLACED_SIDE} px per side ({width} × {height})"
			)));
		}
		let size = reader.output_buffer_size().ok_or_else(|| IoError::Decode("PNG output size overflows".into()))?;
		let mut whole = vec![0u8; size];
		let frame = reader.next_frame(&mut whole).map_err(decode)?;
		for y in 0..height as usize {
			push(y, &whole[y * frame.line_size..][..frame.line_size], &mut band, &mut image)?;
			if y % 256 == 255 && !progress((y + 1) as f32 / height as f32) {
				return Err(IoError::Cancelled);
			}
		}
	} else {
		let mut y = 0usize;
		while let Some(row) = reader.next_row().map_err(decode)? {
			push(y, row.data(), &mut band, &mut image)?;
			y += 1;
			if y % 256 == 0 && !progress(y as f32 / height as f32) {
				return Err(IoError::Cancelled);
			}
		}
		if y != height as usize {
			return Err(IoError::Decode(format!("PNG ended after {y} of {height} rows")));
		}
	}
	band.flush(&mut image, store)?;
	progress(1.0);

	Ok(ImportedImage {
		width,
		height,
		depth: doc_depth,
		profile,
		ppi,
		image,
	})
}

fn decode(error: ::png::DecodingError) -> IoError {
	match error {
		::png::DecodingError::IoError(e) => IoError::Io(e),
		::png::DecodingError::LimitsExceeded => IoError::Unsupported("PNG exceeds the decoder's limits".into()),
		other => IoError::Decode(other.to_string()),
	}
}

/// Gray+alpha or RGBA, 8-bit → RGBA8.
fn to_rgba8(data: &[u8], gray: bool, out: &mut [u8]) {
	if gray {
		for (s, d) in data.chunks_exact(2).zip(out.chunks_exact_mut(4)) {
			d.copy_from_slice(&[s[0], s[0], s[0], s[1]]);
		}
	} else {
		out.copy_from_slice(&data[..out.len()]);
	}
}

/// Gray+alpha or RGBA, 16-bit big-endian (PNG byte order) → RGBA16 native.
fn to_rgba16(data: &[u8], gray: bool, out: &mut [u16]) {
	let be = |i: usize| u16::from_be_bytes([data[2 * i], data[2 * i + 1]]);
	if gray {
		for (p, d) in out.chunks_exact_mut(4).enumerate() {
			let (v, a) = (be(2 * p), be(2 * p + 1));
			d.copy_from_slice(&[v, v, v, a]);
		}
	} else {
		for (i, d) in out.iter_mut().enumerate() {
			*d = be(i);
		}
	}
}
