//! The Art History Brush (M8-T07): every dab spawns short strokes around it,
//! coloured from the source state at their start, shaped by the Style.
//! (The History Brush itself is a clone of the state at offset 0.)
//!
//! The randomness is seeded by the stroke's seed and advanced in dab order,
//! so live = replay. VERIFY: Photoshop's stroke shapes and counts.

use fx_core::stroke::ArtStyle;

use crate::brush::op::{DabContext, DabSequence};
use crate::brush::path::{Dab, Jitter};

pub struct ArtHistory {
	pub style: ArtStyle,
	pub area: f64,
	pub tolerance: f64,
	rng: Jitter,
}

impl ArtHistory {
	pub fn new(style: ArtStyle, area: f64, tolerance: f64, seed: u64) -> Self {
		Self {
			style,
			area,
			tolerance,
			rng: Jitter::new(seed ^ 0xa87_415),
		}
	}

	/// (strokes per dab, length in brush radii, curl in radians per step,
	/// spread factor: loose strokes wander).
	fn shape(&self) -> (usize, f64, f64, f64) {
		match self.style {
			ArtStyle::TightShort => (6, 0.8, 0.0, 0.3),
			ArtStyle::TightMedium => (5, 1.6, 0.0, 0.3),
			ArtStyle::TightLong => (4, 3.0, 0.0, 0.3),
			ArtStyle::LooseMedium => (5, 1.6, 0.05, 1.0),
			ArtStyle::LooseLong => (4, 3.0, 0.05, 1.0),
			ArtStyle::Dab => (8, 0.15, 0.0, 0.5),
			ArtStyle::TightCurl => (5, 1.2, 0.35, 0.3),
			ArtStyle::TightCurlLong => (4, 2.5, 0.35, 0.3),
			ArtStyle::LooseCurl => (5, 1.2, 0.35, 1.0),
			ArtStyle::LooseCurlLong => (4, 2.5, 0.35, 1.0),
		}
	}
}

impl DabSequence for ArtHistory {
	fn dab(&mut self, dab: &Dab, rect: [i64; 4], pixels: &mut [[f64; 4]], coverage: &[f32], source: &dyn Fn(i64, i64) -> [f32; 4], _ctx: &DabContext) {
		let w = rect[2] - rect[0] + 1;
		let h = rect[3] - rect[1] + 1;
		let radius = f64::from(dab.diameter) / 2.0;
		let pen = (radius / 4.0).max(1.0);
		let (count, length, curl, spread) = self.shape();
		let reach = (self.area / 2.0).min(radius).max(1.0);
		for _ in 0..count {
			let a = f64::from(self.rng.next_unit()) * std::f64::consts::TAU;
			let r = f64::from(self.rng.next_unit()).sqrt() * reach;
			let (mut x, mut y) = (dab.x + a.cos() * r, dab.y + a.sin() * r);
			let s = source(x.floor() as i64, y.floor() as i64);
			let sa = f64::from(s[3]);
			if sa <= 0.0 {
				continue;
			}
			let color = [f64::from(s[0]) / sa, f64::from(s[1]) / sa, f64::from(s[2]) / sa];
			// Tolerance: leave pixels already close to the state alone.
			let (ix, iy) = (x.floor() as i64 - rect[0], y.floor() as i64 - rect[1]);
			if ix >= 0 && iy >= 0 && ix < w && iy < h {
				let p = pixels[(iy * w + ix) as usize];
				let pa = p[3].max(1e-9);
				let d = (0..3).map(|c| (p[c] / pa - color[c]).abs()).fold(0.0, f64::max);
				if d < self.tolerance {
					continue;
				}
			}
			let mut heading = f64::from(self.rng.next_unit()) * std::f64::consts::TAU;
			let steps = ((length * radius) / pen).ceil().max(1.0) as usize;
			for _ in 0..steps {
				let x0 = ((x - pen).floor() as i64 - rect[0]).max(0);
				let y0 = ((y - pen).floor() as i64 - rect[1]).max(0);
				let x1 = ((x + pen).ceil() as i64 - rect[0]).min(w - 1);
				let y1 = ((y + pen).ceil() as i64 - rect[1]).min(h - 1);
				for py in y0..=y1 {
					for px in x0..=x1 {
						let i = (py * w + px) as usize;
						let k = f64::from(coverage[i]).min(1.0);
						if k <= 0.0 {
							continue;
						}
						let (cx, cy) = ((rect[0] + px) as f64 + 0.5 - x, (rect[1] + py) as f64 + 0.5 - y);
						let t = (pen - (cx * cx + cy * cy).sqrt() + 0.5).clamp(0.0, 1.0) * k;
						if t <= 0.0 {
							continue;
						}
						let p = &mut pixels[i];
						let target = [color[0], color[1], color[2], 1.0];
						for c in 0..4 {
							p[c] += (target[c] - p[c]) * t;
						}
					}
				}
				heading += curl + f64::from(self.rng.signed()) * 0.3 * spread;
				x += heading.cos() * pen;
				y += heading.sin() * pen;
			}
		}
	}
}
