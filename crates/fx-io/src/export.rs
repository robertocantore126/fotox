//! Flat-image export (M3): TIFF and PNG, streamed one band at a time.
//!
//! The caller renders the flattened image band by band (the engine composites
//! it); this module only converts and encodes. Peak memory is one band of
//! [`EXPORT_BAND_ROWS`] rows, whatever the image height.
//!
//! The file is written as `<name>.part` next to the target and renamed at the
//! end, so a failed or cancelled export never destroys an existing file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::tiff_write::TiffWriter;
use crate::{IoError, Progress};

/// Rows per band (and per TIFF strip).
pub const EXPORT_BAND_ROWS: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
	Tiff,
	Png,
	Jpeg,
}

impl ExportFormat {
	/// The format for a file name's extension (case-insensitive), if Fotox can write it.
	pub fn from_path(path: &Path) -> Option<Self> {
		let ext = path.extension()?.to_str()?.to_ascii_lowercase();
		match ext.as_str() {
			"tif" | "tiff" => Some(Self::Tiff),
			"png" => Some(Self::Png),
			"jpg" | "jpeg" => Some(Self::Jpeg),
			_ => None,
		}
	}
}

/// JPEG chroma subsampling (M3-T07-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JpegChroma {
	/// 4:4:4 — no subsampling, the sharpest chroma.
	#[default]
	Full,
	/// 4:2:0 — half resolution in both directions, smaller files.
	Half,
}

/// Converts straight RGB16 to CMYK16 (0 = no ink), pixel for pixel.
pub type RgbToCmyk = dyn Fn(&[[u16; 3]], &mut [[u16; 4]]) + Send + Sync;

/// CMYK output (M4-T04): the caller's RGB16 → CMYK16 conversion (an lcms2
/// transform in the engine; `fx-io` does no colour management) and the CMYK
/// profile to embed. CMYK is written as TIFF only, flattened onto white.
#[derive(Clone)]
pub struct CmykExport {
	pub convert: Arc<RgbToCmyk>,
	pub icc: Arc<[u8]>,
}

impl std::fmt::Debug for CmykExport {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("CmykExport").field("icc_bytes", &self.icc.len()).finish()
	}
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
	pub format: ExportFormat,
	/// 8 or 16 bits per channel (JPEG is 8 only).
	pub bits: u16,
	/// Keep transparency (never for JPEG). Without it, the image is flattened
	/// onto white, like Photoshop's Flatten Image with a white background.
	pub alpha: bool,
	/// Pixels per inch written in the file (TIFF resolution, PNG `pHYs`).
	pub ppi: f32,
	/// JPEG quality 0..=100 (ignored by TIFF and PNG).
	pub quality: u8,
	/// JPEG chroma subsampling (ignored by TIFF and PNG).
	pub chroma: JpegChroma,
	/// The document's ICC profile, embedded in the file (M4-T03).
	pub icc: Option<Arc<[u8]>>,
	/// Convert to CMYK (TIFF only) instead of writing RGB (M4-T04).
	pub cmyk: Option<CmykExport>,
}

/// Renders one band: `render(y, rows, out)` fills `out` (`width × rows`
/// pixels, row-major) with straight — not premultiplied — RGBA16.
pub type BandRenderer<'a> = &'a mut dyn FnMut(u32, u32, &mut [[u16; 4]]) -> Result<(), IoError>;

/// Export a `width × height` image to `path`, band by band.
pub fn export_image(path: &Path, width: u32, height: u32, options: ExportOptions, render: BandRenderer<'_>, progress: Progress<'_>) -> Result<(), IoError> {
	if options.bits != 8 && options.bits != 16 {
		return Err(IoError::Unsupported(format!("{} bits per channel", options.bits)));
	}
	if width == 0 || height == 0 {
		return Err(IoError::Unsupported("empty image".into()));
	}
	if options.cmyk.is_some() && options.format != ExportFormat::Tiff {
		return Err(IoError::Unsupported("CMYK export writes TIFF files".into()));
	}
	let part = part_path(path);
	let result = write(&part, width, height, &options, render, progress);
	match result {
		Ok(()) => {
			std::fs::rename(&part, path)?;
			Ok(())
		}
		Err(e) => {
			let _ = std::fs::remove_file(&part);
			Err(e)
		}
	}
}

fn part_path(path: &Path) -> PathBuf {
	let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
	name.push(".part");
	path.with_file_name(name)
}

fn write(part: &Path, width: u32, height: u32, options: &ExportOptions, render: BandRenderer<'_>, progress: Progress<'_>) -> Result<(), IoError> {
	if options.format == ExportFormat::Jpeg {
		return write_jpeg(part, width, height, options, render, progress);
	}
	let samples: usize = if options.alpha || options.cmyk.is_some() { 4 } else { 3 };
	let mut band = vec![[0u16; 4]; width as usize * EXPORT_BAND_ROWS as usize];
	let mut bytes = Vec::new();
	let mut sink: Box<dyn Sink> = match options.format {
		ExportFormat::Tiff => {
			let mut writer = TiffWriter::create_with(part, width, height, options.bits, samples as u16, EXPORT_BAND_ROWS, None)?;
			writer.set_ppi(options.ppi);
			match (&options.cmyk, &options.icc) {
				(Some(cmyk), _) => {
					writer.set_cmyk()?;
					writer.set_icc(cmyk.icc.to_vec());
				}
				(None, Some(icc)) => writer.set_icc(icc.to_vec()),
				(None, None) => {}
			}
			Box::new(writer)
		}
		ExportFormat::Png => Box::new(PngSink::create(part, width, height, options)?),
		// JPEG needs full pixel rows, not the byte stream: handled above.
		ExportFormat::Jpeg => unreachable!("JPEG is written by write_jpeg"),
	};
	let mut y = 0;
	while y < height {
		let rows = EXPORT_BAND_ROWS.min(height - y);
		let pixels = &mut band[..width as usize * rows as usize];
		render(y, rows, pixels)?;
		bytes.clear();
		encode(pixels, options, sink.big_endian(), &mut bytes);
		sink.write_band(&bytes)?;
		y += rows;
		if !progress(y as f32 / height as f32) {
			return Err(IoError::Cancelled);
		}
	}
	sink.finish()
}

/// JPEG has no streaming encoder: buffer the whole image (8-bit RGB, flattened
/// onto white) and encode it. Refuses sides above 16 384 px, where the buffer
/// would grow past ~800 MB (JPEG's own limit is 65 535).
fn write_jpeg(part: &Path, width: u32, height: u32, options: &ExportOptions, render: BandRenderer<'_>, progress: Progress<'_>) -> Result<(), IoError> {
	const MAX_SIDE: u32 = 16_384;
	if options.bits != 8 {
		return Err(IoError::Unsupported("JPEG is 8 bits per channel".into()));
	}
	if width > MAX_SIDE || height > MAX_SIDE {
		return Err(IoError::Unsupported(format!(
			"JPEG export supports up to {MAX_SIDE} px per side (this image is {width}×{height})"
		)));
	}
	let stride = width as usize;
	let mut rgb = vec![0u8; stride * height as usize * 3];
	let mut band = vec![[0u16; 4]; stride * EXPORT_BAND_ROWS as usize];
	let mut y = 0;
	while y < height {
		let rows = EXPORT_BAND_ROWS.min(height - y);
		let pixels = &mut band[..stride * rows as usize];
		render(y, rows, pixels)?;
		for (i, &[r, g, b, a]) in pixels.iter().enumerate() {
			let row = y as usize + i / stride;
			let col = i % stride;
			let out = (row * stride + col) * 3;
			rgb[out] = to_8(over_white(r, a));
			rgb[out + 1] = to_8(over_white(g, a));
			rgb[out + 2] = to_8(over_white(b, a));
		}
		y += rows;
		if !progress(y as f32 / height as f32) {
			return Err(IoError::Cancelled);
		}
	}
	let mut encoder = jpeg_encoder::Encoder::new_file(part, options.quality).map_err(jpeg_error)?;
	encoder.set_sampling_factor(match options.chroma {
		JpegChroma::Full => jpeg_encoder::SamplingFactor::F_1_1,
		JpegChroma::Half => jpeg_encoder::SamplingFactor::F_2_2,
	});
	if let Some(icc) = &options.icc {
		encoder.add_icc_profile(icc).map_err(jpeg_error)?;
	}
	encoder
		.encode(&rgb, width as u16, height as u16, jpeg_encoder::ColorType::Rgb)
		.map_err(jpeg_error)
}

fn jpeg_error(e: jpeg_encoder::EncodingError) -> IoError {
	IoError::Decode(format!("jpeg: {e}"))
}

/// Straight RGBA16 → the file's samples (RGB, RGBA or CMYK, 8 or 16 bits).
fn encode(pixels: &[[u16; 4]], options: &ExportOptions, big_endian: bool, out: &mut Vec<u8>) {
	if let Some(cmyk) = &options.cmyk {
		// Flattened onto white, then the caller's RGB → CMYK conversion.
		let rgb: Vec<[u16; 3]> = pixels
			.iter()
			.map(|&[r, g, b, a]| [over_white(r, a), over_white(g, a), over_white(b, a)])
			.collect();
		let mut inks = vec![[0u16; 4]; rgb.len()];
		(cmyk.convert)(&rgb, &mut inks);
		out.reserve(inks.len() * 4 * usize::from(options.bits / 8));
		for px in inks {
			for v in px {
				if options.bits == 8 {
					out.push(to_8(v));
				} else if big_endian {
					out.extend_from_slice(&v.to_be_bytes());
				} else {
					out.extend_from_slice(&v.to_le_bytes());
				}
			}
		}
		return;
	}
	let samples = if options.alpha { 4 } else { 3 };
	out.reserve(pixels.len() * samples * usize::from(options.bits / 8));
	for &[r, g, b, a] in pixels {
		let px = if options.alpha {
			[r, g, b, a]
		} else {
			[over_white(r, a), over_white(g, a), over_white(b, a), 0]
		};
		for &v in &px[..samples] {
			if options.bits == 8 {
				out.push(to_8(v));
			} else if big_endian {
				out.extend_from_slice(&v.to_be_bytes());
			} else {
				out.extend_from_slice(&v.to_le_bytes());
			}
		}
	}
}

/// `c` (straight, alpha `a`) composited onto white.
fn over_white(c: u16, a: u16) -> u16 {
	let (c, a) = (u32::from(c), u32::from(a));
	((c * a + 65535 * (65535 - a) + 32767) / 65535) as u16
}

/// 16 → 8 bits, rounded (the inverse of `v * 257`).
fn to_8(v: u16) -> u8 {
	((u32::from(v) * 255 + 32767) / 65535) as u8
}

trait Sink {
	/// PNG stores 16-bit samples big-endian, the TIFF writer little-endian.
	fn big_endian(&self) -> bool;
	fn write_band(&mut self, bytes: &[u8]) -> Result<(), IoError>;
	fn finish(self: Box<Self>) -> Result<(), IoError>;
}

impl Sink for TiffWriter {
	fn big_endian(&self) -> bool {
		false
	}

	fn write_band(&mut self, bytes: &[u8]) -> Result<(), IoError> {
		self.write_strip(bytes)
	}

	fn finish(self: Box<Self>) -> Result<(), IoError> {
		TiffWriter::finish(*self).map(|_| ())
	}
}

struct PngSink {
	writer: ::png::StreamWriter<'static, BufWriter<File>>,
}

impl PngSink {
	fn create(path: &Path, width: u32, height: u32, options: &ExportOptions) -> Result<Self, IoError> {
		let file = BufWriter::with_capacity(8 << 20, File::create(path)?);
		let mut info = ::png::Info::with_size(width, height);
		info.icc_profile = options.icc.as_ref().map(|icc| std::borrow::Cow::Owned(icc.to_vec()));
		let mut encoder = ::png::Encoder::with_info(file, info).map_err(png_error)?;
		encoder.set_color(if options.alpha { ::png::ColorType::Rgba } else { ::png::ColorType::Rgb });
		encoder.set_depth(if options.bits == 16 {
			::png::BitDepth::Sixteen
		} else {
			::png::BitDepth::Eight
		});
		// Big images: favour speed; PNG's compression ratio barely changes.
		encoder.set_compression(::png::Compression::Fast);
		if options.ppi.is_finite() && options.ppi > 0.0 {
			let per_metre = (f64::from(options.ppi) / 0.0254).round() as u32;
			encoder.set_pixel_dims(Some(::png::PixelDimensions {
				xppu: per_metre,
				yppu: per_metre,
				unit: ::png::Unit::Meter,
			}));
		}
		let writer = encoder.write_header().map_err(png_error)?.into_stream_writer().map_err(png_error)?;
		Ok(Self { writer })
	}
}

impl Sink for PngSink {
	fn big_endian(&self) -> bool {
		true
	}

	fn write_band(&mut self, bytes: &[u8]) -> Result<(), IoError> {
		self.writer.write_all(bytes)?;
		Ok(())
	}

	fn finish(self: Box<Self>) -> Result<(), IoError> {
		self.writer.finish().map_err(png_error)
	}
}

fn png_error(e: ::png::EncodingError) -> IoError {
	match e {
		::png::EncodingError::IoError(e) => IoError::Io(e),
		other => IoError::Decode(other.to_string()),
	}
}

#[cfg(test)]
mod tests {
	use fx_tiles::{PixelFormat, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

	use super::*;
	use crate::import_file;

	const W: u32 = 300;
	const H: u32 = 530; // three bands, the last one short

	fn dir() -> PathBuf {
		let dir = std::env::temp_dir().join("fx-io-export-tests");
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	fn store() -> TileStore {
		let mut config = TileStoreConfig::for_tests(dir().join("scratch"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).unwrap()
	}

	/// The test image: distinct values per pixel and channel, alpha varying.
	fn source(x: u32, y: u32) -> [u16; 4] {
		[
			(x * 211 + y * 7) as u16,
			(y * 123 + x) as u16,
			(x * y) as u16,
			if x < 20 { 0 } else { (65535 - y * 100) as u16 },
		]
	}

	fn renderer() -> impl FnMut(u32, u32, &mut [[u16; 4]]) -> Result<(), IoError> {
		|y0, rows, out| {
			assert_eq!(out.len(), (W * rows) as usize);
			for y in 0..rows {
				for x in 0..W {
					out[(y * W + x) as usize] = source(x, y0 + y);
				}
			}
			Ok(())
		}
	}

	fn pixel(image: &TiledImage, store: &TileStore, x: u32, y: u32) -> [u16; 4] {
		let format = image.format();
		let tile = match image.slot(0, x / 256, y / 256) {
			TileSlot::Empty => TileBuffer::zeroed(format),
			TileSlot::Solid(v) => TileBuffer::filled(format, *v),
			TileSlot::Data(handle) => (*store.get(handle).unwrap()).clone(),
		};
		let i = ((y % 256) * 256 + x % 256) as usize * 4;
		if format == PixelFormat::Rgba16 {
			let s = tile.as_u16();
			[s[i], s[i + 1], s[i + 2], s[i + 3]]
		} else {
			let b = tile.bytes();
			[b[i].into(), b[i + 1].into(), b[i + 2].into(), b[i + 3].into()]
		}
	}

	/// What the file should hold for `source(x, y)`, as the importer returns it.
	fn expected(options: &ExportOptions, x: u32, y: u32) -> [u16; 4] {
		let [r, g, b, a] = source(x, y);
		let px = if options.alpha {
			[r, g, b, a]
		} else {
			[over_white(r, a), over_white(g, a), over_white(b, a), 65535]
		};
		if options.bits == 8 { px.map(|v| u16::from(to_8(v))) } else { px }
	}

	fn round_trip(format: ExportFormat, bits: u16, alpha: bool) {
		let options = ExportOptions {
			format,
			bits,
			alpha,
			ppi: 300.0,
			quality: 90,
			chroma: JpegChroma::Full,
			icc: None,
			cmyk: None,
		};
		let ext = if format == ExportFormat::Tiff { "tif" } else { "png" };
		let path = dir().join(format!("out-{bits}-{alpha}.{ext}"));
		let mut calls = 0;
		export_image(&path, W, H, options.clone(), &mut renderer(), &mut |_| {
			calls += 1;
			true
		})
		.unwrap();
		assert_eq!(calls, 3, "one progress call per band");
		assert!(!part_path(&path).exists(), "the .part file is renamed");

		let store = store();
		let imported = import_file(&path, &store, &mut |_| true).unwrap();
		assert_eq!((imported.width, imported.height), (W, H));
		assert!((imported.ppi - 300.0).abs() < 0.01, "ppi {}", imported.ppi);
		for &(x, y) in &[(0, 0), (19, 3), (20, 3), (299, 0), (255, 255), (256, 256), (123, 511), (299, 529), (7, 512)] {
			let (want, got) = (expected(&options, x, y), pixel(&imported.image, &store, x, y));
			assert_eq!(got, want, "{format:?} {bits}-bit alpha={alpha}: pixel ({x}, {y})");
		}
	}

	#[test]
	fn tiff_round_trips() {
		for bits in [8, 16] {
			for alpha in [false, true] {
				round_trip(ExportFormat::Tiff, bits, alpha);
			}
		}
	}

	#[test]
	fn png_round_trips() {
		for bits in [8, 16] {
			for alpha in [false, true] {
				round_trip(ExportFormat::Png, bits, alpha);
			}
		}
	}

	#[test]
	fn cancel_keeps_the_existing_file() {
		let path = dir().join("keep.png");
		std::fs::write(&path, b"old").unwrap();
		let options = ExportOptions {
			format: ExportFormat::Png,
			bits: 8,
			alpha: false,
			ppi: 72.0,
			quality: 90,
			chroma: JpegChroma::Full,
			icc: None,
			cmyk: None,
		};
		let result = export_image(&path, W, H, options, &mut renderer(), &mut |_| false);
		assert!(matches!(result, Err(IoError::Cancelled)));
		assert_eq!(std::fs::read(&path).unwrap(), b"old");
		assert!(!part_path(&path).exists());
	}

	#[test]
	fn jpeg_round_trips() {
		// JPEG is lossy: a smooth image plus a small tolerance. 8-bit only.
		let options = ExportOptions {
			format: ExportFormat::Jpeg,
			bits: 8,
			alpha: false,
			ppi: 300.0,
			quality: 90,
			chroma: JpegChroma::Full,
			icc: None,
			cmyk: None,
		};
		let path = dir().join("out.jpg");
		let mut smooth = |y0: u32, rows: u32, out: &mut [[u16; 4]]| -> Result<(), IoError> {
			for y in 0..rows {
				for x in 0..W {
					out[(y * W + x) as usize] = [(x * 100) as u16, ((y0 + y) * 50) as u16, 32768, 65535];
				}
			}
			Ok(())
		};
		export_image(&path, W, H, options, &mut smooth, &mut |_| true).unwrap();

		let store = store();
		let imported = import_file(&path, &store, &mut |_| true).unwrap();
		assert_eq!((imported.width, imported.height), (W, H));
		for &(x, y) in &[(10, 10), (200, 300), (299, 529)] {
			let want = [to_8((x * 100) as u16), to_8((y * 50) as u16), to_8(32768), 255];
			let got = pixel(&imported.image, &store, x, y);
			for c in 0..3 {
				let d = (i32::from(got[c]) - i32::from(want[c])).abs();
				assert!(d <= 6, "jpeg pixel ({x},{y}) channel {c}: {got:?} vs {want:?}");
			}
			assert_eq!(got[3], 255, "jpeg has no alpha: opaque white background");
		}
	}

	#[test]
	fn format_from_extension() {
		assert_eq!(ExportFormat::from_path(Path::new("a/b.TIFF")), Some(ExportFormat::Tiff));
		assert_eq!(ExportFormat::from_path(Path::new("b.tif")), Some(ExportFormat::Tiff));
		assert_eq!(ExportFormat::from_path(Path::new("b.png")), Some(ExportFormat::Png));
		assert_eq!(ExportFormat::from_path(Path::new("b.jpg")), Some(ExportFormat::Jpeg));
		assert_eq!(ExportFormat::from_path(Path::new("b.jpeg")), Some(ExportFormat::Jpeg));
		assert_eq!(ExportFormat::from_path(Path::new("b.gif")), None);
		assert_eq!(ExportFormat::from_path(Path::new("b")), None);
	}

	#[test]
	fn the_icc_profile_travels_with_tiff_png_and_jpeg() {
		let icc: Arc<[u8]> = Arc::from(vec![7u8; 600]);
		for (format, ext) in [(ExportFormat::Tiff, "tif"), (ExportFormat::Png, "png"), (ExportFormat::Jpeg, "jpg")] {
			let path = dir().join(format!("icc.{ext}"));
			let options = ExportOptions {
				format,
				bits: 8,
				alpha: false,
				ppi: 72.0,
				quality: 90,
				chroma: JpegChroma::Full,
				icc: Some(icc.clone()),
				cmyk: None,
			};
			export_image(&path, W, H, options, &mut renderer(), &mut |_| true).unwrap();
			let imported = import_file(&path, &store(), &mut |_| true).unwrap();
			assert_eq!(imported.profile, fx_core::ColorProfile::Icc(icc.clone()), "{format:?}");
		}
	}

	#[test]
	fn cmyk_tiff_is_separated_with_the_callers_inks_and_profile() {
		let path = dir().join("cmyk.tif");
		let icc: Arc<[u8]> = Arc::from(vec![9u8; 300]);
		let options = ExportOptions {
			format: ExportFormat::Tiff,
			bits: 16,
			alpha: false,
			ppi: 72.0,
			quality: 90,
			chroma: JpegChroma::Full,
			icc: None,
			cmyk: Some(CmykExport {
				// A stand-in conversion: C, M, Y = 1 − R, G, B; K = 0.
				convert: Arc::new(|rgb: &[[u16; 3]], out: &mut [[u16; 4]]| {
					for (o, p) in out.iter_mut().zip(rgb) {
						*o = [65535 - p[0], 65535 - p[1], 65535 - p[2], 0];
					}
				}),
				icc: icc.clone(),
			}),
		};
		export_image(&path, W, H, options, &mut renderer(), &mut |_| true).unwrap();
		let mut decoder = ::tiff::decoder::Decoder::new(std::io::BufReader::new(File::open(&path).unwrap())).unwrap();
		assert_eq!(decoder.colortype().unwrap(), ::tiff::ColorType::CMYK(16));
		assert_eq!(decoder.get_tag_u32(::tiff::tags::Tag::PhotometricInterpretation).unwrap(), 5);
		assert_eq!(decoder.get_tag_u8_vec(::tiff::tags::Tag::IccProfile).unwrap(), icc.to_vec());
		let ::tiff::decoder::DecodingResult::U16(data) = decoder.read_image().unwrap() else {
			panic!("16-bit")
		};
		// Pixel (25, 3): opaque, so no white mixed in.
		let [r, g, b, a] = source(25, 3);
		assert_eq!(a, 65535 - 300, "the test pixel is almost opaque");
		let i = (3 * W as usize + 25) * 4;
		let expected = [r, g, b].map(|c| 65535 - over_white(c, a));
		assert_eq!(&data[i..i + 3], &expected, "inks of pixel (25, 3)");
		assert_eq!(data[i + 3], 0);
	}

	#[test]
	fn cmyk_is_refused_for_png() {
		let options = ExportOptions {
			format: ExportFormat::Png,
			bits: 8,
			alpha: false,
			ppi: 72.0,
			quality: 90,
			chroma: JpegChroma::Full,
			icc: None,
			cmyk: Some(CmykExport {
				convert: Arc::new(|_: &[[u16; 3]], _: &mut [[u16; 4]]| {}),
				icc: Arc::from(vec![0u8; 4]),
			}),
		};
		let result = export_image(&dir().join("no.png"), W, H, options, &mut renderer(), &mut |_| true);
		assert!(matches!(result, Err(IoError::Unsupported(_))));
	}
}
