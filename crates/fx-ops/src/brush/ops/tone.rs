//! Dodge, Burn and Sponge (M8-T04).
//!
//! VERIFY (D-064): Photoshop's formulas are not documented. These are the
//! usual published approximations: the Range weights a luminance window
//! (Shadows `(1 − v)²`, Midtones `4v(1 − v)`, Highlights `v²`); Dodge moves a
//! value toward white by half the weighted strength, Burn toward black;
//! Protect Tones works on the luminance and keeps hue and saturation (the
//! colour is scaled, then pulled toward grey if it would clip). Sponge moves
//! the colour away from / toward its grey; Vibrance weighs by how unsaturated
//! the pixel is.

use fx_core::stroke::ToneRange;

use crate::brush::op::{DabContext, DabOp};

fn weight(range: ToneRange, v: f64) -> f64 {
	let v = v.clamp(0.0, 1.0);
	match range {
		ToneRange::Shadows => (1.0 - v) * (1.0 - v),
		ToneRange::Midtones => 4.0 * v * (1.0 - v),
		ToneRange::Highlights => v * v,
	}
}

fn luminance(rgb: [f64; 3]) -> f64 {
	0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]
}

/// Dodge (`lighten`) or Burn.
pub struct Tone {
	pub lighten: bool,
	pub range: ToneRange,
	pub protect_tones: bool,
}

impl Tone {
	fn value(&self, v: f64, k: f64) -> f64 {
		let a = 0.5 * k * weight(self.range, v);
		if self.lighten { v + a * (1.0 - v) } else { v - a * v }
	}
}

impl DabOp for Tone {
	fn pixel(&self, backdrop: [f64; 4], _source: Option<[f32; 4]>, k: f64, _ctx: &DabContext) -> [f64; 4] {
		let a = backdrop[3];
		if a <= 0.0 || k <= 0.0 {
			return backdrop;
		}
		let rgb = [backdrop[0] / a, backdrop[1] / a, backdrop[2] / a];
		let out = if self.protect_tones {
			let l = luminance(rgb);
			let l2 = self.value(l, k);
			let mut c = if l > 1e-6 { rgb.map(|x| x * l2 / l) } else { [l2; 3] };
			// Keep inside the gamut by moving toward the grey (the saturation
			// clamp).
			let max = c.iter().cloned().fold(0.0, f64::max);
			if max > 1.0 {
				let t = (1.0 - l2) / (max - l2).max(1e-9);
				c = c.map(|x| l2 + (x - l2) * t);
			}
			c
		} else {
			rgb.map(|v| self.value(v, k))
		};
		[out[0].clamp(0.0, 1.0) * a, out[1].clamp(0.0, 1.0) * a, out[2].clamp(0.0, 1.0) * a, a]
	}

	fn gray(&self, v: f64, k: f64, _ctx: &DabContext) -> f64 {
		self.value(v, k)
	}
}

/// The Sponge.
pub struct Sponge {
	pub saturate: bool,
	pub vibrance: bool,
}

impl DabOp for Sponge {
	fn pixel(&self, backdrop: [f64; 4], _source: Option<[f32; 4]>, k: f64, _ctx: &DabContext) -> [f64; 4] {
		let a = backdrop[3];
		if a <= 0.0 || k <= 0.0 {
			return backdrop;
		}
		let rgb = [backdrop[0] / a, backdrop[1] / a, backdrop[2] / a];
		let l = luminance(rgb);
		let max = rgb.iter().cloned().fold(0.0, f64::max);
		let min = rgb.iter().cloned().fold(1.0, f64::min);
		let saturation = max - min;
		// FAST: Vibrance protects saturated pixels but not skin tones.
		let k = if self.vibrance { k * (1.0 - saturation) } else { k };
		let factor = if self.saturate { 1.0 + k } else { 1.0 - k };
		let mut c = rgb.map(|x| l + (x - l) * factor);
		let hi = c.iter().cloned().fold(0.0, f64::max);
		let lo = c.iter().cloned().fold(1.0, f64::min);
		if hi > 1.0 || lo < 0.0 {
			let t = if hi > 1.0 { (1.0 - l) / (hi - l).max(1e-9) } else { 1.0 }.min(if lo < 0.0 { l / (l - lo).max(1e-9) } else { 1.0 });
			c = c.map(|x| l + (x - l) * t);
		}
		[c[0] * a, c[1] * a, c[2] * a, a]
	}

	/// A grey target has no saturation (Photoshop: nothing happens).
	fn gray(&self, v: f64, _k: f64, _ctx: &DabContext) -> f64 {
		v
	}
}
