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

use crate::tiff_write::TiffWriter;
use crate::{IoError, Progress};

/// Rows per band (and per TIFF strip).
pub const EXPORT_BAND_ROWS: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
	Tiff,
	Png,
}

impl ExportFormat {
	/// The format for a file name's extension (case-insensitive), if Fotox can write it.
	pub fn from_path(path: &Path) -> Option<Self> {
		let ext = path.extension()?.to_str()?.to_ascii_lowercase();
		match ext.as_str() {
			"tif" | "tiff" => Some(Self::Tiff),
			"png" => Some(Self::Png),
			_ => None,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportOptions {
	pub format: ExportFormat,
	/// 8 or 16 bits per channel.
	pub bits: u16,
	/// Keep transparency. Without it, the image is flattened onto white, like
	/// Photoshop's Flatten Image with a white background colour.
	pub alpha: bool,
	/// Pixels per inch written in the file (TIFF resolution, PNG `pHYs`).
	pub ppi: f32,
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
	let part = part_path(path);
	let result = write(&part, width, height, options, render, progress);
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

fn write(part: &Path, width: u32, height: u32, options: ExportOptions, render: BandRenderer<'_>, progress: Progress<'_>) -> Result<(), IoError> {
	let samples: usize = if options.alpha { 4 } else { 3 };
	let mut band = vec![[0u16; 4]; width as usize * EXPORT_BAND_ROWS as usize];
	let mut bytes = Vec::new();
	let mut sink: Box<dyn Sink> = match options.format {
		ExportFormat::Tiff => {
			let mut writer = TiffWriter::create_with(part, width, height, options.bits, samples as u16, EXPORT_BAND_ROWS, None)?;
			writer.set_ppi(options.ppi);
			Box::new(writer)
		}
		ExportFormat::Png => Box::new(PngSink::create(part, width, height, options)?),
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

/// Straight RGBA16 → the file's samples (RGB or RGBA, 8 or 16 bits).
fn encode(pixels: &[[u16; 4]], options: ExportOptions, big_endian: bool, out: &mut Vec<u8>) {
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
	fn create(path: &Path, width: u32, height: u32, options: ExportOptions) -> Result<Self, IoError> {
		let file = BufWriter::with_capacity(8 << 20, File::create(path)?);
		let mut encoder = ::png::Encoder::new(file, width, height);
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
	fn expected(options: ExportOptions, x: u32, y: u32) -> [u16; 4] {
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
		};
		let ext = if format == ExportFormat::Tiff { "tif" } else { "png" };
		let path = dir().join(format!("out-{bits}-{alpha}.{ext}"));
		let mut calls = 0;
		export_image(&path, W, H, options, &mut renderer(), &mut |_| {
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
			let (want, got) = (expected(options, x, y), pixel(&imported.image, &store, x, y));
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
		};
		let result = export_image(&path, W, H, options, &mut renderer(), &mut |_| false);
		assert!(matches!(result, Err(IoError::Cancelled)));
		assert_eq!(std::fs::read(&path).unwrap(), b"old");
		assert!(!part_path(&path).exists());
	}

	#[test]
	fn format_from_extension() {
		assert_eq!(ExportFormat::from_path(Path::new("a/b.TIFF")), Some(ExportFormat::Tiff));
		assert_eq!(ExportFormat::from_path(Path::new("b.tif")), Some(ExportFormat::Tiff));
		assert_eq!(ExportFormat::from_path(Path::new("b.png")), Some(ExportFormat::Png));
		assert_eq!(ExportFormat::from_path(Path::new("b.jpg")), None);
		assert_eq!(ExportFormat::from_path(Path::new("b")), None);
	}
}
