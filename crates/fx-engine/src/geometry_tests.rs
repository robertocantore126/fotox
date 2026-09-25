//! The image-geometry commands through the real engine operations (M6-T02):
//! rotate, flip, canvas size and image size on real tiles.
//!
//! `fx-core`'s own tests run these commands against a stub permutation and
//! `fx-ops` tests the permutation and the sampler on their own. What is only
//! provable here is the pair: a command driving `EngineOps` over a real
//! `TileStore`, so the tile handles, the mips and the document's size are the
//! ones the application builds.

#![cfg(test)]

use std::sync::Arc;

use fx_core::command::MaskFill;
use fx_core::{
	Anchor9, BitDepth, ColorProfile, Command, CommandContext, CommandEffect, Document, DocumentColor, Filter, Layer, LayerId, LayerKind, LayerRef, PixelOps,
};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

use crate::ops::EngineOps;

fn store() -> TileStore {
	let dir = std::env::temp_dir().join("fx-engine-geometry-tests");
	std::fs::create_dir_all(&dir).unwrap();
	TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
}

/// A `width × height` document of one document-sized pixel layer painted by
/// `f`; fully transparent pixels are skipped, so a sparse fixture stays cheap.
fn document_with(store: &TileStore, width: u32, height: u32, f: impl Fn(u32, u32) -> [u16; 4]) -> (Document, LayerId) {
	let mut doc = Document::new(
		width,
		height,
		DocumentColor {
			depth: BitDepth::U16,
			profile: ColorProfile::Srgb,
		},
		72.0,
	);
	let id = doc.allocate_layer_id();
	let mut image = TiledImage::new(width, height, PixelFormat::Rgba16);
	for ty in 0..image.grid(0).rows() {
		for tx in 0..image.grid(0).cols() {
			let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
			let mut used = false;
			for y in 0..TILE_SIZE {
				for x in 0..TILE_SIZE {
					let (gx, gy) = (tx * TILE_SIZE + x, ty * TILE_SIZE + y);
					if gx >= width || gy >= height {
						continue;
					}
					let px = f(gx, gy);
					if px == [0; 4] {
						continue;
					}
					let i = ((y * TILE_SIZE + x) * 4) as usize;
					buffer.as_u16_mut()[i..i + 4].copy_from_slice(&px);
					used = true;
				}
			}
			if used {
				image.put_buffer(store, tx, ty, buffer);
			}
		}
	}
	doc.layers.push(Arc::new(Layer::new(id, "Layer 1", LayerKind::Pixel { image, offset: (0, 0) })));
	doc.selected = vec![id];
	(doc, id)
}

/// Apply `command` with the engine's pixel operations.
fn apply(doc: &mut Document, store: &TileStore, command: Command) -> CommandEffect {
	let ops = EngineOps::default();
	let mut ctx = CommandContext {
		tiles: store,
		ops: Some(&ops as &dyn PixelOps),
	};
	command.apply(doc, &mut ctx).unwrap_or_else(|error| panic!("{command:?}: {error}"))
}

fn image_of(doc: &Document, id: LayerId) -> &TiledImage {
	match &doc.layer(id).expect("layer exists").kind {
		LayerKind::Pixel { image, .. } => image,
		other => panic!("not a pixel layer: {other:?}"),
	}
}

fn offset_of(doc: &Document, id: LayerId) -> (i32, i32) {
	match &doc.layer(id).expect("layer exists").kind {
		LayerKind::Pixel { offset, .. } => *offset,
		other => panic!("not a pixel layer: {other:?}"),
	}
}

/// Every pixel of an image, row-major: what a return trip must reproduce.
fn pixels_of(store: &TileStore, image: &TiledImage) -> Vec<[u16; 4]> {
	let mut out = vec![[0u16; 4]; (image.width() * image.height()) as usize];
	for (tx, ty, slot) in image.grid(0).non_empty() {
		let (x0, y0) = (tx * TILE_SIZE, ty * TILE_SIZE);
		let mut put = |x: u32, y: u32, px: [u16; 4]| {
			if x < image.width() && y < image.height() {
				out[(y * image.width() + x) as usize] = px;
			}
		};
		match slot {
			TileSlot::Empty => {}
			TileSlot::Solid(value) => {
				for y in 0..TILE_SIZE {
					for x in 0..TILE_SIZE {
						put(x0 + x, y0 + y, value.0);
					}
				}
			}
			TileSlot::Data(handle) => {
				let buffer = store.get(handle).unwrap();
				let source = buffer.as_u16();
				for y in 0..TILE_SIZE {
					for x in 0..TILE_SIZE {
						let i = ((y * TILE_SIZE + x) * 4) as usize;
						put(x0 + x, y0 + y, [source[i], source[i + 1], source[i + 2], source[i + 3]]);
					}
				}
			}
		}
	}
	out
}

/// One pixel of an image (what a command must have written there).
fn pixel_of(store: &TileStore, image: &TiledImage, x: u32, y: u32) -> [u16; 4] {
	let tile = match image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
		TileSlot::Empty => return [0; 4],
		TileSlot::Solid(value) => return value.0,
		TileSlot::Data(handle) => store.get(handle).unwrap(),
	};
	let i = (((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) * 4) as usize;
	let source = tile.as_u16();
	[source[i], source[i + 1], source[i + 2], source[i + 3]]
}

/// Whether two images hold the very same tiles (the same handles, or the same
/// solid value): `CanvasSize` must not rewrite one.
fn same_tiles(a: &TiledImage, b: &TiledImage) -> bool {
	if (a.width(), a.height()) != (b.width(), b.height()) {
		return false;
	}
	for ty in 0..a.grid(0).rows() {
		for tx in 0..a.grid(0).cols() {
			if !a.slot(0, tx, ty).same_as(b.slot(0, tx, ty)) {
				return false;
			}
		}
	}
	true
}

fn ramp(x: u32, y: u32) -> [u16; 4] {
	[
		((x * 271 + y * 13) % 65536) as u16,
		((y * 613 + x * 7) % 65536) as u16,
		((x + y) * 3) as u16,
		65_535,
	]
}

#[test]
fn four_quarter_turns_are_the_original_document() {
	let store = store();
	let (mut doc, id) = document_with(&store, 600, 500, ramp);
	let before = pixels_of(&store, image_of(&doc, id));

	for turn in 0..4 {
		apply(&mut doc, &store, Command::RotateCanvas { quarter_turns: 1 });
		let size = if turn % 2 == 0 { (500, 600) } else { (600, 500) };
		assert_eq!((doc.width, doc.height), size, "turn {} swaps the axes", turn + 1);
		assert_eq!(offset_of(&doc, id), (0, 0), "tile (0,0) of the source stays at the origin");
		if turn == 0 {
			// A clockwise turn puts the source's bottom-left pixel at the
			// destination's top-left, and its top-left at the top-right.
			assert_eq!(pixel_of(&store, image_of(&doc, id), 0, 0), ramp(0, 499));
			assert_eq!(pixel_of(&store, image_of(&doc, id), 499, 0), ramp(0, 0));
		}
	}
	assert_eq!((doc.width, doc.height), (600, 500));
	assert_eq!(pixels_of(&store, image_of(&doc, id)), before, "every pixel came home");
}

#[test]
fn a_solid_layer_survives_a_turn_as_one_solid_tile() {
	// A uniform image must not be expanded into pixels: that is what keeps
	// turning a document of fills, masks and empty layers instant. The fixture
	// is a whole number of tiles, so every tile is uniform (a partial last
	// tile holds padding, which is not).
	let store = store();
	let side = 2 * TILE_SIZE;
	let (mut doc, id) = document_with(&store, side, side, |_, _| [12_000, 34_000, 56_000, 65_535]);
	let solid = TileSlot::Solid(fx_tiles::PixelValue([12_000, 34_000, 56_000, 65_535]));
	assert!(image_of(&doc, id).slot(0, 0, 0).same_as(&solid), "the fixture starts uniform");

	apply(&mut doc, &store, Command::RotateCanvas { quarter_turns: 3 });
	let image = image_of(&doc, id);
	assert_eq!((image.width(), image.height()), (side, side));
	for ty in 0..image.grid(0).rows() {
		for tx in 0..image.grid(0).cols() {
			assert!(image.slot(0, tx, ty).same_as(&solid), "tile ({tx}, {ty}) = {:?}", image.slot(0, tx, ty));
		}
	}
}

#[test]
fn flipping_the_canvas_twice_is_the_original_content() {
	let store = store();
	let (mut doc, id) = document_with(&store, 600, 500, ramp);
	let before = pixels_of(&store, image_of(&doc, id));

	apply(&mut doc, &store, Command::FlipCanvas { horizontal: true });
	assert_eq!((doc.width, doc.height), (600, 500), "a flip keeps the canvas size");
	let flipped = image_of(&doc, id);
	assert_eq!(pixel_of(&store, flipped, 599, 0), ramp(0, 0), "left ↔ right");
	assert_eq!(pixel_of(&store, flipped, 0, 499), ramp(599, 499));
	apply(&mut doc, &store, Command::FlipCanvas { horizontal: true });
	assert_eq!(pixels_of(&store, image_of(&doc, id)), before);

	apply(&mut doc, &store, Command::FlipCanvas { horizontal: false });
	let vertical = image_of(&doc, id);
	assert_eq!(pixel_of(&store, vertical, 0, 499), ramp(0, 0), "top ↔ bottom");
	assert_eq!(pixel_of(&store, vertical, 599, 0), ramp(599, 499));
	apply(&mut doc, &store, Command::FlipCanvas { horizontal: false });
	assert_eq!(pixels_of(&store, image_of(&doc, id)), before);
}

#[test]
fn canvas_size_growing_and_shrinking_back_leaves_the_same_tiles() {
	let store = store();
	let (mut doc, id) = document_with(&store, 600, 500, ramp);
	apply(
		&mut doc,
		&store,
		Command::OffsetLayer {
			layer: LayerRef::Id(id),
			dx: 40,
			dy: -30,
		},
	);
	let before = image_of(&doc, id).clone();

	apply(
		&mut doc,
		&store,
		Command::CanvasSize {
			width: 900,
			height: 800,
			anchor: Anchor9::Center,
		},
	);
	assert_eq!((doc.width, doc.height), (900, 800));
	assert_eq!(offset_of(&doc, id), (190, 120), "moved by the anchor's +150");
	assert!(same_tiles(&before, image_of(&doc, id)), "no pixel was rewritten (D-015)");

	apply(
		&mut doc,
		&store,
		Command::CanvasSize {
			width: 600,
			height: 500,
			anchor: Anchor9::Center,
		},
	);
	assert_eq!((doc.width, doc.height), (600, 500));
	assert_eq!(offset_of(&doc, id), (40, -30));
	assert!(same_tiles(&before, image_of(&doc, id)), "content outside the canvas was kept");
}

#[test]
fn image_size_half_then_double_keeps_a_smooth_image() {
	let store = store();
	// A wide, smooth field: the round trip costs half a pixel of shift, so the
	// point is that nothing drifts beyond the local slope.
	let smooth = |x: u32, y: u32| -> [u16; 4] {
		let dx = f64::from(x) - 200.0;
		let dy = f64::from(y) - 200.0;
		let bump = 36_000.0 * (-(dx * dx + dy * dy) / (2.0 * 110.0 * 110.0)).exp();
		let v = (bump + 10_000.0) as u16;
		[v, (v / 2) + 5000, v / 4, 65_535]
	};
	let (mut doc, id) = document_with(&store, 400, 400, smooth);
	let before = pixels_of(&store, image_of(&doc, id));

	for (size, scale) in [(200u32, 0.5f64), (400, 2.0)] {
		apply(
			&mut doc,
			&store,
			Command::ImageSize {
				width: size,
				height: size,
				ppi: 72.0 * scale as f32,
				resample: Some(Filter::Bicubic),
			},
		);
		assert_eq!((doc.width, doc.height), (size, size));
		let image = image_of(&doc, id);
		assert_eq!((image.width(), image.height()), (size, size), "the layer scaled with the canvas");
		assert_eq!(offset_of(&doc, id), (0, 0));
	}
	let after = pixels_of(&store, image_of(&doc, id));
	let (mut worst, mut total, mut count) = (0i32, 0i64, 0i64);
	for y in 8..392u32 {
		for x in 8..392u32 {
			let a = before[(y * 400 + x) as usize];
			let b = after[(y * 400 + x) as usize];
			assert!(b[3] >= 65_534, "({x}, {y}) stayed opaque");
			for c in 0..3 {
				let error = (i32::from(a[c]) - i32::from(b[c])).abs();
				worst = worst.max(error);
				total += i64::from(error);
				count += 1;
			}
		}
	}
	let mean = total / count;
	assert!(worst <= 600, "worst channel error {worst} of 65535");
	assert!(mean <= 150, "mean channel error {mean} of 65535");
}

#[test]
fn a_turn_carries_the_masks_and_the_selection() {
	let store = store();
	let (mut doc, id) = document_with(&store, 600, 500, |_, _| [1000, 2000, 3000, 65_535]);
	apply(
		&mut doc,
		&store,
		Command::AddMask {
			layer: LayerRef::Id(id),
			fill: MaskFill::HideAll,
		},
	);
	// A white square in the mask's top-left corner: turning the canvas carries
	// it to the top-right.
	let mask_format = doc.layer(id).unwrap().mask.as_ref().unwrap().image.format();
	assert_eq!(mask_format, PixelFormat::Gray16);
	let mut buffer = TileBuffer::zeroed(mask_format);
	for y in 0..128 {
		for x in 0..128 {
			let i = ((y * TILE_SIZE + x) * 2) as usize;
			buffer.bytes_mut()[i..i + 2].copy_from_slice(&u16::MAX.to_ne_bytes());
		}
	}
	{
		let layer = doc.layer_mut(id).expect("layer exists");
		layer.mask.as_mut().expect("mask exists").image.put_buffer(&store, 0, 0, buffer);
	}
	apply(&mut doc, &store, Command::SelectAll);

	apply(&mut doc, &store, Command::RotateCanvas { quarter_turns: 1 });
	let mask = &doc.layer(id).unwrap().mask.as_ref().unwrap().image;
	assert_eq!((mask.width(), mask.height()), (500, 600), "the mask is canvas coverage");
	let read = |x: u32, y: u32| -> u16 {
		match mask.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
			TileSlot::Empty => 0,
			TileSlot::Solid(value) => value.0[0],
			TileSlot::Data(handle) => store.get(handle).unwrap().as_u16()[((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize],
		}
	};
	assert_eq!(read(372, 0), u16::MAX, "the old white corner starts here");
	assert_eq!(read(499, 127), u16::MAX, "...and ends at the top-right corner");
	assert_eq!(read(10, 10), 0, "the old top-left is now the bottom-left");
	assert_eq!(read(450, 200), 0);
	// The selection is canvas coverage too, and it turned with the document.
	let selection = doc.selection.as_ref().expect("SelectAll set one");
	assert_eq!((selection.image.width(), selection.image.height()), (500, 600));
	assert_eq!(selection.offset, (0, 0));
}
