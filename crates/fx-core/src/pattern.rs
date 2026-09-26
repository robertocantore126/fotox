//! Patterns (M8-T06): a small RGBA image tiled over the canvas, kept in the
//! document's resources (saved in `.fxd`) and in the user's library.
//!
//! A pattern is capped at [`MAX_SIDE`] per side, so it is never a buffer
//! proportional to the document.

use std::sync::Arc;

/// The largest pattern side (Edit ▸ Define Pattern refuses bigger).
pub const MAX_SIDE: u32 = 2048;

/// What `Command::DefinePattern` carries.
pub type PatternData = Pattern;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pattern {
	/// A hash of the pixels (52 bits, never 0), so the same image is one pattern.
	pub id: u64,
	pub name: String,
	pub width: u32,
	pub height: u32,
	/// Straight 16-bit RGBA, row-major.
	pub pixels: Arc<Vec<[u16; 4]>>,
}

impl Pattern {
	pub fn new(name: impl Into<String>, width: u32, height: u32, pixels: Vec<[u16; 4]>) -> Self {
		let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
		for v in width.to_le_bytes().into_iter().chain(height.to_le_bytes()) {
			hash ^= u64::from(v);
			hash = hash.wrapping_mul(0x0100_0000_01b3);
		}
		for p in &pixels {
			for c in p {
				hash ^= u64::from(*c);
				hash = hash.wrapping_mul(0x0100_0000_01b3);
			}
		}
		Self {
			id: (hash & ((1u64 << 52) - 1)) | 1,
			name: name.into(),
			width: width.max(1),
			height: height.max(1),
			pixels: Arc::new(pixels),
		}
	}

	/// The straight RGBA (`0..=1`) at canvas pixel `(x, y)`, the pattern tiled
	/// from the canvas origin.
	pub fn at(&self, x: i64, y: i64) -> [f64; 4] {
		let px = x.rem_euclid(i64::from(self.width)) as usize;
		let py = y.rem_euclid(i64::from(self.height)) as usize;
		let p = self.pixels.get(py * self.width as usize + px).copied().unwrap_or([0; 4]);
		p.map(|c| f64::from(c) / 65535.0)
	}

	/// Bilinear sample at a fractional pattern position (scale / angle of a
	/// pattern fill layer, T06).
	pub fn sample(&self, x: f64, y: f64) -> [f64; 4] {
		let (x0, y0) = ((x - 0.5).floor(), (y - 0.5).floor());
		let (fx, fy) = (x - 0.5 - x0, y - 0.5 - y0);
		let (a, b) = (self.at(x0 as i64, y0 as i64), self.at(x0 as i64 + 1, y0 as i64));
		let (c, d) = (self.at(x0 as i64, y0 as i64 + 1), self.at(x0 as i64 + 1, y0 as i64 + 1));
		// Premultiplied interpolation.
		let mut out = [0.0; 4];
		let w = [(1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy];
		for (p, w) in [a, b, c, d].iter().zip(w) {
			for i in 0..3 {
				out[i] += p[i] * p[3] * w;
			}
			out[3] += p[3] * w;
		}
		let alpha = out[3];
		if alpha > 0.0 {
			for v in out.iter_mut().take(3) {
				*v /= alpha;
			}
		}
		out
	}
}
