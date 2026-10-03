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
	// VERIFY-FIX(P1): mips are now compressed before they are dropped; a small
	// warm tier keeps this test dropping them. PERF(mips): they also go to
	// scratch when there is room; none here.
	config.warm_budget = 64 * 1024;
	config.scratch_limit = 0;
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

/// Code review 2026-09-27 R06 follow-up: a Smart Object computes the source
/// mips its tiles sample, not the source's whole pyramid. A 4096 × 4096
/// source at 25 % on a 512 × 512 canvas shows its top-left quarter: drawing
/// the canvas reads level 2 of that quarter only (and level 1 below it).
/// Drawn again and again with a trim thread and a small budget, every draw
/// completes.
#[test]
fn a_smart_object_computes_only_the_source_mips_it_samples() {
	let mut config = TileStoreConfig::for_tests(dir("smart"));
	config.background_trim = true;
	let store = TileStore::new(config).unwrap();
	let mut composite = TiledImage::new(4096, 4096, PixelFormat::Rgba8);
	for ty in 0..16 {
		for tx in 0..16 {
			// Pixels where the canvas shows the source, one colour elsewhere.
			let slot = if tx < 8 && ty < 8 {
				let mut tile = TileBuffer::zeroed(PixelFormat::Rgba8);
				for (i, px) in tile.bytes_mut().chunks_exact_mut(4).enumerate() {
					px.copy_from_slice(&[(tx * 30) as u8, (ty * 30) as u8, (i % 251) as u8, 255]);
				}
				TileSlot::Data(store.insert(tile, TileClass::Authoritative))
			} else {
				TileSlot::Solid(fx_tiles::PixelValue::rgba16(0, 0, 65535, 65535))
			};
			composite.set_slot(tx, ty, slot);
		}
	}
	let smart = fx_core::smart::SmartObject {
		source: fx_core::smart::SmartSource {
			doc: Arc::new(doc(4096, 4096)),
			composite,
			linked: None,
			linked_mtime: None,
			uid: 9,
		},
		transform: fx_core::Mapping::scale(0.25, 0.25),
		filters: vec![],
		filters_enabled: true,
	};
	let mut d = doc(512, 512);
	d.layers.push(Arc::new(Layer::new(
		LayerId(1),
		"smart",
		LayerKind::Smart {
			smart,
			cache: TiledImage::derived(512, 512, PixelFormat::Rgba8),
		},
	)));
	let tiles = [(0, 0), (1, 0), (0, 1), (1, 1)];
	let mut luts = LutCache::default();
	for round in 0..8 {
		if let LayerKind::Smart { cache, .. } = &mut Arc::make_mut(&mut d.layers[0]).kind {
			*cache = TiledImage::derived(512, 512, PixelFormat::Rgba8);
		}
		let rendered = fx_engine::derived::render_tiles(&mut d, &store, 0, &tiles, &mut luts).unwrap();
		for tile in &rendered {
			let pixels = tile.pixels.as_ref().unwrap_or_else(|| panic!("round {round}: tile {:?} is empty", tile.tile));
			assert!(pixels.iter().all(|p| p[3] > 0.99), "round {round}: a hole in tile {:?}", tile.tile);
		}
	}
	let LayerKind::Smart { smart, .. } = &d.layers[0].kind else { unreachable!() };
	let source = &smart.source.composite;
	let clean = |level: usize| {
		let grid = source.grid(level);
		(0..grid.rows())
			.flat_map(|ty| (0..grid.cols()).map(move |tx| (tx, ty)))
			.filter(|&(tx, ty)| !source.is_dirty(level, tx, ty))
			.count()
	};
	// Level 2 is 4 × 4 tiles: the visible quarter is 2 × 2 of them, 3 × 3
	// with the filter's apron; level 1 holds their children.
	assert!((4..=9).contains(&clean(2)), "level 2: {} of 16 computed", clean(2));
	assert!((16..=36).contains(&clean(1)), "level 1: {} of 64 computed", clean(1));
	for level in 3..source.level_count() {
		assert_eq!(clean(level), 0, "level {level} is never sampled");
	}
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

/// A red `w × h` rectangle shape layer at `(x, y)` on a `size` canvas, never
/// drawn: its cache is dirty everywhere.
fn rect_layer(id: u64, size: (u32, u32), (x, y, w, h): (f64, f64, f64, f64)) -> Layer {
	Layer::new(
		LayerId(id),
		"rect",
		LayerKind::Shape {
			shape: fx_core::vector::VectorShape::Rect { w, h, radii: [0.0; 4] },
			fill: Some(fx_core::vector::Paint::Solid { rgba: [65535, 0, 0, 65535] }),
			stroke: None,
			transform: [1.0, 0.0, 0.0, 1.0, x, y],
			cache: TiledImage::derived(size.0, size.1, PixelFormat::Rgba8),
		},
	)
}

/// The alpha of an RGBA8 image's pixel `(x, y)`.
fn alpha(store: &TileStore, image: &TiledImage, (x, y): (u32, u32)) -> u8 {
	let t = fx_tiles::TILE_SIZE;
	match image.slot(0, x / t, y / t) {
		TileSlot::Empty => 0,
		TileSlot::Solid(v) => (v.0[3] >> 8) as u8,
		TileSlot::Data(h) => store.get(h).unwrap().bytes()[(((y % t) * t + x % t) * 4 + 3) as usize],
	}
}

/// Code review 2026-09-27 R06 follow-up: whole-document readers compute the
/// shape tiles they read as they go. With a trim thread and a small budget, a
/// copy and a merge of a shape layer the view never drew give the shape's
/// pixels, and the document was not prepared first (its cache is still dirty
/// everywhere).
#[test]
fn a_never_drawn_shape_is_copied_and_merged_from_its_geometry() {
	let mut config = TileStoreConfig::for_tests(dir("lazy"));
	config.background_trim = true;
	let store = TileStore::new(config).unwrap();
	let size = (1024, 768);
	let mut d = doc(size.0, size.1);
	d.layers.push(Arc::new(Layer::new(
		LayerId(1),
		"under",
		LayerKind::Pixel {
			image: TiledImage::new(size.0, size.1, PixelFormat::Rgba8),
			offset: (0, 0),
		},
	)));
	d.layers.push(Arc::new(rect_layer(2, size, (266.0, 266.0, 300.0, 200.0))));
	let (inside, outside) = ((300, 300), (50, 50));
	for round in 0..8 {
		let content = fx_engine::derived::layer_content(&d, &store, LayerId(2)).unwrap();
		assert_eq!(alpha(&store, &content, inside), 255, "round {round}: the copy holds the shape");
		assert_eq!(alpha(&store, &content, outside), 0, "round {round}: and nothing else");
	}
	let LayerKind::Shape { cache, .. } = &d.layer(LayerId(2)).unwrap().kind else {
		unreachable!()
	};
	let tiles = cache.grid(0).cols() * cache.grid(0).rows();
	assert_eq!(cache.dirty_tiles(0).count() as u32, tiles, "nothing was prepared on the document");

	let ops = fx_engine::ops::EngineOps::default();
	Command::MergeLayers {
		layers: vec![fx_core::LayerRef::Id(LayerId(1)), fx_core::LayerRef::Id(LayerId(2))],
	}
	.apply(
		&mut d,
		&mut CommandContext {
			tiles: &store,
			ops: Some(&ops),
		},
	)
	.unwrap();
	assert_eq!(d.layers.len(), 1);
	let LayerKind::Pixel { image, offset } = &d.layers[0].kind else {
		panic!("merged into pixels")
	};
	let at = |(x, y): (u32, u32)| ((x as i32 - offset.0) as u32, (y as i32 - offset.1) as u32);
	assert_eq!(alpha(&store, image, at(inside)), 255, "the merge holds the shape");
	assert_eq!(alpha(&store, image, at(outside)), 0);
}

/// A 300 × 200 shape on a 20 000 × 20 000 canvas, never drawn, copied: the
/// copy walks the 6 241 tiles of the canvas, drawing each shape tile as it
/// reads it; the tiles away from the shape come out empty and cost no
/// memory. Prints the time.
#[test]
#[ignore = "large canvas: seconds; run with --ignored"]
fn copying_a_small_shape_on_a_huge_canvas_stays_bounded() {
	let store = TileStore::new(TileStoreConfig::reference_machine(dir("huge"))).unwrap();
	let size = (20_000, 20_000);
	let mut d = doc(size.0, size.1);
	d.layers.push(Arc::new(rect_layer(1, size, (100.0, 100.0, 300.0, 200.0))));
	let started = Instant::now();
	let content = fx_engine::derived::layer_content(&d, &store, LayerId(1)).unwrap();
	let elapsed = started.elapsed();
	let stored = content.grid(0).non_empty().count();
	let hot = store.stats().hot_bytes;
	eprintln!("copied in {elapsed:?}; {stored} tiles hold pixels; {hot} hot bytes");
	assert!(stored <= 4, "only the tiles the rectangle touches hold pixels, got {stored}");
	assert_eq!(alpha(&store, &content, (200, 200)), 255);
}

/// Thousands of thumbnail jobs queued at once (Undo with every layer's
/// thumbnail wanted refreshes each). A job whose mips are dirty computes
/// them with `par_iter`; queued with `rayon::spawn`, a worker waiting on it
/// ran other queued jobs on the same stack, which waited in turn: the stack
/// grew with the queue until it overflowed (scale.rs `thousands_of_layers`,
/// 2000 layers, 2026-09-27: the process aborted). Through the engine's
/// `ThumbQueue` every one renders.
#[test]
fn thousands_of_queued_thumbnails_do_not_overflow_a_worker_stack() {
	use fx_engine::thumbs::{ThumbQueue, ThumbSource, render};
	let mut config = TileStoreConfig::for_tests(dir("thumb-queue"));
	config.hot_budget = 1 << 30;
	config.warm_budget = 1 << 28;
	let store = Arc::new(TileStore::new(config).unwrap());
	// Every tile holds pixels; each clone has its own dirty mips.
	let (w, h) = (4000, 3000);
	let mut base = TiledImage::new(w, h, PixelFormat::Rgba8);
	let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
	for (i, b) in buffer.bytes_mut().iter_mut().enumerate() {
		*b = (i * 7 % 251) as u8;
	}
	for ty in 0..h.div_ceil(256) {
		for tx in 0..w.div_ceil(256) {
			base.put_buffer(&store, tx, ty, buffer.clone());
		}
	}
	let jobs = 3000;
	let queue = ThumbQueue::new(2);
	let (done, results) = std::sync::mpsc::channel();
	for key in 0..jobs {
		let (store, image, done) = (store.clone(), base.clone(), done.clone());
		queue.push(key, move || {
			let thumb = render(ThumbSource::Pixels { image, offset: (0, 0) }, w, h, 64, &store);
			let _ = done.send(thumb.is_ok());
		});
	}
	drop(done);
	let ok = results.iter().take(jobs).filter(|ok| *ok).count();
	assert_eq!(ok, jobs, "every thumbnail rendered");
}

/// A newer job for a layer whose thumbnail has not started replaces it: 20
/// undos in a row render each thumbnail about once, not 20 times.
#[test]
fn a_waiting_thumbnail_job_is_replaced_not_repeated() {
	let queue = fx_engine::thumbs::ThumbQueue::new(1);
	let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
	let (done, results) = std::sync::mpsc::channel();
	// Hold the one thread so the rest wait.
	queue.push(u32::MAX, move || {
		let _ = gate_rx.recv();
	});
	std::thread::sleep(std::time::Duration::from_millis(50));
	for round in 0..20 {
		for key in 0..5u32 {
			let done = done.clone();
			queue.push(key, move || {
				let _ = done.send((key, round));
			});
		}
	}
	assert_eq!(queue.len(), 5, "one waiting job per key");
	drop(done);
	gate_tx.send(()).unwrap();
	let ran: Vec<(u32, i32)> = results.iter().collect();
	assert_eq!(
		ran,
		(0..5).map(|k| (k, 19)).collect::<Vec<_>>(),
		"each key once, the newest job, in first-queued order"
	);
}
