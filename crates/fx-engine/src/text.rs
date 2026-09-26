//! Text tiles for text layers (M6-T07): the same derived-tile model as
//! [`crate::vector`] shapes, with a layout step in front.
//!
//! Laying out (shaping, wrapping, outlining) does not depend on the level, so
//! one [`TextLayout`] per layer is cached here and shared by every tile the
//! rayon workers draw.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use fx_core::text::TextContent;
use fx_core::{Document, LayerId};
use fx_render::{FontEntry, Fonts, TextLayout};
use fx_tiles::{TileStore, TiledImage};
use rayon::prelude::*;

// FAST: one process-wide font stack and layout cache, keyed by layer id only;
// two documents with the same layer id just re-layout each other's text.
static FONTS: LazyLock<Mutex<Fonts>> = LazyLock::new(|| Mutex::new(Fonts::new()));
static LAYOUTS: LazyLock<Mutex<HashMap<LayerId, (TextContent, f32, Arc<TextLayout>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The system's font families, for the Type option bar.
pub fn families() -> Vec<FontEntry> {
	FONTS.lock().map(|mut fonts| fonts.families()).unwrap_or_default()
}

/// The layout of `content` at `ppi`, from the cache when the content is the
/// one laid out last time for this layer.
pub fn layout_for(id: LayerId, content: &TextContent, ppi: f32) -> Arc<TextLayout> {
	if let Ok(cache) = LAYOUTS.lock()
		&& let Some((cached, cached_ppi, layout)) = cache.get(&id)
		&& cached == content
		&& *cached_ppi == ppi
	{
		return layout.clone();
	}
	let layout = Arc::new(FONTS.lock().expect("font stack poisoned").layout(content, ppi)); // FAST: expect
	if let Ok(mut cache) = LAYOUTS.lock() {
		cache.insert(id, (content.clone(), ppi, layout.clone()));
	}
	layout
}

/// A layout that is not a layer's (the Type tool measures an edit before it
/// is committed).
pub fn layout_uncached(content: &TextContent, ppi: f32) -> TextLayout {
	FONTS.lock().expect("font stack poisoned").layout(content, ppi) // FAST: expect
}

/// Draw the requested tiles of one text layer into its cache.
pub fn draw_text_tiles(doc: &mut Document, id: LayerId, store: &TileStore, tiles: &[(usize, u32, u32)]) -> usize {
	let ppi = doc.ppi;
	let Some(layer) = doc.layer_mut(id) else { return 0 };
	let Some(content) = layer.kind.text_content() else { return 0 };
	let layout = layout_for(id, &content, ppi);
	let fx_core::LayerKind::Text { cache, .. } = &mut layer.kind else { return 0 };
	draw_into(&layout, &content, cache, store, tiles)
}

fn draw_into(layout: &TextLayout, content: &TextContent, cache: &mut TiledImage, store: &TileStore, tiles: &[(usize, u32, u32)]) -> usize {
	let format = cache.format();
	let buffers: Vec<_> = tiles
		.par_iter()
		.map(|&(level, tx, ty)| {
			let buffer = fx_render::render_text_tile(layout, content.transform, level, (tx, ty), format, content.antialias);
			((level, tx, ty), buffer)
		})
		.collect();
	for ((level, tx, ty), buffer) in buffers {
		let slot = crate::vector::slot_for(buffer, format, store);
		cache.set_derived_slot(level, tx, ty, slot);
	}
	tiles.len()
}

/// Map a frame-space rect through the layer matrix to a document-space box.
pub fn doc_box(transform: [f64; 6], rect: fx_render::TextRect) -> [f64; 4] {
	let [a, b, c, d, e, f] = transform;
	let corners = [
		(rect.x, rect.y),
		(rect.x + rect.w, rect.y),
		(rect.x, rect.y + rect.h),
		(rect.x + rect.w, rect.y + rect.h),
	];
	let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
	for (x, y) in corners {
		let (dx, dy) = (a * x + c * y + e, b * x + d * y + f);
		out[0] = out[0].min(dx);
		out[1] = out[1].min(dy);
		out[2] = out[2].max(dx);
		out[3] = out[3].max(dy);
	}
	out
}

/// Map a frame-space rect through the layer matrix to a document quad.
pub fn doc_quad(transform: [f64; 6], rect: fx_render::TextRect) -> [(f64, f64); 4] {
	let [a, b, c, d, e, f] = transform;
	let map = |x: f64, y: f64| (a * x + c * y + e, b * x + d * y + f);
	[
		map(rect.x, rect.y),
		map(rect.x + rect.w, rect.y),
		map(rect.x + rect.w, rect.y + rect.h),
		map(rect.x, rect.y + rect.h),
	]
}
