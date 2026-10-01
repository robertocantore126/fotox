//! Streaming TIFF import (M1-T02).
//!
//! Supported: 8/16-bit unsigned samples; RGB, RGBA, Gray, Gray+alpha
//! (associated alpha is converted to straight); strips or tiles; no
//! compression, LZW, Deflate, PackBits; classic TIFF and BigTIFF. Anything else
//! is refused with [`IoError::Unsupported`] and a reason.
//!
//! Pipeline (the band rule in the crate docs):
//! 1. The file is cut into *units* — one strip, or one row of tiles.
//! 2. A batch of units covering about two bands is decoded **in parallel**
//!    (one `tiff::Decoder` per rayon task, each with its own file handle) and
//!    converted to RGBA of the document depth.
//! 3. Converted rows are copied, in order, into a 256-row band; each full
//!    band is cut into 256² tiles (in parallel) and inserted with
//!    `TiledImage::put_buffer` (in order).
//!
//! Only the batch and one band exist at a time, whatever the image height.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use fx_core::{BitDepth, ColorProfile};
use fx_tiles::{TILE_SIZE, TileStore, TiledImage};
use rayon::prelude::*;
use tiff::ColorType;
use tiff::decoder::{ChunkType, Decoder, DecodingResult, Limits, ifd::Value};
use tiff::tags::{CompressionMethod, Tag};

use crate::band::{Band, RowRef};
use crate::{ImportedImage, IoError, MAX_SIDE, Progress};

/// Rows per output band = tile height.
const BAND: usize = TILE_SIZE as usize;
/// Decode this many band-heights of units per parallel batch.
const BATCH_BANDS: usize = 2;

/// What the source pixels look like.
#[derive(Clone, Copy, Debug)]
struct Source {
	/// Samples per pixel: 1 gray, 2 gray+alpha, 3 RGB, 4 RGBA.
	samples: usize,
	sixteen: bool,
	/// Alpha is premultiplied (ExtraSamples = 1) and must be un-premultiplied.
	associated: bool,
}

/// The image layout in decoding units.
#[derive(Clone, Copy, Debug)]
struct Units {
	width: u32,
	height: u32,
	tiled: bool,
	chunk_w: u32,
	chunk_h: u32,
	across: u32,
	down: u32,
}

impl Units {
	/// Rows of image covered by unit `u` (the last one may be shorter).
	fn rows(&self, u: u32) -> u32 {
		self.chunk_h.min(self.height - u * self.chunk_h)
	}
}

pub(crate) fn import(path: &Path, store: &TileStore, progress: Progress<'_>) -> Result<ImportedImage, IoError> {
	let mut decoder = open(path)?;
	let (width, height) = decoder.dimensions().map_err(decode)?;
	if u64::from(width) > MAX_SIDE || u64::from(height) > MAX_SIDE {
		return Err(IoError::TooLarge {
			width: width.into(),
			height: height.into(),
		});
	}
	// AUDIT-FIX(I2): reject hostile headers before any chunk/pixel allocation.
	crate::check_decoded_size(width, height)?;
	let source = source(&mut decoder)?;
	let depth = if source.sixteen { BitDepth::U16 } else { BitDepth::U8 };
	let format = depth.rgba_format();
	let profile = match decoder.find_tag(Tag::IccProfile).map_err(decode)? {
		Some(value) => ColorProfile::Icc(Arc::from(value.into_u8_vec().map_err(decode)?)),
		None => ColorProfile::Srgb,
	};
	let ppi = resolution(&mut decoder)?;

	let (chunk_w, chunk_h) = decoder.chunk_dimensions();
	let tiled = decoder.get_chunk_type() == ChunkType::Tile;
	let units = Units {
		width,
		height,
		tiled,
		chunk_w,
		chunk_h,
		across: if tiled { width.div_ceil(chunk_w) } else { 1 },
		down: height.div_ceil(chunk_h),
	};
	// AUDIT-FIX(I3): oversized uncompressed strips stream by row rather than by whole strip.
	let compression = decoder.find_tag_unsigned::<u16>(Tag::Compression).map_err(decode)?.unwrap_or(1);
	if !tiled && chunk_h > (BATCH_BANDS * BAND) as u32 && compression == 1 {
		let image = stream_strips(path, &mut decoder, units, source, store, progress)?;
		return Ok(ImportedImage {
			width,
			height,
			depth,
			profile,
			ppi,
			image,
		});
	}
	// AUDIT-FIX(I2+I3): converted unit buffers must fit one band batch; huge compressed strips need a streaming decoder.
	let unit_bytes = u64::from(width) * u64::from(chunk_h.min(height)) * if source.sixteen { 8 } else { 4 };
	if unit_bytes > 256 << 20 {
		return Err(IoError::Unsupported(
			"TIFF strip/tile row exceeds the 256 MiB decode batch limit; re-save with smaller strips or tiles".into(),
		));
	}
	drop(decoder);

	let mut image = TiledImage::new(width, height, format);
	let row_samples = width as usize * 4;
	let mut band = Band::new(format, row_samples);
	let batch_units = ((BATCH_BANDS * BAND) / units.chunk_h as usize).max(1);

	let mut unit = 0;
	while unit < units.down {
		let end = (unit + batch_units as u32).min(units.down);
		let decoded: Vec<Converted> = (unit..end)
			.into_par_iter()
			.map_init(|| open(path), |decoder, u| decode_unit(decoder, units, source, u))
			.collect::<Result<_, _>>()?;
		for (offset, rows) in decoded.into_iter().enumerate() {
			let u = unit + offset as u32;
			let y0 = (u * units.chunk_h) as usize;
			for r in 0..units.rows(u) as usize {
				band.push_row(rows.row(r, row_samples), &mut image, store, y0 + r)?;
			}
			if !progress((y0 + units.rows(u) as usize) as f32 / height as f32) {
				return Err(IoError::Cancelled);
			}
		}
		unit = end;
	}
	band.flush(&mut image, store)?;

	Ok(ImportedImage {
		width,
		height,
		depth,
		profile,
		ppi,
		image,
	})
}

fn open(path: &Path) -> Result<Decoder<BufReader<File>>, IoError> {
	let file = File::open(path)?;
	// AUDIT-FIX(I2): metadata/intermediate/chunk allocations are bounded even before header admission.
	Ok(Decoder::new(BufReader::with_capacity(1 << 20, file))
		.map_err(decode)?
		.with_limits(Limits::default()))
}

fn decode(error: tiff::TiffError) -> IoError {
	match error {
		tiff::TiffError::UnsupportedError(e) => IoError::Unsupported(e.to_string()),
		tiff::TiffError::IoError(e) => IoError::Io(e),
		other => IoError::Decode(other.to_string()),
	}
}

fn source(decoder: &mut Decoder<BufReader<File>>) -> Result<Source, IoError> {
	let compression = decoder.find_tag_unsigned::<u16>(Tag::Compression).map_err(decode)?.unwrap_or(1);
	match CompressionMethod::from_u16_exhaustive(compression) {
		CompressionMethod::None | CompressionMethod::LZW | CompressionMethod::Deflate | CompressionMethod::OldDeflate | CompressionMethod::PackBits => {}
		other => {
			return Err(IoError::Unsupported(format!(
				"TIFF compression {other:?} (supported: none, LZW, Deflate, PackBits)"
			)));
		}
	}
	if decoder.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).map_err(decode)?.unwrap_or(1) != 1 {
		return Err(IoError::Unsupported("planar (separate-plane) TIFF".into()));
	}
	// SampleFormat has one value per sample.
	if let Some(formats) = decoder.find_tag_unsigned_vec::<u16>(Tag::SampleFormat).map_err(decode)?
		&& let Some(format) = formats.iter().find(|&&f| f != 1)
	{
		return Err(IoError::Unsupported(format!("TIFF sample format {format} (only unsigned integers)")));
	}
	let photometric = decoder.find_tag_unsigned::<u16>(Tag::PhotometricInterpretation).map_err(decode)?;
	let (samples, bits) = match decoder.colortype().map_err(decode)? {
		ColorType::Gray(b) => (1, b),
		ColorType::GrayA(b) => (2, b),
		ColorType::RGB(b) => (3, b),
		ColorType::RGBA(b) => (4, b),
		// The crate reports gray/RGB with an *associated* alpha as multiband;
		// the photometric tag says what the samples are. (WhiteIsZero with
		// alpha is not inverted by the crate, so it stays unsupported.)
		ColorType::Multiband { bit_depth, num_samples: 2 } if photometric == Some(1) => (2, bit_depth),
		ColorType::Multiband { bit_depth, num_samples: 4 } if photometric == Some(2) => (4, bit_depth),
		other => {
			return Err(IoError::Unsupported(format!(
				"TIFF colour type {other:?} (supported: gray, gray+alpha, RGB, RGBA)"
			)));
		}
	};
	if bits != 8 && bits != 16 {
		return Err(IoError::Unsupported(format!("{bits}-bit TIFF samples (supported: 8, 16)")));
	}
	// ExtraSamples: one value per extra sample; the first one is our alpha.
	let extra = decoder.find_tag_unsigned_vec::<u16>(Tag::ExtraSamples).map_err(decode)?;
	let associated = samples % 2 == 0 && extra.and_then(|e| e.first().copied()) == Some(1);
	Ok(Source {
		samples,
		sixteen: bits == 16,
		associated,
	})
}

/// Pixels per inch from XResolution/ResolutionUnit; 72 when absent.
fn resolution(decoder: &mut Decoder<BufReader<File>>) -> Result<f32, IoError> {
	let Some(value) = decoder.find_tag(Tag::XResolution).map_err(decode)? else {
		return Ok(72.0);
	};
	let x = match value {
		Value::Rational(n, d) if d != 0 => f64::from(n) / f64::from(d),
		Value::Short(v) => f64::from(v),
		Value::Unsigned(v) => f64::from(v),
		_ => return Ok(72.0),
	};
	let ppi = match decoder.find_tag_unsigned::<u16>(Tag::ResolutionUnit).map_err(decode)?.unwrap_or(2) {
		2 => x,
		3 => x * 2.54,
		// 1 = "no absolute unit": the value is only an aspect ratio.
		_ => return Ok(72.0),
	};
	Ok(if ppi.is_finite() && ppi > 0.0 { ppi as f32 } else { 72.0 })
}

/// One unit converted to RGBA of the document depth: `rows × width × 4`.
enum Converted {
	U8(Vec<u8>),
	U16(Vec<u16>),
}

impl Converted {
	fn row(&self, r: usize, row_samples: usize) -> RowRef<'_> {
		match self {
			Converted::U8(v) => RowRef::U8(&v[r * row_samples..(r + 1) * row_samples]),
			Converted::U16(v) => RowRef::U16(&v[r * row_samples..(r + 1) * row_samples]),
		}
	}
}

fn decode_unit(decoder: &mut Result<Decoder<BufReader<File>>, IoError>, units: Units, source: Source, u: u32) -> Result<Converted, IoError> {
	let decoder = match decoder {
		Ok(decoder) => decoder,
		Err(error) => return Err(IoError::Decode(format!("cannot reopen the file: {error}"))),
	};
	let rows = units.rows(u) as usize;
	let width = units.width as usize;
	// AUDIT-FIX(I2): fallible converted unit allocations.
	let mut out16 = if source.sixteen {
		crate::zeroed::<u16>(rows * width * 4)?
	} else {
		Vec::new()
	};
	let mut out8 = if source.sixteen { Vec::new() } else { crate::zeroed::<u8>(rows * width * 4)? };
	for tx in 0..units.across {
		let index = if units.tiled { u * units.across + tx } else { u };
		let (data_w, data_h) = decoder.chunk_data_dimensions(index);
		let x0 = (tx * units.chunk_w) as usize;
		let chunk = decoder.read_chunk(index).map_err(decode)?;
		let copy_w = (data_w as usize).min(width - x0);
		let copy_h = (data_h as usize).min(rows);
		match (chunk, source.sixteen) {
			(DecodingResult::U16(samples), true) => {
				for r in 0..copy_h {
					let src = &samples[r * data_w as usize * source.samples..][..copy_w * source.samples];
					let dst = &mut out16[(r * width + x0) * 4..][..copy_w * 4];
					expand(src, dst, source, u16::MAX);
				}
			}
			(DecodingResult::U8(samples), false) => {
				for r in 0..copy_h {
					let src = &samples[r * data_w as usize * source.samples..][..copy_w * source.samples];
					let dst = &mut out8[(r * width + x0) * 4..][..copy_w * 4];
					expand(src, dst, source, u8::MAX);
				}
			}
			_ => return Err(IoError::Decode("TIFF chunk decoded to an unexpected sample type".into())),
		}
	}
	Ok(if source.sixteen { Converted::U16(out16) } else { Converted::U8(out8) })
}

// AUDIT-FIX(I3): classic/BigTIFF strip ranges are validated, then decoded in bounded rows (8/16 bit).
fn stream_strips(
	path: &Path,
	decoder: &mut Decoder<BufReader<File>>,
	units: Units,
	source: Source,
	store: &TileStore,
	progress: Progress<'_>,
) -> Result<TiledImage, IoError> {
	use std::io::{Read, Seek, SeekFrom};
	let offsets = decoder
		.find_tag_unsigned_vec::<u64>(Tag::StripOffsets)
		.map_err(decode)?
		.ok_or_else(|| IoError::Decode("TIFF without strip offsets".into()))?;
	let counts = decoder
		.find_tag_unsigned_vec::<u64>(Tag::StripByteCounts)
		.map_err(decode)?
		.ok_or_else(|| IoError::Decode("TIFF without strip lengths".into()))?;
	let predictor = decoder.find_tag_unsigned::<u16>(Tag::Predictor).map_err(decode)?.unwrap_or(1);
	let fill_order = decoder.find_tag_unsigned::<u16>(Tag::FillOrder).map_err(decode)?.unwrap_or(1);
	let white = decoder.find_tag_unsigned::<u16>(Tag::PhotometricInterpretation).map_err(decode)? == Some(0);
	if ![1, 2].contains(&predictor) || fill_order != 1 {
		return Err(IoError::Unsupported("large TIFF strip predictor/fill order".into()));
	}
	if offsets.len() != units.down as usize || counts.len() != offsets.len() {
		return Err(IoError::Decode("TIFF strip table length mismatch".into()));
	}
	let mut file = BufReader::with_capacity(1 << 20, File::open(path)?);
	let len = file.get_ref().metadata()?.len();
	let mut endian = [0u8; 2];
	file.read_exact(&mut endian)?;
	let little = endian == *b"II";
	let samples = units.width as usize * source.samples;
	let row_bytes = samples * if source.sixteen { 2 } else { 1 };
	let mut raw = crate::zeroed::<u8>(row_bytes)?;
	let mut src16 = crate::zeroed::<u16>(if source.sixteen { samples } else { 0 })?;
	let mut out8 = crate::zeroed::<u8>(if source.sixteen { 0 } else { units.width as usize * 4 })?;
	let mut out16 = crate::zeroed::<u16>(if source.sixteen { units.width as usize * 4 } else { 0 })?;
	let format = if source.sixteen { BitDepth::U16 } else { BitDepth::U8 }.rgba_format();
	let mut image = TiledImage::new(units.width, units.height, format);
	let mut band = Band::new(format, units.width as usize * 4);
	for unit in 0..units.down {
		let (offset, count) = (offsets[unit as usize], counts[unit as usize]);
		let rows = units.rows(unit);
		if count < rows as u64 * row_bytes as u64 || offset.checked_add(count).is_none_or(|end| end > len) {
			return Err(IoError::Decode("TIFF strip extends beyond file or has too few bytes".into()));
		}
		file.seek(SeekFrom::Start(offset))?;
		for row in 0..rows {
			let y = (unit * units.chunk_h + row) as usize;
			if y.is_multiple_of(BAND) && !progress(y as f32 / units.height as f32) {
				return Err(IoError::Cancelled);
			}
			file.read_exact(&mut raw)?;
			if source.sixteen {
				for (out, bytes) in src16.iter_mut().zip(raw.chunks_exact(2)) {
					*out = if little {
						u16::from_le_bytes([bytes[0], bytes[1]])
					} else {
						u16::from_be_bytes([bytes[0], bytes[1]])
					};
				}
				if predictor == 2 {
					for i in source.samples..samples {
						src16[i] = src16[i].wrapping_add(src16[i - source.samples]);
					}
				}
				if white {
					for px in src16.chunks_exact_mut(source.samples) {
						px[0] = u16::MAX - px[0];
					}
				}
				expand(&src16, &mut out16, source, u16::MAX);
				band.push_row(RowRef::U16(&out16), &mut image, store, y)?;
			} else {
				if predictor == 2 {
					for i in source.samples..samples {
						raw[i] = raw[i].wrapping_add(raw[i - source.samples]);
					}
				}
				if white {
					for px in raw.chunks_exact_mut(source.samples) {
						px[0] = u8::MAX - px[0];
					}
				}
				expand(&raw, &mut out8, source, u8::MAX);
				band.push_row(RowRef::U8(&out8), &mut image, store, y)?;
			}
		}
	}
	band.flush(&mut image, store)?;
	progress(1.0);
	Ok(image)
}

/// Sample types the importer handles.
trait Sample: Copy + Into<u64> + TryFrom<u64> {}
impl Sample for u8 {}
impl Sample for u16 {}

/// Gray / gray+alpha / RGB / RGBA → straight RGBA, same depth.
fn expand<T: Sample>(src: &[T], dst: &mut [T], source: Source, max: T) {
	for (s, d) in src.chunks_exact(source.samples).zip(dst.chunks_exact_mut(4)) {
		let (rgb, a) = match source.samples {
			1 => ([s[0]; 3], max),
			2 => ([s[0]; 3], s[1]),
			3 => ([s[0], s[1], s[2]], max),
			_ => ([s[0], s[1], s[2]], s[3]),
		};
		let rgb = if source.associated { unpremultiply(rgb, a, max) } else { rgb };
		d[..3].copy_from_slice(&rgb);
		d[3] = a;
	}
}

/// `c · max / a`, rounded, clamped (associated → straight alpha).
fn unpremultiply<T: Sample>(rgb: [T; 3], a: T, max: T) -> [T; 3] {
	let (a, max): (u64, u64) = (a.into(), max.into());
	if a == 0 {
		return rgb.map(|_| T::try_from(0).ok().expect("0 fits every sample type"));
	}
	rgb.map(|c| {
		let c: u64 = c.into();
		let v = ((2 * c * max + a) / (2 * a)).min(max);
		T::try_from(v).ok().expect("clamped to the sample maximum")
	})
}
