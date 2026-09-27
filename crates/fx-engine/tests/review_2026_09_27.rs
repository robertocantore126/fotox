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

/// Every pixel of a document's composite at level 0, premultiplied.
fn composite_all(d: &mut Document, store: &TileStore) -> Vec<[f64; 4]> {
	let t = fx_tiles::TILE_SIZE;
	let (cols, rows) = (d.width.div_ceil(t), d.height.div_ceil(t));
	let tiles: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let rendered = fx_engine::derived::render_tiles(d, store, 0, &tiles, &mut fx_render::adjust::LutCache::default()).unwrap();
	let mut out = vec![[0.0; 4]; (d.width * d.height) as usize];
	for r in rendered {
		let Some(pixels) = r.pixels else { continue };
		for y in 0..t {
			for x in 0..t {
				let (dx, dy) = (r.tile.0 * t + x, r.tile.1 * t + y);
				if dx < d.width && dy < d.height {
					out[(dy * d.width + dx) as usize] = pixels[(y * t + x) as usize];
				}
			}
		}
	}
	out
}

/// R03 follow-up: Perspective Crop used to leave shape and text layers where
/// they were. Their matrix cannot hold a homography, so they become Smart
/// Objects around their own content, keeping their properties; the result
/// matches the same layer rasterised first.
#[test]
fn perspective_crop_takes_shape_layers_along() {
	let store = store("perspective-shape");
	let make = || {
		let mut d = doc(300, 200);
		let mut layer = Layer::new(
			LayerId(1),
			"rect",
			LayerKind::Shape {
				shape: fx_core::vector::VectorShape::Rect {
					w: 100.0,
					h: 80.0,
					radii: [0.0; 4],
				},
				fill: Some(fx_core::vector::Paint::Solid { rgba: [65535, 0, 0, 65535] }),
				stroke: None,
				transform: [1.0, 0.0, 0.0, 1.0, 100.0, 60.0],
				cache: TiledImage::derived(300, 200, PixelFormat::Rgba8),
			},
		);
		layer.opacity = 0.5;
		d.layers.push(Arc::new(layer));
		d
	};
	let crop = Command::PerspectiveCrop {
		quad: [(20.0, 10.0), (280.0, 30.0), (260.0, 190.0), (40.0, 170.0)],
		width: 300,
		height: 200,
	};
	let mut shape = make();
	apply(&mut shape, &store, crop.clone());
	let layer = &shape.layers[0];
	assert_eq!((layer.name.as_str(), layer.opacity), ("rect", 0.5), "the layer keeps its properties");
	let LayerKind::Smart { smart, .. } = &layer.kind else {
		panic!("a Smart Object, not {:?}", layer.kind)
	};
	assert!(matches!(smart.transform, fx_core::Mapping::Projective(_)));

	let mut raster = make();
	apply(
		&mut raster,
		&store,
		Command::Rasterize {
			layers: vec![LayerRef::Id(LayerId(1))],
		},
	);
	apply(&mut raster, &store, crop);
	let (a, b) = (composite_all(&mut shape, &store), composite_all(&mut raster, &store));
	let diffs: Vec<f64> = a.iter().zip(&b).map(|(p, q)| (p[3] - q[3]).abs()).collect();
	let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
	let max = diffs.iter().copied().fold(0.0, f64::max);
	let covered = a.iter().filter(|p| p[3] > 0.45).count();
	assert!(covered > 5_000, "the shape is on the cropped canvas ({covered} px)");
	assert!(
		mean < 0.005 && max < 0.3,
		"Smart Object and rasterised crops differ: mean {mean:.4}, max {max:.4}"
	);
}

/// Follow-up: Free Transform's preview of a Smart Object is what the command
/// will make of it (its source through the box's mapping composed with its
/// transform), the snapshot shows it (the layer used to stay still until the
/// commit), and nothing is drawn up front: no mip of the source is built
/// before a preview reads it.
#[test]
fn a_smart_object_previews_its_source_through_the_box() {
	use fx_engine::transform_preview::{Prepared, PreviewJob, TransformPreview};
	let store = store("smart-preview");
	let mut composite = TiledImage::new(1024, 1024, PixelFormat::Rgba8);
	composite.set_slot(0, 0, TileSlot::Solid(PixelValue([65535, 0, 0, 65535])));
	let mut d = doc(512, 512);
	d.layers.push(Arc::new(Layer::new(
		LayerId(1),
		"smart",
		LayerKind::Smart {
			smart: fx_core::smart::SmartObject {
				source: fx_core::smart::SmartSource {
					doc: Arc::new(doc(1024, 1024)),
					composite,
					linked: None,
					linked_mtime: None,
					uid: 11,
				},
				transform: fx_core::Mapping::scale(0.5, 0.5),
				filters: vec![],
				filters_enabled: true,
			},
			cache: TiledImage::derived(512, 512, PixelFormat::Rgba8),
		},
	)));
	let prepared = Arc::new(Prepared::new(&d, LayerId(1), &store).unwrap().expect("something to move"));
	assert!(prepared.placement.is_some(), "previewed from the source");
	{
		let source = prepared.source.lock().unwrap();
		assert!(source.is_dirty(1, 0, 0), "no mip is built up front");
	}
	let job = PreviewJob {
		request: 1,
		latest: Arc::new(std::sync::atomic::AtomicU64::new(1)),
		prepared: prepared.clone(),
		mapping: fx_core::Mapping::translation(100.0, 50.0),
		filter: fx_core::Filter::Bilinear,
		coarser: false,
		view: fx_render::ViewTransform {
			zoom: 1.0,
			center_x: 256.0,
			center_y: 256.0,
			rotation: 0.0,
		},
		viewport: fx_render::ViewportSize { width: 512, height: 512 },
		canvas: (512, 512),
	};
	let shown = job.run(&store).unwrap().expect("a preview");
	let preview = TransformPreview {
		layer: LayerId(1),
		prepared: Some(prepared),
		shown: Some(shown),
		request: 1,
	};
	let mut snapshot = d.clone();
	preview.apply(&mut snapshot);
	let pixels = composite_all(&mut snapshot, &store);
	// The source's red tile (256 px) at 50 %, moved by (100, 50): canvas
	// (100..228, 50..178).
	let at = |x: u32, y: u32| pixels[(y * 512 + x) as usize];
	assert!(at(150, 80)[3] > 0.99 && at(150, 80)[0] > 0.99, "moved and shown: {:?}", at(150, 80));
	assert!(at(50, 20)[3] < 0.01, "nothing left where it was: {:?}", at(50, 20));
	assert!(at(240, 190)[3] < 0.01, "and nothing past it: {:?}", at(240, 190));
}

/// Move ▸ Auto-Select (on by default): a click picks the topmost visible layer
/// that shows a pixel there. Shapes and text were skipped unless their
/// full-resolution tiles happened to be drawn, Smart Objects and fill layers
/// always; masks and hidden groups were not taken into account.
#[test]
fn a_click_picks_the_topmost_layer_that_shows_a_pixel_there() {
	let store = store("auto-select");
	let mut d = doc(512, 512);
	let solid = |rgba: [u16; 4]| {
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba8);
		for ty in 0..2 {
			for tx in 0..2 {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue(rgba)));
			}
		}
		image
	};
	d.layers.push(Arc::new(Layer::new(
		LayerId(1),
		"white",
		LayerKind::Pixel {
			image: solid([65535; 4]),
			offset: (0, 0),
		},
	)));
	// A shape whose cache was never drawn.
	d.layers.push(Arc::new(Layer::new(
		LayerId(2),
		"shape",
		LayerKind::Shape {
			shape: fx_core::vector::VectorShape::Rect {
				w: 100.0,
				h: 100.0,
				radii: [0.0; 4],
			},
			fill: Some(fx_core::vector::Paint::Solid { rgba: [0, 0, 65535, 65535] }),
			stroke: None,
			transform: [1.0, 0.0, 0.0, 1.0, 50.0, 50.0],
			cache: TiledImage::derived(512, 512, PixelFormat::Rgba8),
		},
	)));
	// A Smart Object: a 64 × 64 red source placed at (300, 300).
	let mut source = TiledImage::new(64, 64, PixelFormat::Rgba8);
	source.set_slot(0, 0, TileSlot::Solid(PixelValue([65535, 0, 0, 65535])));
	d.layers.push(Arc::new(Layer::new(
		LayerId(3),
		"smart",
		LayerKind::Smart {
			smart: fx_core::smart::SmartObject {
				source: fx_core::smart::SmartSource {
					doc: Arc::new(doc(64, 64)),
					composite: source,
					linked: None,
					linked_mtime: None,
					uid: 5,
				},
				transform: fx_core::Mapping::translation(300.0, 300.0),
				filters: vec![],
				filters_enabled: true,
			},
			cache: TiledImage::derived(512, 512, PixelFormat::Rgba8),
		},
	)));
	// A hidden group holding a fill that covers everything.
	let mut group = Layer::new(
		LayerId(4),
		"hidden group",
		LayerKind::Group {
			expanded: true,
			children: vec![Arc::new(Layer::new(LayerId(5), "fill", LayerKind::SolidFill { rgba: [0, 65535, 0, 65535] }))],
		},
	);
	group.visible = false;
	d.layers.push(Arc::new(group));
	// On top: blue everywhere, but masked out everywhere.
	let mut masked = Layer::new(
		LayerId(6),
		"masked",
		LayerKind::Pixel {
			image: solid([0, 0, 65535, 65535]),
			offset: (0, 0),
		},
	);
	masked.mask = Some(fx_core::Mask {
		image: TiledImage::new(512, 512, PixelFormat::Gray8),
		enabled: true,
		linked: true,
		outside_value: 0,
	});
	d.layers.push(Arc::new(masked));

	let pick = |x: f64, y: f64| fx_engine::tools::move_tool::layer_at(&d, &store, x, y, false);
	assert_eq!(pick(100.0, 100.0), Some(LayerId(2)), "the shape, never drawn");
	assert_eq!(pick(330.0, 330.0), Some(LayerId(3)), "the Smart Object");
	assert_eq!(pick(10.0, 10.0), Some(LayerId(1)), "below the masked-out layer and the hidden group");
	assert_eq!(pick(600.0, 10.0), None, "outside the canvas");
}
