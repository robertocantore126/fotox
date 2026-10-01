//! The Object Selection tool reads the active layer's whole content, the
//! parts other layers cover included (Rob, 2026-09-30), unless Sample All
//! Layers is on: `ai::layer_alone`.
//!
//! The scene: a red square on an otherwise transparent layer, and above it
//! an opaque blue layer that hides it completely.

use std::sync::Arc;

use fx_core::select_ops::SelectOp;
use fx_core::{BitDepth, ColorProfile, Command, CommandContext, Document, DocumentColor, Layer, LayerId, LayerKind, SelectMode};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileStore, TileStoreConfig, TiledImage};

const S: u32 = 768;
/// The square: `[A, B)` on both axes.
const A: u32 = 260;
const B: u32 = 500;

fn layer(store: &TileStore, id: u64, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Layer> {
	let mut image = TiledImage::new(S, S, PixelFormat::Rgba8);
	let t = TILE_SIZE;
	for ty in 0..S.div_ceil(t) {
		for tx in 0..S.div_ceil(t) {
			let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
			let bytes = buffer.bytes_mut();
			for py in 0..t {
				for px in 0..t {
					let (x, y) = (tx * t + px, ty * t + py);
					if x < S && y < S {
						let i = ((py * t + px) * 4) as usize;
						bytes[i..i + 4].copy_from_slice(&pixel(x, y));
					}
				}
			}
			image.put_buffer(store, tx, ty, buffer);
		}
	}
	Arc::new(Layer::new(LayerId(id), format!("layer {id}"), LayerKind::Pixel { image, offset: (0, 0) }))
}

fn inside(x: u32, y: u32) -> bool {
	(A..B).contains(&x) && (A..B).contains(&y)
}

fn scene(store: &TileStore) -> Document {
	let mut doc = Document::new(
		S,
		S,
		DocumentColor {
			depth: BitDepth::U8,
			profile: ColorProfile::Srgb,
		},
		72.0,
	);
	doc.layers.push(layer(store, 1, |x, y| if inside(x, y) { [200, 30, 30, 255] } else { [0; 4] }));
	doc.layers.push(layer(store, 2, |_, _| [30, 60, 200, 255]));
	doc.selected = vec![LayerId(1)];
	doc
}

fn store(name: &str) -> TileStore {
	let dir = std::env::temp_dir().join(format!("fx-object-layer-{name}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
}

#[test]
fn the_model_reads_the_covered_layer_over_grey() {
	let store = store("plumbing");
	let mut doc = scene(&store);
	let at = |pixels: &[[f32; 4]], x: usize, y: usize| pixels[y * S as usize + x];
	let rect = (0, 0, i64::from(S), i64::from(S));

	let all = fx_engine::ai::composite_rect(&mut doc, &store, 0, rect).unwrap();
	let blue = at(&all, 380, 380);
	assert!(blue[2] > 0.7 && blue[0] < 0.2, "the composite shows the blue layer: {blue:?}");

	let mut solo = fx_engine::ai::layer_alone(&doc, LayerId(1)).expect("a pixel layer has content");
	let alone = fx_engine::ai::composite_rect(&mut solo, &store, 0, rect).unwrap();
	let red = at(&alone, 380, 380);
	assert!(red[0] > 0.7 && red[2] < 0.2 && red[3] > 0.99, "the covered square is read: {red:?}");
	let grey = at(&alone, 40, 40);
	assert!((grey[0] - 0.5).abs() < 0.01 && grey[3] > 0.99, "transparency reads as grey: {grey:?}");

	// The top layer alone is all blue: nothing of the square below.
	let mut top = fx_engine::ai::layer_alone(&doc, LayerId(2)).unwrap();
	let only_top = fx_engine::ai::composite_rect(&mut top, &store, 0, rect).unwrap();
	assert!(at(&only_top, 380, 380)[2] > 0.7);
}

#[test]
#[ignore = "real models and ONNX Runtime"]
fn object_selection_finds_an_object_another_layer_hides() {
	if fx_ai::runtime::find_library().is_none() || !fx_ai::models::BIREFNET.installed() {
		eprintln!("skipped: ONNX Runtime or BiRefNet is missing");
		return;
	}
	let store = store("model");
	let mut doc = scene(&store);
	let boxed = [f64::from(A) - 60.0, f64::from(A) - 60.0, f64::from(B) + 60.0, f64::from(B) + 60.0];
	let mut solo = fx_engine::ai::layer_alone(&doc, LayerId(1)).unwrap();
	let mask = fx_engine::ai::birefnet_object(&mut solo, &store, boxed, &[], 1024).expect("the square is found");
	let ops = fx_engine::ops::EngineOps::default();
	Command::SelectBy {
		select: SelectOp::Model(mask),
		mode: SelectMode::Replace,
	}
	.apply(&mut doc, &mut CommandContext { tiles: &store, ops: Some(&ops) })
	.unwrap();
	let selection = doc.selection.as_ref().expect("a selection");
	let mut reader = fx_engine::ai::CoverageReader::new(selection, &store, (S, S));
	let (mut both, mut either) = (0u64, 0u64);
	for y in 0..S {
		for x in 0..S {
			let truth = inside(x, y);
			let picked = reader.at(i64::from(x), i64::from(y)) >= 0.5;
			both += u64::from(truth && picked);
			either += u64::from(truth || picked);
		}
	}
	let iou = both as f64 / either.max(1) as f64;
	eprintln!("covered square: IoU {iou:.3}");
	assert!(iou > 0.9, "IoU {iou}");
}
