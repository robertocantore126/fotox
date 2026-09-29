//! How precisely the Object Selection tool follows an object's outline, at
//! full resolution, against an exact ground truth (Rob, 2026-09-29: "the
//! Photoshop one is more precise"). The old path shrank the whole document
//! to 1024 px before EfficientSAM saw it; a box now reads a crop around it
//! at up to full resolution (`ai::object_crop`).
//!
//! The scene: a 3000 × 2000 canvas of textured "foliage", and a 360 px
//! textured red object with thin limbs and a hole — the parts a coarse mask
//! loses first. Ignored by default (real models, ONNX Runtime):
//!
//! `FOTOX_ORT_DYLIB=… cargo test --release -p fx-engine --test object_precision -- --ignored --nocapture`

use std::sync::Arc;
use std::time::Instant;

use fx_core::select_ops::SelectOp;
use fx_core::{BitDepth, ColorProfile, Command, CommandContext, Document, DocumentColor, Layer, LayerId, LayerKind, SelectMode};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileStore, TileStoreConfig, TiledImage};

const W: u32 = 3000;
const H: u32 = 2000;
const CX: f64 = 1700.0;
const CY: f64 = 1100.0;

/// The object, exactly: an elliptic body with a round hole, five thin limbs.
fn inside(x: f64, y: f64) -> bool {
	let (dx, dy) = (x - CX, y - CY);
	let body = (dx / 120.0).powi(2) + (dy / 90.0).powi(2) <= 1.0;
	let hole = (dx - 30.0).hypot(dy + 10.0) <= 24.0;
	if body && !hole {
		return true;
	}
	// Limbs: 12 px wide, from the body outward, at five angles.
	[-2.4f64, -1.2, 0.3, 1.1, 2.2].iter().enumerate().any(|(i, &a)| {
		let (c, s) = (a.cos(), a.sin());
		let along = dx * c + dy * s;
		let across = -dx * s + dy * c;
		let width = 5.0 + i as f64; // 5 to 9 px half-width
		along > 60.0 && along < 190.0 && across.abs() <= width
	})
}

fn hash(x: u32, y: u32) -> f64 {
	let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
	h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
	f64::from(h ^ (h >> 16)) / f64::from(u32::MAX)
}

fn pixel(x: u32, y: u32) -> [u8; 4] {
	let (fx, fy) = (f64::from(x), f64::from(y));
	let n = hash(x, y) * 0.12;
	let rgb = if inside(fx + 0.5, fy + 0.5) {
		// Red, shaded and textured.
		let shade = 0.75 + 0.2 * ((fx * 0.05).sin() * (fy * 0.04).cos());
		[0.72 * shade + n, 0.18 * shade + n * 0.5, 0.14 * shade + n * 0.5]
	} else {
		// Foliage: greens and browns in blotches, busy enough to confuse.
		let t = 0.5 + 0.25 * ((fx * 0.013).sin() + (fy * 0.017).cos()) + 0.15 * ((fx + fy) * 0.041).sin();
		[0.25 + 0.25 * t + n, 0.35 + 0.2 * (1.0 - t) + n, 0.15 + 0.1 * t + n]
	};
	let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
	[c(rgb[0]), c(rgb[1]), c(rgb[2]), 255]
}

fn scene(store: &TileStore) -> Document {
	let mut doc = Document::new(
		W,
		H,
		DocumentColor {
			depth: BitDepth::U8,
			profile: ColorProfile::Srgb,
		},
		72.0,
	);
	let mut image = TiledImage::new(W, H, PixelFormat::Rgba8);
	let t = TILE_SIZE;
	for ty in 0..H.div_ceil(t) {
		for tx in 0..W.div_ceil(t) {
			let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
			let bytes = buffer.bytes_mut();
			for py in 0..t {
				for px in 0..t {
					let (x, y) = (tx * t + px, ty * t + py);
					if x < W && y < H {
						let i = ((py * t + px) * 4) as usize;
						bytes[i..i + 4].copy_from_slice(&pixel(x, y));
					}
				}
			}
			image.put_buffer(store, tx, ty, buffer);
		}
	}
	doc.layers.push(Arc::new(Layer::new(LayerId(1), "scene", LayerKind::Pixel { image, offset: (0, 0) })));
	doc
}

struct Score {
	iou: f64,
	/// IoU within 6 px of the true outline: how well the edge is placed.
	edge_iou: f64,
	/// Limb pixels (the thin parts) that were selected.
	limbs: f64,
}

fn score(doc: &Document, store: &TileStore) -> Score {
	let selection = doc.selection.as_ref().expect("a selection");
	let mut reader = fx_engine::ai::CoverageReader::new(selection, store, (W, H));
	let (x0, y0, x1, y1) = ((CX - 350.0) as i64, (CY - 350.0) as i64, (CX + 350.0) as i64, (CY + 350.0) as i64);
	let (mut both, mut either, mut eb, mut ee, mut limb_in, mut limb) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
	let near_edge = |x: f64, y: f64| {
		let v = inside(x, y);
		[(-6.0, 0.0), (6.0, 0.0), (0.0, -6.0), (0.0, 6.0), (-4.2, -4.2), (4.2, 4.2), (-4.2, 4.2), (4.2, -4.2)]
			.iter()
			.any(|(ox, oy)| inside(x + ox, y + oy) != v)
	};
	for y in y0..y1 {
		for x in x0..x1 {
			let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
			let truth = inside(fx, fy);
			let picked = reader.at(x, y) >= 0.5;
			both += u64::from(truth && picked);
			either += u64::from(truth || picked);
			if near_edge(fx, fy) {
				eb += u64::from(truth && picked);
				ee += u64::from(truth || picked);
			}
			let body = ((fx - CX) / 120.0).powi(2) + ((fy - CY) / 90.0).powi(2) <= 1.0;
			if truth && !body {
				limb += 1;
				limb_in += u64::from(picked);
			}
		}
	}
	Score {
		iou: both as f64 / either.max(1) as f64,
		edge_iou: eb as f64 / ee.max(1) as f64,
		limbs: limb_in as f64 / limb.max(1) as f64,
	}
}

fn select(doc: &mut Document, store: &TileStore, mask: fx_core::select_ops::ModelMask) {
	let ops = fx_engine::ops::EngineOps::default();
	Command::SelectBy {
		select: SelectOp::Model(mask),
		mode: SelectMode::Replace,
	}
	.apply(doc, &mut CommandContext { tiles: store, ops: Some(&ops) })
	.expect("the selection applies");
}

#[test]
#[ignore = "real models and ONNX Runtime"]
fn object_selection_follows_a_small_object_at_full_resolution() {
	if fx_ai::runtime::find_library().is_none() || !fx_ai::models::EFFICIENT_SAM.installed() {
		eprintln!("skipped: ONNX Runtime or EfficientSAM is missing");
		return;
	}
	let dir = std::env::temp_dir().join(format!("fx-object-precision-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let store = TileStore::new(TileStoreConfig::for_tests(dir)).unwrap();
	let mut doc = scene(&store);
	// A box around the object, as a user drags it: a little loose.
	let boxed = [CX - 215.0, CY - 205.0, CX + 215.0, CY + 205.0];

	// Before: the whole document at ≤ 1024 px.
	let t = Instant::now();
	let work = fx_engine::ai::working_composite(&mut doc, &store, 1024).unwrap();
	let embedding = fx_engine::ai::sam_embedding(&work).unwrap();
	let mask = fx_engine::ai::sam_mask(&embedding, Some(boxed), &[]).unwrap();
	let whole_time = t.elapsed();
	select(&mut doc, &store, mask);
	let whole = score(&doc, &store);

	// After: a crop around the box.
	let t = Instant::now();
	let area = fx_engine::ai::object_crop(boxed, (W, H));
	let work = fx_engine::ai::working_crop(&mut doc, &store, area, 1024).unwrap();
	let embedding = fx_engine::ai::sam_embedding(&work).unwrap();
	let mask = fx_engine::ai::sam_mask(&embedding, Some(boxed), &[]).unwrap();
	let crop_time = t.elapsed();
	select(&mut doc, &store, mask);
	let crop = score(&doc, &store);

	eprintln!(
		"whole document (level {}): IoU {:.3}, edge IoU {:.3}, limbs {:.3}, {whole_time:?}",
		fx_engine::ai::working_composite(&mut doc, &store, 1024).unwrap().level,
		whole.iou,
		whole.edge_iou,
		whole.limbs
	);
	eprintln!("crop {area:?}: IoU {:.3}, edge IoU {:.3}, limbs {:.3}, {crop_time:?}", crop.iou, crop.edge_iou, crop.limbs);
	assert!(crop.edge_iou >= whole.edge_iou, "the crop places the edge at least as well");
	assert!(crop.iou > 0.9, "IoU {}", crop.iou);

	// A single click on the body: the whole document finds it, a crop
	// around what it found outlines it.
	let click = [((CX - 60.0, CY + 20.0), true)];
	let work = fx_engine::ai::working_composite(&mut doc, &store, 1024).unwrap();
	let embedding = fx_engine::ai::sam_embedding(&work).unwrap();
	let coarse = fx_engine::ai::sam_mask(&embedding, None, &click).unwrap();
	select(&mut doc, &store, coarse.clone());
	let click_whole = score(&doc, &store);
	let t = Instant::now();
	let refined = fx_engine::ai::click_refine(&mut doc, &store, coarse, &click, 1024).unwrap();
	let refine_time = t.elapsed();
	select(&mut doc, &store, refined);
	let click_crop = score(&doc, &store);
	eprintln!(
		"click, whole document: IoU {:.3}, edge IoU {:.3}, limbs {:.3}",
		click_whole.iou, click_whole.edge_iou, click_whole.limbs
	);
	eprintln!(
		"click, second pass on a crop: IoU {:.3}, edge IoU {:.3}, limbs {:.3}, +{refine_time:?}",
		click_crop.iou, click_crop.edge_iou, click_crop.limbs
	);
	assert!(click_crop.edge_iou >= click_whole.edge_iou, "the second pass places the edge at least as well");
}
