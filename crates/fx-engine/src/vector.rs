//! Shape tiles for shape layers (M6-T06).
//!
//! A shape layer's cache is a `TiledImage::derived`: nothing in it is
//! authoritative, every tile starts dirty, and a tile that the compositor
//! needs is drawn here from the layer's geometry at *that* level. Mips are
//! never taken from the level below, so a shape is as sharp as the zoom asks
//! for.
//!
//! The work is the same shape as [`crate::mips`] — the render thread asks in
//! `build_program`, the engine thread answers — except that the tiles of one
//! layer are drawn in parallel on the rayon pool (`render_shape_tile` is pure
//! CPU work in tiny-skia, D-050) and then installed in one pass.

use std::collections::HashMap;

use fx_core::LayerKind;
use fx_core::vector::{Paint, StrokeStyle, VectorShape};
use fx_tiles::{PixelFormat, TileBuffer, TileClass, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

/// Everything the renderer needs from a shape layer.
pub struct ShapeGeometry<'a> {
	pub shape: &'a VectorShape,
	pub fill: Option<&'a Paint>,
	pub stroke: Option<&'a StrokeStyle>,
	/// Local → document, `[a, b, c, d, e, f]`.
	pub transform: [f64; 6],
}

/// Draw the requested tiles of one shape layer and install them in its cache.
///
/// `tiles` are `(level, tx, ty)` triples; the caller has already deduplicated
/// them. Returns how many tiles were drawn.
pub fn draw_shape_tiles(geometry: &ShapeGeometry<'_>, cache: &mut TiledImage, store: &TileStore, tiles: &[(usize, u32, u32)]) -> usize {
	let format = cache.format();
	let buffers: Vec<((usize, u32, u32), TileBuffer)> = tiles
		.par_iter()
		.map(|&(level, tx, ty)| {
			let buffer = fx_render::render_shape_tile(geometry.shape, geometry.fill, geometry.stroke, geometry.transform, level, (tx, ty), format);
			((level, tx, ty), buffer)
		})
		.collect();
	for ((level, tx, ty), buffer) in buffers {
		let slot = slot_for(buffer, format, store);
		cache.set_derived_slot(level, tx, ty, slot);
	}
	tiles.len()
}

/// The slot a freshly rendered buffer becomes: uniform tiles cost no memory.
pub(crate) fn slot_for(buffer: TileBuffer, format: PixelFormat, store: &TileStore) -> TileSlot {
	match buffer.uniform_value() {
		Some(v) if v.is_transparent(format) || (!format.has_alpha() && v.0[0] == 0) => TileSlot::Empty,
		Some(v) => TileSlot::Solid(v),
		None => TileSlot::Data(store.insert(buffer, TileClass::Derived)),
	}
}

/// Draw the level-0 tiles of the shape layers in `layers` (`None` means every
/// shape layer of `doc`) that are still dirty, so a reader that looks at the
/// document as a whole sees the shapes' pixels (M6-T06).
///
/// The viewport only ever asks for the tiles it displays, but merge, flatten,
/// crop, rotate, export, copy and rasterise read level 0 of layers the view may
/// never have shown. Returns how many tiles were drawn.
pub fn prepare_level0(doc: &mut fx_core::Document, store: &TileStore, layers: Option<&[fx_core::LayerId]>) -> usize {
	let mut requests: Vec<(fx_core::LayerId, usize, u32, u32)> = Vec::new();
	doc.walk(|layer, _| {
		if layers.is_some_and(|ids| !ids.contains(&layer.id)) {
			return;
		}
		if let Some(cache) = layer.kind.derived_cache() {
			requests.extend(cache.dirty_tiles(0).map(|(tx, ty)| (layer.id, 0, tx, ty)));
		}
	});
	if requests.is_empty() {
		return 0;
	}
	draw_requests(doc, store, &requests)
}

/// Every shape tile of `requests`, grouped by the layer it belongs to: one
/// parallel batch per layer. Returns how many tiles were drawn.
///
/// A layer that is not a shape (or has gone) is skipped: the request was made
/// against an older snapshot.
pub fn draw_requests(doc: &mut fx_core::Document, store: &TileStore, requests: &[(fx_core::LayerId, usize, u32, u32)]) -> usize {
	let mut by_layer: HashMap<fx_core::LayerId, Vec<(usize, u32, u32)>> = HashMap::new();
	for &(layer, level, tx, ty) in requests {
		by_layer.entry(layer).or_default().push((level, tx, ty));
	}
	let mut drawn = 0;
	for (id, tiles) in by_layer {
		// Text layers (M6-T07) share the request path: lay out, then draw.
		if doc.layer(id).is_some_and(|layer| layer.kind.is_text()) {
			drawn += crate::text::draw_text_tiles(doc, id, store, &tiles);
			continue;
		}
		let Some(layer) = doc.layer_mut(id) else { continue };
		let LayerKind::Shape {
			shape,
			fill,
			stroke,
			transform,
			cache,
		} = &mut layer.kind
		else {
			continue;
		};
		// A shape with no paint draws nothing; leave its tiles Empty rather
		// than re-rendering them every frame.
		if fill.is_none() && stroke.is_none() {
			for (level, tx, ty) in tiles {
				cache.set_derived_slot(level, tx, ty, TileSlot::Empty);
			}
			continue;
		}
		let geometry = ShapeGeometry {
			shape,
			fill: fill.as_ref(),
			stroke: stroke.as_ref(),
			transform: *transform,
		};
		drawn += draw_shape_tiles(&geometry, cache, store, &tiles);
	}
	drawn
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_core::command::NewLayer;
	use fx_core::{BitDepth, ColorProfile, Command, CommandContext, Document, DocumentColor, PixelOps};
	use fx_tiles::{TILE_SIZE, TileStoreConfig};

	use crate::ops::EngineOps;

	const RED: Paint = Paint::Solid { rgba: [65_535, 0, 0, 65_535] };

	fn store() -> TileStore {
		let mut config = TileStoreConfig::for_tests(std::env::temp_dir().join("fx-engine-vector-tests"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).expect("test store")
	}

	fn document() -> Document {
		Document::new(
			400,
			300,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			72.0,
		)
	}

	/// A 100 × 50 red rectangle at (50, 50), the way the Rectangle tool adds it.
	fn rect_layer(doc: &mut Document, store: &TileStore) -> fx_core::LayerId {
		let shape = VectorShape::Rect {
			w: 100.0,
			h: 50.0,
			radii: [0.0; 4],
		};
		let layer = NewLayer::Shape {
			shape,
			fill: Some(RED),
			stroke: None,
			transform: [1.0, 0.0, 0.0, 1.0, 50.0, 50.0],
		};
		let mut ctx = CommandContext { tiles: store, ops: None };
		Command::AddLayer { layer, name: None }.apply(doc, &mut ctx).expect("the shape layer is added");
		doc.active_layer().expect("AddLayer selects the new layer")
	}

	/// The straight-alpha 16-bit pixel `(x, y)` of a composited image.
	fn pixel(image: &TiledImage, store: &TileStore, x: u32, y: u32) -> [u16; 4] {
		let tile = match image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
			TileSlot::Solid(value) => TileBuffer::filled(image.format(), *value),
			TileSlot::Data(handle) => (*store.get(handle).expect("a tile of the composite")).clone(),
			TileSlot::Empty => TileBuffer::zeroed(image.format()),
		};
		let i = ((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize * 4;
		let words = tile.as_u16();
		[words[i], words[i + 1], words[i + 2], words[i + 3]]
	}

	#[test]
	fn a_whole_document_reader_needs_the_shape_tiles_first() {
		// Merge, flatten, crop, export and rasterise all read level 0 of layers
		// the viewport may never have shown, and a shape layer's tiles are only
		// drawn when something asks for them (M6-T06).
		let store = store();
		let mut doc = document();
		let id = rect_layer(&mut doc, &store);
		let ops = EngineOps {
			progress: None,
			clipboard: Default::default(),
		};
		let error = ops.composite(&doc, &[id], None, &store).expect_err("a fresh shape has no tiles to compose yet");
		assert!(error.to_string().contains("tiles are missing"), "{error}");

		let drawn = prepare_level0(&mut doc, &store, None);
		assert_eq!(drawn, 4, "400 × 300 is 2 × 2 tiles and a new cache is dirty everywhere");
		let image = ops.composite(&doc, &[id], None, &store).expect("the shape composites now");
		assert_eq!(pixel(&image, &store, 50, 50), [65_535, 0, 0, 65_535], "the rect's first pixel is the fill");
		assert_eq!(pixel(&image, &store, 149, 99), [65_535, 0, 0, 65_535], "and its last one");
		assert_eq!(pixel(&image, &store, 150, 99), [0; 4], "the pixel after it is empty");
		assert_eq!(pixel(&image, &store, 49, 50), [0; 4], "and the one before it too");
		assert_eq!(pixel(&image, &store, 10, 10), [0; 4], "as is the rest of the canvas");
	}

	#[test]
	fn preparing_one_layer_leaves_the_others_dirty() {
		let store = store();
		let mut doc = document();
		let first = rect_layer(&mut doc, &store);
		let second = rect_layer(&mut doc, &store);
		assert_ne!(first, second);
		assert_eq!(prepare_level0(&mut doc, &store, Some(&[second])), 4, "the named layer is drawn");
		assert_eq!(dirty_level0(&doc, first), 4, "the other one is not");
		assert_eq!(dirty_level0(&doc, second), 0);
	}

	/// How many level-0 tiles of a shape layer still have to be drawn.
	fn dirty_level0(doc: &Document, id: fx_core::LayerId) -> usize {
		match doc.layer(id).map(|layer| &layer.kind) {
			Some(LayerKind::Shape { cache, .. }) => cache.dirty_tiles(0).count(),
			other => panic!("not a shape layer: {other:?}"),
		}
	}

	#[test]
	fn drawing_the_same_tiles_twice_is_a_no_op() {
		let store = store();
		let mut doc = document();
		rect_layer(&mut doc, &store);
		assert_eq!(prepare_level0(&mut doc, &store, None), 4);
		assert_eq!(prepare_level0(&mut doc, &store, None), 0, "nothing is dirty any more");
	}
}
