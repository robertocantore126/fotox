//! Regression tests for the code review of 2026-09-27
//! (`docs/reports/CODE-REVIEW-2026-09-27.md`), built from the review's own
//! reproduction programs. CPU only: the reference compositor, no GPU.

use std::sync::Arc;

use fx_core::{BitDepth, ColorProfile, Command, CommandContext, Document, DocumentColor, Layer, LayerId, LayerKind, LayerRef};
use fx_tiles::{PixelFormat, PixelValue, TileSlot, TileStore, TileStoreConfig, TiledImage};

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
