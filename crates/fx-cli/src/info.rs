//! `fotox-cli info`: what a TIFF file contains, read from its header and
//! first IFD only (no pixel data), so it is instant even for a 5 GB file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

/// How the pixel data is laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
	Strips { rows_per_strip: u32, count: u64 },
	Tiles { width: u32, height: u32, count: u64 },
}

/// The facts `info` prints.
#[derive(Clone, Debug)]
pub struct Info {
	pub file_size: u64,
	pub big: bool,
	pub little_endian: bool,
	pub width: u32,
	pub height: u32,
	pub bits: u16,
	pub channels: u16,
	pub compression: u16,
	pub photometric: u16,
	/// 0 none, 1 associated alpha, 2 unassociated alpha (ExtraSamples).
	pub extra_samples: Vec<u16>,
	pub layout: Layout,
	pub icc: Option<u64>,
}

/// Read the header and first IFD of `path`.
pub fn read(path: &Path) -> Result<Info> {
	let mut file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
	let file_size = file.metadata()?.len();
	let mut header = [0u8; 16];
	file.read_exact(&mut header[..8]).context("file too short for a TIFF header")?;
	let little_endian = match &header[..2] {
		b"II" => true,
		b"MM" => false,
		_ => bail!("not a TIFF file (no II/MM byte order mark)"),
	};
	let r = Reader { little_endian };
	let (big, ifd) = match r.u16(&header[2..4]) {
		42 => (false, u64::from(r.u32(&header[4..8]))),
		43 => {
			file.read_exact(&mut header[8..16])?;
			ensure!(r.u16(&header[4..6]) == 8, "BigTIFF with an offset size other than 8");
			(true, r.u64(&header[8..16]))
		}
		other => bail!("not a TIFF file (version {other})"),
	};

	file.seek(SeekFrom::Start(ifd))?;
	let count = if big {
		let mut b = [0u8; 8];
		file.read_exact(&mut b)?;
		r.u64(&b)
	} else {
		let mut b = [0u8; 2];
		file.read_exact(&mut b)?;
		u64::from(r.u16(&b))
	};
	ensure!(count < 10_000, "implausible IFD entry count {count}");
	let entry_size = if big { 20 } else { 12 };
	let mut entries = vec![0u8; count as usize * entry_size];
	file.read_exact(&mut entries)?;

	let mut info = Info {
		file_size,
		big,
		little_endian,
		width: 0,
		height: 0,
		bits: 1,
		channels: 1,
		compression: 1,
		photometric: 0,
		extra_samples: Vec::new(),
		layout: Layout::Strips { rows_per_strip: 0, count: 0 },
		icc: None,
	};
	let (mut rows_per_strip, mut strip_count) = (None, 0u64);
	let (mut tile_w, mut tile_h, mut tile_count) = (None, None, 0u64);

	for e in entries.chunks_exact(entry_size) {
		let tag = r.u16(&e[0..2]);
		let kind = r.u16(&e[2..4]);
		let (n, value) = if big {
			(r.u64(&e[4..12]), &e[12..20])
		} else {
			(u64::from(r.u32(&e[4..8])), &e[8..12])
		};
		// First value of the entry (only used for scalar-ish tags).
		let first = || -> Result<u64> {
			let size = type_size(kind)?;
			let inline = if big { 8 } else { 4 };
			let bytes: Vec<u8> = if size * n.max(1) as usize <= inline {
				value[..size].to_vec()
			} else {
				let offset = if big { r.u64(value) } else { u64::from(r.u32(value)) };
				let mut f = File::open(path)?;
				f.seek(SeekFrom::Start(offset))?;
				let mut b = vec![0u8; size];
				f.read_exact(&mut b)?;
				b
			};
			Ok(match kind {
				1 | 7 => u64::from(bytes[0]),
				3 => u64::from(r.u16(&bytes)),
				4 => u64::from(r.u32(&bytes)),
				16 => r.u64(&bytes),
				_ => 0,
			})
		};
		match tag {
			256 => info.width = first()? as u32,
			257 => info.height = first()? as u32,
			258 => info.bits = first()? as u16,
			259 => info.compression = first()? as u16,
			262 => info.photometric = first()? as u16,
			273 => strip_count = n,
			277 => info.channels = first()? as u16,
			278 => rows_per_strip = Some(first()? as u32),
			322 => tile_w = Some(first()? as u32),
			323 => tile_h = Some(first()? as u32),
			324 => tile_count = n,
			338 => info.extra_samples = vec![first()? as u16; n as usize],
			34675 => info.icc = Some(n),
			_ => {}
		}
	}
	ensure!(info.width > 0 && info.height > 0, "TIFF without image dimensions");
	info.layout = match (tile_w, tile_h) {
		(Some(width), Some(height)) => Layout::Tiles {
			width,
			height,
			count: tile_count,
		},
		_ => Layout::Strips {
			rows_per_strip: rows_per_strip.unwrap_or(info.height).min(info.height),
			count: strip_count,
		},
	};
	Ok(info)
}

/// Print `info` the way `fotox-cli info` shows it.
pub fn print(path: &Path, info: &Info) {
	println!("{}", path.display());
	println!("  size         {} × {} px", info.width, info.height);
	println!("  samples      {} × {}-bit ({})", info.channels, info.bits, photometric_name(info.photometric));
	if !info.extra_samples.is_empty() {
		let alpha = match info.extra_samples[0] {
			1 => "associated alpha",
			2 => "unassociated alpha",
			_ => "unspecified extra sample",
		};
		println!("  extra        {alpha}");
	}
	println!("  compression  {}", compression_name(info.compression));
	match &info.layout {
		Layout::Strips { rows_per_strip, count } => println!("  layout       {count} strips of {rows_per_strip} rows"),
		Layout::Tiles { width, height, count } => println!("  layout       {count} tiles of {width} × {height}"),
	}
	println!("  BigTIFF      {}", if info.big { "yes" } else { "no" });
	println!("  byte order   {}", if info.little_endian { "little-endian (II)" } else { "big-endian (MM)" });
	match info.icc {
		Some(bytes) => println!("  ICC profile  yes ({bytes} bytes)"),
		None => println!("  ICC profile  no"),
	}
	println!("  file size    {:.2} GB ({} bytes)", info.file_size as f64 / 1e9, info.file_size);
}

fn compression_name(c: u16) -> String {
	match c {
		1 => "none".into(),
		5 => "LZW".into(),
		7 => "JPEG".into(),
		8 | 32946 => "Deflate".into(),
		32773 => "PackBits".into(),
		50000 => "Zstd".into(),
		other => format!("code {other}"),
	}
}

fn photometric_name(p: u16) -> &'static str {
	match p {
		0 => "gray, white is zero",
		1 => "gray",
		2 => "RGB",
		3 => "palette",
		5 => "CMYK",
		6 => "YCbCr",
		8 => "CIE L*a*b*",
		_ => "other",
	}
}

fn type_size(kind: u16) -> Result<usize> {
	Ok(match kind {
		1 | 2 | 6 | 7 => 1,
		3 | 8 => 2,
		4 | 9 | 11 | 13 => 4,
		5 | 10 | 12 | 16 | 17 | 18 => 8,
		other => bail!("unknown TIFF field type {other}"),
	})
}

struct Reader {
	little_endian: bool,
}

impl Reader {
	fn u16(&self, b: &[u8]) -> u16 {
		let a = [b[0], b[1]];
		if self.little_endian { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) }
	}
	fn u32(&self, b: &[u8]) -> u32 {
		let a = [b[0], b[1], b[2], b[3]];
		if self.little_endian { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
	}
	fn u64(&self, b: &[u8]) -> u64 {
		let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
		if self.little_endian { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) }
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_io::tiff_write::TiffWriter;

	#[test]
	fn reads_a_forced_bigtiff() {
		let dir = std::env::temp_dir().join("fx-cli-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("big-small.tif");
		let mut w = TiffWriter::create_as(&path, 10, 5, 8, 2, Some(true)).unwrap();
		for strip in 0..w.strip_count() {
			let bytes = vec![strip as u8; w.strip_bytes(strip)];
			w.write_strip(&bytes).unwrap();
		}
		w.finish().unwrap();
		let info = read(&path).unwrap();
		assert!(info.big);
		assert_eq!((info.width, info.height, info.bits, info.channels, info.photometric), (10, 5, 8, 3, 2));
		assert_eq!(info.layout, Layout::Strips { rows_per_strip: 2, count: 3 });
		// Pixel data of strip 1 sits where StripOffsets says (offset 16 + 60 bytes).
		let bytes = std::fs::read(&path).unwrap();
		assert_eq!(bytes[16 + 60], 1);
		assert_eq!(info.icc, None);
	}

	#[test]
	fn rejects_a_non_tiff() {
		let dir = std::env::temp_dir().join("fx-cli-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("not-a-tiff.bin");
		std::fs::write(&path, b"hello world, not a tiff").unwrap();
		assert!(read(&path).is_err());
	}
}
