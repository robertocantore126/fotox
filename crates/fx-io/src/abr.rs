//! Photoshop brush files (`.abr`, M8-T01, D-063): the **sampled tips** of
//! version 6 and later (Photoshop 7 onward).
//!
//! Layout (big-endian): `u16 version` (6..=10), `u16 subversion` (1 or 2),
//! then `8BIM` sections of `key, u32 size`. The `samp` section is a list of
//! brushes: `u32 length` (padded to 4), an id, then the tip's bounds
//! (`top, left, bottom, right`), `u16 depth`, `u8 compression` and the grey
//! pixels, raw or PackBits with a `u16` byte count per row. The skip before
//! the bounds is 47 bytes for subversion 1 and 301 for subversion 2 (what
//! GIMP's loader does).
//!
//! FAST: the `desc` section (names, spacing, dynamics) is not read; tips
//! are named "Sampled Brush N" and get 25 % spacing.

use crate::IoError;

/// One sampled tip of an `.abr` file.
#[derive(Clone, Debug)]
pub struct AbrTip {
	pub name: String,
	pub width: u32,
	pub height: u32,
	/// 8-bit grey, 255 = paint.
	pub gray: Vec<u8>,
	/// Spacing as a fraction of the diameter.
	pub spacing: f32,
}

struct Reader<'a> {
	data: &'a [u8],
	at: usize,
}

impl<'a> Reader<'a> {
	fn take(&mut self, n: usize) -> Result<&'a [u8], IoError> {
		let end = self
			.at
			.checked_add(n)
			.filter(|&e| e <= self.data.len())
			.ok_or_else(|| IoError::Decode("the brush file ends early".into()))?;
		let s = &self.data[self.at..end];
		self.at = end;
		Ok(s)
	}
	fn u8(&mut self) -> Result<u8, IoError> {
		Ok(self.take(1)?[0])
	}
	fn u16(&mut self) -> Result<u16, IoError> {
		let b = self.take(2)?;
		Ok(u16::from_be_bytes([b[0], b[1]]))
	}
	fn u32(&mut self) -> Result<u32, IoError> {
		let b = self.take(4)?;
		Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
	}
	fn i32(&mut self) -> Result<i32, IoError> {
		Ok(self.u32()? as i32)
	}
}

/// The sampled tips of an `.abr` file.
pub fn read_abr(data: &[u8]) -> Result<Vec<AbrTip>, IoError> {
	let mut r = Reader { data, at: 0 };
	let version = r.u16()?;
	if !(6..=10).contains(&version) {
		return Err(IoError::Unsupported(format!("brush file version {version} (only 6 and later are read)")));
	}
	let subversion = r.u16()?;
	let skip = if subversion == 1 { 47 } else { 301 };
	let mut tips = Vec::new();
	while r.at + 12 <= data.len() {
		let tag = r.take(4)?;
		if tag != b"8BIM" {
			break;
		}
		let key = r.take(4)?;
		let size = r.u32()? as usize;
		let end = r.at.saturating_add(size).min(data.len());
		if key != b"samp" {
			r.at = end;
			continue;
		}
		while r.at + 4 <= end {
			let length = r.u32()? as usize;
			let padded = length.div_ceil(4) * 4;
			let next = r.at + padded;
			if let Ok(tip) = read_tip(&mut r, skip, tips.len() + 1) {
				tips.push(tip);
			}
			r.at = next;
		}
		r.at = end;
	}
	Ok(tips)
}

fn read_tip(r: &mut Reader<'_>, skip: usize, n: usize) -> Result<AbrTip, IoError> {
	r.take(skip)?;
	let top = r.i32()?;
	let left = r.i32()?;
	let bottom = r.i32()?;
	let right = r.i32()?;
	let depth = r.u16()?;
	let compression = r.u8()?;
	let (width, height) = ((right - left).max(0) as u32, (bottom - top).max(0) as u32);
	if width == 0 || height == 0 || width > 5000 || height > 5000 {
		return Err(IoError::Decode("a brush tip has no size".into()));
	}
	let bytes = usize::from(depth / 8).max(1);
	let row = width as usize * bytes;
	let mut raw = Vec::with_capacity(row * height as usize);
	if compression == 0 {
		raw.extend_from_slice(r.take(row * height as usize)?);
	} else {
		let mut counts = Vec::with_capacity(height as usize);
		for _ in 0..height {
			counts.push(usize::from(r.u16()?));
		}
		for count in counts {
			let packed = r.take(count)?;
			let mut out = unpack_bits(packed);
			out.resize(row, 0);
			raw.extend_from_slice(&out);
		}
	}
	// 16-bit tips: the high byte.
	let gray = if bytes == 2 { raw.chunks_exact(2).map(|c| c[0]).collect() } else { raw };
	Ok(AbrTip {
		name: format!("Sampled Brush {n}"),
		width,
		height,
		gray,
		spacing: 0.25,
	})
}

/// PackBits decoding.
fn unpack_bits(packed: &[u8]) -> Vec<u8> {
	let mut out = Vec::new();
	let mut i = 0;
	while i < packed.len() {
		let n = packed[i] as i8;
		i += 1;
		if n >= 0 {
			let count = n as usize + 1;
			let end = (i + count).min(packed.len());
			out.extend_from_slice(&packed[i..end]);
			i = end;
		} else if n != -128 {
			let count = (1 - i32::from(n)) as usize;
			if i < packed.len() {
				out.extend(std::iter::repeat_n(packed[i], count));
			}
			i += 1;
		}
	}
	out
}
