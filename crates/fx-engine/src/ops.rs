//! The engine's implementation of [`fx_core::PixelOps`] (M4-T05, recipe R1a):
//! filters with `fx-ops` over the tile store, compositing with the CPU
//! reference compositor (M4-T08).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fx_core::{CommandError, Document, FilterParams, LayerId, PixelOps};
use fx_ops::filter::{self, Geometry};
use fx_ops::neighbourhood::{LevelSource, TileRef};
use fx_tiles::{PixelFormat, TILE_SIZE, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

use crate::mips;

/// One output tile of a filter.
type FilteredTile = ((u32, u32), fx_tiles::TileBuffer);

/// Reports a job's progress, 0..=1.
pub type ProgressFn = dyn Fn(f32) + Send + Sync;

/// The engine's pixel operations. `progress` (optional) hears about long
/// operations, for the status bar.
#[derive(Default)]
pub struct EngineOps {
	pub progress: Option<Arc<ProgressFn>>,
}

impl PixelOps for EngineOps {
	fn filter(&self, image: &TiledImage, offset: (i32, i32), canvas: (u32, u32), filter: &FilterParams, store: &TileStore) -> Result<TiledImage, CommandError> {
		let geometry = Geometry {
			offset,
			canvas,
			image: (image.width(), image.height()),
		};
		let tiles = filter::output_tiles(image, &geometry, filter, 0);
		let mut source_image = image.clone();
		prepare_levels(&mut source_image, store, filter, 0, None)?;
		let done = AtomicUsize::new(0);
		let total = tiles.len().max(1);
		let source = ImageSource { image: &source_image, store };
		let results: Vec<Result<FilteredTile, TileError>> = tiles
			.par_iter()
			.map(|&(tx, ty)| {
				let tile = filter::filter_tile(&source, &geometry, filter, 0, tx, ty)?;
				let n = done.fetch_add(1, Ordering::Relaxed) + 1;
				if let Some(progress) = &self.progress
					&& (n * 100 / total) != ((n - 1) * 100 / total)
				{
					progress(n as f32 / total as f32);
				}
				Ok(((tx, ty), tile))
			})
			.collect();
		let mut out = image.clone();
		for result in results {
			let ((tx, ty), tile) = result?;
			out.put_buffer(store, tx, ty, tile);
		}
		Ok(out)
	}

	fn composite(&self, doc: &Document, layers: &[LayerId], background: Option<[u16; 4]>, store: &TileStore) -> Result<TiledImage, CommandError> {
		crate::export::composite_layers(doc, layers, background, store, self.progress.as_deref())
	}
}

/// Make the mip levels a filter at `level` reads valid in `image` (a working
/// copy): `level` itself and the coarser level a large blur uses. `tiles`
/// limits the work to a region (a preview's visible tiles, in tiles of
/// `level`); `None` = the whole image.
pub(crate) fn prepare_levels(
	image: &mut TiledImage,
	store: &TileStore,
	filter: &FilterParams,
	level: usize,
	tiles: Option<(u32, u32, u32, u32)>,
) -> Result<(), TileError> {
	let blur_level = level + filter::extra_levels(filter, level);
	for lvl in [level, blur_level] {
		if lvl == 0 || lvl >= image.level_count() {
			continue;
		}
		match tiles {
			None => {
				let grid = image.grid(lvl).clone();
				for ty in 0..grid.rows() {
					for tx in 0..grid.cols() {
						// Recomputes only what is dirty or was evicted.
						mips::ensure_mip(image, store, lvl, tx, ty)?;
					}
				}
			}
			Some((x0, y0, x1, y1)) => {
				// The region at `lvl`, grown by the blur's apron (a few tiles at most).
				let shift = lvl - level;
				let apron = 1 + (3.0 * 32.0 / TILE_SIZE as f32).ceil() as u32;
				let grid = image.grid(lvl).clone();
				let (gx0, gy0) = ((x0 >> shift).saturating_sub(apron), (y0 >> shift).saturating_sub(apron));
				let (gx1, gy1) = (
					((x1 >> shift) + apron).min(grid.cols().saturating_sub(1)),
					((y1 >> shift) + apron).min(grid.rows().saturating_sub(1)),
				);
				for ty in gy0..=gy1 {
					for tx in gx0..=gx1 {
						// Recomputes only what is dirty or was evicted.
						mips::ensure_mip(image, store, lvl, tx, ty)?;
					}
				}
			}
		}
	}
	Ok(())
}

/// A `TiledImage` as the filters' [`LevelSource`].
pub(crate) struct ImageSource<'a> {
	pub image: &'a TiledImage,
	pub store: &'a TileStore,
}

impl LevelSource for ImageSource<'_> {
	fn format(&self) -> PixelFormat {
		self.image.format()
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		if level >= self.image.level_count() {
			return Ok(None);
		}
		let grid = self.image.grid(level);
		if tx < 0 || ty < 0 || tx >= i64::from(grid.cols()) || ty >= i64::from(grid.rows()) {
			return Ok(None);
		}
		Ok(match self.image.slot(level, tx as u32, ty as u32) {
			TileSlot::Empty => None,
			TileSlot::Solid(value) => Some(TileRef::Solid(value.0)),
			TileSlot::Data(handle) => Some(TileRef::Data(self.store.get(handle)?)),
		})
	}
}
