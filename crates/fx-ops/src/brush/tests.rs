//! The brush engine's spec tests (M5-T06, M5-T08).

use std::sync::Arc;

use fx_core::BlendMode;
use fx_core::stroke::{BrushParams, StrokeSample, StrokeTool};
use fx_tiles::{PixelFormat, PixelValue, TILE_SIZE, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

use super::stroke::{LayerSource, SourceTiles, Stroke, StrokeSetup, replay};

fn store() -> TileStore {
	let dir = std::env::temp_dir().join("fx-ops-brush-tests");
	std::fs::create_dir_all(&dir).unwrap();
	TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
}

fn sample(x: f64, y: f64) -> StrokeSample {
	StrokeSample {
		x,
		y,
		pressure: 1.0,
		tilt_x: 0.0,
		tilt_y: 0.0,
		time_us: 0,
	}
}

fn solid(size: (u32, u32), format: PixelFormat, value: PixelValue) -> TiledImage {
	let mut image = TiledImage::new(size.0, size.1, format);
	for ty in 0..size.1.div_ceil(TILE_SIZE) {
		for tx in 0..size.0.div_ceil(TILE_SIZE) {
			image.set_slot(tx, ty, TileSlot::Solid(value));
		}
	}
	image
}

/// Straight RGBA `0..=1` at layer pixel `(x, y)`.
fn px(image: &TiledImage, store: &TileStore, x: u32, y: u32) -> [f32; 4] {
	let i = ((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize;
	let buffer = match image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
		TileSlot::Empty => return [0.0; 4],
		TileSlot::Solid(v) => return v.0.map(|c| f32::from(c) / 65535.0),
		TileSlot::Data(h) => store.get(h).unwrap(),
	};
	match image.format() {
		PixelFormat::Rgba16 => {
			let s = &buffer.as_u16()[i * 4..i * 4 + 4];
			[s[0], s[1], s[2], s[3]].map(|c| f32::from(c) / 65535.0)
		}
		_ => {
			let s = &buffer.bytes()[i * 4..i * 4 + 4];
			[s[0], s[1], s[2], s[3]].map(|c| f32::from(c) / 255.0)
		}
	}
}

fn setup<'a>(image: &'a TiledImage, tool: StrokeTool, brush: BrushParams, color: [u16; 4]) -> StrokeSetup<'a> {
	StrokeSetup {
		image,
		offset: (0, 0),
		canvas: (image.width(), image.height()),
		selection: None,
		tool,
		brush,
		color,
		lock_alpha: false,
		source: None,
	}
}

const RED: [u16; 4] = [65535, 0, 0, 65535];

fn hard(diameter: f32) -> BrushParams {
	BrushParams {
		diameter,
		hardness: 1.0,
		..Default::default()
	}
}

#[test]
fn a_single_hard_dab_covers_the_disc() {
	let store = store();
	let image = TiledImage::new(300, 300, PixelFormat::Rgba16);
	let (out, _) = replay(setup(&image, StrokeTool::Brush, hard(60.0), RED), &[sample(150.0, 150.0)], &store).unwrap();
	let mut area = 0.0f64;
	for y in 100..200 {
		for x in 100..200 {
			area += f64::from(px(&out, &store, x, y)[3]);
		}
	}
	let exact = std::f64::consts::PI * 30.0 * 30.0;
	assert!((area - exact).abs() / exact < 0.005, "{area} vs {exact}");
	assert_eq!(px(&out, &store, 150, 150), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn batching_does_not_change_the_pixels() {
	let store = store();
	let image = solid((600, 300), PixelFormat::Rgba16, PixelValue::rgba16(65535, 65535, 65535, 65535));
	let brush = BrushParams {
		diameter: 37.0,
		hardness: 0.4,
		flow: 0.6,
		opacity: 0.8,
		spacing: 0.2,
		..Default::default()
	};
	let samples: Vec<StrokeSample> = (0..40)
		.map(|i| sample(20.0 + f64::from(i) * 14.1, 150.0 + (f64::from(i) * 0.5).sin() * 60.0))
		.collect();
	let (once, _) = replay(setup(&image, StrokeTool::Brush, brush, RED), &samples, &store).unwrap();
	let mut live = Stroke::begin(setup(&image, StrokeTool::Brush, brush, RED), &store).unwrap();
	for chunk in samples.chunks(7) {
		live.add(chunk).unwrap();
	}
	let (batched, _) = live.finish().unwrap();
	for ty in 0..2 {
		for tx in 0..3 {
			let (a, b) = (once.slot(0, tx, ty), batched.slot(0, tx, ty));
			match (a, b) {
				(TileSlot::Data(x), TileSlot::Data(y)) => assert_eq!(store.get(x).unwrap().bytes(), store.get(y).unwrap().bytes(), "tile ({tx}, {ty})"),
				_ => assert!(a.same_as(b), "tile ({tx}, {ty})"),
			}
		}
	}
}

#[test]
fn flow_builds_up_and_opacity_is_a_ceiling() {
	let store = store();
	let image = TiledImage::new(200, 100, PixelFormat::Rgba16);
	let half_flow = BrushParams { flow: 0.5, ..hard(40.0) };
	// Two passes over the same point within one stroke: 1 − 0.5² = 75 %.
	let (out, _) = replay(
		setup(&image, StrokeTool::Brush, half_flow, RED),
		&[sample(50.0, 50.0), sample(50.0, 50.0)],
		&store,
	)
	.unwrap();
	// A zero-length segment places no dab; go away and come back.
	assert!((px(&out, &store, 50, 50)[3] - 0.5).abs() < 1e-3);
	let there_and_back = [sample(50.0, 50.0), sample(150.0, 50.0), sample(50.0, 50.0)];
	let brush = BrushParams { spacing: 2.5, ..half_flow };
	let (out, _) = replay(setup(&image, StrokeTool::Brush, brush, RED), &there_and_back, &store).unwrap();
	assert!((px(&out, &store, 50, 50)[3] - 0.75).abs() < 1e-3, "{:?}", px(&out, &store, 50, 50));
	// Opacity 50 %: never above, however many passes.
	let ceiling = BrushParams { opacity: 0.5, ..hard(40.0) };
	let scribble: Vec<StrokeSample> = (0..30).map(|i| sample(if i % 2 == 0 { 40.0 } else { 60.0 }, 50.0)).collect();
	let (out, _) = replay(setup(&image, StrokeTool::Brush, ceiling, RED), &scribble, &store).unwrap();
	assert!(
		(px(&out, &store, 50, 50)[3] - 0.5).abs() < 1.0 / 65535.0 * 2.0,
		"{:?}",
		px(&out, &store, 50, 50)
	);
}

#[test]
fn a_stroke_across_four_tiles_has_no_seam() {
	let store = store();
	let image = solid((512, 512), PixelFormat::Rgba8, PixelValue::rgba8(255, 255, 255, 255));
	let brush = BrushParams {
		diameter: 60.0,
		hardness: 0.0,
		..Default::default()
	};
	let (out, _) = replay(
		setup(&image, StrokeTool::Brush, brush, RED),
		&[sample(200.0, 256.0), sample(312.0, 256.0)],
		&store,
	)
	.unwrap();
	// Around the tile corner, the result is symmetric in y about 256 and
	// continuous across x = 256.
	for x in 240..272 {
		let (above, below) = (px(&out, &store, x, 255)[1], px(&out, &store, x, 256)[1]);
		assert!((above - below).abs() < 2.0 / 255.0, "x {x}: {above} {below}");
	}
	for y in 230..282 {
		let (left, right) = (px(&out, &store, 255, y)[1], px(&out, &store, 256, y)[1]);
		assert!((left - right).abs() < 2.0 / 255.0, "y {y}: {left} {right}");
	}
}

#[test]
fn a_multiply_brush_on_white_is_the_colour() {
	let store = store();
	let image = solid((100, 100), PixelFormat::Rgba16, PixelValue::rgba16(65535, 65535, 65535, 65535));
	let brush = BrushParams {
		mode: BlendMode::Multiply,
		..hard(30.0)
	};
	let color = [20000, 40000, 60000, 65535];
	let (out, _) = replay(setup(&image, StrokeTool::Brush, brush, color), &[sample(50.0, 50.0)], &store).unwrap();
	let p = px(&out, &store, 50, 50);
	for c in 0..3 {
		assert!((p[c] - f32::from(color[c]) / 65535.0).abs() < 1.0 / 65535.0, "{p:?}");
	}
}

#[test]
fn the_eraser_goes_to_transparency_and_the_lock_keeps_alpha() {
	let store = store();
	let image = solid((100, 100), PixelFormat::Rgba8, PixelValue::rgba8(0, 0, 255, 255));
	let (erased, _) = replay(setup(&image, StrokeTool::Eraser, hard(30.0), RED), &[sample(50.0, 50.0)], &store).unwrap();
	assert_eq!(px(&erased, &store, 50, 50)[3], 0.0);
	let transparent = TiledImage::new(100, 100, PixelFormat::Rgba8);
	let mut locked = setup(&transparent, StrokeTool::Brush, hard(30.0), RED);
	locked.lock_alpha = true;
	let (out, _) = replay(locked, &[sample(50.0, 50.0)], &store).unwrap();
	assert_eq!(px(&out, &store, 50, 50)[3], 0.0, "no pixel to colour");
}

#[test]
fn the_clone_stamp_copies_the_source_exactly() {
	let store = store();
	// A layer with a pattern on the left half.
	let mut image = TiledImage::new(512, 256, PixelFormat::Rgba16);
	let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
	for y in 0..256u32 {
		for x in 0..256u32 {
			let i = ((y * 256 + x) * 4) as usize;
			buffer.as_u16_mut()[i..i + 4].copy_from_slice(&[(x * 250) as u16, (y * 250) as u16, 1234, 65535]);
		}
	}
	image.put_buffer(&store, 0, 0, buffer);
	let source: Arc<dyn SourceTiles> = Arc::new(LayerSource {
		image: image.clone(),
		offset: (0, 0),
		store: store.clone(),
	});
	let mut s = setup(
		&image,
		StrokeTool::Clone {
			dx: 256.0,
			dy: 0.0,
			sample_all: false,
		},
		hard(40.0),
		RED,
	);
	s.source = Some(source);
	let (out, _) = replay(s, &[sample(356.0, 100.0)], &store).unwrap();
	// Inside the dab: the pixel 256 px to the left.
	for (x, y) in [(356u32, 100u32), (350, 110), (370, 95)] {
		assert_eq!(px(&out, &store, x, y), px(&image, &store, x - 256, y), "({x}, {y})");
	}
}

#[test]
fn healing_takes_the_destinations_level_without_a_seam() {
	let store = store();
	// Destination: flat grey 0.3 on the right, a darker patch in it; source:
	// flat 0.7 on the left.
	let mut image = TiledImage::new(512, 256, PixelFormat::Rgba16);
	image.set_slot(0, 0, TileSlot::Solid(PixelValue::rgba16(45875, 45875, 45875, 65535)));
	let mut right = TileBuffer::filled(PixelFormat::Rgba16, PixelValue::rgba16(19661, 19661, 19661, 65535));
	for y in 95..105u32 {
		for x in 95..105u32 {
			let i = ((y * 256 + x) * 4) as usize;
			right.as_u16_mut()[i..i + 3].copy_from_slice(&[5000, 5000, 5000]);
		}
	}
	image.put_buffer(&store, 1, 0, right);
	let source: Arc<dyn SourceTiles> = Arc::new(LayerSource {
		image: image.clone(),
		offset: (0, 0),
		store: store.clone(),
	});
	let mut s = setup(
		&image,
		StrokeTool::Heal {
			dx: 256.0,
			dy: 0.0,
			sample_all: false,
		},
		hard(40.0),
		RED,
	);
	s.source = Some(source);
	let (out, _) = replay(s, &[sample(356.0, 100.0)], &store).unwrap();
	for (x, y) in [(356u32, 100u32), (340, 100), (372, 110), (356, 82)] {
		let v = px(&out, &store, x, y)[0];
		assert!((v - 0.3).abs() < 1.0 / 255.0, "({x}, {y}) = {v}");
	}
}

#[test]
fn spot_healing_removes_a_dot_on_a_gradient() {
	let store = store();
	let mut image = TiledImage::new(256, 256, PixelFormat::Rgba16);
	let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
	let level = |x: u32| (0.2 + 0.6 * x as f32 / 255.0) * 65535.0;
	for y in 0..256u32 {
		for x in 0..256u32 {
			let v = if (x as i32 - 128).pow(2) + (y as i32 - 128).pow(2) <= 16 {
				0.0
			} else {
				level(x)
			};
			let i = ((y * 256 + x) * 4) as usize;
			buffer.as_u16_mut()[i..i + 4].copy_from_slice(&[v as u16, v as u16, v as u16, 65535]);
		}
	}
	image.put_buffer(&store, 0, 0, buffer);
	let source: Arc<dyn SourceTiles> = Arc::new(LayerSource {
		image: image.clone(),
		offset: (0, 0),
		store: store.clone(),
	});
	let mut s = setup(&image, StrokeTool::SpotHeal, hard(20.0), RED);
	s.source = Some(source);
	let (out, _) = replay(s, &[sample(128.5, 128.5)], &store).unwrap();
	let worst = (120..137u32)
		.flat_map(|y| (120..137u32).map(move |x| (x, y)))
		.map(|(x, y)| (px(&out, &store, x, y)[0] - level(x) / 65535.0).abs())
		.fold(0.0f32, f32::max);
	assert!(worst < 2.0 / 255.0, "residual {worst}");
}

/// HARDEN W4: a heal stroke longer than one block is solved block by block
/// (bounded buffers); both ends still heal, and the seam between blocks does
/// not show on the gradient.
#[test]
fn a_long_spot_heal_stroke_heals_across_blocks() {
	let store = store();
	let (w, h) = (1536u32, 64u32);
	let mut image = TiledImage::new(w, h, PixelFormat::Rgba16);
	let level = |x: u32| (0.2 + 0.6 * x as f32 / (w - 1) as f32) * 65535.0;
	let dots = [(100i32, 32i32), (1400, 32)];
	for tx in 0..w / 256 {
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		for y in 0..64u32 {
			for lx in 0..256u32 {
				let x = tx * 256 + lx;
				let dot = dots.iter().any(|(dx, dy)| (x as i32 - dx).pow(2) + (y as i32 - dy).pow(2) <= 16);
				let v = if dot { 0.0 } else { level(x) };
				let i = ((y * 256 + lx) * 4) as usize;
				buffer.as_u16_mut()[i..i + 4].copy_from_slice(&[v as u16, v as u16, v as u16, 65535]);
			}
		}
		image.put_buffer(&store, tx, 0, buffer);
	}
	let source: Arc<dyn SourceTiles> = Arc::new(LayerSource {
		image: image.clone(),
		offset: (0, 0),
		store: store.clone(),
	});
	let mut s = setup(&image, StrokeTool::SpotHeal, hard(20.0), RED);
	s.source = Some(source);
	let samples: Vec<_> = (0..=260).map(|i| sample(100.5 + f64::from(i) * 5.0, 32.5)).collect();
	let (out, _) = replay(s, &samples, &store).unwrap();
	for (dx, dy) in dots {
		let worst = (dy - 6..=dy + 6)
			.flat_map(|y| (dx - 6..=dx + 6).map(move |x| (x as u32, y as u32)))
			.map(|(x, y)| (px(&out, &store, x, y)[0] - level(x) / 65535.0).abs())
			.fold(0.0f32, f32::max);
		assert!(worst < 4.0 / 255.0, "dot at {dx}: residual {worst}");
	}
	// Across the block seam (x = 1124 is inside the second block's core).
	let seam = (1100..1150u32)
		.map(|x| (px(&out, &store, x, 32)[0] - level(x) / 65535.0).abs())
		.fold(0.0f32, f32::max);
	assert!(seam < 4.0 / 255.0, "seam residual {seam}");
}
