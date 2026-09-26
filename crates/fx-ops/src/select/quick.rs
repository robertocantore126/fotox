//! The Quick Selection tool (M9-T06, D-069): region growing from the pixels
//! under the stroke, on colour and edge strength.
//!
//! The stroke's window (its dabs' bounds plus a margin) is read once; the
//! seeds' mean colour and spread set the colour tolerance; a 4-connected
//! flood from the seeds takes a neighbour when its colour is within the
//! tolerance and the luminance gradient there is below an edge threshold, so
//! it stops at strong edges. Enhance Edge softens the result's border
//! (σ 1 blur, then a steep ramp).
//!
//! FAST: computed at level 0 in a window capped at 1536² around the stroke
//! (D-069's coarse-level pass and boundary refinement are not written);
//! VERIFY: the tolerance and edge constants.

use std::collections::VecDeque;

use fx_core::selection::{Selection, TileCoverage};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};

use super::focus::blur;
use super::{Windows, assemble, luma, straight};
use crate::flood::WandSource;

const MAX_WINDOW: i64 = 1536;

pub fn quick_select(
	source: &dyn WandSource,
	size: (u32, u32),
	dabs: &[(f64, f64, f64)],
	enhance_edge: bool,
	depth: BitDepth,
	store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	if dabs.is_empty() {
		return Ok(None);
	}
	let max_r = dabs.iter().map(|d| d.2).fold(1.0, f64::max);
	let margin = (max_r * 4.0).max(64.0);
	let (mut bx0, mut by0, mut bx1, mut by1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
	for &(x, y, r) in dabs {
		bx0 = bx0.min(x - r);
		by0 = by0.min(y - r);
		bx1 = bx1.max(x + r);
		by1 = by1.max(y + r);
	}
	let x0 = ((bx0 - margin).floor() as i64).max(0);
	let y0 = ((by0 - margin).floor() as i64).max(0);
	let x1 = ((bx1 + margin).ceil() as i64).min(i64::from(size.0));
	let y1 = ((by1 + margin).ceil() as i64).min(i64::from(size.1));
	// Keep the window bounded, centred on the stroke.
	let (cx, cy) = ((bx0 + bx1) / 2.0, (by0 + by1) / 2.0);
	let x0 = x0.max(cx as i64 - MAX_WINDOW / 2);
	let y0 = y0.max(cy as i64 - MAX_WINDOW / 2);
	let x1 = x1.min(x0 + MAX_WINDOW);
	let y1 = y1.min(y0 + MAX_WINDOW);
	if x1 <= x0 || y1 <= y0 {
		return Ok(None);
	}
	let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
	let px = Windows::new(source, size).window(x0, y0, w, h)?;
	let rgb: Vec<[f32; 3]> = px.iter().map(|p| straight(*p)).collect();
	let lum: Vec<f32> = rgb.iter().map(|c| luma(*c)).collect();
	// Gradient magnitude (central differences).
	let mut grad = vec![0.0f32; w * h];
	for y in 1..h.saturating_sub(1) {
		for x in 1..w.saturating_sub(1) {
			let gx = lum[y * w + x + 1] - lum[y * w + x - 1];
			let gy = lum[(y + 1) * w + x] - lum[(y - 1) * w + x];
			grad[y * w + x] = (gx * gx + gy * gy).sqrt();
		}
	}
	// Seeds: the pixels under the dabs.
	let mut region = vec![false; w * h];
	let mut queue = VecDeque::new();
	let mut sum = [0.0f64; 3];
	let mut sum2 = [0.0f64; 3];
	let mut n = 0.0f64;
	for &(dx, dy, r) in dabs {
		let r2 = r * r;
		for y in ((dy - r).floor() as i64).max(y0)..((dy + r).ceil() as i64).min(y1) {
			for x in ((dx - r).floor() as i64).max(x0)..((dx + r).ceil() as i64).min(x1) {
				let (fx, fy) = (x as f64 + 0.5 - dx, y as f64 + 0.5 - dy);
				if fx * fx + fy * fy > r2 {
					continue;
				}
				let i = (y - y0) as usize * w + (x - x0) as usize;
				if !region[i] {
					region[i] = true;
					queue.push_back(i);
					for c in 0..3 {
						let v = f64::from(rgb[i][c]);
						sum[c] += v;
						sum2[c] += v * v;
					}
					n += 1.0;
				}
			}
		}
	}
	if n == 0.0 {
		return Ok(None);
	}
	let mean = [0, 1, 2].map(|c| (sum[c] / n) as f32);
	let spread = (0..3)
		.map(|c| ((sum2[c] / n - (sum[c] / n).powi(2)).max(0.0)).sqrt() as f32)
		.fold(0.0, f32::max);
	let tolerance = spread * 2.5 + 0.08;
	let edge = 0.12f32;
	while let Some(i) = queue.pop_front() {
		let (x, y) = (i % w, i / w);
		let neighbours = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)];
		for (nx, ny) in neighbours {
			if nx >= w || ny >= h {
				continue;
			}
			let j = ny * w + nx;
			if region[j] || grad[j] > edge {
				continue;
			}
			let d = (0..3).map(|c| (rgb[j][c] - mean[c]).abs()).fold(0.0, f32::max);
			if d <= tolerance {
				region[j] = true;
				queue.push_back(j);
			}
		}
	}
	let mut cov: Vec<f32> = region.iter().map(|&r| if r { 1.0 } else { 0.0 }).collect();
	if enhance_edge {
		cov = blur(&cov, w, h, 1.0).into_iter().map(|v| ((v - 0.5) * 3.0 + 0.5).clamp(0.0, 1.0)).collect();
	}
	let tile = i64::from(TILE_SIZE);
	assemble(size, depth, store, &|tx, ty| {
		let (ox, oy) = (i64::from(tx) * tile, i64::from(ty) * tile);
		if ox + tile <= x0 || oy + tile <= y0 || ox >= x1 || oy >= y1 {
			return Ok(None);
		}
		let mut values = vec![0.0f32; TILE_PIXELS];
		for py in 0..tile {
			for px_ in 0..tile {
				let (x, y) = (ox + px_, oy + py);
				if x >= x0 && y >= y0 && x < x1 && y < y1 {
					values[(py * tile + px_) as usize] = cov[(y - y0) as usize * w + (x - x0) as usize];
				}
			}
		}
		Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
	})
}
