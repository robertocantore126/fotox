//! The Layer Style options added with the Photoshop-style window
//! (2026-10-01): Layer Knocks Out Drop Shadow, Layer Mask Hides Effects,
//! several instances of one effect, Precise glows, Create Layers. CPU only:
//! the composite through the engine's derived tiles and the reference
//! compositor.

use std::sync::Arc;

use fx_core::styles::{DropShadow, GlowTechnique, LayerStyles, OuterGlow, Stroke, StrokePosition};
use fx_core::{BitDepth, BlendMode, ColorProfile, Command, CommandContext, Document, DocumentColor, Layer, LayerId, LayerKind, LayerRef};
use fx_tiles::{PixelFormat, TileBuffer, TileStore, TileStoreConfig, TiledImage};

const S: u32 = 256;
/// The square: `[A, B)` on both axes.
const A: u32 = 64;
const B: u32 = 192;

fn store(name: &str) -> TileStore {
	let dir = std::env::temp_dir().join(format!("fx-style-options-{name}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
}

/// A document with one pixel layer: an opaque red square.
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
	let mut image = TiledImage::new(S, S, PixelFormat::Rgba8);
	let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
	let bytes = buffer.bytes_mut();
	for y in A..B {
		for x in A..B {
			let i = ((y * S + x) * 4) as usize;
			bytes[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
		}
	}
	image.put_buffer(store, 0, 0, buffer);
	doc.layers.push(Arc::new(Layer::new(LayerId(1), "Square", LayerKind::Pixel { image, offset: (0, 0) })));
	doc.selected = vec![LayerId(1)];
	doc
}

fn style(doc: &mut Document, styles: LayerStyles) {
	let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
	let layer = doc.layer_mut(LayerId(1)).unwrap();
	layer.effects = styles.caches(w, h, format);
	layer.styles = Some(styles);
}

/// The composite's straight RGBA at `(x, y)`.
fn at(doc: &mut Document, store: &TileStore, x: i64, y: i64) -> [f32; 4] {
	fx_engine::ai::composite_rect(doc, store, 0, (x, y, x + 1, y + 1)).unwrap()[0]
}

fn black_shadow(knocks_out: bool) -> DropShadow {
	DropShadow {
		blend: BlendMode::Normal,
		opacity: 1.0,
		distance: 0.0,
		size: 0.0,
		knocks_out,
		..DropShadow::default()
	}
}

#[test]
fn the_layer_knocks_out_its_drop_shadow() {
	let store = store("knockout");
	for (knocks_out, expect_shadow) in [(true, false), (false, true)] {
		let mut doc = scene(&store);
		// Fill 0: the content is invisible, the effects are not.
		doc.layer_mut(LayerId(1)).unwrap().fill = 0.0;
		style(
			&mut doc,
			LayerStyles {
				drop_shadow: vec![black_shadow(knocks_out)],
				..LayerStyles::default()
			},
		);
		let p = at(&mut doc, &store, 128, 128);
		assert_eq!(p[3] > 0.5, expect_shadow, "knocks out {knocks_out}: {p:?}");
	}
}

#[test]
fn a_mask_shapes_the_effects_unless_it_hides_them() {
	let store = store("mask-hides");
	// The mask hides the square's left half (x < 128).
	let mut mask = TiledImage::new(S, S, PixelFormat::Gray8);
	let mut m = TileBuffer::zeroed(PixelFormat::Gray8);
	for y in 0..S {
		for x in 128..S {
			m.bytes_mut()[(y * S + x) as usize] = 255;
		}
	}
	mask.put_buffer(&store, 0, 0, m);
	let green = Stroke {
		size: 6.0,
		position: StrokePosition::Outside,
		color: [0, 65535, 0, 65535],
		..Stroke::default()
	};
	for (hides, stroke_at_mask_edge) in [(false, true), (true, false)] {
		let mut doc = scene(&store);
		doc.layer_mut(LayerId(1)).unwrap().mask = Some(fx_core::Mask {
			image: mask.clone(),
			enabled: true,
			linked: true,
			offset: (0, 0),
		outside_value: 0,
		});
		style(
			&mut doc,
			LayerStyles {
				stroke: vec![green.clone()],
				layer_mask_hides: hides,
				..LayerStyles::default()
			},
		);
		// Just left of the mask's edge, inside the square: the stroke of the
		// masked shape, or nothing when the mask cuts the stroke.
		let p = at(&mut doc, &store, 125, 128);
		let green_there = p[3] > 0.5 && p[1] > 0.8 && p[0] < 0.2;
		assert_eq!(green_there, stroke_at_mask_edge, "hides {hides}: {p:?}");
		// The unmasked half keeps its content either way.
		let q = at(&mut doc, &store, 160, 128);
		assert!(q[0] > 0.9 && q[3] > 0.99, "hides {hides}: {q:?}");
	}
}

#[test]
fn two_strokes_stack_with_the_first_on_top() {
	let store = store("instances");
	let mut doc = scene(&store);
	let stroke = |size: f64, rgba: [u16; 4]| Stroke {
		size,
		position: StrokePosition::Outside,
		color: rgba,
		..Stroke::default()
	};
	style(
		&mut doc,
		LayerStyles {
			// The first (top) is thin and blue, the second wide and green.
			stroke: vec![stroke(4.0, [0, 0, 65535, 65535]), stroke(12.0, [0, 65535, 0, 65535])],
			..LayerStyles::default()
		},
	);
	let near = at(&mut doc, &store, i64::from(A) - 2, 128);
	assert!(near[2] > 0.8 && near[1] < 0.2, "the top stroke wins where both reach: {near:?}");
	let far = at(&mut doc, &store, i64::from(A) - 9, 128);
	assert!(far[1] > 0.8 && far[2] < 0.2, "the wider one shows beyond: {far:?}");
}

#[test]
fn a_precise_glow_keeps_the_corner_a_softer_glow_rounds() {
	let store = store("precise");
	let glow = |technique| OuterGlow {
		blend: BlendMode::Normal,
		opacity: 1.0,
		color: [0, 0, 65535, 65535],
		size: 16.0,
		spread: 0.0,
		technique,
		..OuterGlow::default()
	};
	let mut alpha = Vec::new();
	for technique in [GlowTechnique::Softer, GlowTechnique::Precise] {
		let mut doc = scene(&store);
		style(
			&mut doc,
			LayerStyles {
				outer_glow: vec![glow(technique)],
				..LayerStyles::default()
			},
		);
		// Diagonally off the square's corner, and straight off its side, at
		// the same distance.
		let corner = at(&mut doc, &store, i64::from(A) - 5, i64::from(A) - 5)[3];
		let side = at(&mut doc, &store, i64::from(A) - 7, 128)[3];
		alpha.push((corner, side));
	}
	let (softer, precise) = (alpha[0], alpha[1]);
	assert!(precise.0 > 0.0 && precise.1 > 0.0, "the precise glow reaches: {precise:?}");
	// Precise falls off with the true distance: the corner at 7 px is as
	// strong as the side at 7 px; a blur weakens the corner more.
	assert!((precise.0 - precise.1).abs() < 0.08, "precise {precise:?}");
	assert!(softer.0 < softer.1, "softer {softer:?}");
}

#[test]
fn create_layers_turns_each_effect_into_a_layer() {
	let store = store("create-layers");
	let mut doc = scene(&store);
	style(
		&mut doc,
		LayerStyles {
			drop_shadow: vec![DropShadow {
				distance: 10.0,
				..black_shadow(true)
			}],
			stroke: vec![Stroke {
				size: 4.0,
				position: StrokePosition::Inside,
				color: [0, 65535, 0, 65535],
				..Stroke::default()
			}],
			..LayerStyles::default()
		},
	);
	let before = at(&mut doc, &store, i64::from(A) + 1, 128);
	let ops = fx_engine::ops::EngineOps::default();
	Command::CreateEffectLayers {
		layer: LayerRef::Id(LayerId(1)),
	}
	.apply(&mut doc, &mut CommandContext { tiles: &store, ops: Some(&ops) })
	.unwrap();
	let names: Vec<(&str, bool)> = doc.layers.iter().map(|l| (l.name.as_str(), l.clipped)).collect();
	assert_eq!(names, vec![("Square's Drop Shadow", false), ("Square", false), ("Square's Inner Stroke", true)]);
	assert!(doc.layer(LayerId(1)).unwrap().styles.is_none(), "the effects left the layer");
	// The picture is the same: the inner stroke still shows inside the edge.
	let after = at(&mut doc, &store, i64::from(A) + 1, 128);
	for k in 0..4 {
		assert!((before[k] - after[k]).abs() < 0.02, "before {before:?}, after {after:?}");
	}
}
