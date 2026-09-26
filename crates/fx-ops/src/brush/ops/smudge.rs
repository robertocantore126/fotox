//! The Smudge tool (M8-T05): each dab lays down the previous dab's result,
//! shifted to where the brush is now, blended by Strength × tip coverage; the
//! result becomes what the next dab carries. Finger Painting starts the
//! stroke with the foreground colour.
//!
//! FAST: Sample All Layers is ignored (the layer's own pixels are smudged).
//! VERIFY: Photoshop's strength curve.

use crate::brush::op::{DabContext, DabSequence};
use crate::brush::path::Dab;

pub struct Smudge {
	pub finger: bool,
	/// The previous dab: centre, rectangle and resulting pixels.
	previous: Option<((f64, f64), [i64; 4], Vec<[f64; 4]>)>,
}

impl Smudge {
	pub fn new(finger: bool) -> Self {
		Self { finger, previous: None }
	}
}

impl DabSequence for Smudge {
	fn dab(&mut self, dab: &Dab, rect: [i64; 4], pixels: &mut [[f64; 4]], coverage: &[f32], _source: &dyn Fn(i64, i64) -> [f32; 4], ctx: &DabContext) {
		let w = (rect[2] - rect[0] + 1) as usize;
		match &self.previous {
			None => {
				if self.finger {
					let c = [ctx.color[0], ctx.color[1], ctx.color[2], 1.0];
					for (p, k) in pixels.iter_mut().zip(coverage) {
						let k = f64::from(*k);
						for i in 0..4 {
							p[i] += (c[i] - p[i]) * k;
						}
					}
				}
			}
			Some(((px, py), prect, carried)) => {
				let (dx, dy) = ((dab.x - px).round() as i64, (dab.y - py).round() as i64);
				let pw = (prect[2] - prect[0] + 1) as usize;
				for (i, (p, k)) in pixels.iter_mut().zip(coverage).enumerate() {
					let k = f64::from(*k);
					if k <= 0.0 {
						continue;
					}
					let (x, y) = (rect[0] + (i % w) as i64 - dx, rect[1] + (i / w) as i64 - dy);
					if x < prect[0] || y < prect[1] || x > prect[2] || y > prect[3] {
						continue;
					}
					let c = carried[(y - prect[1]) as usize * pw + (x - prect[0]) as usize];
					for j in 0..4 {
						p[j] += (c[j] - p[j]) * k;
					}
				}
			}
		}
		self.previous = Some(((dab.x, dab.y), rect, pixels.to_vec()));
	}
}
