//! Selections computed from pixels (M9-T02..T07): Grow and Similar, Color
//! Range, Focus Area, Select and Mask's refinement, Quick Selection and the
//! Magnetic Lasso's live-wire.
//!
//! The pixels come through the Magic Wand's [`WandSource`] (the active layer
//! or the composite, canvas tiles, premultiplied `0..=1`). Results are
//! assembled one row of tiles at a time, so nothing the size of the document
//! is held (rule 2).

use std::collections::HashMap;
use std::sync::Mutex;

use fx_core::selection::{OutTile, Selection, TileCoverage, canvas_grid, out_tile, uniform_slot, valid_extent};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};
use rayon::prelude::*;

use crate::flood::{WandSource, WandTile};

pub mod focus;
pub mod grow;
pub mod livewire;
pub mod model;
pub mod quick;
pub mod range;
pub mod refine;

/// A canvas tile's pixels as a full vector (premultiplied RGBA `0..=1`).
pub fn pixels(tile: WandTile) -> Vec<[f32; 4]> {
	match tile {
		WandTile::Uniform(c) => vec![c; TILE_PIXELS],
		WandTile::Data(p) => p,
	}
}

/// Straight RGB of a premultiplied pixel.
pub fn straight(p: [f32; 4]) -> [f32; 3] {
	if p[3] > 0.0 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { [0.0; 3] }
}

/// Build a selection from a per-tile coverage function, a row of tiles at a
/// time in parallel. `f(tx, ty)` returns the tile's coverage, `None` = 0.
pub fn assemble(
	size: (u32, u32),
	depth: BitDepth,
	store: &TileStore,
	f: &(dyn Fn(u32, u32) -> Result<Option<TileCoverage>, CommandError> + Sync),
) -> Result<Option<Selection>, CommandError> {
	let (cols, rows) = canvas_grid(size);
	let mut result = Selection::empty(size, depth);
	let format = result.image.format();
	let mut any = false;
	for ty in 0..rows {
		let row: Result<Vec<(u32, Option<OutTile>)>, CommandError> = (0..cols)
			.into_par_iter()
			.map(|tx| {
				Ok((
					tx,
					f(tx, ty)?.map(|c| match c {
						TileCoverage::Uniform(v) => OutTile::Uniform(v),
						c => out_tile(format, &c.into_values(), valid_extent(size, tx, ty)),
					}),
				))
			})
			.collect();
		for (tx, tile) in row? {
			match tile {
				None => {}
				Some(OutTile::Uniform(v)) => {
					let slot = uniform_slot(format, v);
					any |= !slot.is_empty();
					result.image.set_slot(tx, ty, slot);
				}
				Some(OutTile::Data(buffer)) => {
					result.image.put_buffer(store, tx, ty, buffer);
					any = true;
				}
			}
		}
	}
	Ok(any.then_some(result))
}

/// Reads windows of canvas pixels across tile borders (an apron for a
/// filter), caching the tiles it has read. Outside the canvas: the nearest
/// canvas pixel (D-036's edge replicate).
pub struct Windows<'a> {
	source: &'a dyn WandSource,
	size: (u32, u32),
	cache: Mutex<HashMap<(u32, u32), std::sync::Arc<Vec<[f32; 4]>>>>,
}

impl<'a> Windows<'a> {
	pub fn new(source: &'a dyn WandSource, size: (u32, u32)) -> Self {
		Self {
			source,
			size,
			cache: Mutex::new(HashMap::new()),
		}
	}

	fn tile(&self, tx: u32, ty: u32) -> Result<std::sync::Arc<Vec<[f32; 4]>>, CommandError> {
		if let Some(t) = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(tx, ty)) {
			return Ok(t.clone());
		}
		let t = std::sync::Arc::new(pixels(self.source.tile(tx, ty)?));
		let mut cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		// FAST: a crude bound on the cache (a filter pass reads each tile a few times).
		if cache.len() > 512 {
			cache.clear();
		}
		cache.insert((tx, ty), t.clone());
		Ok(t)
	}

	/// The `w × h` window whose top-left canvas pixel is `(x0, y0)`.
	pub fn window(&self, x0: i64, y0: i64, w: usize, h: usize) -> Result<Vec<[f32; 4]>, CommandError> {
		let tile = i64::from(TILE_SIZE);
		let (cw, ch) = (i64::from(self.size.0), i64::from(self.size.1));
		let mut out = vec![[0.0f32; 4]; w * h];
		let mut local: HashMap<(i64, i64), std::sync::Arc<Vec<[f32; 4]>>> = HashMap::new();
		for y in 0..h as i64 {
			let cy = (y0 + y).clamp(0, ch - 1);
			for x in 0..w as i64 {
				let cx = (x0 + x).clamp(0, cw - 1);
				let key = (cx / tile, cy / tile);
				if !local.contains_key(&key) {
					local.insert(key, self.tile(key.0 as u32, key.1 as u32)?);
				}
				out[y as usize * w + x as usize] = local[&key][((cy % tile) * tile + cx % tile) as usize];
			}
		}
		Ok(out)
	}
}

/// Luminance of straight RGB.
pub fn luma(c: [f32; 3]) -> f32 {
	0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}
