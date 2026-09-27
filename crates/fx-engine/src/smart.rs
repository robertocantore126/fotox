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
use fx_ops::resample::SourceInfo;
use fx_tiles::{TileError, TileStore};

use crate::mips::LazyMips;

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
	let composite = Mutex::new(smart.source.composite.clone());
	let view = LazyMips::new(&composite, store);
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
	drop(view);
	smart.source.composite = composite.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
	Ok(drawn)
}
