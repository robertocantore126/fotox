//! A model's mask as a selection (M13-T02/T04): the working-resolution
//! mask upsampled to the canvas, then refined in the edge band only by
//! Select and Mask's guided filter (M9-T05), so the outline follows the
//! full-resolution edges instead of the model's 1024² grid.
//!
//! Tiles the mask sees as all 0 or all 1 are uniform and cost nothing: a big
//! document costs its outline's length.

use fx_core::select_ops::{ModelMask, Refine};
use fx_core::selection::{Selection, TileCoverage};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};

use super::assemble;
use crate::flood::WandSource;

pub fn model_selection(
	source: &dyn WandSource,
	mask: &ModelMask,
	size: (u32, u32),
	depth: BitDepth,
	store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	if mask.values.len() != mask.width as usize * mask.height as usize {
		return Err(CommandError::NotAllowed("the model's mask is malformed".into()));
	}
	let tile = i64::from(TILE_SIZE);
	let coarse = assemble(size, depth, store, &|tx, ty| {
		let (x0, y0) = (i64::from(tx) * tile, i64::from(ty) * tile);
		let (lo, hi) = mask.range_over(x0, y0, x0 + tile, y0 + tile);
		if hi == 0 {
			return Ok(None);
		}
		if lo == u8::MAX {
			return Ok(Some(TileCoverage::Uniform(1.0)));
		}
		let mut values = vec![0.0f32; TILE_PIXELS];
		for y in 0..tile {
			for x in 0..tile {
				values[(y * tile + x) as usize] = mask.at((x0 + x) as f64 + 0.5, (y0 + y) as f64 + 0.5);
			}
		}
		Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
	})?;
	let Some(coarse) = coarse else { return Ok(None) };
	if mask.refine_radius < 1.0 {
		return Ok(Some(coarse));
	}
	// VERIFY: Photoshop's Select Subject edge quality; Smart Radius keeps hard
	// edges hard.
	let refine = Refine {
		radius: mask.refine_radius,
		smart_radius: true,
		smooth: 0.0,
		feather: 0.0,
		contrast: 0.0,
		shift_edge: 0.0,
	};
	super::refine::refine(source, &coarse, size, &refine, depth, store)
}
