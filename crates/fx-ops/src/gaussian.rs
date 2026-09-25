//! Gaussian blur of a premultiplied neighbourhood (M4-T06).
//!
//! Exact separable convolution with a normalised kernel of half-width
//! `ceil(3σ)`. Large radii never reach this function with σ > 32: the filter
//! driver blurs a coarser mip level instead (see `filter.rs`), which keeps the
//! apron — and the memory of one tile's neighbourhood — bounded.

use crate::neighbourhood::Px;

/// Below this σ the blur is the identity (the kernel's side weights vanish).
pub const MIN_SIGMA: f32 = 0.3;

/// Normalised 1-D kernel for `sigma` (SNIPPETS §5); `[1.0]` below [`MIN_SIGMA`].
pub fn kernel(sigma: f32) -> Vec<f32> {
	if sigma < MIN_SIGMA {
		return vec![1.0];
	}
	let r = (3.0 * sigma).ceil() as i32;
	let mut k: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
	let sum: f32 = k.iter().sum();
	k.iter_mut().for_each(|v| *v /= sum);
	k
}

/// Half-width of [`kernel`] in pixels: the apron a blur of `sigma` needs.
pub fn radius(sigma: f32) -> usize {
	if sigma < MIN_SIGMA { 0 } else { (3.0 * sigma).ceil() as usize }
}

/// Blur a `w × h` premultiplied image; returns the `(w − 2r) × (h − 2r)`
/// interior (`r` = [`radius`]), row-major. The input's apron is consumed.
pub fn blur(src: &[Px], w: usize, h: usize, sigma: f32) -> Vec<Px> {
	let k = kernel(sigma);
	let r = k.len() / 2;
	debug_assert_eq!(src.len(), w * h);
	debug_assert!(w > 2 * r && h > 2 * r, "neighbourhood smaller than the kernel");
	if r == 0 {
		return src.to_vec();
	}
	let (ow, oh) = (w - 2 * r, h - 2 * r);
	// Horizontal pass over every row (the vertical pass needs the apron rows).
	let mut tmp = vec![[0.0f32; 4]; ow * h];
	for y in 0..h {
		let row = &src[y * w..(y + 1) * w];
		for x in 0..ow {
			let mut acc = [0.0f32; 4];
			for (i, &weight) in k.iter().enumerate() {
				let p = row[x + i];
				for c in 0..4 {
					acc[c] += p[c] * weight;
				}
			}
			tmp[y * ow + x] = acc;
		}
	}
	// Vertical pass.
	let mut out = vec![[0.0f32; 4]; ow * oh];
	for y in 0..oh {
		for x in 0..ow {
			let mut acc = [0.0f32; 4];
			for (i, &weight) in k.iter().enumerate() {
				let p = tmp[(y + i) * ow + x];
				for c in 0..4 {
					acc[c] += p[c] * weight;
				}
			}
			out[y * ow + x] = acc;
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_kernel_is_normalised_and_symmetric() {
		for sigma in [0.5, 1.0, 4.0, 13.7, 32.0] {
			let k = kernel(sigma);
			assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
			assert_eq!(k.len(), 2 * radius(sigma) + 1);
			for i in 0..k.len() / 2 {
				assert_eq!(k[i], k[k.len() - 1 - i]);
			}
		}
		assert_eq!(kernel(0.1), vec![1.0]);
	}

	#[test]
	fn a_constant_image_stays_constant() {
		let sigma = 3.0;
		let r = radius(sigma);
		let (w, h) = (20 + 2 * r, 10 + 2 * r);
		let src = vec![[0.2, 0.4, 0.6, 1.0]; w * h];
		for p in blur(&src, w, h, sigma) {
			for (c, v) in [0.2, 0.4, 0.6, 1.0].into_iter().enumerate() {
				assert!((p[c] - v).abs() < 1e-5, "{p:?}");
			}
		}
	}

	#[test]
	fn matches_a_double_precision_reference() {
		let sigma = 2.5f32;
		let r = radius(sigma);
		let (w, h) = (16 + 2 * r, 16 + 2 * r);
		let value = |x: usize, y: usize| (((x * 7919 + y * 104_729) % 1000) as f64) / 1000.0;
		let src: Vec<Px> = (0..w * h).map(|i| [value(i % w, i / w) as f32, 0.0, 0.0, 1.0]).collect();
		let out = blur(&src, w, h, sigma);
		let s = f64::from(sigma);
		let g = |d: i64| (-((d * d) as f64) / (2.0 * s * s)).exp();
		let norm: f64 = (-(r as i64)..=r as i64).map(g).sum();
		for (oy, ox) in [(0usize, 0usize), (7, 3), (15, 15)] {
			let mut expected = 0.0;
			for dy in -(r as i64)..=r as i64 {
				for dx in -(r as i64)..=r as i64 {
					let (x, y) = ((ox + r) as i64 + dx, (oy + r) as i64 + dy);
					expected += value(x as usize, y as usize) * g(dx) * g(dy);
				}
			}
			expected /= norm * norm;
			let got = f64::from(out[oy * 16 + ox][0]);
			assert!((got - expected).abs() < 4.0 / 65535.0, "({ox},{oy}): {got} vs {expected}");
		}
	}
}
