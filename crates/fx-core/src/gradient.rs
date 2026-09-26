//! Gradients (M8-T03): stops, interpolation, the five geometries, dither.
//!
//! One evaluator serves the Gradient tool's command (`Command::FillGradient`)
//! and gradient fill layers (`LayerKind::FillLayer`), so a fill layer drawn at
//! level 0 is what the tool would have painted.
//!
//! VERIFY (D-064): Photoshop's midpoint curve (a power curve here), its
//! "Perceptual" space (Oklab here) and its dither pattern (4 × 4 Bayer here).

use serde::{Deserialize, Serialize};

/// A colour stop: `location` and `midpoint` (toward the next stop) `0..=1`,
/// straight sRGB `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorStop {
	pub location: f32,
	#[serde(default = "half")]
	pub midpoint: f32,
	pub color: [f32; 3],
}

/// An opacity stop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpacityStop {
	pub location: f32,
	#[serde(default = "half")]
	pub midpoint: f32,
	pub opacity: f32,
}

fn half() -> f32 {
	0.5
}

/// How colours are mixed between stops (Photoshop 2021+).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
	#[default]
	Perceptual,
	Linear,
	Classic,
}

/// A gradient: colour and opacity stops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
	pub colors: Vec<ColorStop>,
	#[serde(default)]
	pub opacities: Vec<OpacityStop>,
	#[serde(default)]
	pub method: Method,
}

impl Gradient {
	/// From `a` to `b` (straight sRGB), both opaque.
	pub fn two(a: [f32; 3], b: [f32; 3]) -> Self {
		Self {
			colors: vec![
				ColorStop {
					location: 0.0,
					midpoint: 0.5,
					color: a,
				},
				ColorStop {
					location: 1.0,
					midpoint: 0.5,
					color: b,
				},
			],
			opacities: Vec::new(),
			method: Method::Perceptual,
		}
	}

	/// Straight RGBA at `t` (`0..=1`).
	pub fn eval(&self, t: f64) -> [f64; 4] {
		let t = t.clamp(0.0, 1.0);
		let rgb = self.color_at(t);
		let alpha = self.opacity_at(t);
		[rgb[0], rgb[1], rgb[2], alpha]
	}

	fn color_at(&self, t: f64) -> [f64; 3] {
		let stops = &self.colors;
		let Some(first) = stops.first() else { return [0.0; 3] };
		let c = |s: &ColorStop| s.color.map(f64::from);
		if stops.len() == 1 || t <= f64::from(first.location) {
			return c(first);
		}
		for pair in stops.windows(2) {
			let (a, b) = (&pair[0], &pair[1]);
			let (la, lb) = (f64::from(a.location), f64::from(b.location));
			if t <= lb {
				let f = if lb > la { (t - la) / (lb - la) } else { 1.0 };
				let f = midpoint(f, f64::from(a.midpoint));
				return mix(c(a), c(b), f, self.method);
			}
		}
		c(stops.last().expect("not empty"))
	}

	fn opacity_at(&self, t: f64) -> f64 {
		let stops = &self.opacities;
		let Some(first) = stops.first() else { return 1.0 };
		if stops.len() == 1 || t <= f64::from(first.location) {
			return f64::from(first.opacity);
		}
		for pair in stops.windows(2) {
			let (a, b) = (&pair[0], &pair[1]);
			let (la, lb) = (f64::from(a.location), f64::from(b.location));
			if t <= lb {
				let f = if lb > la { (t - la) / (lb - la) } else { 1.0 };
				let f = midpoint(f, f64::from(a.midpoint));
				return f64::from(a.opacity) + (f64::from(b.opacity) - f64::from(a.opacity)) * f;
			}
		}
		f64::from(stops.last().expect("not empty").opacity)
	}
}

/// The midpoint remap: `f = m` gives 0.5.
fn midpoint(f: f64, m: f64) -> f64 {
	let m = m.clamp(0.05, 0.95);
	if (m - 0.5).abs() < 1e-6 {
		return f;
	}
	f.clamp(0.0, 1.0).powf(0.5f64.ln() / m.ln())
}

fn to_linear(c: f64) -> f64 {
	if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(c: f64) -> f64 {
	let c = c.max(0.0);
	if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn oklab(rgb: [f64; 3]) -> [f64; 3] {
	let [r, g, b] = rgb.map(to_linear);
	let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
	let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
	let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
	[
		0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
		1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
		0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
	]
}

fn from_oklab([l, a, b]: [f64; 3]) -> [f64; 3] {
	let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
	let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
	let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
	[
		4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
		-1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
		-0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701 * s_,
	]
	.map(to_srgb)
}

fn mix(a: [f64; 3], b: [f64; 3], f: f64, method: Method) -> [f64; 3] {
	let lerp = |x: [f64; 3], y: [f64; 3]| [0, 1, 2].map(|i| x[i] + (y[i] - x[i]) * f);
	match method {
		Method::Classic => lerp(a, b),
		Method::Linear => lerp(a.map(to_linear), b.map(to_linear)).map(to_srgb),
		Method::Perceptual => from_oklab(lerp(oklab(a), oklab(b))).map(|c| c.clamp(0.0, 1.0)),
	}
}

/// The five gradient geometries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradientKind {
	#[default]
	Linear,
	Radial,
	Angle,
	Reflected,
	Diamond,
}

/// A gradient placed on the canvas: what the tool's drag and a fill layer
/// resolve to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientFill {
	pub gradient: Gradient,
	pub kind: GradientKind,
	/// Document pixels.
	pub start: (f64, f64),
	pub end: (f64, f64),
	#[serde(default)]
	pub reverse: bool,
	#[serde(default = "yes")]
	pub dither: bool,
	/// Use the opacity stops (off = opaque everywhere).
	#[serde(default = "yes")]
	pub transparency: bool,
}

fn yes() -> bool {
	true
}

/// The 4 × 4 Bayer matrix, `0..16`.
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

impl GradientFill {
	/// Position along the gradient (`0..=1`) of the point `(x, y)`.
	pub fn t_at(&self, x: f64, y: f64) -> f64 {
		let (sx, sy) = self.start;
		let (dx, dy) = (self.end.0 - sx, self.end.1 - sy);
		let len2 = dx * dx + dy * dy;
		let len = len2.sqrt();
		let (px, py) = (x - sx, y - sy);
		let t = if len2 <= 1e-12 {
			1.0
		} else {
			match self.kind {
				GradientKind::Linear => (px * dx + py * dy) / len2,
				GradientKind::Reflected => ((px * dx + py * dy) / len2).abs(),
				GradientKind::Radial => (px * px + py * py).sqrt() / len,
				GradientKind::Diamond => {
					// Distance in the frame of the drag, L∞.
					let (ux, uy) = (dx / len, dy / len);
					let a = (px * ux + py * uy).abs();
					let b = (-px * uy + py * ux).abs();
					a.max(b) / len
				}
				GradientKind::Angle => {
					let base = dy.atan2(dx);
					let a = py.atan2(px) - base;
					// Photoshop sweeps counter-clockwise on screen (VERIFY).
					(-a).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU
				}
			}
		};
		let t = t.clamp(0.0, 1.0);
		if self.reverse { 1.0 - t } else { t }
	}

	/// Straight RGBA at the canvas pixel whose centre is `(x + 0.5, y + 0.5)`,
	/// dithered by ±½ of an 8-bit level when `dither` is on.
	pub fn color_at(&self, x: i64, y: i64) -> [f64; 4] {
		self.color_at_point(x as f64 + 0.5, y as f64 + 0.5, x, y)
	}

	/// The colour at a document point; `(bx, by)` picks the dither cell.
	pub fn color_at_point(&self, fx: f64, fy: f64, bx: i64, by: i64) -> [f64; 4] {
		let mut c = self.gradient.eval(self.t_at(fx, fy));
		if !self.transparency {
			c[3] = 1.0;
		}
		if self.dither {
			let d = (f64::from(BAYER[by.rem_euclid(4) as usize][bx.rem_euclid(4) as usize]) + 0.5) / 16.0 - 0.5;
			for v in c.iter_mut().take(3) {
				*v = (*v + d / 255.0).clamp(0.0, 1.0);
			}
		}
		c
	}
}

/// A gradient fill layer's parameters (Layer ▸ New Fill Layer ▸ Gradient):
/// the gradient's geometry follows the canvas, like Photoshop's dialog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientLayer {
	pub gradient: Gradient,
	pub kind: GradientKind,
	/// Degrees, counter-clockwise (Photoshop's Angle).
	pub angle: f64,
	/// Percent (`10..=150` in Photoshop).
	pub scale: f64,
	#[serde(default)]
	pub reverse: bool,
	#[serde(default = "yes")]
	pub dither: bool,
	/// Offset of the centre from the canvas centre, document pixels.
	#[serde(default)]
	pub offset: (f64, f64),
}

impl GradientLayer {
	/// The placed gradient on a canvas of `size`.
	pub fn placed(&self, size: (u32, u32)) -> GradientFill {
		let (w, h) = (f64::from(size.0), f64::from(size.1));
		let a = self.angle.to_radians();
		let (ux, uy) = (a.cos(), -a.sin());
		let half = (w * ux.abs() + h * uy.abs()) / 2.0 * (self.scale / 100.0).max(0.01);
		let c = (w / 2.0 + self.offset.0, h / 2.0 + self.offset.1);
		let (start, end) = match self.kind {
			GradientKind::Linear | GradientKind::Angle => ((c.0 - ux * half, c.1 - uy * half), (c.0 + ux * half, c.1 + uy * half)),
			_ => (c, (c.0 + ux * half, c.1 + uy * half)),
		};
		GradientFill {
			gradient: self.gradient.clone(),
			kind: self.kind,
			start,
			end,
			reverse: self.reverse,
			dither: self.dither,
			transparency: true,
		}
	}
}
