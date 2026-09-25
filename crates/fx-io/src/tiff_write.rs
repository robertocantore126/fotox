//! A minimal streaming TIFF writer: RGB or RGBA (unassociated alpha), 8 or 16
//! bits per sample, uncompressed strips, little-endian, classic TIFF or BigTIFF.
//!
//! Written by hand for `fotox-cli gen` (M1-T01), used by export too (M3):
//! strips are written as they are produced, so a 5.4 GB BigTIFF never has to
//! exist in memory, and every byte of the output is determined by the pixels
//! (no timestamps, no software tag), so two runs with the same seed give
//! identical files.
//!
//! Layout: header (IFD offset patched at the end) → strip data → IFD with its
//! out-of-line arrays.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::IoError;

type Result<T> = std::result::Result<T, IoError>;

macro_rules! ensure {
	($cond:expr, $($fmt:tt)+) => {
		if !$cond {
			return Err(IoError::Unsupported(format!($($fmt)+)));
		}
	};
}

/// Tag types.
const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;
const LONG8: u16 = 16;

/// Strips to write for an image; the writer checks every strip against it.
pub struct TiffWriter {
	out: BufWriter<File>,
	big: bool,
	width: u32,
	height: u32,
	bits: u16,
	/// 3 (RGB) or 4 (RGBA, unassociated alpha).
	samples: u16,
	rows_per_strip: u32,
	offsets: Vec<u64>,
	counts: Vec<u64>,
	position: u64,
}

impl TiffWriter {
	/// Create `path` for a `width × height` RGB image. BigTIFF is chosen when
	/// the file cannot stay below 4 GiB.
	pub fn create(path: &Path, width: u32, height: u32, bits: u16, rows_per_strip: u32) -> Result<Self> {
		Self::create_as(path, width, height, bits, rows_per_strip, None)
	}

	/// [`create`](Self::create) with the classic/BigTIFF choice forced
	/// (`Some`) or automatic (`None`). Forcing BigTIFF lets tests cover it
	/// without writing 4 GiB.
	pub fn create_as(path: &Path, width: u32, height: u32, bits: u16, rows_per_strip: u32, big: Option<bool>) -> Result<Self> {
		Self::create_with(path, width, height, bits, 3, rows_per_strip, big)
	}

	/// [`create_as`](Self::create_as) with the number of samples per pixel:
	/// 3 (RGB) or 4 (RGBA, unassociated alpha).
	pub fn create_with(path: &Path, width: u32, height: u32, bits: u16, samples: u16, rows_per_strip: u32, big: Option<bool>) -> Result<Self> {
		ensure!(bits == 8 || bits == 16, "bits must be 8 or 16, got {bits}");
		ensure!(samples == 3 || samples == 4, "samples must be 3 or 4, got {samples}");
		ensure!(width > 0 && height > 0 && rows_per_strip > 0, "empty image");
		let data = u64::from(width) * u64::from(height) * u64::from(samples) * u64::from(bits / 8);
		// Headroom for the IFD and strip tables.
		let needs_big = data + (1 << 20) >= u64::from(u32::MAX);
		ensure!(big != Some(false) || !needs_big, "{width} × {height} does not fit a classic TIFF");
		let big = big.unwrap_or(needs_big);
		let file = File::create(path)?;
		let mut out = BufWriter::with_capacity(8 << 20, file);
		let header_len = if big {
			// "II", 43, offset size 8, reserved 0, first IFD offset (patched later)
			out.write_all(b"II")?;
			out.write_all(&43u16.to_le_bytes())?;
			out.write_all(&8u16.to_le_bytes())?;
			out.write_all(&0u16.to_le_bytes())?;
			out.write_all(&0u64.to_le_bytes())?;
			16
		} else {
			out.write_all(b"II")?;
			out.write_all(&42u16.to_le_bytes())?;
			out.write_all(&0u32.to_le_bytes())?;
			8
		};
		Ok(Self {
			out,
			big,
			width,
			height,
			bits,
			samples,
			rows_per_strip,
			offsets: Vec::new(),
			counts: Vec::new(),
			position: header_len,
		})
	}

	/// Whether the file is a BigTIFF.
	pub fn is_big(&self) -> bool {
		self.big
	}

	/// Number of strips the image is split into.
	pub fn strip_count(&self) -> u32 {
		self.height.div_ceil(self.rows_per_strip)
	}

	/// Bytes of one full strip.
	pub fn strip_bytes(&self, strip: u32) -> usize {
		let rows = self.rows_per_strip.min(self.height - strip * self.rows_per_strip);
		self.width as usize * rows as usize * usize::from(self.samples) * usize::from(self.bits / 8)
	}

	/// Append the next strip (samples interleaved RGB or RGBA, little-endian 16-bit).
	pub fn write_strip(&mut self, data: &[u8]) -> Result<()> {
		let strip = self.offsets.len() as u32;
		ensure!(strip < self.strip_count(), "more strips than the image has");
		ensure!(
			data.len() == self.strip_bytes(strip),
			"strip {strip} has {} bytes, expected {}",
			data.len(),
			self.strip_bytes(strip)
		);
		self.out.write_all(data)?;
		self.offsets.push(self.position);
		self.counts.push(data.len() as u64);
		self.position += data.len() as u64;
		Ok(())
	}

	/// Write the IFD, patch the header and flush. Every strip must be written.
	pub fn finish(mut self) -> Result<u64> {
		ensure!(
			self.offsets.len() as u32 == self.strip_count(),
			"only {} of {} strips written",
			self.offsets.len(),
			self.strip_count()
		);
		// Word-align the IFD.
		if self.position % 2 == 1 {
			self.out.write_all(&[0])?;
			self.position += 1;
		}
		let ifd_offset = self.position;
		let bits = vec![self.bits; usize::from(self.samples)];

		// Out-of-line data goes after the IFD; compute where.
		let alpha = self.samples == 4;
		let entries: u64 = if alpha { 14 } else { 13 };
		let (entry_size, count_size, next_size) = if self.big { (20u64, 8u64, 8u64) } else { (12, 2, 4) };
		let mut extra = ifd_offset + count_size + entries * entry_size + next_size;
		let inline_limit = if self.big { 8 } else { 4 };

		let off_type = if self.big { LONG8 } else { LONG };
		let off_width = if self.big { 8 } else { 4 };
		let n_strips = self.offsets.len() as u64;

		// (tag, type, count, inline bytes or out-of-line bytes)
		let mut tags: Vec<(u16, u16, u64, Vec<u8>)> = vec![
			(256, LONG, 1, self.width.to_le_bytes().to_vec()),
			(257, LONG, 1, self.height.to_le_bytes().to_vec()),
			(258, SHORT, u64::from(self.samples), bits.iter().flat_map(|b| b.to_le_bytes()).collect()),
			(259, SHORT, 1, 1u16.to_le_bytes().to_vec()), // no compression
			(262, SHORT, 1, 2u16.to_le_bytes().to_vec()), // RGB
			(273, off_type, n_strips, pack(&self.offsets, off_width)),
			(277, SHORT, 1, self.samples.to_le_bytes().to_vec()),
			(278, LONG, 1, self.rows_per_strip.to_le_bytes().to_vec()),
			(279, off_type, n_strips, pack(&self.counts, off_width)),
			(282, RATIONAL, 1, [72u32.to_le_bytes(), 1u32.to_le_bytes()].concat()),
			(283, RATIONAL, 1, [72u32.to_le_bytes(), 1u32.to_le_bytes()].concat()),
			(284, SHORT, 1, 1u16.to_le_bytes().to_vec()), // chunky
			(296, SHORT, 1, 2u16.to_le_bytes().to_vec()), // inch
		];
		if alpha {
			tags.push((338, SHORT, 1, 2u16.to_le_bytes().to_vec())); // unassociated alpha
		}
		debug_assert_eq!(tags.len() as u64, entries);
		tags.sort_by_key(|t| t.0);

		let mut ifd = Vec::new();
		let mut tail = Vec::new();
		if self.big {
			ifd.extend_from_slice(&entries.to_le_bytes());
		} else {
			ifd.extend_from_slice(&(entries as u16).to_le_bytes());
		}
		for (tag, kind, count, bytes) in &tags {
			ifd.extend_from_slice(&tag.to_le_bytes());
			ifd.extend_from_slice(&kind.to_le_bytes());
			if self.big {
				ifd.extend_from_slice(&count.to_le_bytes());
			} else {
				ifd.extend_from_slice(&(*count as u32).to_le_bytes());
			}
			if bytes.len() <= inline_limit {
				let mut value = bytes.clone();
				value.resize(inline_limit, 0);
				ifd.extend_from_slice(&value);
			} else {
				if self.big {
					ifd.extend_from_slice(&extra.to_le_bytes());
				} else {
					ifd.extend_from_slice(&(extra as u32).to_le_bytes());
				}
				tail.extend_from_slice(bytes);
				extra += bytes.len() as u64;
				if extra % 2 == 1 {
					tail.push(0);
					extra += 1;
				}
			}
		}
		if self.big {
			ifd.extend_from_slice(&0u64.to_le_bytes());
		} else {
			ifd.extend_from_slice(&0u32.to_le_bytes());
		}
		self.out.write_all(&ifd)?;
		self.out.write_all(&tail)?;
		let total = ifd_offset + ifd.len() as u64 + tail.len() as u64;

		// Patch the first-IFD offset in the header.
		self.out.flush()?;
		let file = self.out.get_mut();
		if self.big {
			file.seek(SeekFrom::Start(8))?;
			file.write_all(&ifd_offset.to_le_bytes())?;
		} else {
			ensure!(ifd_offset <= u64::from(u32::MAX), "classic TIFF offset overflow");
			file.seek(SeekFrom::Start(4))?;
			file.write_all(&(ifd_offset as u32).to_le_bytes())?;
		}
		file.flush()?;
		Ok(total)
	}
}

fn pack(values: &[u64], width: usize) -> Vec<u8> {
	let mut out = Vec::with_capacity(values.len() * width);
	for v in values {
		if width == 8 {
			out.extend_from_slice(&v.to_le_bytes());
		} else {
			out.extend_from_slice(&(*v as u32).to_le_bytes());
		}
	}
	out
}
