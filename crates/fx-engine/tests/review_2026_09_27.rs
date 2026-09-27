//! Regression tests for the code review of 2026-09-27
//! (`docs/reports/CODE-REVIEW-2026-09-27.md`), built from the review's own
//! reproduction programs. CPU only: the reference compositor, no GPU.

use std::sync::Arc;

use fx_core::{BitDepth, ColorProfile, Command, CommandContext, Document, DocumentColor, Layer, LayerId, LayerKind, LayerRef};
use fx_tiles::{PixelFormat, PixelValue, TileBuffer, TileClass, TileSlot, TileStore, TileStoreConfig, TiledImage};

fn doc(w: u32, h: u32) -> Document {
	Document::new(
		w,
		h,
		DocumentColor {
			depth: BitDepth::U8,
			profile: ColorProfile::Srgb,
		},
		72.0,
	)
}

fn store(name: &str) -> TileStore {
	let dir = std::env::temp_dir().join(format!("fx-engine-review-{name}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
}

fn apply(d: &mut Document, store: &TileStore, command: Command) {
	let ops = fx_engine::ops::EngineOps::default();
	command
		.apply(
			d,
			&mut CommandContext {
				tiles: store,
				ops: Some(&ops),
			},
		)
		.unwrap();
}

/// The composite's pixel `i` of tile (0, 0) at level 0, premultiplied.
fn composite_pixel(d: &Document, store: &TileStore, i: usize) -> [f64; 4] {
	let mut luts = fx_render::adjust::LutCache::default();
	let program = fx_render::build_program(d, 0, 0, 0, &mut |a| luts.get(a)).unwrap();
	fx_render::reference::render_tile(&program, &|h| store.get(h).unwrap())[i]
}

/// R02: Rasterize baked a 50 % pixel mask into the pixels and kept the mask,
/// so the layer rendered at 25 %.
#[test]
fn rasterize_does_not_apply_a_mask_twice() {
	let store = store("rasterize");
	let mut d = doc(256, 256);
	let mut layer = Layer::new(LayerId(1), "fill", LayerKind::SolidFill { rgba: [65535; 4] });
	let mut mask = TiledImage::new(256, 256, PixelFormat::Gray8);
	mask.set_slot(0, 0, TileSlot::Solid(PixelValue([32768; 4])));
	layer.mask = Some(fx_core::Mask {
		image: mask,
		enabled: true,
		linked: true,
		outside_value: 65535,
	});
	d.layers.push(Arc::new(layer));
	let before = composite_pixel(&d, &store, 0)[3];
	apply(
		&mut d,
		&store,
		Command::Rasterize {
			layers: vec![LayerRef::Id(LayerId(1))],
		},
	);
	assert!(matches!(d.layers[0].kind, LayerKind::Pixel { .. }));
	assert!(d.layers[0].mask.is_some(), "the mask stays on the layer");
	let after = composite_pixel(&d, &store, 0)[3];
	assert!((before - after).abs() < 0.01, "alpha before {before:.4}, after {after:.4}");
}

/// R08: an enabled vector mask can hide an opaque fill, but the automatic
/// export options called the document opaque and flattened it onto white.
#[test]
fn a_vector_masked_background_keeps_the_alpha_channel() {
	let store = store("export-alpha");
	let mut d = doc(20, 10);
	let mut layer = Layer::new(LayerId(1), "masked", LayerKind::SolidFill { rgba: [65535; 4] });
	layer.vector_mask = Some(fx_core::layer::VectorMask {
		path: fx_core::path::Path::default(),
		enabled: true,
		feather: 0.0,
		density: 1.0,
		cache: TiledImage::new(20, 10, PixelFormat::Gray8),
	});
	d.layers.push(Arc::new(layer));
	assert!(!fx_engine::export::opaque_background(&d, &store));
}

/// A grey 256 × 256 tile: 255 where `keep(x, y)`, else 0.
fn gray_tile(store: &TileStore, keep: impl Fn(usize, usize) -> bool) -> TileSlot {
	let mut tile = TileBuffer::zeroed(PixelFormat::Gray8);
	for (i, v) in tile.bytes_mut().iter_mut().enumerate() {
		*v = if keep(i % 256, i / 256) { 255 } else { 0 };
	}
	TileSlot::Data(store.insert(tile, TileClass::Authoritative))
}

/// R12: with a pixel mask enabled, the vector mask was ignored. The two
/// multiply: the fill shows only where both reveal it.
#[test]
fn pixel_and_vector_masks_multiply() {
	let store = store("two-masks");
	let mut d = doc(256, 256);
	let mut layer = Layer::new(LayerId(1), "fill", LayerKind::SolidFill { rgba: [65535; 4] });
	// Pixel mask: the left half. Vector mask: the top half.
	let mut mask = TiledImage::new(256, 256, PixelFormat::Gray8);
	mask.set_slot(0, 0, gray_tile(&store, |x, _| x < 128));
	layer.mask = Some(fx_core::Mask {
		image: mask,
		enabled: true,
		linked: true,
		outside_value: 0,
	});
	let mut cache = TiledImage::derived(256, 256, PixelFormat::Gray8);
	cache.set_derived_slot(0, 0, 0, gray_tile(&store, |_, y| y < 128));
	layer.vector_mask = Some(fx_core::layer::VectorMask {
		path: fx_core::path::Path::default(),
		enabled: true,
		feather: 0.0,
		density: 1.0,
		cache,
	});
	d.layers.push(Arc::new(layer));
	let alpha = |x: usize, y: usize| composite_pixel(&d, &store, y * 256 + x)[3];
	assert!((alpha(10, 10) - 1.0).abs() < 1e-3, "both masks reveal");
	assert!(alpha(200, 10) < 1e-3, "the pixel mask hides");
	assert!(alpha(10, 200) < 1e-3, "the vector mask hides");
	assert!(alpha(200, 200) < 1e-3, "both hide");

	// A constant vector mask (density 50 %, the whole tile outside the path:
	// the cache holds 1 − density) scales the varying pixel mask.
	let mut layer = (*d.layers[0]).clone();
	let vm = layer.vector_mask.as_mut().unwrap();
	vm.density = 0.5;
	vm.cache = TiledImage::derived(256, 256, PixelFormat::Gray8);
	vm.cache.set_derived_slot(0, 0, 0, TileSlot::Solid(PixelValue([32768; 4])));
	d.layers[0] = Arc::new(layer);
	let alpha = |x: usize, y: usize| composite_pixel(&d, &store, y * 256 + x)[3];
	assert!((alpha(10, 200) - 0.5).abs() < 1e-3, "{}", alpha(10, 200));
	assert!((alpha(10, 10) - 0.5).abs() < 1e-3);
	assert!(alpha(200, 200) < 1e-3);
}
