//! Smart Object tiles (M12-T01, D-082): a requested cache tile at level L is
//! resampled from the source composite's mips through the Smart Object's
//! transform (M6-T01's sampler picks the source level from the scale), then
//! run through the Smart Filters (M12-T03).
//!
//! The source's mips are computed as the sampler reads them, and held until
//! the batch is drawn (code review 2026-09-27 R06: the first draw used to make
//! every mip level of the composite valid, a cost proportional to the whole
//! source, and a mip the trim dropped before the sampler read it failed the
//! draw). FAST: Bicubic always.

use std::collections::HashMap;
use std::sync::Mutex;

use fx_core::LayerKind;
use fx_ops::neighbourhood::{LevelSource, TileRef};
use fx_ops::resample::SourceInfo;
use fx_tiles::{PixelFormat, TileError, TileSlot, TileStore, TiledImage};

/// How often a tile is recomputed when the trim drops it between being
/// computed and being read.
const MAX_RETRIES: usize = 4;

/// The source composite as the sampler's [`LevelSource`]: a mip tile is
/// computed when it is first read (from the level below, itself computed on
/// demand), and every tile handed out is held until the draw ends.
struct LazyMips<'a> {
	image: Mutex<TiledImage>,
	store: &'a TileStore,
	format: PixelFormat,
	held: Mutex<HashMap<(usize, u32, u32), Option<TileRef>>>,
}

impl<'a> LazyMips<'a> {
	fn new(image: TiledImage, store: &'a TileStore) -> Self {
		Self {
			format: image.format(),
			image: Mutex::new(image),
			store,
			held: Mutex::new(HashMap::new()),
		}
	}

	/// The composite with the mips computed so far.
	fn into_image(self) -> TiledImage {
		self.image.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
	}

	fn read(&self, level: usize, tx: u32, ty: u32) -> Result<Option<TileRef>, TileError> {
		for _ in 0..MAX_RETRIES {
			let slot = {
				let mut image = self.image.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
				if level == 0 {
					image.slot(0, tx, ty).clone()
				} else {
					crate::mips::ensure_mip(&mut image, self.store, level, tx, ty)?
				}
			};
			match slot {
				TileSlot::Empty => return Ok(None),
				TileSlot::Solid(value) => return Ok(Some(TileRef::Solid(value.0))),
				TileSlot::Data(handle) => match self.store.get(&handle) {
					Ok(buffer) => return Ok(Some(TileRef::Data(buffer))),
					// Dropped between being computed and being read: again.
					Err(TileError::Evicted) => continue,
					Err(error) => return Err(error),
				},
			}
		}
		Err(TileError::Evicted)
	}
}

impl LevelSource for LazyMips<'_> {
	fn format(&self) -> PixelFormat {
		self.format
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		let (cols, rows) = {
			let image = self.image.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
			if level >= image.level_count() {
				return Ok(None);
			}
			let grid = image.grid(level);
			(grid.cols(), grid.rows())
		};
		if tx < 0 || ty < 0 || tx >= i64::from(cols) || ty >= i64::from(rows) {
			return Ok(None);
		}
		let key = (level, tx as u32, ty as u32);
		if let Some(tile) = self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
			return Ok(tile.clone());
		}
		let tile = self.read(level, key.1, key.2)?;
		self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, tile.clone());
		Ok(tile)
	}
}

/// Draw `tiles` (`(level, tx, ty)`) of Smart Object `id`.
pub fn draw_smart_tiles(doc: &mut fx_core::Document, id: fx_core::LayerId, store: &TileStore, tiles: &[(usize, u32, u32)]) -> usize {
	match draw(doc, id, store, tiles) {
		Ok(n) => n,
		Err(error) => {
			tracing::warn!("smart object tiles: {error}");
			0
		}
	}
}

fn draw(doc: &mut fx_core::Document, id: fx_core::LayerId, store: &TileStore, tiles: &[(usize, u32, u32)]) -> Result<usize, TileError> {
	let canvas = (doc.width, doc.height);
	let Some(layer) = doc.layer_mut(id) else { return Ok(0) };
	let LayerKind::Smart { smart, cache } = &mut layer.kind else {
		return Ok(0);
	};
	let source = SourceInfo {
		size: (smart.source.composite.width(), smart.source.composite.height()),
		levels: smart.source.composite.level_count(),
	};
	let view = LazyMips::new(smart.source.composite.clone(), store);
	let mut by_level: HashMap<usize, Vec<(u32, u32)>> = HashMap::new();
	for &(level, tx, ty) in tiles {
		by_level.entry(level).or_default().push((tx, ty));
	}
	let format = cache.format();
	let filters: Vec<fx_core::smart::SmartFilter> = if smart.filters_enabled {
		smart.filters.iter().filter(|f| f.enabled).cloned().collect()
	} else {
		Vec::new()
	};
	let transform = smart.transform;
	let mut drawn = 0;
	for (level, list) in by_level {
		let buffers = fx_ops::resample::resample(&view, source, transform, fx_core::Filter::Bicubic, level, &list)?;
		let buffers = if filters.is_empty() {
			buffers
		} else {
			crate::smart_filters::apply(&view, source, transform, &filters, level, canvas, buffers, store)?
		};
		for ((tx, ty), buffer) in buffers {
			let slot = crate::vector::slot_for(buffer, format, store);
			cache.set_derived_slot(level, tx, ty, slot);
			drawn += 1;
		}
	}
	// The mips computed stay valid until the source changes: keep them.
	smart.source.composite = view.into_image();
	Ok(drawn)
}
