//! Resampling kernels and the tap weights over one axis (M6-T01, D-052).
//!
//! Pixel centres are at `i + 0.5` (SNIPPETS §11), so a source coordinate `u`
//! lies in pixel `i` when `i ≤ u < i + 1` and the tap set is built from
//! `base = floor(u − 0.5)`. Getting this half-pixel wrong shifts the whole
//! image on every transform.

use fx_core::Filter;

/// Largest number of taps a filter takes over one axis (Lanczos-3 → 6).
pub const MAX_TAPS: usize = 6;

/// The triangle kernel (bilinear).
#[inline]
pub fn linear(x: f64) -> f64 {
	(1.0 - x.abs()).max(0.0)
}

/// Keys' cubic; `a = −0.5` is Photoshop's "Bicubic", `a = −0.75` "Bicubic
/// Sharper".
#[inline]
pub fn keys(x: f64, a: f64) -> f64 {
	let x = x.abs();
	if x < 1.0 {
		((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
	} else if x < 2.0 {
		((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a
	} else {
		0.0
	}
}

/// Mitchell–Netravali with `B = C = 1/3` — Photoshop's "Bicubic Smoother".
#[inline]
pub fn mitchell(x: f64) -> f64 {
	const B: f64 = 1.0 / 3.0;
	const C: f64 = 1.0 / 3.0;
	let x = x.abs();
	if x < 1.0 {
		((12.0 - 9.0 * B - 6.0 * C) * x * x * x + (-18.0 + 12.0 * B + 6.0 * C) * x * x + (6.0 - 2.0 * B)) / 6.0
	} else if x < 2.0 {
		((-B - 6.0 * C) * x * x * x + (6.0 * B + 30.0 * C) * x * x + (-12.0 * B - 48.0 * C) * x + (8.0 * B + 24.0 * C)) / 6.0
	} else {
		0.0
	}
}

/// The normalised cardinal sine.
#[inline]
fn sinc(x: f64) -> f64 {
	if x == 0.0 {
		1.0
	} else {
		let p = std::f64::consts::PI * x;
		p.sin() / p
	}
}

/// Lanczos with 3 lobes.
#[inline]
pub fn lanczos3(x: f64) -> f64 {
	let x = x.abs();
	if x < 3.0 { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// The kernel value of `filter` at distance `x` from the sample point, in
/// source pixels. `BicubicAutomatic` behaves as `Bicubic` (it is resolved
/// before sampling, see [`Filter::resolve`]).
#[inline]
pub fn kernel(filter: Filter, x: f64) -> f64 {
	match filter {
		Filter::Nearest => {
			if x.abs() < 0.5 {
				1.0
			} else {
				0.0
			}
		}
		Filter::Bilinear => linear(x),
		Filter::Bicubic | Filter::BicubicAutomatic => keys(x, -0.5),
		Filter::BicubicSharper => keys(x, -0.75),
		Filter::BicubicSmoother => mitchell(x),
		Filter::Lanczos3 => lanczos3(x),
	}
}

/// The taps of `filter` around source coordinate `u`, as `(index, weight)`
/// pairs written into `out`; the return value is how many were written.
///
/// Weights are normalised so a constant image stays constant even when the
/// truncated kernel does not sum to exactly 1. A numerically empty sum (a
/// degenerate position) falls back to the nearest pixel.
pub fn taps(filter: Filter, u: f64, out: &mut [(i64, f64); MAX_TAPS]) -> usize {
	if matches!(filter, Filter::Nearest) {
		out[0] = ((u - 0.5).round() as i64, 1.0);
		return 1;
	}
	let s = filter.support() as i64;
	let base = (u - 0.5).floor() as i64;
	let count = (2 * s) as usize;
	let mut sum = 0.0;
	for (k, slot) in out.iter_mut().take(count).enumerate() {
		let index = base - s + 1 + k as i64;
		let weight = kernel(filter, u - (index as f64 + 0.5));
		*slot = (index, weight);
		sum += weight;
	}
	if sum.abs() < 1e-9 {
		out[0] = ((u - 0.5).round() as i64, 1.0);
		return 1;
	}
	for slot in out.iter_mut().take(count) {
		slot.1 /= sum;
	}
	count
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn interpolating_kernels_are_exact_at_the_pixel_centre() {
		for filter in [Filter::Bilinear, Filter::Bicubic, Filter::BicubicSharper, Filter::Lanczos3] {
			let mut out = [(0i64, 0.0); MAX_TAPS];
			let n = taps(filter, 5.5, &mut out);
			let centre = out[..n].iter().find(|(i, _)| *i == 5).expect("the centre tap is present");
			assert!((centre.1 - 1.0).abs() < 1e-12, "{filter:?}: {out:?}");
			let rest: f64 = out[..n].iter().filter(|(i, _)| *i != 5).map(|(_, w)| w.abs()).sum();
			assert!(rest < 1e-12, "{filter:?}: other taps are zero: {out:?}");
		}
	}

	#[test]
	fn bicubic_smoother_approximates_instead_of_interpolating() {
		// Mitchell–Netravali is an *approximating* kernel: at a pixel centre the
		// centre tap is 8/9, not 1, so "Bicubic Smoother" is a mild blur. That is
		// Photoshop's behaviour for that filter too; the exact cases (identity,
		// whole-pixel moves) are handled by the sampler's copy shortcut.
		let mut out = [(0i64, 0.0); MAX_TAPS];
		let n = taps(Filter::BicubicSmoother, 5.5, &mut out);
		let centre = out[..n].iter().find(|(i, _)| *i == 5).expect("the centre tap is present");
		assert!((centre.1 - 8.0 / 9.0).abs() < 1e-9, "{out:?}");
		let neighbours: f64 = out[..n].iter().filter(|(i, _)| *i == 4 || *i == 6).map(|(_, w)| w).sum();
		assert!((neighbours - 1.0 / 9.0).abs() < 1e-9, "{out:?}");
	}

	#[test]
	fn taps_are_symmetric_at_the_half_pixel() {
		for filter in [Filter::Bilinear, Filter::Bicubic, Filter::BicubicSmoother, Filter::Lanczos3] {
			let mut out = [(0i64, 0.0); MAX_TAPS];
			let n = taps(filter, 5.0, &mut out);
			// u = 5.0 sits between pixel 4 (centre 4.5) and pixel 5 (5.5).
			for k in 0..n / 2 {
				assert!((out[k].1 - out[n - 1 - k].1).abs() < 1e-12, "{filter:?}: {out:?}");
			}
			assert_eq!(out[0].0, 5 - (n as i64) / 2);
		}
	}

	#[test]
	fn weights_sum_to_one_everywhere() {
		for filter in [
			Filter::Bilinear,
			Filter::Bicubic,
			Filter::BicubicSmoother,
			Filter::BicubicSharper,
			Filter::Lanczos3,
		] {
			for k in 0..40 {
				let u = -3.0 + k as f64 * 0.23;
				let mut out = [(0i64, 0.0); MAX_TAPS];
				let n = taps(filter, u, &mut out);
				let sum: f64 = out[..n].iter().map(|(_, w)| w).sum();
				assert!((sum - 1.0).abs() < 1e-9, "{filter:?} at {u}: {sum}");
			}
		}
	}

	#[test]
	fn nearest_picks_the_closest_centre() {
		// Pixel `i` has its centre at `i + 0.5`.
		let mut out = [(0i64, 0.0); MAX_TAPS];
		assert_eq!(taps(Filter::Nearest, 4.4, &mut out), 1);
		assert_eq!(out[0], (4, 1.0), "4.4 is closer to 4.5 than to 3.5");
		assert_eq!(taps(Filter::Nearest, 5.6, &mut out), 1);
		assert_eq!(out[0], (5, 1.0), "5.6 is closer to 5.5 than to 6.5");
		assert_eq!(taps(Filter::Nearest, -0.4, &mut out), 1);
		assert_eq!(out[0], (-1, 1.0));
	}

	#[test]
	fn lanczos_has_six_taps_and_bicubic_four() {
		let mut out = [(0i64, 0.0); MAX_TAPS];
		assert_eq!(taps(Filter::Lanczos3, 10.3, &mut out), 6);
		assert_eq!(taps(Filter::Bicubic, 10.3, &mut out), 4);
		assert_eq!(taps(Filter::Bilinear, 10.3, &mut out), 2);
	}
}
