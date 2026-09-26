//! The Color Replacement tool (M8-T08): where a pixel matches the sampled
//! colour within the tolerance, the foreground is blended in by the Mode
//! (Hue, Saturation, Color, Luminosity), keeping the pixel's alpha.
//!
//! FAST: the Limits act as Discontiguous and Continuous sampling samples once
//! (like the Background Eraser). VERIFY (D-064): Photoshop's match edge.

use fx_core::BlendMode;
use fx_core::blend::composite;

use super::background_eraser::match_amount;
use crate::brush::op::{DabContext, DabOp};

pub struct ColorReplace {
	pub sample: [f64; 3],
	pub tolerance: f64,
	pub mode: BlendMode,
}

impl DabOp for ColorReplace {
	fn pixel(&self, backdrop: [f64; 4], _source: Option<[f32; 4]>, k: f64, ctx: &DabContext) -> [f64; 4] {
		let a = backdrop[3];
		if a <= 0.0 || k <= 0.0 {
			return backdrop;
		}
		let rgb = [backdrop[0] / a, backdrop[1] / a, backdrop[2] / a];
		let m = match_amount(rgb, self.sample, self.tolerance);
		if m <= 0.0 {
			return backdrop;
		}
		composite(self.mode, backdrop, ctx.color, k * m, true)
	}
}
