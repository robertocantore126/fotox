//! The brush-plugin boundary (D-096): `plugins/plain-eraser` is the native
//! Eraser written as a plugin, so a stroke must paint the same pixels both
//! ways, and the time difference is what the wasm boundary costs.
//!
//! Needs the plugins built (`cargo xtask plugins`, or `cargo build --release`
//! in `plugins/`); without them the tests say so and pass. The timing is
//! `#[ignore]`d — run it in release:
//!
//! ```text
//! cargo test --release -p fx-ops --test plugin_boundary -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fx_core::stroke::{BrushParams, StrokeSample, StrokeTool};
use fx_ops::brush::{Stroke, StrokeSetup, replay};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

fn wasm(name: &str) -> Option<PathBuf> {
	let dir = std::env::var_os("FOTOX_PLUGIN_BUILD")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/target/wasm32-unknown-unknown/release"));
	let path = dir.join(format!("{name}.wasm"));
	if path.exists() {
		Some(path)
	} else {
		eprintln!("SKIPPED: {} not built (cargo xtask plugins)", path.display());
		None
	}
}

fn store() -> TileStore {
	let dir = std::env::temp_dir().join("fx-ops-plugin-boundary");
	std::fs::create_dir_all(&dir).unwrap();
	TileStore::new(TileStoreConfig {
		hot_budget: 2 << 30,
		warm_budget: 512 << 20,
		scratch_dir: dir,
		scratch_limit: 1 << 30,
		background_trim: false,
	})
	.unwrap()
}

/// An RGBA8 image whose every pixel differs (colour and alpha from a hash of
/// the position): uniform data would hide a channel or premultiply mix-up.
fn varied(size: u32, store: &TileStore) -> TiledImage {
	let mut image = TiledImage::new(size, size, PixelFormat::Rgba8);
	let tiles = size.div_ceil(TILE_SIZE);
	for ty in 0..tiles {
		for tx in 0..tiles {
			let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
			for (i, px) in buffer.bytes_mut().chunks_exact_mut(4).enumerate() {
				let x = tx * TILE_SIZE + i as u32 % TILE_SIZE;
				let y = ty * TILE_SIZE + i as u32 / TILE_SIZE;
				let h = (x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b)).wrapping_mul(0xc2b2_ae35);
				px.copy_from_slice(&[(h >> 24) as u8, (h >> 16) as u8, (x / 8) as u8, 64 + (h >> 9) as u8 % 192]);
			}
			image.put_buffer(store, tx, ty, buffer);
		}
	}
	image
}

/// A wavy stroke across the image.
fn samples(size: u32) -> Vec<StrokeSample> {
	let s = f64::from(size);
	(0..=400)
		.map(|i| {
			let t = f64::from(i) / 400.0;
			StrokeSample {
				x: s * (0.05 + 0.9 * t),
				y: s * (0.5 + 0.3 * (t * 9.0).sin()),
				pressure: 1.0,
				tilt_x: 0.0,
				tilt_y: 0.0,
				time_us: 0,
			}
		})
		.collect()
}

fn brush(diameter: f32) -> BrushParams {
	BrushParams {
		diameter,
		hardness: 0.0,
		spacing: 0.1,
		flow: 0.5,
		opacity: 0.8,
		..Default::default()
	}
}

fn paint(image: &TiledImage, tool: StrokeTool, brush: BrushParams, samples: &[StrokeSample], store: &TileStore) -> (TiledImage, Duration) {
	let setup = StrokeSetup {
		image,
		offset: (0, 0),
		canvas: (image.width(), image.height()),
		selection: None,
		tool,
		brush,
		color: [65535, 65535, 65535, 65535],
		lock_alpha: false,
		source: None,
	};
	let start = Instant::now();
	let (out, _) = replay(setup, samples, store).unwrap();
	(out, start.elapsed())
}

/// The live path: samples arrive two at a time (a pen at ~120 Hz), each
/// batch repaints only what its dabs touched — many small calls.
fn paint_live(image: &TiledImage, tool: StrokeTool, brush: BrushParams, samples: &[StrokeSample], store: &TileStore) -> Duration {
	let setup = StrokeSetup {
		image,
		offset: (0, 0),
		canvas: (image.width(), image.height()),
		selection: None,
		tool,
		brush,
		color: [65535, 65535, 65535, 65535],
		lock_alpha: false,
		source: None,
	};
	let start = Instant::now();
	let mut stroke = Stroke::begin(setup, store).unwrap();
	for chunk in samples.chunks(2) {
		stroke.add(chunk).unwrap();
	}
	stroke.finish().unwrap();
	start.elapsed()
}

fn bytes(image: &TiledImage, store: &TileStore, tx: u32, ty: u32) -> Vec<u8> {
	match image.slot(0, tx, ty) {
		TileSlot::Empty => vec![0; (TILE_SIZE * TILE_SIZE * 4) as usize],
		TileSlot::Solid(v) => v.0.iter().map(|c| (c >> 8) as u8).cycle().take((TILE_SIZE * TILE_SIZE * 4) as usize).collect(),
		TileSlot::Data(h) => store.get(h).unwrap().bytes().to_vec(),
	}
}

/// Largest per-channel difference (8-bit levels) and how many channels differ.
fn compare(a: &TiledImage, b: &TiledImage, store: &TileStore) -> (u8, usize) {
	let tiles = a.width().div_ceil(TILE_SIZE);
	let mut worst = 0u8;
	let mut differ = 0;
	for ty in 0..tiles {
		for tx in 0..tiles {
			for (x, y) in bytes(a, store, tx, ty).iter().zip(bytes(b, store, tx, ty)) {
				let d = x.abs_diff(y);
				worst = worst.max(d);
				differ += usize::from(d > 0);
			}
		}
	}
	(worst, differ)
}

fn plain_eraser() -> Option<StrokeTool> {
	let plugin = fx_plugin::load_file(&wasm("plain_eraser")?).unwrap();
	Some(StrokeTool::Plugin {
		id: plugin.key,
		params: [0.0; 16],
	})
}

#[test]
fn plain_eraser_plugin_paints_what_the_native_eraser_paints() {
	let Some(plugin) = plain_eraser() else { return };
	let store = store();
	let image = varied(1024, &store);
	let samples = samples(1024);
	let (native, _) = paint(&image, StrokeTool::Eraser, brush(120.0), &samples, &store);
	let (wasm, _) = paint(&image, plugin, brush(120.0), &samples, &store);
	let (worst, differ) = compare(&native, &wasm, &store);
	// f32 in the plugin vs f64 natively: a rounding step at most.
	assert!(worst <= 1, "native and plugin differ by {worst} levels");
	let (erased, _) = compare(&image, &native, &store);
	assert!(erased > 50, "the stroke must actually erase something");
	eprintln!("{differ} channels differ by one level");
	assert!(fx_plugin::take_errors().is_empty());
}

#[test]
fn blend_eraser_dissolves_shadows_first() {
	let Some(path) = wasm("blend_eraser") else { return };
	let plugin = fx_plugin::load_file(&path).unwrap();
	let store = store();
	// Left half dark, right half light, both opaque; one light dab over the
	// middle at a k that should reach the darks only.
	let mut image = TiledImage::new(256, 256, PixelFormat::Rgba8);
	let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
	for (i, px) in buffer.bytes_mut().chunks_exact_mut(4).enumerate() {
		let v = if i % 256 < 128 { 30 } else { 230 };
		px.copy_from_slice(&[v, v, v, 255]);
	}
	image.put_buffer(&store, 0, 0, buffer);
	let mut soft = brush(400.0);
	soft.opacity = 0.35;
	soft.flow = 1.0;
	soft.hardness = 1.0;
	let one = [StrokeSample {
		x: 128.0,
		y: 128.0,
		pressure: 1.0,
		tilt_x: 0.0,
		tilt_y: 0.0,
		time_us: 0,
	}];
	// Tone 0 = Shadows First, softness 20 %, no grain.
	let mut params = [0.0f32; 16];
	params[1] = 20.0;
	let (out, _) = paint(&image, StrokeTool::Plugin { id: plugin.key, params }, soft, &one, &store);
	let px = bytes(&out, &store, 0, 0);
	let alpha = |x: usize| px[(128 * 256 + x) * 4 + 3];
	assert!(alpha(40) < 10, "the dark side is gone: alpha {}", alpha(40));
	assert!(alpha(220) > 245, "the light side stays: alpha {}", alpha(220));
}

#[test]
#[ignore = "timing; run in release"]
fn boundary_cost() {
	let Some(plugin) = plain_eraser() else { return };
	let store = store();
	for (size, diameter) in [(2048, 60.0), (4096, 300.0), (4096, 1200.0)] {
		let image = varied(size, &store);
		let samples = samples(size);
		// Warm both (thread-local instances, tile caches).
		paint(&image, StrokeTool::Eraser, brush(diameter), &samples[..20], &store);
		paint(&image, plugin, brush(diameter), &samples[..20], &store);
		let best = |tool| (0..5).map(|_| paint(&image, tool, brush(diameter), &samples, &store).1).min().unwrap();
		let best_live = |tool| (0..5).map(|_| paint_live(&image, tool, brush(diameter), &samples, &store)).min().unwrap();
		let report = |what: &str, native: Duration, wasm: Duration| {
			eprintln!(
				"{size}² canvas, {diameter} px soft brush, 401 samples, {what}: native {:.1} ms, plugin {:.1} ms ({:+.0} %)",
				native.as_secs_f64() * 1e3,
				wasm.as_secs_f64() * 1e3,
				(wasm.as_secs_f64() / native.as_secs_f64() - 1.0) * 100.0
			)
		};
		report("replay", best(StrokeTool::Eraser), best(plugin));
		report("live  ", best_live(StrokeTool::Eraser), best_live(plugin));
	}
}
