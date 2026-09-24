//! Tests of program building + the CPU reference compositor.

use std::sync::Arc;

use fx_core::layer::Adjustment;
use fx_core::{BlendMode, Document, LayerKind};
use fx_tiles::{TILE_SIZE, TileHandle, TileStore};

use crate::adjust::{Lut, LutCache};
use crate::blend::Premul;
use crate::program::{TileProgram, build_program};
use crate::reference::render_tile;
use crate::testing::*;

fn build(doc: &Document, tx: u32, ty: u32) -> TileProgram {
	let mut luts = LutCache::default();
	build_program(doc, 0, tx, ty, &mut |a: &Adjustment| -> Arc<Lut> { luts.get(a) }).expect("level 0 is never dirty")
}

fn render(doc: &Document, store: &TileStore, tx: u32, ty: u32) -> Vec<Premul> {
	let fetch = |h: &TileHandle| store.get(h).unwrap();
	render_tile(&build(doc, tx, ty), &fetch)
}

fn px(tile: &[Premul], x: u32, y: u32) -> Premul {
	tile[(y * TILE_SIZE + x) as usize]
}

fn close(a: Premul, b: Premul) -> bool {
	a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

#[test]
fn empty_document_has_empty_program() {
	let doc = doc(600, 600);
	assert!(build(&doc, 1, 1).is_empty());
}

#[test]
fn single_layer_reproduces_its_pixels() {
	let store = store();
	let mut doc = doc(600, 400);
	let layer = busy_layer(&mut doc, &store, 1);
	let LayerKind::Pixel { image, .. } = &layer.kind else { unreachable!() };
	let image = image.clone();
	doc.layers.push(Arc::new(layer));
	for (tx, ty) in [(0, 0), (1, 0), (2, 1)] {
		let out = render(&doc, &store, tx, ty);
		let fetch = |h: &TileHandle| store.get(h).unwrap();
		for (x, y) in [(0, 0), (17, 200), (255, 255), (100, 3)] {
			let expected = match image.slot(0, tx, ty) {
				fx_tiles::TileSlot::Empty => [0.0; 4],
				fx_tiles::TileSlot::Solid(v) => v.0.map(|c| c as f64 / 65535.0),
				fx_tiles::TileSlot::Data(h) => crate::reference::read_pixel(&fetch(h), x, y),
			};
			let a = expected[3];
			let premul = [expected[0] * a, expected[1] * a, expected[2] * a, a];
			assert!(close(px(&out, x, y), premul), "tile ({tx},{ty}) px ({x},{y})");
		}
	}
}

#[test]
fn offsets_shift_content_without_touching_pixels() {
	let store = store();
	let mut doc = doc(1024, 1024);
	let f = |x: u32, y: u32| [hash16(x, y, 9), 1000, 2000, 65535];
	let mut layer = pixel_layer(&mut doc, &store, &f);
	if let LayerKind::Pixel { offset, .. } = &mut layer.kind {
		*offset = (100, -30);
	}
	doc.layers.push(Arc::new(layer));
	let out = render(&doc, &store, 1, 1); // output pixels 256..512
	for (x, y) in [(0, 0), (155, 10), (156, 225), (255, 255)] {
		let (dx, dy) = (256 + x, 256 + y);
		let (sx, sy) = (dx - 100, dy + 30);
		let expected = f(sx, sy)[0] as f64 / 65535.0;
		assert!((px(&out, x, y)[0] - expected).abs() < 1e-9, "({x},{y})");
	}
}

#[test]
fn keys_track_exactly_what_matters() {
	let store = store();
	let mut doc = doc(1024, 512);
	let a = busy_layer(&mut doc, &store, 2);
	let a_id = a.id;
	doc.layers.push(Arc::new(a));
	let k = build(&doc, 0, 0).key;
	assert_eq!(k, build(&doc, 0, 0).key, "deterministic");

	// A different tile of the layer changes: this tile's key is unchanged.
	if let LayerKind::Pixel { image, .. } = &mut doc.layer_mut(a_id).unwrap().kind {
		image.set_slot(3, 1, fx_tiles::TileSlot::Solid(fx_tiles::PixelValue::rgba8(1, 2, 3, 255)));
	}
	assert_eq!(k, build(&doc, 0, 0).key);

	// A property change changes it.
	doc.layer_mut(a_id).unwrap().opacity = 0.5;
	assert_ne!(k, build(&doc, 0, 0).key);
}

#[test]
fn hidden_and_empty_layers_are_skipped() {
	let store = store();
	let mut doc = doc(512, 512);
	let mut hidden = solid_layer(&mut doc, [65535, 0, 0, 65535]);
	hidden.visible = false;
	let empty = pixel_layer(&mut doc, &store, &|_, _| [0; 4]);
	doc.layers.push(Arc::new(hidden));
	doc.layers.push(Arc::new(empty));
	assert!(build(&doc, 0, 0).is_empty());

	let mut masked = solid_layer(&mut doc, [0, 65535, 0, 65535]);
	masked.mask = Some(mask(&doc, &store, &|_, _| 0));
	doc.layers.push(Arc::new(masked));
	assert!(build(&doc, 0, 0).is_empty(), "fully hidden mask removes the layer");
}

#[test]
fn clipped_layer_is_limited_to_base_alpha() {
	let store = store();
	let mut doc = doc(256, 256);
	let base = solid_layer(&mut doc, [0, 0, 0, 32768]); // black, 50 %
	let mut clipped = solid_layer(&mut doc, [65535, 65535, 65535, 65535]); // white, opaque
	clipped.clipped = true;
	doc.layers.push(Arc::new(base));
	doc.layers.push(Arc::new(clipped));
	let out = render(&doc, &store, 0, 0);
	let p = px(&out, 10, 10);
	let a = 32768.0 / 65535.0;
	assert!(close(p, [a, a, a, a]), "white at the base's alpha: {p:?}");
}

#[test]
fn hidden_base_hides_clipped_layers() {
	let store = store();
	let mut doc = doc(256, 256);
	let mut base = solid_layer(&mut doc, [0, 0, 0, 65535]);
	base.visible = false;
	let mut clipped = solid_layer(&mut doc, [65535; 4]);
	clipped.clipped = true;
	doc.layers.push(Arc::new(base));
	doc.layers.push(Arc::new(clipped));
	assert!(build(&doc, 0, 0).is_empty());
	let _ = store;
}

#[test]
fn pass_through_vs_isolated_groups() {
	let store = store();
	let mut doc_pass = doc(256, 256);
	let bg = solid_layer(&mut doc_pass, [30000, 40000, 50000, 65535]);
	let mut mult = solid_layer(&mut doc_pass, [32768, 32768, 32768, 65535]);
	mult.blend = BlendMode::Multiply;
	let g = group(&mut doc_pass, BlendMode::PassThrough, vec![mult.clone()]);
	doc_pass.layers.push(Arc::new(bg.clone()));
	doc_pass.layers.push(Arc::new(g));

	let mut doc_flat = doc(256, 256);
	doc_flat.layers.push(Arc::new(bg.clone()));
	doc_flat.layers.push(Arc::new(mult.clone()));
	assert!(
		close(px(&render(&doc_pass, &store, 0, 0), 5, 5), px(&render(&doc_flat, &store, 0, 0), 5, 5)),
		"pass-through = no group"
	);

	let mut doc_iso = doc(256, 256);
	let g = group(&mut doc_iso, BlendMode::Normal, vec![mult]);
	doc_iso.layers.push(Arc::new(bg));
	doc_iso.layers.push(Arc::new(g));
	let p = px(&render(&doc_iso, &store, 0, 0), 5, 5);
	let grey = 32768.0 / 65535.0;
	assert!(
		close(p, [grey, grey, grey, 1.0]),
		"isolated: multiply sees a transparent backdrop → plain grey over bg: {p:?}"
	);
}

#[test]
fn pass_through_group_opacity_lerps_with_backdrop() {
	let store = store();
	let mut doc = doc(256, 256);
	let bg = solid_layer(&mut doc, [0, 0, 0, 65535]);
	let white = solid_layer(&mut doc, [65535; 4]);
	let mut g = group(&mut doc, BlendMode::PassThrough, vec![white]);
	g.opacity = 0.25;
	doc.layers.push(Arc::new(bg));
	doc.layers.push(Arc::new(g));
	let p = px(&render(&doc, &store, 0, 0), 0, 0);
	assert!(close(p, [0.25, 0.25, 0.25, 1.0]), "{p:?}");
}

#[test]
fn adjustment_does_not_create_pixels_on_transparency() {
	let store = store();
	let mut doc = doc(512, 256);
	// left tile opaque grey, right tile transparent
	let content = pixel_layer(&mut doc, &store, &|x, _| if x < 256 { [20000, 20000, 20000, 65535] } else { [0; 4] });
	let id = doc.allocate_layer_id();
	let invert = fx_core::Layer::new(id, "Invert", LayerKind::Adjustment(Adjustment::Invert));
	doc.layers.push(Arc::new(content));
	doc.layers.push(Arc::new(invert));
	let left = px(&render(&doc, &store, 0, 0), 3, 3);
	let expected = 1.0 - 20000.0 / 65535.0;
	assert!((left[0] - expected).abs() < 1e-3 && left[3] == 1.0, "{left:?}");
	let right = px(&render(&doc, &store, 1, 0), 3, 3);
	assert_eq!(right, [0.0; 4]);
}

#[test]
fn masks_modulate_alpha() {
	let store = store();
	let mut doc = doc(256, 256);
	let mut layer = solid_layer(&mut doc, [65535, 0, 0, 65535]);
	layer.mask = Some(mask(&doc, &store, &|x, _| if x < 128 { 65535 } else { 16384 }));
	doc.layers.push(Arc::new(layer));
	let out = render(&doc, &store, 0, 0);
	assert!(close(px(&out, 10, 0), [1.0, 0.0, 0.0, 1.0]));
	let a = 16384.0 / 65535.0;
	assert!(close(px(&out, 200, 0), [a, 0.0, 0.0, a]));
}
