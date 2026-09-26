//! Select ▸ Focus Area (M9-T04): the in-focus parts of the image.
//!
//! Per canvas tile, with an apron: the luminance's detail at two scales
//! (`|L − G_σ(L)|` for σ = 1 and 2, a difference-of-Gaussians stand-in for
//! the Laplacian of Gaussian), averaged over a 9 × 9 neighbourhood, minus the
//! noise floor, normalised to `e / (e + 0.02)`, thresholded softly at
//! `1 − in_focus`. Soften Edge blurs the result (σ 2).
//!
//! VERIFY: the normalisation constant and the threshold curve.
//! FAST: the add / subtract brushes of Photoshop's dialog are not there
//! (use Quick Selection or Quick Mask afterwards).

use fx_core::selection::{Selection, TileCoverage};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};

use super::{Windows, assemble, luma, straight};
use crate::flood::WandSource;

const APRON: usize = 16;

/// A separable Gaussian blur of a `w × h` buffer (edges clamped).
pub fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
	if sigma <= 0.0 {
		return src.to_vec();
	}
	let r = (sigma * 3.0).ceil() as isize;
	let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
	let sum: f32 = k.iter().sum();
	let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
	let mut tmp = vec![0.0f32; w * h];
	for y in 0..h {
		for x in 0..w {
			let mut acc = 0.0;
			for (i, kv) in k.iter().enumerate() {
				let xx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
				acc += src[y * w + xx] * kv;
			}
			tmp[y * w + x] = acc;
		}
	}
	let mut out = vec![0.0f32; w * h];
	for y in 0..h {
		for x in 0..w {
			let mut acc = 0.0;
			for (i, kv) in k.iter().enumerate() {
				let yy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
				acc += tmp[yy * w + x] * kv;
			}
			out[y * w + x] = acc;
		}
	}
	out
}

/// A box mean of radius `r` (edges clamped).
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
	let n = (2 * r + 1) as f32;
	let mut tmp = vec![0.0f32; w * h];
	for y in 0..h {
		for x in 0..w {
			let mut acc = 0.0;
			for d in 0..=2 * r {
				let xx = (x as isize + d as isize - r as isize).clamp(0, w as isize - 1) as usize;
				acc += src[y * w + xx];
			}
			tmp[y * w + x] = acc / n;
		}
	}
	let mut out = vec![0.0f32; w * h];
	for y in 0..h {
		for x in 0..w {
			let mut acc = 0.0;
			for d in 0..=2 * r {
				let yy = (y as isize + d as isize - r as isize).clamp(0, h as isize - 1) as usize;
				acc += tmp[yy * w + x];
			}
			out[y * w + x] = acc / n;
		}
	}
	out
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
	let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
	t * t * (3.0 - 2.0 * t)
}

pub fn focus_area(
	source: &dyn WandSource,
	size: (u32, u32),
	in_focus: f32,
	noise: f32,
	soften: bool,
	depth: BitDepth,
	store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	let windows = Windows::new(source, size);
	let side = TILE_SIZE as usize + 2 * APRON;
	let threshold = 1.0 - in_focus.clamp(0.0, 1.0);
	assemble(size, depth, store, &|tx, ty| {
		let x0 = i64::from(tx * TILE_SIZE) - APRON as i64;
		let y0 = i64::from(ty * TILE_SIZE) - APRON as i64;
		let px = windows.window(x0, y0, side, side)?;
		let l: Vec<f32> = px.iter().map(|p| luma(straight(*p)) * p[3].min(1.0)).collect();
		let b1 = blur(&l, side, side, 1.0);
		let b2 = blur(&l, side, side, 2.0);
		let detail: Vec<f32> = (0..side * side).map(|i| (l[i] - b1[i]).abs() + 0.5 * (l[i] - b2[i]).abs()).collect();
		let energy = box_mean(&detail, side, side, 4);
		let floor = noise.clamp(0.0, 1.0) * 0.03;
		let mut cov: Vec<f32> = energy
			.iter()
			.map(|e| {
				let e = (e - floor).max(0.0);
				let s = e / (e + 0.02);
				smoothstep(threshold - 0.08, threshold + 0.08, s)
			})
			.collect();
		if soften {
			cov = blur(&cov, side, side, 2.0);
		}
		let mut values = vec![0.0f32; TILE_PIXELS];
		for y in 0..TILE_SIZE as usize {
			for x in 0..TILE_SIZE as usize {
				values[y * TILE_SIZE as usize + x] = cov[(y + APRON) * side + x + APRON];
			}
		}
		Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
	})
}
