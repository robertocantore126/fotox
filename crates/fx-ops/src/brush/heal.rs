//! The healing blend (M5-T08, D-045): inside the stroke, solve
//! `Δf = Δsource` with `f = before` on the boundary, per channel. With
//! `g = f − source` that is Laplace's equation `Δg = 0` with `g = before −
//! source` on the boundary: the source's texture, shifted to the
//! destination's shading. Solved on a pyramid (coarse solution → initial
//! guess → red-black Gauss–Seidel), memory ∝ the stroke's area.

/// Iterations of Gauss–Seidel on each level after the coarser one.
const SMOOTHING: usize = 60;
/// Below this many pixels, solve directly with many iterations.
const DIRECT_BELOW: usize = 48 * 48;

/// The eight spot-heal source candidates: `1.5 × diameter` away in the eight
/// compass directions (offsets destination − source).
pub fn spot_candidates(diameter: f64) -> Vec<(i64, i64)> {
	let r = (1.5 * diameter).round().max(2.0);
	(0..8)
		.map(|i| {
			let a = std::f64::consts::FRAC_PI_4 * f64::from(i);
			((r * a.cos()).round() as i64, (r * a.sin()).round() as i64)
		})
		.collect()
}

/// How badly a source fits around the stroke: the squared difference between
/// the destination and the source on a ring just outside the covered area.
pub fn ring_ssd(coverage: &[f32], before: &[[f32; 4]], source: &[[f32; 4]], w: usize, h: usize) -> f64 {
	let mut sum = 0.0f64;
	let mut n = 0usize;
	for y in 0..h {
		for x in 0..w {
			let i = y * w + x;
			if coverage[i] > 0.0 {
				continue;
			}
			// On the ring: a covered pixel within two pixels.
			let near = (y.saturating_sub(2)..(y + 3).min(h)).any(|yy| (x.saturating_sub(2)..(x + 3).min(w)).any(|xx| coverage[yy * w + xx] > 0.0));
			if !near {
				continue;
			}
			for c in 0..4 {
				let d = f64::from(before[i][c] - source[i][c]);
				sum += d * d;
			}
			n += 1;
		}
	}
	if n == 0 { f64::INFINITY } else { sum / n as f64 }
}

/// The healed pixels over the `w × h` area: `source + g`, where `g` is
/// harmonic inside the covered pixels and `before − source` elsewhere.
pub fn poisson(coverage: &[f32], before: &[[f32; 4]], source: &[[f32; 4]], w: usize, h: usize) -> Vec<[f32; 4]> {
	let unknown: Vec<bool> = coverage.iter().map(|s| *s > 0.0).collect();
	let mut out = vec![[0.0f32; 4]; w * h];
	for c in 0..4 {
		let mut g: Vec<f32> = (0..w * h).map(|i| if unknown[i] { 0.0 } else { before[i][c] - source[i][c] }).collect();
		// A start guess inside: the mean of the known values.
		let known: Vec<f32> = (0..w * h).filter(|i| !unknown[*i]).map(|i| g[i]).collect();
		let mean = if known.is_empty() {
			0.0
		} else {
			known.iter().sum::<f32>() / known.len() as f32
		};
		for (v, u) in g.iter_mut().zip(&unknown) {
			if *u {
				*v = mean;
			}
		}
		solve(&unknown, &mut g, w, h);
		for i in 0..w * h {
			out[i][c] = if unknown[i] { source[i][c] + g[i] } else { before[i][c] };
		}
	}
	// Premultiplied colour never exceeds its alpha.
	for p in &mut out {
		p[3] = p[3].clamp(0.0, 1.0);
		for c in 0..3 {
			p[c] = p[c].clamp(0.0, p[3]);
		}
	}
	out
}

/// Laplace's equation for the `unknown` pixels of `value` (the others are
/// fixed), on a pyramid.
fn solve(unknown: &[bool], value: &mut [f32], w: usize, h: usize) {
	if w * h <= DIRECT_BELOW || w < 4 || h < 4 {
		gauss_seidel(unknown, value, w, h, 400);
		return;
	}
	let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
	let mut coarse_unknown = vec![false; cw * ch];
	let mut coarse = vec![0.0f32; cw * ch];
	for cy in 0..ch {
		for cx in 0..cw {
			let (mut sum, mut n, mut any_unknown) = (0.0f32, 0u32, false);
			for y in cy * 2..(cy * 2 + 2).min(h) {
				for x in cx * 2..(cx * 2 + 2).min(w) {
					// Known children give a coarse boundary value; unknown ones
					// carry the current guess.
					let i = y * w + x;
					any_unknown |= unknown[i];
					sum += value[i];
					n += 1;
				}
			}
			coarse_unknown[cy * cw + cx] = any_unknown;
			coarse[cy * cw + cx] = if n > 0 { sum / n as f32 } else { 0.0 };
		}
	}
	solve(&coarse_unknown, &mut coarse, cw, ch);
	for y in 0..h {
		for x in 0..w {
			let i = y * w + x;
			if unknown[i] {
				value[i] = coarse[(y / 2) * cw + x / 2];
			}
		}
	}
	gauss_seidel(unknown, value, w, h, SMOOTHING);
}

/// Red-black Gauss–Seidel sweeps: each unknown pixel becomes the mean of its
/// neighbours (those inside the area).
fn gauss_seidel(unknown: &[bool], value: &mut [f32], w: usize, h: usize, sweeps: usize) {
	for _ in 0..sweeps {
		for colour in 0..2 {
			for y in 0..h {
				let start = (y + colour) % 2;
				for x in (start..w).step_by(2) {
					let i = y * w + x;
					if !unknown[i] {
						continue;
					}
					let (mut sum, mut n) = (0.0f32, 0u32);
					if x > 0 {
						sum += value[i - 1];
						n += 1;
					}
					if x + 1 < w {
						sum += value[i + 1];
						n += 1;
					}
					if y > 0 {
						sum += value[i - w];
						n += 1;
					}
					if y + 1 < h {
						sum += value[i + w];
						n += 1;
					}
					if n > 0 {
						value[i] = sum / n as f32;
					}
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_flat_source_takes_the_destinations_level() {
		// Destination flat 0.3; the patch copied from a flat 0.7 source.
		let (w, h) = (40, 30);
		let mut coverage = vec![0.0f32; w * h];
		for y in 8..22 {
			for x in 10..30 {
				coverage[y * w + x] = 1.0;
			}
		}
		let before = vec![[0.3, 0.3, 0.3, 1.0]; w * h];
		let source = vec![[0.7, 0.7, 0.7, 1.0]; w * h];
		let healed = poisson(&coverage, &before, &source, w, h);
		for (i, p) in healed.iter().enumerate() {
			assert!((p[0] - 0.3).abs() < 1.0 / 255.0, "pixel {i}: {p:?}");
		}
	}

	#[test]
	fn a_gradient_is_reproduced_inside_a_large_area() {
		// Linear destination, flat source: the solution is the plane.
		let (w, h) = (120, 90);
		let mut coverage = vec![0.0f32; w * h];
		for y in 5..85 {
			for x in 5..115 {
				coverage[y * w + x] = 1.0;
			}
		}
		let before: Vec<[f32; 4]> = (0..w * h)
			.map(|i| {
				let v = 0.2 + 0.5 * (i % w) as f32 / w as f32;
				[v, v, v, 1.0]
			})
			.collect();
		let source = vec![[0.5, 0.5, 0.5, 1.0]; w * h];
		let healed = poisson(&coverage, &before, &source, w, h);
		let worst = healed.iter().zip(&before).map(|(a, b)| (a[0] - b[0]).abs()).fold(0.0f32, f32::max);
		assert!(worst < 2.0 / 255.0, "worst {worst}");
	}
}
