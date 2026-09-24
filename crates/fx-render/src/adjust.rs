//! Adjustment layers as per-channel lookup tables.
//!
//! Most Photoshop adjustments are a function of each channel alone; they are
//! baked on the CPU into a [`Lut`] (4096 entries per channel, f32) and applied
//! by the compositor with linear interpolation — identical math on CPU and GPU.
//!
//! Implemented here: Invert, Levels, Curves, Exposure.
//! M2-T04 (DeepSeek): Brightness/Contrast (bake as LUT here) and
//! Hue/Saturation (per pixel, `AdjustKind::HueSaturation` in the compositor).
//! Formulas marked VERIFY are checked against Photoshop in M7.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

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
		Adjustment::BrightnessContrast { .. } => todo!("M2-T04: Brightness/Contrast curve"),
		Adjustment::HueSaturation { .. } => panic!("Hue/Saturation is not a per-channel LUT"),
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
}
