//! Smart Object tiles (M12-T01, D-082): a requested cache tile at level L is
//! resampled from the source composite's mips through the Smart Object's
//! transform (M6-T01's sampler picks the source level from the scale), then
//! run through the Smart Filters (M12-T03).
//!
//! FAST: the first draw makes every mip level of the composite valid (a
//! one-off cost proportional to the source's tiles); Bicubic always.

use std::collections::HashMap;

use fx_core::LayerKind;
use fx_ops::resample::SourceInfo;
use fx_tiles::{TileError, TileStore};

use crate::ops::ImageSource;

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
	// Every mip of the composite valid (they stay valid: the composite is
	// immutable until the source changes).
	let composite = &mut smart.source.composite;
	let levels = composite.level_count();
	for level in 1..levels {
		let grid = composite.grid(level).clone();
		for ty in 0..grid.rows() {
			for tx in 0..grid.cols() {
				if composite.is_dirty(level, tx, ty) {
					crate::mips::ensure_mip(composite, store, level, tx, ty)?;
				}
			}
		}
	}
	let composite = composite.clone();
	let source = SourceInfo {
		size: (composite.width(), composite.height()),
		levels,
	};
	let view = ImageSource { image: &composite, store };
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
	Ok(drawn)
}
