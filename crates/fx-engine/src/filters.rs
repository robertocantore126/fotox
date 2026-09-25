//! Live filter previews (M4-T05).
//!
//! While a filter dialog is open, the layer is shown filtered: only the
//! **visible** tiles, at the **view level** L, with distances scaled by 2⁻ᴸ
//! (docs/ARCHITECTURE.md §4.5). The preview tiles go into a copy of the
//! layer's image (`FilterPreview::image`) that the render snapshot uses in
//! place of the layer's own. The latest request wins: a job checks
//! `latest` between tiles and stops when a newer request (a slider moved, the
//! view changed) or a cancel superseded it. The UI never waits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fx_core::{FilterParams, LayerId};
use fx_ops::filter::{self, Geometry};
use fx_render::{ViewTransform, ViewportSize};
use fx_tiles::{TILE_SIZE, TileBuffer, TileError, TileStore, TiledImage};
use rayon::prelude::*;

use crate::ops::{ImageSource, prepare_levels};

/// Tiles computed per batch: each batch is shown as soon as it is ready.
const BATCH: usize = 8;

/// The preview state of one document.
pub struct FilterPreview {
	pub layer: LayerId,
	pub params: FilterParams,
	/// The layer's pixels when the preview started (the source).
	pub base: TiledImage,
	/// `base` with the preview tiles of `level` put in: what is displayed.
	pub image: TiledImage,
	/// The view level the preview is computed at.
	pub level: usize,
	/// The tile rectangle (inclusive, tiles of `level`) the current request covers.
	pub region: (u32, u32, u32, u32),
	/// The request whose tiles `image` holds / is receiving.
	pub request: u64,
}

/// What a preview job needs; runs on the rayon pool.
pub struct PreviewJob {
	pub request: u64,
	pub latest: Arc<AtomicU64>,
	pub params: FilterParams,
	pub base: TiledImage,
	pub geometry: Geometry,
	pub level: usize,
	pub region: (u32, u32, u32, u32),
	/// The region's tiles, nearest to the view centre first.
	pub tiles: Vec<(u32, u32)>,
}

impl PreviewJob {
	/// Compute the tiles, handing each finished batch to `send`. Stops early
	/// (without error) when a newer request superseded this one.
	pub fn run(self, store: &TileStore, send: impl Fn(Vec<((u32, u32), TileBuffer)>)) -> Result<(), TileError> {
		let current = || self.latest.load(Ordering::Relaxed) == self.request;
		let mut source_image = self.base.clone();
		prepare_levels(&mut source_image, store, &self.params, self.level, Some(self.region))?;
		let source = ImageSource { image: &source_image, store };
		for batch in self.tiles.chunks(BATCH) {
			if !current() {
				return Ok(());
			}
			let done = batch
				.par_iter()
				.map(|&(tx, ty)| filter::filter_tile(&source, &self.geometry, &self.params, self.level, tx, ty).map(|tile| ((tx, ty), tile)))
				.collect::<Result<Vec<_>, _>>()?;
			send(done);
		}
		Ok(())
	}
}

/// The tiles of `image` (a layer at `offset` in a `doc` of this size) visible
/// at `level`, as an inclusive rectangle of tiles and their list, nearest to
/// the view centre first. `None` when nothing of the layer is on screen.
#[allow(clippy::type_complexity)]
pub fn visible_tiles(
	view: &ViewTransform,
	viewport: ViewportSize,
	doc: (u32, u32),
	image: &TiledImage,
	offset: (i32, i32),
	level: usize,
) -> Option<((u32, u32, u32, u32), Vec<(u32, u32)>)> {
	let (x0, y0, x1, y1) = view.visible_doc_rect(viewport, doc.0, doc.1)?;
	let scale = f64::from(1u32 << level) * f64::from(TILE_SIZE);
	let grid = image.grid(level);
	let (cols, rows) = (i64::from(grid.cols()), i64::from(grid.rows()));
	let to_tile = |v: f64, o: i32| ((v - f64::from(o)) / scale).floor() as i64;
	let (tx0, ty0) = (to_tile(x0, offset.0).max(0), to_tile(y0, offset.1).max(0));
	let (tx1, ty1) = (to_tile(x1, offset.0).min(cols - 1), to_tile(y1, offset.1).min(rows - 1));
	if tx0 > tx1 || ty0 > ty1 {
		return None;
	}
	let (cx, cy) = ((tx0 + tx1) as f64 / 2.0, (ty0 + ty1) as f64 / 2.0);
	let mut tiles: Vec<(u32, u32)> = (ty0..=ty1).flat_map(|ty| (tx0..=tx1).map(move |tx| (tx as u32, ty as u32))).collect();
	tiles.sort_by(|a, b| {
		let d = |t: &(u32, u32)| (f64::from(t.0) - cx).powi(2) + (f64::from(t.1) - cy).powi(2);
		d(a).total_cmp(&d(b))
	});
	Some(((tx0 as u32, ty0 as u32, tx1 as u32, ty1 as u32), tiles))
}

/// Put a batch of preview tiles into the displayed image.
pub fn install(preview: &mut FilterPreview, store: &TileStore, tiles: Vec<((u32, u32), TileBuffer)>) {
	for ((tx, ty), tile) in tiles {
		if preview.level == 0 {
			preview.image.put_buffer(store, tx, ty, tile);
		} else {
			// A preview tile is derived data: evictable, never on scratch.
			let slot = match tile.uniform_value() {
				Some(value) if value.0 == [0; 4] => fx_tiles::TileSlot::Empty,
				Some(value) => fx_tiles::TileSlot::Solid(value),
				None => fx_tiles::TileSlot::Data(store.insert(tile, fx_tiles::TileClass::Derived)),
			};
			preview.image.set_derived_slot(preview.level, tx, ty, slot);
		}
	}
}
