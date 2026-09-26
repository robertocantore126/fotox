//! Adjustment layers as per-channel lookup tables.
//!
//! Most Photoshop adjustments are a function of each channel alone; they are
//! baked on the CPU into a [`Lut`] (4096 entries per channel, f32) and applied
//! by the compositor with linear interpolation — identical math on CPU and GPU.
//!
//! Implemented here: Invert, Levels, Curves, Exposure, Brightness/Contrast
//! (LUTs), and [`hue_saturation`] (per pixel).
//! Hue/Saturation is applied per pixel by the compositor
//! (`AdjustKind::HueSaturation`), same math on CPU and GPU.
//! Formulas marked VERIFY are checked against Photoshop in M7.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use fx_core::GradientStop;
use fx_core::layer::{Adjustment, LevelsChannel};

pub const LUT_SIZE: usize = 4096;

/// Per-channel curve: `entries[i][c]` = output of channel `c` for input `i / (LUT_SIZE - 1)`.
#[derive(Debug, PartialEq)]
pub struct Lut {
	pub entries: Box<[[f32; 3]]>,
	/// Hash of the adjustment parameters (part of composite-cache keys).
	pub key: u64,
}

impl Lut {
	/// Linear interpolation, exactly as `composite.wgsl` does it.
	pub fn apply(&self, rgb: [f64; 3]) -> [f64; 3] {
		[0, 1, 2].map(|c| {
			let x = rgb[c].clamp(0.0, 1.0) as f32 * (LUT_SIZE - 1) as f32;
			let i = (x.floor() as usize).min(LUT_SIZE - 2);
			let t = x - i as f32;
			let a = self.entries[i][c];
			let b = self.entries[i + 1][c];
			(a + (b - a) * t) as f64
		})
	}
}

/// Bake a LUT. Panics for `HueSaturation`, which is not a per-channel function.
pub fn bake(adjustment: &Adjustment) -> Lut {
	// M12-T05: tables of their own, one LUT row each.
	match adjustment {
		Adjustment::ColorLookup { .. } => return bake_3d(adjustment),
		Adjustment::SelectiveColor { .. } => return bake_selective(adjustment),
		_ => {}
	}
	let f: Box<dyn Fn(usize, f64) -> f64> = match adjustment {
		Adjustment::Invert => Box::new(|_, x| 1.0 - x),
		Adjustment::Levels { channels } => {
			let channels = *channels;
			// Channel first, then composite (VERIFY order against Photoshop).
			Box::new(move |c, x| levels(&channels[0], levels(&channels[c + 1], x)))
		}
		Adjustment::Curves { channels } => {
			let master = Spline::new(&channels[0]);
			let per: [Spline; 3] = [Spline::new(&channels[1]), Spline::new(&channels[2]), Spline::new(&channels[3])];
			Box::new(move |c, x| master.eval(per[c].eval(x)))
		}
		Adjustment::Exposure { exposure, offset, gamma } => {
			let (e, o, g) = (*exposure as f64, *offset as f64, (*gamma as f64).max(0.01));
			// VERIFY: Photoshop applies exposure in linear light.
			Box::new(move |_, x| {
				let linear = srgb_to_linear(x) * 2f64.powf(e) + o;
				linear_to_srgb(linear.max(0.0).powf(1.0 / g))
			})
		}
		Adjustment::BrightnessContrast { brightness, contrast, legacy } => {
			let (b, c, legacy) = (*brightness as f64, *contrast as f64, *legacy);
			Box::new(move |_, x| brightness_contrast(x, b, c, legacy))
		}
		Adjustment::Posterize { levels } => {
			let n = f64::from((*levels).max(2));
			// VERIFY (M7): Photoshop's level boundaries.
			Box::new(move |_, x| ((x * n).floor().min(n - 1.0)) / (n - 1.0))
		}
		// Luminance LUTs: entry `i` is the RGB output for luminance `i`
		// (`AdjustKind::LumaLut`, the channel index picks the output channel).
		Adjustment::Threshold { level } => {
			let t = f64::from(*level) / 255.0;
			Box::new(move |_, y| if y >= t { 1.0 } else { 0.0 })
		}
		Adjustment::GradientMap { stops, reverse } => {
			let stops = sorted_stops(stops);
			let reverse = *reverse;
			Box::new(move |c, y| gradient_at(&stops, if reverse { 1.0 - y } else { y })[c])
		}
		Adjustment::HueSaturation { .. }
		| Adjustment::ChannelMixer { .. }
		| Adjustment::PhotoFilter { .. }
		| Adjustment::ColorBalance { .. }
		| Adjustment::Vibrance { .. }
		| Adjustment::BlackWhite { .. }
		| Adjustment::ColorLookup { .. }
		| Adjustment::SelectiveColor { .. } => panic!("{adjustment:?} is not a LUT adjustment"),
	};
	let entries = (0..LUT_SIZE)
		.map(|i| {
			let x = i as f64 / (LUT_SIZE - 1) as f64;
			[0, 1, 2].map(|c| f(c, x).clamp(0.0, 1.0) as f32)
		})
		.collect();
	Lut {
		entries,
		key: params_key(adjustment),
	}
}

/// Caches baked LUTs by parameters, so programs of every tile share one LUT.
#[derive(Default)]
pub struct LutCache {
	map: HashMap<u64, Arc<Lut>>,
}

impl LutCache {
	pub fn get(&mut self, adjustment: &Adjustment) -> Arc<Lut> {
		let key = params_key(adjustment);
		self.map.entry(key).or_insert_with(|| Arc::new(bake(adjustment))).clone()
	}

	/// Drop LUTs no longer used by any program (call occasionally).
	pub fn retain_used(&mut self) {
		self.map.retain(|_, lut| Arc::strong_count(lut) > 1);
	}
}

fn params_key(adjustment: &Adjustment) -> u64 {
	let mut h = std::collections::hash_map::DefaultHasher::new();
	// serde_json is deterministic for these types and avoids hashing f32 by hand.
	serde_json::to_string(adjustment).expect("adjustments serialise").hash(&mut h);
	h.finish()
}

/// Brightness/Contrast of one channel value `x` in 0..=1 (M2-T04).
///
/// Parameters are in the dialog's units: brightness −150..=150, contrast
/// −100..=100.
/// * **Legacy** (`legacy: true`): linear, `(x − 0.5)·k + 0.5 + b` with
///   `b = brightness / 255` and the contrast slope
///   `k = 1 + c` for `c = contrast / 100 < 0`, `k = 1 / (1 − c)` for `c ≥ 0`
///   (so +100 is a step at 0.5). Clips at 0 and 1.
/// * **Modern**: a curve through (0, 0) and (1, 1) that never clips —
///   brightness as a gamma `x^(1/g)`, `g = 2^(brightness / 100)`, then
///   contrast as the S-curve `x^p / (x^p + (1 − x)^p)` through (0.5, 0.5),
///   `p = 1 + 2c` for more contrast, `p = 1 + 0.8c` for less.
///
/// VERIFY both against Photoshop (M7).
pub fn brightness_contrast(x: f64, brightness: f64, contrast: f64, legacy: bool) -> f64 {
	let c = (contrast / 100.0).clamp(-1.0, 1.0);
	if legacy {
		let k = if c < 0.0 {
			1.0 + c
		} else if c < 1.0 {
			1.0 / (1.0 - c)
		} else {
			f64::INFINITY
		};
		let y = if k.is_infinite() {
			if x >= 0.5 { 1.0 } else { 0.0 }
		} else {
			(x - 0.5) * k + 0.5
		};
		return (y + brightness / 255.0).clamp(0.0, 1.0);
	}
	let g = 2f64.powf(brightness / 100.0);
	let y = x.clamp(0.0, 1.0).powf(1.0 / g);
	let p = if c >= 0.0 { 1.0 + 2.0 * c } else { 1.0 + 0.8 * c };
	if y <= 0.0 || y >= 1.0 {
		return y;
	}
	let (a, b) = (y.powf(p), (1.0 - y).powf(p));
	a / (a + b)
}

/// Master Hue/Saturation of one straight RGB pixel (M2-T04). Per pixel, not a
/// LUT; `gpu/composite.wgsl` (`hue_saturation`) is the same math in f32.
///
/// Parameters in the dialog's units: hue −180..=180° (colorize: 0..=360°),
/// saturation −100..=100 (colorize: 0..=100), lightness −100..=100.
/// * Normal: in HSL, hue rotated, saturation scaled by `1 + saturation/100`
///   (so greys stay grey), lightness kept.
/// * Colorize: hue and saturation replaced, HSL lightness kept.
/// * Then lightness blends towards white (> 0) or black (< 0).
///
/// VERIFY against Photoshop (M7).
pub fn hue_saturation(rgb: [f64; 3], hue: f64, saturation: f64, lightness: f64, colorize: bool) -> [f64; 3] {
	let (h, s, l) = rgb_to_hsl(rgb);
	let out = if colorize {
		hsl_to_rgb((hue / 360.0).rem_euclid(1.0), (saturation / 100.0).clamp(0.0, 1.0), l)
	} else {
		let s = (s * (1.0 + saturation / 100.0)).clamp(0.0, 1.0);
		hsl_to_rgb((h + hue / 360.0).rem_euclid(1.0), s, l)
	};
	let k = (lightness / 100.0).clamp(-1.0, 1.0);
	out.map(|v| if k >= 0.0 { v + (1.0 - v) * k } else { v * (1.0 + k) })
}

/// Luminance weights (Rec. 601), for Threshold, Gradient Map, Vibrance and
/// "preserve luminosity". VERIFY (M7): Photoshop's exact weights.
pub const LUMA: [f64; 3] = [0.299, 0.587, 0.114];

/// Luminance of straight RGB.
pub fn luma(rgb: [f64; 3]) -> f64 {
	rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// `rows · rgb + constant` (Channel Mixer, Photo Filter); `preserve_luma`
/// shifts the result back to the input's luminance. Clamped to 0..=1.
pub fn apply_matrix(rgb: [f64; 3], rows: &[[f32; 4]; 3], preserve_luma: bool) -> [f64; 3] {
	let mut out = rows.map(|r| f64::from(r[0]) * rgb[0] + f64::from(r[1]) * rgb[1] + f64::from(r[2]) * rgb[2] + f64::from(r[3]));
	if preserve_luma {
		let d = luma(rgb) - luma(out);
		out = out.map(|v| v + d);
	}
	out.map(|v| v.clamp(0.0, 1.0))
}

/// Color Balance: GIMP's colour-balance transfer (documented reconstruction;
/// VERIFY against Photoshop in M7). Shifts are −1..=1 per channel for each
/// tone range, weighted by the pixel's HSL lightness; "preserve luminosity"
/// keeps the HSL lightness.
pub fn color_balance(rgb: [f64; 3], shadows: [f32; 3], midtones: [f32; 3], highlights: [f32; 3], preserve_luma: bool) -> [f64; 3] {
	let (_, _, l) = rgb_to_hsl(rgb);
	let (a, b, scale) = (0.25, 0.333, 0.7);
	let ws = ((l - b) / -a + 0.5).clamp(0.0, 1.0) * scale;
	let wm = ((l - b) / a + 0.5).clamp(0.0, 1.0) * ((l + b - 1.0) / -a + 0.5).clamp(0.0, 1.0) * scale;
	let wh = ((l + b - 1.0) / a + 0.5).clamp(0.0, 1.0) * scale;
	let out: [f64; 3] =
		std::array::from_fn(|c| (rgb[c] + f64::from(shadows[c]) * ws + f64::from(midtones[c]) * wm + f64::from(highlights[c]) * wh).clamp(0.0, 1.0));
	if preserve_luma {
		let (h, s, _) = rgb_to_hsl(out);
		hsl_to_rgb(h, s, l)
	} else {
		out
	}
}

/// Vibrance: saturation raised more where it is low (−1..=1 each; a
/// reconstruction, VERIFY in M7), then the plain saturation factor.
pub fn vibrance(rgb: [f64; 3], vibrance: f32, saturation: f32) -> [f64; 3] {
	let (v, s) = (f64::from(vibrance), f64::from(saturation));
	let sat = rgb[0].max(rgb[1]).max(rgb[2]) - rgb[0].min(rgb[1]).min(rgb[2]);
	let k = if v >= 0.0 { 1.0 + v * (1.0 - sat) } else { 1.0 + v };
	let y = luma(rgb);
	rgb.map(|c| (y + (c - y) * k * (1.0 + s)).clamp(0.0, 1.0))
}

/// Black & White: grey = min + (mid − min)·w(secondary) + (max − mid)·w(primary),
/// the primary being the largest channel's hue (reds, greens, blues) and the
/// secondary the two largest (yellows, cyans, magentas) — the widely used
/// reconstruction of Photoshop's formula (VERIFY in M7). `weights` are
/// fractions in the order reds, yellows, greens, cyans, blues, magentas.
/// `tint`: `[hue°, saturation %]`, applied as HSL with the grey as lightness.
pub fn black_white(rgb: [f64; 3], weights: [f32; 6], tint: Option<[f32; 2]>) -> [f64; 3] {
	let mut order = [0usize, 1, 2];
	order.sort_by(|&a, &b| rgb[b].total_cmp(&rgb[a]));
	let (hi, mid, lo) = (order[0], order[1], order[2]);
	let primary = [0usize, 2, 4][hi];
	// The pair of the two largest channels: R+G yellows, G+B cyans, R+B magentas.
	let secondary = match (hi.min(mid), hi.max(mid)) {
		(0, 1) => 1,
		(1, 2) => 3,
		_ => 5,
	};
	let (max, midv, min) = (rgb[hi], rgb[mid], rgb[lo]);
	let grey = (min + (midv - min) * f64::from(weights[secondary]) + (max - midv) * f64::from(weights[primary])).clamp(0.0, 1.0);
	match tint {
		Some([hue, sat]) => hsl_to_rgb((f64::from(hue) / 360.0).rem_euclid(1.0), (f64::from(sat) / 100.0).clamp(0.0, 1.0), grey),
		None => [grey; 3],
	}
}

/// Gradient stops sorted by position; black → white when there are none.
fn sorted_stops(stops: &[GradientStop]) -> Vec<(f64, [f64; 3])> {
	let mut out: Vec<(f64, [f64; 3])> = stops
		.iter()
		.map(|s| (f64::from(s.position).clamp(0.0, 1.0), s.color.map(|v| f64::from(v).clamp(0.0, 1.0))))
		.collect();
	out.sort_by(|a, b| a.0.total_cmp(&b.0));
	if out.is_empty() {
		out = vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])];
	}
	out
}

/// The gradient's colour at `t` (linear between stops, flat outside).
fn gradient_at(stops: &[(f64, [f64; 3])], t: f64) -> [f64; 3] {
	let first = stops[0];
	let last = stops[stops.len() - 1];
	if t <= first.0 {
		return first.1;
	}
	if t >= last.0 {
		return last.1;
	}
	let i = stops.partition_point(|s| s.0 <= t).saturating_sub(1).min(stops.len() - 2);
	let (a, b) = (stops[i], stops[i + 1]);
	let f = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
	std::array::from_fn(|c| a.1[c] + (b.1[c] - a.1[c]) * f)
}

/// Straight RGB → (hue 0..1, saturation, lightness).
fn rgb_to_hsl([r, g, b]: [f64; 3]) -> (f64, f64, f64) {
	let max = r.max(g).max(b);
	let min = r.min(g).min(b);
	let l = (max + min) / 2.0;
	let d = max - min;
	if d <= 0.0 {
		return (0.0, 0.0, l);
	}
	let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
	let h = if max == r {
		(g - b) / d + if g < b { 6.0 } else { 0.0 }
	} else if max == g {
		(b - r) / d + 2.0
	} else {
		(r - g) / d + 4.0
	};
	(h / 6.0, s, l)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
	if s <= 0.0 {
		return [l; 3];
	}
	let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
	let p = 2.0 * l - q;
	let channel = |t: f64| {
		let t = t.rem_euclid(1.0);
		if t < 1.0 / 6.0 {
			p + (q - p) * 6.0 * t
		} else if t < 0.5 {
			q
		} else if t < 2.0 / 3.0 {
			p + (q - p) * (2.0 / 3.0 - t) * 6.0
		} else {
			p
		}
	};
	[channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0)]
}

fn levels(ch: &LevelsChannel, x: f64) -> f64 {
	let (ib, iw) = (ch.in_black as f64, ch.in_white as f64);
	let v = if iw > ib {
		((x - ib) / (iw - ib)).clamp(0.0, 1.0)
	} else {
		(x >= ib) as u8 as f64
	};
	let v = v.powf(1.0 / (ch.gamma as f64).max(0.01));
	ch.out_black as f64 + v * (ch.out_white as f64 - ch.out_black as f64)
}

pub fn srgb_to_linear(x: f64) -> f64 {
	if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(x: f64) -> f64 {
	if x <= 0.003_130_8 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

/// Natural cubic spline through the curve points, flat outside the first and
/// last point, identity for fewer than 2 points. (VERIFY: Photoshop's exact
/// interpolation; this matches it closely for typical curves.)
struct Spline {
	xs: Vec<f64>,
	ys: Vec<f64>,
	m: Vec<f64>,
}

impl Spline {
	fn new(points: &[(f32, f32)]) -> Self {
		let mut pts: Vec<(f64, f64)> = points.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
		pts.sort_by(|a, b| a.0.total_cmp(&b.0));
		pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
		if pts.len() < 2 {
			return Self {
				xs: vec![0.0, 1.0],
				ys: vec![0.0, 1.0],
				m: vec![0.0, 0.0],
			};
		}
		let n = pts.len();
		let xs: Vec<f64> = pts.iter().map(|p| p.0).collect();
		let ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
		// Second derivatives, natural boundary (tridiagonal solve).
		let mut m = vec![0.0; n];
		let mut c = vec![0.0; n];
		let mut d = vec![0.0; n];
		for i in 1..n - 1 {
			let h0 = xs[i] - xs[i - 1];
			let h1 = xs[i + 1] - xs[i];
			let a = h0;
			let b = 2.0 * (h0 + h1);
			let r = 6.0 * ((ys[i + 1] - ys[i]) / h1 - (ys[i] - ys[i - 1]) / h0);
			let denom = b - a * c[i - 1];
			c[i] = h1 / denom;
			d[i] = (r - a * d[i - 1]) / denom;
		}
		for i in (1..n - 1).rev() {
			m[i] = d[i] - c[i] * m[i + 1];
		}
		Self { xs, ys, m }
	}

	fn eval(&self, x: f64) -> f64 {
		let n = self.xs.len();
		if x <= self.xs[0] {
			return self.ys[0];
		}
		if x >= self.xs[n - 1] {
			return self.ys[n - 1];
		}
		let i = self.xs.partition_point(|&v| v <= x).saturating_sub(1).min(n - 2);
		let h = self.xs[i + 1] - self.xs[i];
		let a = (self.xs[i + 1] - x) / h;
		let b = (x - self.xs[i]) / h;
		(a * self.ys[i] + b * self.ys[i + 1] + ((a * a * a - a) * self.m[i] + (b * b * b - b) * self.m[i + 1]) * h * h / 6.0).clamp(0.0, 1.0)
	}
}

/// Side of the 3D table a Color Lookup is resampled to: 16³ = one LUT row
/// (`LUT_SIZE` entries). FAST: a 33³ `.cube` loses detail (trilinear).
pub const LUT3D: usize = 16;

/// Color Lookup (M12-T05): the table resampled to [`LUT3D`]³ entries, red
/// fastest, padded to `LUT_SIZE`.
pub fn bake_3d(adjustment: &Adjustment) -> Lut {
	let Adjustment::ColorLookup { size, table, .. } = adjustment else {
		panic!("not a Color Lookup");
	};
	let n = (*size as usize).max(2);
	let at = |r: usize, g: usize, b: usize| -> [f32; 3] { table.get(r + n * (g + n * b)).copied().unwrap_or([0.0; 3]) };
	let sample = |x: f32, y: f32, z: f32| -> [f32; 3] {
		let f = |v: f32| {
			let p = v.clamp(0.0, 1.0) * (n - 1) as f32;
			let i = (p.floor() as usize).min(n - 2);
			(i, p - i as f32)
		};
		let ((r, fr), (g, fg), (b, fb)) = (f(x), f(y), f(z));
		let mut out = [0.0f32; 3];
		for (dr, wr) in [(0, 1.0 - fr), (1, fr)] {
			for (dg, wg) in [(0, 1.0 - fg), (1, fg)] {
				for (db, wb) in [(0, 1.0 - fb), (1, fb)] {
					let v = at(r + dr, g + dg, b + db);
					for c in 0..3 {
						out[c] += v[c] * wr * wg * wb;
					}
				}
			}
		}
		out
	};
	let m = (LUT3D - 1) as f32;
	let mut entries = vec![[0.0f32; 3]; LUT_SIZE];
	for b in 0..LUT3D {
		for g in 0..LUT3D {
			for r in 0..LUT3D {
				entries[r + LUT3D * (g + LUT3D * b)] = sample(r as f32 / m, g as f32 / m, b as f32 / m).map(|v| v.clamp(0.0, 1.0));
			}
		}
	}
	Lut {
		entries: entries.into_boxed_slice(),
		key: params_key(adjustment),
	}
}

/// Trilinear lookup in a [`bake_3d`] table, as `composite.wgsl` does it.
pub fn lut3d(lut: &Lut, rgb: [f64; 3]) -> [f64; 3] {
	let m = (LUT3D - 1) as f64;
	let f = |v: f64| {
		let p = v.clamp(0.0, 1.0) * m;
		let i = (p.floor() as usize).min(LUT3D - 2);
		(i, p - i as f64)
	};
	let ((r, fr), (g, fg), (b, fb)) = (f(rgb[0]), f(rgb[1]), f(rgb[2]));
	let mut out = [0.0f64; 3];
	for (dr, wr) in [(0, 1.0 - fr), (1, fr)] {
		for (dg, wg) in [(0, 1.0 - fg), (1, fg)] {
			for (db, wb) in [(0, 1.0 - fb), (1, fb)] {
				let v = lut.entries[(r + dr) + LUT3D * ((g + dg) + LUT3D * (b + db))];
				for c in 0..3 {
					out[c] += f64::from(v[c]) * wr * wg * wb;
				}
			}
		}
	}
	out
}

/// Selective Color's table (M12-T05): entries 0..9 are (C, M, Y) of each
/// range, 9..18 its K; the rest is padding.
pub fn bake_selective(adjustment: &Adjustment) -> Lut {
	let Adjustment::SelectiveColor { ranges, .. } = adjustment else {
		panic!("not Selective Color");
	};
	let mut entries = vec![[0.0f32; 3]; LUT_SIZE];
	for (i, r) in ranges.iter().enumerate() {
		entries[i] = [r[0], r[1], r[2]];
		entries[9 + i] = [r[3], 0.0, 0.0];
	}
	Lut {
		entries: entries.into_boxed_slice(),
		key: params_key(adjustment),
	}
}

/// Selective Color of one straight RGB value. Range weights: a chromatic
/// range by the distance between the channels that define it, Whites /
/// Blacks by how far the extremes pass ½, Neutrals by the rest. CMY act on
/// R, G, B; K on all three; Relative scales by the ink already there.
/// VERIFY: Photoshop's exact weights and Relative / Absolute formulas.
pub fn selective_color(lut: &Lut, rgb: [f64; 3], relative: bool) -> [f64; 3] {
	let [r, g, b] = rgb;
	let max = r.max(g).max(b);
	let min = r.min(g).min(b);
	let mid = r + g + b - max - min;
	let mut w = [0.0f64; 9];
	if r == max {
		w[0] = max - mid;
	} else if g == max {
		w[2] = max - mid;
	} else {
		w[4] = max - mid;
	}
	if b == min {
		w[1] = mid - min;
	} else if r == min {
		w[3] = mid - min;
	} else {
		w[5] = mid - min;
	}
	w[6] = ((min - 0.5) * 2.0).max(0.0);
	w[8] = ((0.5 - max) * 2.0).max(0.0);
	w[7] = (1.0 - (max - min) - w[6] - w[8]).clamp(0.0, 1.0);
	let mut out = rgb;
	for (i, wi) in w.iter().enumerate() {
		if *wi <= 0.0 {
			continue;
		}
		let cmy = lut.entries[i];
		let k = f64::from(lut.entries[9 + i][0]);
		for c in 0..3 {
			let ink = 1.0 - rgb[c];
			let a = f64::from(cmy[c]);
			let delta = if relative { (a + k) * ink } else { a + k };
			out[c] -= wi * delta;
		}
	}
	out.map(|v| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn invert_and_identity_curves() {
		let inv = bake(&Adjustment::Invert);
		assert_eq!(inv.apply([0.0, 0.25, 1.0]), [1.0, 0.75, 0.0]);
		let identity = bake(&Adjustment::Curves {
			channels: [vec![(0.0, 0.0), (1.0, 1.0)], vec![], vec![], vec![]],
		});
		let out = identity.apply([0.1, 0.5, 0.9]);
		for (o, e) in out.iter().zip([0.1, 0.5, 0.9]) {
			assert!((o - e).abs() < 1e-4);
		}
	}

	#[test]
	fn levels_black_white_points() {
		let mut ch = [LevelsChannel::default(); 4];
		ch[0].in_black = 0.2;
		ch[0].in_white = 0.8;
		let lut = bake(&Adjustment::Levels { channels: ch });
		let out = lut.apply([0.1, 0.5, 0.9]);
		assert!(out[0].abs() < 1e-6 && (out[1] - 0.5).abs() < 1e-3 && (out[2] - 1.0).abs() < 1e-6);
	}

	#[test]
	fn curve_passes_through_points() {
		let lut = bake(&Adjustment::Curves {
			channels: [vec![(0.0, 0.0), (0.25, 0.4), (0.75, 0.8), (1.0, 1.0)], vec![], vec![], vec![]],
		});
		assert!((lut.apply([0.25; 3])[0] - 0.4).abs() < 1e-3);
		assert!((lut.apply([0.75; 3])[1] - 0.8).abs() < 1e-3);
	}

	#[test]
	fn cache_shares_luts() {
		let mut cache = LutCache::default();
		let a = cache.get(&Adjustment::Invert);
		let b = cache.get(&Adjustment::Invert);
		assert!(Arc::ptr_eq(&a, &b));
	}

	#[test]
	fn brightness_contrast_identity_endpoints_and_direction() {
		for legacy in [false, true] {
			for i in 0..=20 {
				let x = i as f64 / 20.0;
				assert!(
					(brightness_contrast(x, 0.0, 0.0, legacy) - x).abs() < 1e-12,
					"identity at {x} (legacy {legacy})"
				);
			}
		}
		// Modern: endpoints never move, brightness lifts the midtones, contrast
		// makes an S around 0.5, and the curve stays monotonic.
		for (b, c) in [(150.0, 0.0), (-150.0, 0.0), (0.0, 100.0), (0.0, -100.0), (60.0, 40.0)] {
			assert_eq!(brightness_contrast(0.0, b, c, false), 0.0);
			assert_eq!(brightness_contrast(1.0, b, c, false), 1.0);
			let mut last = 0.0;
			for i in 0..=100 {
				let y = brightness_contrast(i as f64 / 100.0, b, c, false);
				assert!(y >= last - 1e-12, "monotonic for ({b}, {c})");
				last = y;
			}
		}
		assert!(brightness_contrast(0.5, 50.0, 0.0, false) > 0.5);
		assert!(brightness_contrast(0.25, 0.0, 50.0, false) < 0.25);
		assert!(brightness_contrast(0.75, 0.0, 50.0, false) > 0.75);
		assert!((brightness_contrast(0.5, 0.0, 80.0, false) - 0.5).abs() < 1e-12);
		// Legacy is the linear formula, clipped.
		assert!((brightness_contrast(0.6, 25.5, 0.0, true) - 0.7).abs() < 1e-12);
		assert!((brightness_contrast(0.75, 0.0, 50.0, true) - 1.0).abs() < 1e-12, "slope 2 around 0.5");
		assert!((brightness_contrast(0.75, 0.0, -50.0, true) - 0.625).abs() < 1e-12, "slope 0.5");
		// A LUT bakes the same function.
		let lut = bake(&Adjustment::BrightnessContrast {
			brightness: 30.0,
			contrast: 20.0,
			legacy: false,
		});
		let x = 1234.0 / (LUT_SIZE - 1) as f64;
		assert!((lut.apply([x; 3])[0] - brightness_contrast(x, 30.0, 20.0, false)).abs() < 1e-6);
	}

	#[test]
	fn hue_saturation_basics() {
		let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
		let red = [1.0, 0.0, 0.0];
		assert!(close(hue_saturation(red, 0.0, 0.0, 0.0, false), red), "identity");
		assert!(close(hue_saturation(red, 120.0, 0.0, 0.0, false), [0.0, 1.0, 0.0]), "red + 120° = green");
		assert!(close(hue_saturation(red, -120.0, 0.0, 0.0, false), [0.0, 0.0, 1.0]), "red − 120° = blue");
		let grey = [0.4, 0.4, 0.4];
		assert!(close(hue_saturation(grey, 90.0, 100.0, 0.0, false), grey), "greys stay grey");
		assert!(
			close(hue_saturation([0.8, 0.4, 0.2], 0.0, -100.0, 0.0, false), [0.5; 3]),
			"desaturate to HSL lightness"
		);
		assert!(close(hue_saturation(grey, 0.0, 0.0, 100.0, false), [1.0; 3]), "lightness +100 = white");
		assert!(close(hue_saturation(grey, 0.0, 0.0, -100.0, false), [0.0; 3]), "lightness −100 = black");
		// Colorize: fixed hue and saturation, lightness of the source.
		let tinted = hue_saturation(grey, 240.0, 100.0, 0.0, true);
		assert!(tinted[2] > tinted[0] && tinted[0] == tinted[1], "blue tint, got {tinted:?}");
		let (_, _, l) = rgb_to_hsl(tinted);
		assert!((l - 0.4).abs() < 1e-9);
	}

	#[test]
	fn m4_identities_leave_colours_unchanged() {
		let samples = [[0.2, 0.5, 0.9], [1.0, 0.0, 0.3], [0.4, 0.4, 0.4], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
		let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
		for rgb in samples {
			let close = |a: [f64; 3], what: &str| {
				for c in 0..3 {
					assert!((a[c] - rgb[c]).abs() < 1e-9, "{what}: {rgb:?} → {a:?}");
				}
			};
			close(apply_matrix(rgb, &identity, false), "channel mixer identity");
			close(apply_matrix(rgb, &identity, true), "photo filter density 0");
			close(color_balance(rgb, [0.0; 3], [0.0; 3], [0.0; 3], false), "color balance at zero");
			close(vibrance(rgb, 0.0, 0.0), "vibrance at zero");
		}
	}

	#[test]
	fn m4_hand_computed_values() {
		// Black & White: greys stay the same grey; pure red weighs "reds".
		let w = [0.4, 0.6, 0.4, 0.6, 0.2, 0.8];
		assert_eq!(black_white([0.3, 0.3, 0.3], w, None), [0.3; 3]);
		assert!((black_white([1.0, 0.0, 0.0], w, None)[0] - 0.4).abs() < 1e-6);
		// Yellow (1, 1, 0): min 0, mid 1 (yellows 0.6), max 1 → 0.6.
		assert!((black_white([1.0, 1.0, 0.0], w, None)[0] - 0.6).abs() < 1e-6);
		// Threshold at 128/255 through its luminance LUT.
		let lut = bake(&Adjustment::Threshold { level: 128 });
		assert_eq!(lut.apply([0.49; 3]), [0.0; 3]);
		assert_eq!(lut.apply([0.51; 3]), [1.0; 3]);
		// Posterize 2: two levels, black and white.
		let lut = bake(&Adjustment::Posterize { levels: 2 });
		assert_eq!(lut.apply([0.2, 0.7, 0.6]), [0.0, 1.0, 1.0]);
		// A black → white gradient map is the luminance.
		let lut = bake(&Adjustment::GradientMap {
			stops: Vec::new(),
			reverse: false,
		});
		let y = luma([0.2, 0.5, 0.9]);
		assert!((lut.apply([y; 3])[1] - y).abs() < 1e-3);
		// Photo filter: a full-density red filter keeps only red.
		let red = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]];
		assert_eq!(apply_matrix([0.5, 0.5, 0.5], &red, false), [0.5, 0.0, 0.0]);
		// Vibrance −1: fully desaturated to the luminance.
		let v = vibrance([1.0, 0.0, 0.0], -1.0, 0.0);
		assert!((v[0] - v[1]).abs() < 1e-9 && (v[0] - LUMA[0]).abs() < 1e-9);
	}
}
