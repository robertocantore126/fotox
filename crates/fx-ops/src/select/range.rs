//! Select ▸ Color Range (M9-T03): a soft coverage per pixel, per tile.
//!
//! * Sampled Colors: `1 − d / fuzziness` for the nearest sample, `d` the
//!   largest channel difference in 8-bit levels; Localized Color Clusters
//!   fade it with the distance to `(x, y)` over `range` pixels.
//! * Reds … Magentas: a hue window of ±30° (full within ±15°), weighted by
//!   saturation.
//! * Highlights / Midtones / Shadows: the luminance against the `low` /
//!   `high` split points (8-bit), with a 20-level ramp (CC's range sliders).
//! * Skin Tones: a hue / saturation / luminance box of typical skin.
//!
//! Transparent pixels are never selected. VERIFY (D-064): every curve.
//! FAST: Out of Gamut selects nothing (it needs the proof LUT in fx-ops).

use fx_core::select_ops::{RangeKind, SelectOp};
use fx_core::selection::{Selection, TileCoverage};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};

use super::{assemble, luma, pixels, straight};
use crate::flood::WandSource;

fn hue_sat(c: [f32; 3]) -> (f32, f32) {
	let max = c[0].max(c[1]).max(c[2]);
	let min = c[0].min(c[1]).min(c[2]);
	let d = max - min;
	if d <= 1e-6 {
		return (0.0, 0.0);
	}
	let h = if max == c[0] {
		((c[1] - c[2]) / d).rem_euclid(6.0)
	} else if max == c[1] {
		(c[2] - c[0]) / d + 2.0
	} else {
		(c[0] - c[1]) / d + 4.0
	};
	(h * 60.0, d / max.max(1e-6))
}

fn ramp_up(v: f32, at: f32, width: f32) -> f32 {
	((v - (at - width)) / width).clamp(0.0, 1.0)
}

/// The coverage of one straight pixel at canvas `(x, y)`.
fn coverage(op: &SelectOp, c: [f32; 3], x: f64, y: f64) -> f32 {
	let SelectOp::ColorRange {
		range,
		samples,
		fuzziness,
		localized,
		low,
		high,
		..
	} = op
	else {
		return 0.0;
	};
	match range {
		RangeKind::Sampled => {
			let fuzz = fuzziness.max(0.5);
			let mut best = 0.0f32;
			for s in samples {
				let d = (0..3).map(|i| (c[i] - s[i]).abs() * 255.0).fold(0.0, f32::max);
				best = best.max((1.0 - d / fuzz).clamp(0.0, 1.0));
			}
			if let Some((cx, cy, r)) = localized {
				let dist = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
				best *= (1.0 - dist / r.max(1.0)).clamp(0.0, 1.0) as f32;
			}
			best
		}
		RangeKind::Reds | RangeKind::Yellows | RangeKind::Greens | RangeKind::Cyans | RangeKind::Blues | RangeKind::Magentas => {
			let centre = match range {
				RangeKind::Reds => 0.0,
				RangeKind::Yellows => 60.0,
				RangeKind::Greens => 120.0,
				RangeKind::Cyans => 180.0,
				RangeKind::Blues => 240.0,
				_ => 300.0,
			};
			let (h, s) = hue_sat(c);
			let dh = ((h - centre + 180.0).rem_euclid(360.0) - 180.0).abs();
			let w = (1.0 - (dh - 15.0) / 15.0).clamp(0.0, 1.0);
			w * (s * 2.0).min(1.0)
		}
		RangeKind::Highlights => ramp_up(luma(c) * 255.0, *high, 20.0),
		RangeKind::Shadows => 1.0 - ramp_up(luma(c) * 255.0, *low + 20.0, 20.0),
		RangeKind::Midtones => {
			let l = luma(c) * 255.0;
			ramp_up(l, *low + 20.0, 20.0) * (1.0 - ramp_up(l, *high, 20.0))
		}
		RangeKind::SkinTones => {
			let (h, s) = hue_sat(c);
			let l = luma(c);
			let hue = (1.0 - ((h + 20.0).rem_euclid(360.0) - 40.0).abs() / 30.0).clamp(0.0, 1.0);
			let sat = ramp_up(s, 0.12, 0.08) * (1.0 - ramp_up(s, 0.7, 0.1));
			let lum = ramp_up(l, 0.2, 0.1) * (1.0 - ramp_up(l, 0.97, 0.05));
			hue * sat * lum
		}
		RangeKind::OutOfGamut => 0.0,
	}
}

pub fn color_range(source: &dyn WandSource, size: (u32, u32), op: &SelectOp, depth: BitDepth, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let invert = matches!(op, SelectOp::ColorRange { invert: true, .. });
	assemble(size, depth, store, &|tx, ty| {
		let px = pixels(source.tile(tx, ty)?);
		let values: Vec<f32> = (0..TILE_PIXELS)
			.map(|i| {
				let p = px[i];
				let (x, y) = (
					f64::from(tx * TILE_SIZE) + (i as u32 % TILE_SIZE) as f64 + 0.5,
					f64::from(ty * TILE_SIZE) + (i as u32 / TILE_SIZE) as f64 + 0.5,
				);
				let v = if p[3] <= 0.0 { 0.0 } else { coverage(op, straight(p), x, y) * p[3].min(1.0) };
				if invert { 1.0 - v } else { v }
			})
			.collect();
		Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
	})
}
