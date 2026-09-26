//! Select and Mask's refinement (M9-T05, D-070).
//!
//! Only the **edge band** is worked: a canvas tile whose neighbourhood (the
//! apron) is uniformly selected or unselected is copied as it is, so a big
//! document costs its boundary's length. In the band:
//!
//! 1. Edge Detection: a grey **guided filter** (He et al.) of the coverage
//!    with the image's luminance as the guide, window = Radius, applied where
//!    the coverage blurred over the radius is neither 0 nor 1 (the band).
//!    Smart Radius narrows the window where the guide has a strong edge.
//! 2. Global Refinements, in Photoshop's order: Smooth (a small blur), Feather
//!    (Gaussian σ = px), Contrast (a steeper ramp about ½), Shift Edge
//!    (± half a unit of coverage at ±100 %).
//!
//! FAST: Decontaminate Colors is not written (it changes pixels, the dialog
//! outputs a selection); Radius is capped at 64 px and Feather at 50 px.
//! VERIFY: every constant against Photoshop.

use fx_core::select_ops::Refine;
use fx_core::selection::{PatchReader, Selection, TileCoverage};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};

use super::focus::{blur, box_mean};
use super::{Windows, assemble, luma, straight};
use crate::flood::WandSource;

pub fn refine(
	source: &dyn WandSource,
	selection: &Selection,
	size: (u32, u32),
	params: &Refine,
	depth: BitDepth,
	store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	let radius = params.radius.clamp(0.0, 64.0);
	let feather = params.feather.clamp(0.0, 50.0);
	let smooth = params.smooth.clamp(0.0, 100.0) / 33.0;
	let apron = (radius.max(feather * 3.0).max(smooth * 3.0).ceil() as usize + 8).min(160);
	let side = TILE_SIZE as usize + 2 * apron;
	let windows = Windows::new(source, size);
	let tile_error = |e: fx_tiles::TileError| CommandError::Tile(e);
	assemble(size, depth, store, &|tx, ty| {
		let mut reader = PatchReader::new(selection, store, size);
		let x0 = i64::from(tx * TILE_SIZE) - apron as i64;
		let y0 = i64::from(ty * TILE_SIZE) - apron as i64;
		let (cx0, cy0) = (x0.max(0), y0.max(0));
		let (cx1, cy1) = ((x0 + side as i64).min(i64::from(size.0)), (y0 + side as i64).min(i64::from(size.1)));
		// Away from any edge: nothing to refine (Shift Edge and Contrast
		// leave 0 and 1 alone).
		for value in [0.0, 1.0] {
			if reader.is_uniform(cx0, cy0, cx1, cy1, value).map_err(tile_error)? {
				return Ok(Some(TileCoverage::Uniform(value)));
			}
		}
		let p = reader.patch(x0, y0, side, side).map_err(tile_error)?;
		let mut q = p.clone();
		if radius >= 1.0 {
			let px = windows.window(x0, y0, side, side)?;
			let guide: Vec<f32> = px.iter().map(|c| luma(straight(*c))).collect();
			let r = (radius / 2.0).max(1.0) as usize;
			let eps = 1e-3f32;
			let mean_i = box_mean(&guide, side, side, r);
			let mean_p = box_mean(&p, side, side, r);
			let ip: Vec<f32> = guide.iter().zip(&p).map(|(a, b)| a * b).collect();
			let ii: Vec<f32> = guide.iter().map(|a| a * a).collect();
			let corr_ip = box_mean(&ip, side, side, r);
			let corr_ii = box_mean(&ii, side, side, r);
			let a: Vec<f32> = (0..side * side)
				.map(|i| (corr_ip[i] - mean_i[i] * mean_p[i]) / (corr_ii[i] - mean_i[i] * mean_i[i] + eps))
				.collect();
			let b: Vec<f32> = (0..side * side).map(|i| mean_p[i] - a[i] * mean_i[i]).collect();
			let mean_a = box_mean(&a, side, side, r);
			let mean_b = box_mean(&b, side, side, r);
			// The band: where the coverage blurred over the radius is mixed.
			let band = box_mean(&p, side, side, radius as usize);
			// Smart Radius: where the guide has a strong edge, keep more of the
			// input (a narrower effective band).
			let grad = if params.smart_radius {
				let g = blur(&guide, side, side, 1.0);
				Some((0..side * side).map(|i| (guide[i] - g[i]).abs() * 8.0).collect::<Vec<f32>>())
			} else {
				None
			};
			for i in 0..side * side {
				if band[i] > 0.01 && band[i] < 0.99 {
					let filtered = (mean_a[i] * guide[i] + mean_b[i]).clamp(0.0, 1.0);
					let keep = grad.as_ref().map_or(0.0, |g| g[i].min(1.0) * 0.5);
					q[i] = filtered * (1.0 - keep) + p[i] * keep;
				}
			}
		}
		if smooth > 0.0 {
			q = blur(&q, side, side, smooth);
		}
		if feather > 0.0 {
			q = blur(&q, side, side, feather);
		}
		let contrast = params.contrast.clamp(0.0, 100.0) / 100.0;
		let shift = params.shift_edge.clamp(-100.0, 100.0) / 100.0;
		let mut values = vec![0.0f32; TILE_PIXELS];
		for y in 0..TILE_SIZE as usize {
			for x in 0..TILE_SIZE as usize {
				let mut v = q[(y + apron) * side + x + apron];
				if contrast > 0.0 {
					v = ((v - 0.5) * (1.0 + contrast * 9.0) + 0.5).clamp(0.0, 1.0);
				}
				if shift != 0.0 && v > 0.0 && v < 1.0 {
					v = (v + shift * 0.5).clamp(0.0, 1.0);
				}
				values[y * TILE_SIZE as usize + x] = v;
			}
		}
		Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
	})
}
