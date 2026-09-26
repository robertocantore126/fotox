//! The Mixer Brush (M8-T09, D-066 — an **approximation**, Photoshop's model
//! is not documented).
//!
//! * The reservoir holds the loaded colour (the foreground) and an amount
//!   (Load) that each dab spends.
//! * The pickup is the paint the brush carries from the canvas: the previous
//!   dab's result, moved to the new position (like the Smudge), mixed with
//!   the canvas under the brush by Wet.
//! * Each dab lays down `mix(reservoir, pickup, Mix)` — the reservoir only
//!   while it is loaded — by the dab's coverage (Flow).
//!
//! FAST: the brush is loaded and cleaned at every stroke (the menu's "Load /
//! Clean after each stroke" on); a dry, unloaded brush paints nothing.

use crate::brush::op::{DabContext, DabSequence};
use crate::brush::path::Dab;

pub struct Mixer {
	/// `0..=1`.
	pub wet: f64,
	pub load: f64,
	pub mix: f64,
	/// What is left in the reservoir, `0..=1`.
	left: f64,
	previous: Option<((f64, f64), [i64; 4], Vec<[f64; 4]>)>,
}

impl Mixer {
	pub fn new(wet: f64, load: f64, mix: f64) -> Self {
		Self {
			wet: wet.clamp(0.0, 1.0),
			load: load.clamp(0.0, 1.0),
			mix: mix.clamp(0.0, 1.0),
			left: if load > 0.0 { 1.0 } else { 0.0 },
			previous: None,
		}
	}
}

impl DabSequence for Mixer {
	fn dab(&mut self, dab: &Dab, rect: [i64; 4], pixels: &mut [[f64; 4]], coverage: &[f32], _source: &dyn Fn(i64, i64) -> [f32; 4], ctx: &DabContext) {
		let w = (rect[2] - rect[0] + 1) as usize;
		let reservoir = [ctx.color[0], ctx.color[1], ctx.color[2], 1.0];
		let under = pixels.to_vec();
		for (i, (p, k)) in pixels.iter_mut().zip(coverage).enumerate() {
			let k = f64::from(*k);
			if k <= 0.0 {
				continue;
			}
			// What the brush carries here: the previous dab's paint, or the
			// canvas when there is none.
			let carried = match &self.previous {
				Some(((px, py), prect, carried)) => {
					let (dx, dy) = ((dab.x - px).round() as i64, (dab.y - py).round() as i64);
					let (x, y) = (rect[0] + (i % w) as i64 - dx, rect[1] + (i / w) as i64 - dy);
					if x < prect[0] || y < prect[1] || x > prect[2] || y > prect[3] {
						under[i]
					} else {
						let pw = (prect[2] - prect[0] + 1) as usize;
						carried[(y - prect[1]) as usize * pw + (x - prect[0]) as usize]
					}
				}
				None => under[i],
			};
			// Wet: how much of the canvas the carried paint takes up.
			let pickup: [f64; 4] = std::array::from_fn(|c| carried[c] + (under[i][c] - carried[c]) * self.wet);
			// Mix: canvas (pickup) vs reservoir, the reservoir only while loaded.
			let reservoir_share = (1.0 - self.mix) * self.left;
			let pickup_share = self.mix * self.wet.max(if self.left > 0.0 { 0.0 } else { 1.0 });
			let total = reservoir_share + pickup_share;
			if total <= 0.0 {
				continue;
			}
			let paint: [f64; 4] = std::array::from_fn(|c| (reservoir[c] * reservoir_share + pickup[c] * pickup_share) / total);
			for c in 0..4 {
				p[c] += (paint[c] - p[c]) * k;
			}
		}
		// The reservoir runs dry: Load is how many dabs it lasts (VERIFY).
		if self.load < 1.0 {
			self.left = (self.left - (1.0 - self.load) * 0.02).max(0.0);
		}
		self.previous = Some(((dab.x, dab.y), rect, pixels.to_vec()));
	}
}
