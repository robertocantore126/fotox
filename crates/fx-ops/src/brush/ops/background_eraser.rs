//! The Background Eraser (M8-T02): erases the pixels that match a sampled
//! colour within the tolerance, `alpha × (1 − k × match)`, keeping the
//! protected foreground colour.
//!
//! FAST: "Contiguous" and "Find Edges" limits act like "Discontiguous" (the
//! stroke model has no per-dab flood); "Continuous" sampling samples once at
//! the press.

use crate::brush::op::{DabContext, DabOp};

pub struct BackgroundEraser {
	/// Straight RGB `0..=1`.
	pub sample: [f64; 3],
	/// `0..=1` (the option bar's percent).
	pub tolerance: f64,
	pub protect: Option<[f64; 3]>,
}

/// How much `rgb` matches `target` within `tolerance`, with a soft edge of a
/// few levels (VERIFY: Photoshop's edge).
pub fn match_amount(rgb: [f64; 3], target: [f64; 3], tolerance: f64) -> f64 {
	let d = (0..3).map(|i| (rgb[i] - target[i]).abs()).fold(0.0, f64::max);
	let edge = 4.0 / 255.0;
	((tolerance + edge - d) / edge).clamp(0.0, 1.0)
}

impl DabOp for BackgroundEraser {
	fn pixel(&self, backdrop: [f64; 4], _source: Option<[f32; 4]>, k: f64, _ctx: &DabContext) -> [f64; 4] {
		let a = backdrop[3];
		if a <= 0.0 {
			return backdrop;
		}
		let rgb = [backdrop[0] / a, backdrop[1] / a, backdrop[2] / a];
		let mut m = match_amount(rgb, self.sample, self.tolerance);
		if let Some(protect) = self.protect {
			m *= 1.0 - match_amount(rgb, protect, self.tolerance);
		}
		let keep = 1.0 - k * m;
		backdrop.map(|c| c * keep)
	}
}
