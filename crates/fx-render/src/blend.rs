//! Blend-mode math, f64, straight colour. The CPU reference for `composite.wgsl`.
//! Formulas: docs/BLEND_MODES.md. Keep this obviously correct, not fast.

use fx_core::BlendMode;

pub type Rgb = [f64; 3];

/// `B(Cb, Cs)` for any mode (Dissolve and PassThrough behave like Normal
/// here; their special handling happens in the compositor).
pub fn blend(mode: BlendMode, cb: Rgb, cs: Rgb) -> Rgb {
	use BlendMode::*;
	match mode {
		DarkerColor => {
			if sum(cs) <= sum(cb) {
				cs
			} else {
				cb
			}
		}
		LighterColor => {
			if sum(cs) >= sum(cb) {
				cs
			} else {
				cb
			}
		}
		Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
		Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
		Color => set_lum(cs, lum(cb)),
		Luminosity => set_lum(cb, lum(cs)),
		separable => [0, 1, 2].map(|i| blend_channel(separable, cb[i], cs[i])),
	}
}

/// Separable modes, one channel.
pub fn blend_channel(mode: BlendMode, b: f64, s: f64) -> f64 {
	use BlendMode::*;
	match mode {
		PassThrough | Normal | Dissolve => s,
		Darken => b.min(s),
		Multiply => b * s,
		ColorBurn => color_burn(b, s),
		LinearBurn => (b + s - 1.0).max(0.0),
		Lighten => b.max(s),
		Screen => screen(b, s),
		ColorDodge => color_dodge(b, s),
		LinearDodge => (b + s).min(1.0),
		Overlay => hard_light(s, b),
		SoftLight => {
			if s <= 0.5 {
				2.0 * b * s + b * b * (1.0 - 2.0 * s)
			} else {
				2.0 * b * (1.0 - s) + b.sqrt() * (2.0 * s - 1.0)
			}
		}
		HardLight => hard_light(b, s),
		VividLight => {
			if s <= 0.5 {
				color_burn(b, 2.0 * s)
			} else {
				color_dodge(b, 2.0 * (s - 0.5))
			}
		}
		LinearLight => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
		PinLight => {
			if s <= 0.5 {
				b.min(2.0 * s)
			} else {
				b.max(2.0 * s - 1.0)
			}
		}
		HardMix => {
			if b + s >= 1.0 {
				1.0
			} else {
				0.0
			}
		}
		Difference => (b - s).abs(),
		Exclusion => b + s - 2.0 * b * s,
		Subtract => (b - s).max(0.0),
		Divide => {
			if s == 0.0 {
				if b == 0.0 { 0.0 } else { 1.0 }
			} else {
				(b / s).min(1.0)
			}
		}
		DarkerColor | LighterColor | Hue | Saturation | Color | Luminosity => unreachable!("non-separable mode {mode:?} in blend_channel"),
	}
}

fn screen(b: f64, s: f64) -> f64 {
	b + s - b * s
}

fn hard_light(b: f64, s: f64) -> f64 {
	if s <= 0.5 { b * 2.0 * s } else { screen(b, 2.0 * s - 1.0) }
}

fn color_burn(b: f64, s: f64) -> f64 {
	if b >= 1.0 {
		1.0
	} else if s <= 0.0 {
		0.0
	} else {
		1.0 - ((1.0 - b) / s).min(1.0)
	}
}

fn color_dodge(b: f64, s: f64) -> f64 {
	if b <= 0.0 {
		0.0
	} else if s >= 1.0 {
		1.0
	} else {
		(b / (1.0 - s)).min(1.0)
	}
}

fn sum(c: Rgb) -> f64 {
	c[0] + c[1] + c[2]
}

pub fn lum(c: Rgb) -> f64 {
	0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: Rgb) -> Rgb {
	let l = lum(c);
	let n = c[0].min(c[1]).min(c[2]);
	let x = c[0].max(c[1]).max(c[2]);
	let mut out = c;
	if n < 0.0 {
		out = out.map(|v| l + (v - l) * l / (l - n));
	}
	if x > 1.0 {
		out = out.map(|v| l + (v - l) * (1.0 - l) / (x - l));
	}
	out
}

fn set_lum(c: Rgb, l: f64) -> Rgb {
	let d = l - lum(c);
	clip_color(c.map(|v| v + d))
}

fn sat(c: Rgb) -> f64 {
	c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// W3C SetSat: scale the middle component, max → s, min → 0.
fn set_sat(c: Rgb, s: f64) -> Rgb {
	let mut idx = [0usize, 1, 2];
	idx.sort_by(|&a, &b| c[a].total_cmp(&c[b]));
	let (min, mid, max) = (idx[0], idx[1], idx[2]);
	let mut out = [0.0; 3];
	if c[max] > c[min] {
		out[mid] = (c[mid] - c[min]) * s / (c[max] - c[min]);
		out[max] = s;
	}
	out[min] = 0.0;
	out
}

/// Premultiplied RGBA accumulator pixel.
pub type Premul = [f64; 4];

/// Composite a straight-colour source onto a premultiplied backdrop
/// (docs/BLEND_MODES.md §2). `atop` = source-atop (clipped layers and
/// adjustments): the backdrop alpha is kept.
pub fn composite(mode: BlendMode, backdrop: Premul, cs: Rgb, alpha_s: f64, atop: bool) -> Premul {
	let ab = backdrop[3];
	let cb = unpremultiply(backdrop);
	let mixed = blend(mode, cb, cs);
	// Cs' = (1 − αb)·Cs + αb·B(Cb, Cs)
	let cs2: Rgb = [0, 1, 2].map(|i| (1.0 - ab) * cs[i] + ab * mixed[i]);
	if atop {
		// co = αs·αb·Cs' + (1 − αs)·cb,  αo = αb
		let mut out = [0.0; 4];
		for i in 0..3 {
			out[i] = alpha_s * ab * cs2[i] + (1.0 - alpha_s) * backdrop[i];
		}
		out[3] = ab;
		out
	} else {
		// co = αs·Cs' + (1 − αs)·cb,  αo = αs + αb·(1 − αs)
		let mut out = [0.0; 4];
		for i in 0..3 {
			out[i] = alpha_s * cs2[i] + (1.0 - alpha_s) * backdrop[i];
		}
		out[3] = alpha_s + ab * (1.0 - alpha_s);
		out
	}
}

pub fn unpremultiply(p: Premul) -> Rgb {
	if p[3] <= 0.0 { [0.0; 3] } else { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] }
}

#[cfg(test)]
mod tests {
	use super::*;
	use BlendMode::*;

	const ALL: [BlendMode; 27] = [
		Normal,
		Dissolve,
		Darken,
		Multiply,
		ColorBurn,
		LinearBurn,
		DarkerColor,
		Lighten,
		Screen,
		ColorDodge,
		LinearDodge,
		LighterColor,
		Overlay,
		SoftLight,
		HardLight,
		VividLight,
		LinearLight,
		PinLight,
		HardMix,
		Difference,
		Exclusion,
		Subtract,
		Divide,
		Hue,
		Saturation,
		Color,
		Luminosity,
	];

	#[test]
	fn known_values() {
		let b = [0.25, 0.5, 0.75];
		let s = [0.5, 0.5, 0.5];
		assert_eq!(blend(Multiply, b, s), [0.125, 0.25, 0.375]);
		assert_eq!(blend(Screen, b, s), [0.625, 0.75, 0.875]);
		assert_eq!(blend(Difference, b, s), [0.25, 0.0, 0.25]);
		assert_eq!(blend_channel(Overlay, 0.25, 0.8), 0.5 * 0.8, "overlay = hard light with swapped args");
		assert_eq!(blend_channel(ColorDodge, 0.5, 1.0), 1.0);
		assert_eq!(blend_channel(ColorBurn, 0.5, 0.0), 0.0);
		assert_eq!(blend_channel(Divide, 0.0, 0.0), 0.0);
	}

	#[test]
	fn outputs_stay_in_range() {
		let steps = [0.0, 0.001, 0.25, 0.5, 0.5001, 0.75, 0.999, 1.0];
		for mode in ALL {
			for &r in &steps {
				for &g in &steps {
					let b = [r, g, 1.0 - r];
					let s = [g, 1.0 - g, r];
					for v in blend(mode, b, s) {
						assert!((-1e-12..=1.0 + 1e-12).contains(&v), "{mode:?} {b:?} {s:?} → {v}");
					}
				}
			}
		}
	}

	#[test]
	fn normal_opaque_replaces_and_transparent_keeps() {
		let backdrop = [0.2, 0.3, 0.4, 1.0];
		assert_eq!(composite(Normal, backdrop, [1.0, 0.0, 0.0], 1.0, false), [1.0, 0.0, 0.0, 1.0]);
		assert_eq!(composite(Multiply, backdrop, [1.0, 0.0, 0.0], 0.0, false), backdrop);
	}

	#[test]
	fn onto_transparent_backdrop_mode_does_not_matter() {
		for mode in ALL {
			let out = composite(mode, [0.0; 4], [0.3, 0.6, 0.9], 0.5, false);
			let expected = [0.15, 0.3, 0.45, 0.5];
			for i in 0..4 {
				assert!((out[i] - expected[i]).abs() < 1e-12, "{mode:?}");
			}
		}
	}

	#[test]
	fn atop_keeps_backdrop_alpha() {
		let out = composite(Normal, [0.1, 0.1, 0.1, 0.5], [1.0, 1.0, 1.0], 1.0, true);
		assert_eq!(out[3], 0.5);
		assert!((out[0] - 0.5).abs() < 1e-12, "clipped white over 50 % base → white at 50 %");
		assert_eq!(
			composite(Normal, [0.0; 4], [1.0; 3], 1.0, true),
			[0.0; 4],
			"nothing where the base is transparent"
		);
	}
}
