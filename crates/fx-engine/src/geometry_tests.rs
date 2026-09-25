//! The image-geometry commands through the real engine operations (M6-T02 and
//! M6-T03): rotate, flip, canvas size, image size and crop on real tiles.
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
fn crop_without_deleting_keeps_the_tiles_and_moves_the_offsets() {
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
		Command::Crop {
			rect: (100, 80, 400, 300),
			angle_deg: 0.0,
			delete_cropped: false,
		},
	);
	assert_eq!((doc.width, doc.height), (400, 300));
	assert_eq!(offset_of(&doc, id), (-60, -110), "the layer moved with the canvas");
	assert!(same_tiles(&before, image_of(&doc, id)), "no pixel was rewritten (D-056)");
}

#[test]
fn crop_with_delete_clears_everything_outside_the_rectangle() {
	let store = store();
	// Content over the first two tile columns and the first tile row.
	let (mut doc, id) = document_with(&store, 600, 500, |x, y| if x < 400 && y < 200 { [1000, 2000, 3000, 65_535] } else { [0; 4] });
	apply(
		&mut doc,
		&store,
		Command::AddMask {
			layer: LayerRef::Id(id),
			fill: MaskFill::RevealAll,
		},
	);
	apply(&mut doc, &store, Command::SelectAll);

	// The rectangle starts in the middle of tile (0, 0): that tile is crossed,
	// tile (1, 0) holds its right part and everything below goes.
	apply(
		&mut doc,
		&store,
		Command::Crop {
			rect: (256, 0, 100, 150),
			angle_deg: 0.0,
			delete_cropped: true,
		},
	);
	assert_eq!((doc.width, doc.height), (100, 150));
	assert_eq!(offset_of(&doc, id), (-256, 0));
	let image = image_of(&doc, id);
	assert_eq!(pixel_of(&store, image, 256, 10), [1000, 2000, 3000, 65_535], "inside the rectangle");
	assert_eq!(pixel_of(&store, image, 255, 10), [0; 4], "just left of it");
	assert_eq!(pixel_of(&store, image, 300, 200), [0; 4], "below it");
	assert!(matches!(image.slot(0, 0, 0), TileSlot::Empty), "a tile wholly outside went");
	assert!(matches!(image.slot(0, 1, 1), TileSlot::Empty), "and a whole row of them");
	// The mask is canvas coverage: it is clipped and then follows the canvas.
	let mask = &doc.layer(id).expect("layer exists").mask.as_ref().expect("mask exists").image;
	assert!(matches!(mask.slot(0, 0, 0), TileSlot::Empty));
	assert!(matches!(mask.slot(0, 1, 0), TileSlot::Data(_)), "the crossed tile was rewritten");
	// The selection too, and it moved with the canvas.
	let selection = doc.selection.as_ref().expect("SelectAll set one");
	assert_eq!(selection.offset, (-256, 0));
	assert!(matches!(selection.image.slot(0, 1, 1), TileSlot::Empty));
}

#[test]
fn a_ten_degree_straighten_levels_a_horizon() {
	let store = store();
	// A horizon tilted 10° down to the right: bright sky above, black ground
	// below.
	let slope = 10f64.to_radians().tan();
	let horizon = |x: u32, y: u32| -> [u16; 4] {
		if f64::from(y) < 150.0 + slope * (f64::from(x) - 200.0) {
			[60_000, 60_000, 60_000, 65_535]
		} else {
			[2_000, 2_000, 2_000, 65_535]
		}
	};
	let (mut doc, id) = document_with(&store, 400, 300, horizon);
	// −10° levels a horizon that leans +10°.
	apply(
		&mut doc,
		&store,
		Command::Crop {
			rect: (0, 0, 400, 300),
			angle_deg: -10.0,
			delete_cropped: false,
		},
	);
	assert_eq!((doc.width, doc.height), (400, 300), "the crop box is the new canvas");

	// The straighten *is* T01's rotation in the crop box's frame: the very same
	// pixels as an arbitrary rotation of the same angle, which is what the card
	// asks us to compare against.
	let (mut turned, turned_id) = document_with(&store, 400, 300, horizon);
	apply(
		&mut turned,
		&store,
		Command::RotateCanvasArbitrary {
			angle_deg: -10.0,
			filter: Filter::BicubicAutomatic,
		},
	);
	let (straight, rotated) = (image_of(&doc, id), image_of(&turned, turned_id));
	for y in (20..280).step_by(7) {
		for x in (20..380).step_by(7) {
			assert_eq!(pixel_of(&store, straight, x, y), pixel_of(&store, rotated, x, y), "({x}, {y})");
		}
	}

	// The horizon the user gets is level: every column crosses from the sky to
	// the ground at the same row, give or take the kernel's one-row reach. A
	// transparent pixel (the corner the turn left empty) is not the ground.
	let ground = |x: u32, y: u32| {
		let p = pixel_of(&store, straight, x, y);
		p[3] > 60_000 && p[0] < 30_000
	};
	let rows: Vec<u32> = (60..340)
		.map(|x| (40..260).find(|y| ground(x, *y)).unwrap_or_else(|| panic!("a horizon in column {x}")))
		.collect();
	let min = *rows.iter().min().expect("columns");
	let max = *rows.iter().max().expect("columns");
	assert!(max - min <= 2, "the horizon is level: rows {min}..={max}");
	// The image starts at the rotated box's corner, which the turn left outside
	// the canvas: the horizon is level in the canvas, right at its middle.
	let middle = rows[140] as i32 + offset_of(&doc, id).1;
	assert!((145..156).contains(&middle), "the horizon sits at the box's middle: {middle}");
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

/// A mask's gray value at mask pixel `(x, y)`, or its outside value off it.
fn mask_at(store: &TileStore, doc: &Document, id: LayerId, canvas: (i32, i32)) -> u16 {
	let layer = doc.layer(id).expect("layer exists");
	let mask = layer.mask.as_ref().expect("mask exists");
	let origin = match (&layer.kind, mask.linked) {
		(LayerKind::Pixel { offset, .. }, true) => *offset,
		_ => (0, 0),
	};
	let (x, y) = (canvas.0 - origin.0, canvas.1 - origin.1);
	if x < 0 || y < 0 || x >= mask.image.width() as i32 || y >= mask.image.height() as i32 {
		return mask.outside_value;
	}
	let (x, y) = (x as u32, y as u32);
	match mask.image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
		TileSlot::Empty => 0,
		TileSlot::Solid(value) => value.0[0],
		TileSlot::Data(handle) => store.get(handle).unwrap().as_u16()[((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize],
	}
}

/// A document whose layer sits at (40, 30) with a hide-all mask holding a
/// white 128² square in its top-left corner.
fn moved_layer_with_a_mask(store: &TileStore, linked: bool) -> (Document, LayerId) {
	let (mut doc, id) = document_with(store, 600, 500, |_, _| [1000, 2000, 3000, 65_535]);
	apply(
		&mut doc,
		store,
		Command::OffsetLayer {
			layer: LayerRef::Id(id),
			dx: 40,
			dy: 30,
		},
	);
	apply(
		&mut doc,
		store,
		Command::AddMask {
			layer: LayerRef::Id(id),
			fill: MaskFill::HideAll,
		},
	);
	let mut buffer = TileBuffer::zeroed(PixelFormat::Gray16);
	for y in 0..128 {
		for x in 0..128 {
			buffer.as_u16_mut()[(y * TILE_SIZE + x) as usize] = u16::MAX;
		}
	}
	let mask = doc.layer_mut(id).expect("layer exists").mask.as_mut().expect("mask exists");
	mask.image.put_buffer(store, 0, 0, buffer);
	mask.linked = linked;
	(doc, id)
}

#[test]
fn a_linked_mask_of_a_moved_layer_turns_with_its_layer() {
	let store = store();
	let (mut doc, id) = moved_layer_with_a_mask(&store, true);
	// The square covers canvas (40..168, 30..158): a quarter turn clockwise of
	// a 600 × 500 canvas sends (x, y) to (499 − y, x), so it lands on
	// (342..470, 40..168) — wherever the turn puts the layer.
	apply(&mut doc, &store, Command::RotateCanvas { quarter_turns: 1 });
	assert_eq!(mask_at(&store, &doc, id, (400, 100)), u16::MAX, "inside the turned square");
	assert_eq!(mask_at(&store, &doc, id, (343, 41)), u16::MAX, "its corner");
	assert_eq!(mask_at(&store, &doc, id, (300, 100)), 0, "left of it");
	assert_eq!(mask_at(&store, &doc, id, (400, 200)), 0, "below it");

	// Image Size moves the layer too: the mask follows.
	let (mut doc, id) = moved_layer_with_a_mask(&store, true);
	apply(
		&mut doc,
		&store,
		Command::ImageSize {
			width: 300,
			height: 250,
			ppi: 72.0,
			resample: Some(Filter::Bilinear),
		},
	);
	// The square is now canvas (20..84, 15..79).
	assert_eq!(mask_at(&store, &doc, id, (50, 45)), u16::MAX);
	assert_eq!(mask_at(&store, &doc, id, (100, 45)), 0);
	assert_eq!(mask_at(&store, &doc, id, (50, 100)), 0);
}

#[test]
fn an_unlinked_mask_moves_with_the_canvas() {
	let store = store();
	let (mut doc, id) = moved_layer_with_a_mask(&store, false);
	// Unlinked, the square is canvas (0..128, 0..128). A crop at (60, 70)
	// moves it to (−60..68, −70..58).
	apply(
		&mut doc,
		&store,
		Command::Crop {
			rect: (60, 70, 400, 300),
			angle_deg: 0.0,
			delete_cropped: false,
		},
	);
	assert_eq!(mask_at(&store, &doc, id, (10, 10)), u16::MAX);
	assert_eq!(mask_at(&store, &doc, id, (67, 57)), u16::MAX, "its last pixel");
	assert_eq!(mask_at(&store, &doc, id, (70, 10)), 0, "right of it");
	assert_eq!(mask_at(&store, &doc, id, (10, 60)), 0, "below it");
	// Canvas Size shifts it the same way.
	apply(
		&mut doc,
		&store,
		Command::CanvasSize {
			width: 500,
			height: 400,
			anchor: Anchor9::BottomRight,
		},
	);
	assert_eq!(mask_at(&store, &doc, id, (110, 110)), u16::MAX, "anchored bottom-right: 100 px further");
	assert_eq!(mask_at(&store, &doc, id, (90, 90)), 0, "what the crop dropped stays dropped");
}

/// The pixel a placed layer shows at canvas `(x, y)` (transparent off it).
fn canvas_pixel(store: &TileStore, doc: &Document, id: LayerId, x: i32, y: i32) -> [u16; 4] {
	let (image, (ox, oy)) = (image_of(doc, id), offset_of(doc, id));
	let (lx, ly) = (x - ox, y - oy);
	if lx < 0 || ly < 0 || lx >= image.width() as i32 || ly >= image.height() as i32 {
		return [0; 4];
	}
	pixel_of(store, image, lx as u32, ly as u32)
}

#[test]
fn an_instant_quarter_turn_of_a_layer_copies_its_pixels_exactly() {
	let store = store();
	let (mut doc, id) = document_with(&store, 300, 200, ramp);
	let linear = crate::tools::transform::instant_turn("xf:rot90cw").expect("known");
	let mapping = crate::tools::transform::turn_mapping(linear, [0.0, 0.0, 300.0, 200.0]);
	apply(
		&mut doc,
		&store,
		Command::Transform {
			layer: LayerRef::Id(id),
			mapping: Box::new(mapping),
			filter: Filter::Bicubic,
		},
	);
	// About (150, 100): source pixel (i, j) lands on canvas (249 − j, i − 50).
	for (i, j) in [(50, 0), (120, 37), (299, 199), (200, 150)] {
		assert_eq!(
			canvas_pixel(&store, &doc, id, 249 - j, i - 50),
			ramp(i as u32, j as u32),
			"({i}, {j}) moved exactly"
		);
	}
	let image = image_of(&doc, id);
	assert_eq!((image.width(), image.height()), (200, 300), "the layer turned");
}

#[test]
fn transforming_a_selection_leaves_a_hole_and_one_undo_restores_both() {
	let store = store();
	let colour = |x: u32, _: u32| {
		if x < 200 {
			[60_000, 1_000, 1_000, 65_535]
		} else {
			[1_000, 1_000, 60_000, 65_535]
		}
	};
	let (mut doc, id) = document_with(&store, 400, 300, colour);
	let ops = EngineOps::default();
	let mut ctx = CommandContext {
		tiles: &store,
		ops: Some(&ops as &dyn PixelOps),
	};
	let mut history = fx_core::History::default();
	history
		.execute(
			&mut doc,
			Command::Select {
				shape: fx_core::SelectionShape::Rect {
					x: 50.0,
					y: 50.0,
					w: 100.0,
					h: 100.0,
				},
				mode: fx_core::SelectMode::Replace,
				feather: 0.0,
				anti_alias: false,
			},
			&mut ctx,
		)
		.expect("select");
	let effect = history
		.execute(
			&mut doc,
			Command::Transform {
				layer: LayerRef::Id(id),
				mapping: Box::new(fx_core::Mapping::translation(200.0, 0.0)),
				filter: Filter::Bicubic,
			},
			&mut ctx,
		)
		.expect("transform");
	assert_eq!(effect.label, "Free Transform");
	assert_eq!(canvas_pixel(&store, &doc, id, 100, 100), [0; 4], "a hole where the pixels were");
	assert_eq!(
		canvas_pixel(&store, &doc, id, 300, 100),
		[60_000, 1_000, 1_000, 65_535],
		"the red pixels, moved"
	);
	assert_eq!(
		canvas_pixel(&store, &doc, id, 300, 200),
		[1_000, 1_000, 60_000, 65_535],
		"outside the moved square: the layer"
	);
	assert_eq!(
		canvas_pixel(&store, &doc, id, 20, 100),
		[60_000, 1_000, 1_000, 65_535],
		"outside the hole: the layer"
	);
	let selection = doc.selection.as_ref().expect("still selected");
	assert!(selection.offset.0 >= 200, "the selection moved with the pixels: {:?}", selection.offset);

	assert!(history.undo(&mut doc), "one step");
	assert_eq!(canvas_pixel(&store, &doc, id, 100, 100), [60_000, 1_000, 1_000, 65_535]);
	assert_eq!(canvas_pixel(&store, &doc, id, 300, 100), [1_000, 1_000, 60_000, 65_535]);
}

#[test]
fn a_transformed_layer_carries_its_linked_mask() {
	let store = store();
	let (mut doc, id) = moved_layer_with_a_mask(&store, true);
	apply(
		&mut doc,
		&store,
		Command::Transform {
			layer: LayerRef::Id(id),
			mapping: Box::new(fx_core::Mapping::translation(30.0, 20.0)),
			filter: Filter::Bicubic,
		},
	);
	// The square was canvas (40..168, 30..158): now (70..198, 50..178).
	assert_eq!(mask_at(&store, &doc, id, (100, 60)), u16::MAX);
	assert_eq!(mask_at(&store, &doc, id, (197, 177)), u16::MAX, "its last pixel");
	assert_eq!(mask_at(&store, &doc, id, (60, 40)), 0, "where it was");
	assert_eq!(canvas_pixel(&store, &doc, id, 45, 35), [0; 4], "the layer moved too");
}
