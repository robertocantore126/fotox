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
	command.apply(d, &mut CommandContext { tiles: store, ops: Some(&ops) }).unwrap();
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

/// R03: rotating the canvas turned pixels, pixel masks and shapes, but left
/// vector masks, alpha channels, Smart Objects, paths and guides behind.
/// Undo brings every part back.
#[test]
fn rotating_the_canvas_turns_the_whole_document() {
	use fx_core::path::{Anchor, Path, Subpath};
	let store = store("rotate");
	let mut d = doc(20, 10);
	let path = Path {
		subpaths: vec![Subpath {
			anchors: vec![Anchor::corner((1.0, 2.0)), Anchor::corner((5.0, 2.0)), Anchor::corner((5.0, 6.0))],
			closed: true,
			op: Default::default(),
		}],
	};
	let mut fill = Layer::new(LayerId(1), "masked", LayerKind::SolidFill { rgba: [65535; 4] });
	fill.vector_mask = Some(fx_core::layer::VectorMask {
		path: path.clone(),
		enabled: true,
		feather: 0.0,
		density: 1.0,
		cache: TiledImage::derived(20, 10, PixelFormat::Gray8),
	});
	d.layers.push(Arc::new(fill));
	let smart = fx_core::smart::SmartObject {
		source: fx_core::smart::SmartSource {
			doc: Arc::new(doc(20, 10)),
			composite: TiledImage::new(20, 10, PixelFormat::Rgba8),
			linked: None,
			linked_mtime: None,
			uid: 1,
		},
		transform: fx_core::Mapping::identity(),
		filters: vec![],
		filters_enabled: true,
	};
	d.layers.push(Arc::new(Layer::new(
		LayerId(2),
		"smart",
		LayerKind::Smart {
			smart,
			cache: TiledImage::derived(20, 10, PixelFormat::Rgba8),
		},
	)));
	d.channels
		.push(fx_core::channel::Channel::new("alpha", TiledImage::new(20, 10, PixelFormat::Gray8)));
	d.work_path = Some(path);
	d.guides.push(fx_core::document::Guide { vertical: true, position: 4.0 });
	d.annotations.samplers.push(fx_core::annotations::Sampler { x: 1.0, y: 2.0 });
	let before = d.clone();

	let ops = fx_engine::ops::EngineOps::default();
	let mut history = fx_core::History::default();
	history
		.execute(
			&mut d,
			Command::RotateCanvas { quarter_turns: 1 },
			&mut CommandContext {
				tiles: &store,
				ops: Some(&ops),
			},
		)
		.unwrap();
	// 90° clockwise on a 20 × 10 canvas: (x, y) → (10 − y, x).
	assert_eq!((d.width, d.height), (10, 20));
	let vm = d.layers[0].vector_mask.as_ref().unwrap();
	assert_eq!((vm.cache.width(), vm.cache.height()), (10, 20), "the vector mask's cache");
	assert_eq!(vm.path.subpaths[0].anchors[0].pos, (8.0, 1.0), "the vector mask's path");
	assert_eq!((d.channels[0].image.width(), d.channels[0].image.height()), (10, 20), "the alpha channel");
	let LayerKind::Smart { smart, cache } = &d.layers[1].kind else {
		unreachable!()
	};
	assert_eq!((cache.width(), cache.height()), (10, 20), "the Smart Object's cache");
	assert_eq!(smart.transform.forward_point(1.0, 2.0), Some((8.0, 1.0)), "the Smart Object's transform");
	assert_eq!(d.work_path.as_ref().unwrap().subpaths[0].anchors[0].pos, (8.0, 1.0), "the work path");
	assert_eq!(
		d.guides[0],
		fx_core::document::Guide {
			vertical: false,
			position: 4.0
		},
		"a vertical guide becomes horizontal"
	);
	assert_eq!((d.annotations.samplers[0].x, d.annotations.samplers[0].y), (8.0, 1.0));

	assert!(history.undo(&mut d));
	assert_eq!((d.width, d.height), (20, 10));
	assert_eq!(d.work_path, before.work_path);
	assert_eq!(d.guides, before.guides);
	let vm = d.layers[0].vector_mask.as_ref().unwrap();
	assert_eq!((vm.cache.width(), vm.cache.height(), vm.path.subpaths[0].anchors[0].pos), (20, 10, (1.0, 2.0)));
}

/// R03: Canvas Size moved pixel layers and shapes by their offsets but left
/// the alpha channels where they were.
#[test]
fn canvas_size_moves_the_alpha_channels_with_the_content() {
	let store = store("canvas-size");
	let mut d = doc(20, 10);
	let mut alpha = TiledImage::new(20, 10, PixelFormat::Gray8);
	alpha.set_slot(0, 0, TileSlot::Solid(PixelValue([65535; 4])));
	d.channels.push(fx_core::channel::Channel::new("alpha", alpha));
	apply(
		&mut d,
		&store,
		Command::CanvasSize {
			width: 40,
			height: 30,
			anchor: fx_core::transform::Anchor9::Center,
		},
	);
	assert_eq!((d.width, d.height), (40, 30));
	// The content moved by (10, 10): the channel's (0, 0) tile now starts
	// with 10 unselected pixels.
	let channel = &d.channels[0].image;
	let TileSlot::Data(handle) = channel.slot(0, 0, 0) else {
		panic!("a partly selected tile, got {:?}", channel.slot(0, 0, 0))
	};
	let tile = store.get(handle).unwrap();
	let bytes = tile.bytes();
	assert_eq!(bytes[0], 0, "(0, 0) is new canvas");
	assert_eq!(bytes[10 * 256 + 10], 255, "(10, 10) is the old (0, 0)");
}

/// An RGBA8 image's pixel `(x, y)`.
fn rgba8(store: &TileStore, image: &TiledImage, x: u32, y: u32) -> [u8; 4] {
	let t = fx_tiles::TILE_SIZE;
	match image.slot(0, x / t, y / t) {
		TileSlot::Empty => [0; 4],
		TileSlot::Solid(v) => v.0.map(|c| (c >> 8) as u8),
		TileSlot::Data(h) => {
			let i = (((y % t) * t + x % t) * 4) as usize;
			store.get(h).unwrap().bytes()[i..i + 4].try_into().unwrap()
		}
	}
}

/// Found while fixing R03's warps: Free Transform's Warp names its source
/// rectangle in canvas pixels, so a layer that does not sit at the canvas
/// origin must see it moved back by its offset. An identity warp over the
/// layer's own bounds leaves an offset layer exactly where it was.
#[test]
fn an_identity_warp_leaves_an_offset_layer_alone() {
	let store = store("warp-offset");
	let mut d = doc(256, 256);
	let mut tile = TileBuffer::zeroed(PixelFormat::Rgba8);
	for (i, px) in tile.bytes_mut().chunks_exact_mut(4).enumerate() {
		let x = i % fx_tiles::TILE_SIZE as usize;
		px.copy_from_slice(if x < 32 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
	}
	let mut image = TiledImage::new(64, 64, PixelFormat::Rgba8);
	image.set_slot(0, 0, TileSlot::Data(store.insert(tile, TileClass::Authoritative)));
	d.layers
		.push(Arc::new(Layer::new(LayerId(1), "offset", LayerKind::Pixel { image, offset: (100, 60) })));
	let bounds = [100.0, 60.0, 164.0, 124.0];
	apply(
		&mut d,
		&store,
		Command::Transform {
			layer: LayerRef::Id(LayerId(1)),
			mapping: Box::new(fx_core::Mapping::Warp(fx_core::BezierPatch::rect(bounds, bounds))),
			filter: fx_core::Filter::Bilinear,
		},
	);
	let LayerKind::Pixel { image, offset } = &d.layers[0].kind else {
		panic!("a pixel layer")
	};
	assert_eq!(*offset, (100, 60));
	assert_eq!(rgba8(&store, image, 10, 20), [255, 0, 0, 255], "the left half stays red");
	assert_eq!(rgba8(&store, image, 50, 20), [0, 0, 255, 255], "the right half stays blue");
}

/// R03 follow-up: a warped Smart Object follows Perspective Crop (a homography
/// after its surface), and a warp can go over a turned Smart Object (a
/// homography before it). Every point of the surface still finds the source
/// point it came from.
#[test]
fn a_warp_composes_with_a_homography_on_either_side() {
	use fx_core::command::m12::compose;
	use fx_core::{BezierPatch, Mapping};
	use fx_ops::resample::Transform;
	use fx_ops::resample::warp::evaluate;
	let mut patch = BezierPatch::rect([40.0, 30.0, 240.0, 180.0], [40.0, 30.0, 240.0, 180.0]);
	patch.set_point(1, 2, 190.0, 40.0);
	patch.set_point(2, 1, 70.0, 150.0);
	let source = |(u, v): (f64, f64)| (40.0 + 200.0 * u, 30.0 + 150.0 * v);
	let samples: Vec<(f64, f64)> = (1..8)
		.flat_map(|i| (1..8).map(move |j| (f64::from(i) / 8.0, f64::from(j) / 8.0 + 0.01)))
		.collect();
	let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 0.25 && (a.1 - b.1).abs() < 0.25;

	let crop = Mapping::from_quad([0.0, 0.0, 300.0, 200.0], [(10.0, 5.0), (290.0, 20.0), (270.0, 190.0), (30.0, 180.0)]).unwrap();
	assert!(matches!(crop, Mapping::Projective(_)));
	let Some(Mapping::Warp(after)) = compose(crop, Mapping::Warp(patch)) else {
		panic!("a homography after a warp is a warp")
	};
	assert!(after.post.is_some());
	let t = Transform::new(Mapping::Warp(after));
	for &uv in &samples {
		let on_canvas = evaluate(&patch, uv.0, uv.1);
		let d = crop.forward_point(on_canvas.0, on_canvas.1).unwrap();
		let s = t.inverse_point(None, d).expect("covered");
		assert!(close(s, source(uv)), "after: {uv:?} → {s:?}, want {:?}", source(uv));
	}

	let turn = Mapping::rotation_about(0.4, 150.0, 100.0);
	let Some(Mapping::Warp(before)) = compose(Mapping::Warp(patch), turn) else {
		panic!("a warp over a turn is a warp")
	};
	assert!(before.pre.is_some());
	let t = Transform::new(Mapping::Warp(before));
	for &uv in &samples {
		let d = evaluate(&patch, uv.0, uv.1);
		let s = t.inverse_point(None, d).expect("covered");
		let turned = turn.forward_point(s.0, s.1).unwrap();
		assert!(close(turned, source(uv)), "before: {uv:?} → {turned:?}, want {:?}", source(uv));
	}
	// A warp of a warp is still refused.
	assert!(compose(Mapping::Warp(patch), Mapping::Warp(patch)).is_none());
}
