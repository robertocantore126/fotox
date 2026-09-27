//! Stress tests for the code review of 2026-09-27: the fixes that depend on
//! timing (eviction racing a reader, overlapping exports, mixed documents
//! through repeated geometry) run many times under pressure. CPU only.
//!
//! The large-canvas one is `#[ignore]`d (seconds, not milliseconds):
//! `cargo test -p fx-engine --test stress -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Instant;

use fx_core::{BitDepth, ColorProfile, Command, CommandContext, Document, DocumentColor, Layer, LayerId, LayerKind};
use fx_io::export::{ExportFormat, ExportOptions, JpegChroma, export_image};
use fx_render::adjust::LutCache;
use fx_tiles::{PixelFormat, TileBuffer, TileClass, TileSlot, TileStore, TileStoreConfig, TiledImage};

fn dir(name: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("fx-engine-stress-{name}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

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

/// R01 under a live trim thread: a 2048 × 2048 layer whose tiles do not fit
/// the hot budget (8 tiles), read at every level again and again while the
/// background trim drops mips and demotes level 0. Every read completes and
/// no tile comes back transparent (a hole is a dropped tile read as nothing).
#[test]
fn readers_survive_a_trim_thread_dropping_their_inputs() {
	let mut config = TileStoreConfig::for_tests(dir("evict"));
	config.background_trim = true;
	let store = TileStore::new(config).unwrap();
	let mut d = doc(2048, 2048);
	let mut image = TiledImage::new(2048, 2048, PixelFormat::Rgba8);
	for ty in 0..8 {
		for tx in 0..8 {
			let mut tile = TileBuffer::zeroed(PixelFormat::Rgba8);
			for (i, px) in tile.bytes_mut().chunks_exact_mut(4).enumerate() {
				// Opaque, and different per pixel so no tile collapses to Solid.
				px.copy_from_slice(&[(tx * 30) as u8, (ty * 30) as u8, (i % 251) as u8, 255]);
			}
			image.set_slot(tx, ty, TileSlot::Data(store.insert(tile, TileClass::Authoritative)));
		}
	}
	d.layers
		.push(Arc::new(Layer::new(LayerId(1), "noise", LayerKind::Pixel { image, offset: (0, 0) })));
	let levels = fx_tiles::level_count_for(2048, 2048);
	let mut luts = LutCache::default();
	let started = Instant::now();
	let mut reads = 0;
	// `evicted_tiles` is a gauge (dropped tiles whose handles still live):
	// its peak shows the trim really dropped what the readers were using.
	let mut peak_evicted = 0;
	for round in 0..12 {
		// A fresh copy each round: the mips computed in the last one are in
		// the store, where the trim is free to drop them.
		let mut reader = d.clone();
		for level in (0..levels).rev() {
			let side = 8u32.div_ceil(1 << level).max(1);
			let tiles: Vec<(u32, u32)> = (0..side).flat_map(|ty| (0..side).map(move |tx| (tx, ty))).collect();
			let rendered =
				fx_engine::derived::render_tiles(&mut reader, &store, level, &tiles, &mut luts).unwrap_or_else(|e| panic!("round {round} level {level}: {e}"));
			for tile in &rendered {
				let pixels = tile
					.pixels
					.as_ref()
					.unwrap_or_else(|| panic!("round {round} level {level} tile {:?} is empty", tile.tile));
				// Only the part of the tile inside this level's image.
				let size = (2048u32 >> level).clamp(1, 256) as usize;
				for y in 0..size {
					for x in 0..size {
						let a = pixels[y * 256 + x][3];
						assert!(
							(a - 1.0).abs() < 1e-6,
							"round {round} level {level} tile {:?} pixel ({x}, {y}): alpha {a}",
							tile.tile
						);
					}
				}
			}
			reads += tiles.len();
			peak_evicted = peak_evicted.max(store.stats().evicted_tiles);
		}
	}
	eprintln!("{reads} tile reads in {:?}; up to {peak_evicted} dropped tiles at once", started.elapsed());
	assert!(peak_evicted > 0, "the trim never dropped anything: the test did not test eviction");
}

/// R04 under contention: eight exports of eight colours to one path, all at
/// once. Every one succeeds, the file left is one whole image (a single
/// colour everywhere), and no temporary file is left behind.
#[test]
fn eight_simultaneous_exports_to_one_path_leave_one_whole_image() {
	let folder = dir("exports");
	let path = folder.join("same.png");
	let colours: Vec<[u16; 4]> = (0..8u16).map(|i| [i * 8000, 65535 - i * 8000, (i * 3000) % 65535, 65535]).collect();
	let barrier = Arc::new(std::sync::Barrier::new(colours.len()));
	let threads: Vec<_> = colours
		.iter()
		.map(|&colour| {
			let (path, barrier) = (path.clone(), barrier.clone());
			std::thread::spawn(move || {
				let options = ExportOptions {
					format: ExportFormat::Png,
					bits: 16,
					alpha: true,
					ppi: 72.0,
					quality: 90,
					chroma: JpegChroma::Full,
					icc: None,
					cmyk: None,
				};
				barrier.wait();
				export_image(
					&path,
					300,
					600,
					options,
					&mut |_, _, out| {
						out.fill(colour);
						std::thread::yield_now();
						Ok(())
					},
					&mut |_| true,
				)
			})
		})
		.collect();
	for thread in threads {
		thread.join().unwrap().unwrap();
	}
	let store = TileStore::new(TileStoreConfig::for_tests(folder.join("scratch"))).unwrap();
	let imported = fx_io::import_file(&path, &store, &mut |_| true).unwrap();
	let mut seen = std::collections::HashSet::new();
	for ty in 0..3 {
		for tx in 0..2 {
			match imported.image.slot(0, tx, ty) {
				TileSlot::Solid(v) => {
					seen.insert(v.0);
				}
				TileSlot::Data(h) => {
					let tile = store.get(h).unwrap();
					for px in tile.as_u16().chunks_exact(4).take(10) {
						seen.insert([px[0], px[1], px[2], px[3]]);
					}
				}
				TileSlot::Empty => panic!("a transparent tile in an opaque export"),
			}
		}
	}
	assert_eq!(seen.len(), 1, "one export's colour everywhere: {seen:?}");
	assert!(colours.contains(seen.iter().next().unwrap()));
	let leftovers: Vec<_> = std::fs::read_dir(&folder)
		.unwrap()
		.filter_map(Result::ok)
		.filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
		.collect();
	assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
}

/// R03: four quarter turns and two flips of each kind bring a mixed document
/// back exactly (paths, guides, samplers, slices, Smart Object transform,
/// vector-mask path, channel size), not just its pixel layers.
#[test]
fn full_turns_and_double_flips_are_the_identity_for_every_part() {
	use fx_core::path::{Anchor, Path, Subpath};
	let dir = dir("turns");
	let store = TileStore::new(TileStoreConfig::for_tests(dir)).unwrap();
	let mut d = doc(300, 200);
	let path = Path {
		subpaths: vec![Subpath {
			anchors: vec![Anchor::corner((10.0, 20.0)), Anchor::corner((250.0, 30.0)), Anchor::corner((120.0, 180.0))],
			closed: true,
			op: Default::default(),
		}],
	};
	let mut fill = Layer::new(LayerId(1), "masked", LayerKind::SolidFill { rgba: [65535; 4] });
	fill.vector_mask = Some(fx_core::layer::VectorMask {
		path: path.clone(),
		enabled: true,
		feather: 2.0,
		density: 1.0,
		cache: TiledImage::derived(300, 200, PixelFormat::Gray8),
	});
	d.layers.push(Arc::new(fill));
	let smart = fx_core::smart::SmartObject {
		source: fx_core::smart::SmartSource {
			doc: Arc::new(doc(50, 40)),
			composite: TiledImage::new(50, 40, PixelFormat::Rgba8),
			linked: None,
			linked_mtime: None,
			uid: 7,
		},
		transform: fx_core::Mapping::affine(1.5, 0.0, 0.0, 1.5, 40.0, 30.0),
		filters: vec![],
		filters_enabled: true,
	};
	d.layers.push(Arc::new(Layer::new(
		LayerId(2),
		"smart",
		LayerKind::Smart {
			smart,
			cache: TiledImage::derived(300, 200, PixelFormat::Rgba8),
		},
	)));
	d.channels
		.push(fx_core::channel::Channel::new("alpha", TiledImage::new(300, 200, PixelFormat::Gray8)));
	d.work_path = Some(path);
	d.guides.push(fx_core::document::Guide {
		vertical: true,
		position: 64.0,
	});
	d.guides.push(fx_core::document::Guide {
		vertical: false,
		position: 16.0,
	});
	d.annotations.samplers.push(fx_core::annotations::Sampler { x: 12.0, y: 34.0 });
	d.slices.push(fx_core::comps::Slice {
		name: "s".into(),
		rect: (10, 20, 30, 40),
	});
	let before = d.clone();
	let ops = fx_engine::ops::EngineOps::default();
	let run = |d: &mut Document, command: Command| {
		command
			.apply(
				d,
				&mut CommandContext {
					tiles: &store,
					ops: Some(&ops),
				},
			)
			.unwrap();
	};
	for _ in 0..4 {
		run(&mut d, Command::RotateCanvas { quarter_turns: 1 });
	}
	for horizontal in [true, true, false, false] {
		run(&mut d, Command::FlipCanvas { horizontal });
	}
	assert_eq!((d.width, d.height), (before.width, before.height));
	assert_eq!(d.work_path, before.work_path);
	assert_eq!(d.guides, before.guides);
	assert_eq!(d.annotations, before.annotations);
	assert_eq!(d.slices, before.slices);
	assert_eq!(
		d.layers[0].vector_mask.as_ref().unwrap().path,
		before.layers[0].vector_mask.as_ref().unwrap().path
	);
	let transform = |d: &Document| match &d.layers[1].kind {
		LayerKind::Smart { smart, .. } => smart.transform.forward_point(3.0, 4.0).unwrap(),
		_ => unreachable!(),
	};
	let (a, b) = (transform(&d), transform(&before));
	assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9, "{a:?} vs {b:?}");
	assert_eq!((d.channels[0].image.width(), d.channels[0].image.height()), (300, 200));
}

/// R06 on a large canvas: a small shape on a 20 000 × 20 000 document. The
/// whole-document preparation draws every level-0 tile of the shape's cache
/// (6 241 of them) in bounded batches; the tiles away from the shape end up
/// Empty and cost no memory. Prints the time.
#[test]
#[ignore = "large canvas: seconds; run with --ignored"]
fn preparing_a_small_shape_on_a_huge_canvas_stays_bounded() {
	let store = TileStore::new(TileStoreConfig::reference_machine(dir("huge"))).unwrap();
	let mut d = doc(20_000, 20_000);
	let shape = fx_core::vector::VectorShape::Rect {
		w: 300.0,
		h: 200.0,
		radii: [0.0; 4],
	};
	let layer = Layer::new(
		LayerId(1),
		"rect",
		LayerKind::Shape {
			shape,
			fill: Some(fx_core::vector::Paint::Solid { rgba: [65535, 0, 0, 65535] }),
			stroke: None,
			transform: [1.0, 0.0, 0.0, 1.0, 100.0, 100.0],
			cache: TiledImage::derived(20_000, 20_000, PixelFormat::Rgba8),
		},
	);
	d.layers.push(Arc::new(layer));
	let started = Instant::now();
	let drawn = fx_engine::vector::prepare_level0(&mut d, &store, None);
	let elapsed = started.elapsed();
	let LayerKind::Shape { cache, .. } = &d.layers[0].kind else { unreachable!() };
	let stored = cache.grid(0).non_empty().count();
	let hot = store.stats().hot_bytes;
	eprintln!("drew {drawn} tiles in {elapsed:?}; {stored} are stored; {hot} hot bytes");
	assert_eq!(drawn, 79 * 79);
	assert!(stored <= 4, "only the tiles the rectangle touches hold pixels, got {stored}");
}
