//! Audit 2026-10-01 — native Smart Objects and engine-level save behaviour,
//! through a running engine (engine + render threads on the GPU, the same
//! messages the UI sends). Pixels are checked by exporting the flattened
//! document to a 16-bit TIFF and reading it back.
//!
//!   cargo test --release -p fx-engine --test audit_smart -- --ignored --nocapture --test-threads 1
//!
//! Set `APPDATA` to a scratch folder first: the engine writes the recent-
//! files list into `%APPDATA%\Fotox\preferences.json`.
//!
//! Each test prints `AUDIT` lines. Assertions mark the behaviour a user
//! relies on; a failing one is a finding.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Harness, Seen, gpu};
use fx_core::command::{LayerPropsPatch, NewLayer};
use fx_core::{Adjustment, Command, Filter, FilterParams, LayerRef, Mapping};
use fx_engine::{EngineInput, ExportChoice};
use fx_protocol::{CloseAnswer, DocId, EngineToUi, UiToEngine};
use fx_tiles::{TileSlot, TileStore, TileStoreConfig};

// ------------------------------------------------------------------ helpers

fn dir(name: &str) -> PathBuf {
	let root = std::env::var_os("FOTOX_AUDIT_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
	let dir = root.join(format!("fx-audit-{name}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

fn hash(x: u32, y: u32, c: u32) -> u16 {
	let mut z = u64::from(x) << 40 ^ u64::from(y) << 16 ^ u64::from(c);
	z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
	z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
	(z ^ (z >> 31)) as u16
}

/// A 16-bit RGB TIFF of per-pixel noise: any resampling shows.
fn noise_tiff(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
	let path = dir.join(name);
	let rows = 64.min(h);
	let mut writer = fx_io::tiff_write::TiffWriter::create(&path, w, h, 16, rows).unwrap();
	let mut y0 = 0;
	while y0 < h {
		let n = rows.min(h - y0);
		let mut strip = Vec::with_capacity((w * n * 6) as usize);
		for y in y0..y0 + n {
			for x in 0..w {
				for c in 0..3 {
					strip.extend_from_slice(&hash(x, y, c).to_le_bytes());
				}
			}
		}
		writer.write_strip(&strip).unwrap();
		y0 += n;
	}
	writer.finish().unwrap();
	path
}

struct Audit {
	h: Harness,
	dir: PathBuf,
	store: TileStore,
	exports: usize,
}

impl Audit {
	fn start(name: &str) -> Option<Self> {
		let (device, queue) = gpu()?;
		let dir = dir(name);
		let mut config = TileStoreConfig::for_tests(dir.join("verify-scratch"));
		config.hot_budget = 2 << 30;
		let store = TileStore::new(config).unwrap();
		Some(Self {
			h: Harness::start(device, queue, &dir),
			dir,
			store,
			exports: 0,
		})
	}

	fn open(&self, path: &Path) -> DocId {
		self.h.engine.send(EngineInput::Open(vec![path.to_path_buf()]));
		common::opened(&self.h)
	}

	fn action(&self, id: &str, args: serde_json::Value) {
		self.h.ui(UiToEngine::Action { id: id.into(), args });
	}

	/// The label of the next History message for `doc`, or the error/toast.
	fn next_step(&self, doc: DocId) -> String {
		self.h.wait("a history message", |s| match s {
			Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => {
				Some(labels.get(current.wrapping_sub(1)).cloned().unwrap_or_else(|| "(start)".into()))
			}
			Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
			Seen::Ui(EngineToUi::Toast { text }) if text != "Engine connected" => Some(format!("toast: {text}")),
			_ => None,
		})
	}

	fn command(&self, doc: DocId, command: Command) -> String {
		self.h.ui(UiToEngine::Command { doc, command });
		self.next_step(doc)
	}

	/// Export `doc` flattened to a 16-bit TIFF with alpha and read it back
	/// as straight RGBA16, row-major.
	fn export(&mut self, doc: DocId) -> Image {
		self.exports += 1;
		let path = self.dir.join(format!("export-{}.tif", self.exports));
		self.h.engine.send(EngineInput::Export {
			doc,
			path: path.clone(),
			choice: Some(ExportChoice {
				eight_bit: false,
				transparency: Some(true),
				quality: 100,
				chroma_half: false,
				cmyk: None,
			}),
		});
		let done = self.h.wait("the export", |s| match s {
			Seen::Ui(EngineToUi::Toast { text }) if text.starts_with("Exported") => Some(Ok(())),
			Seen::Ui(EngineToUi::Error { text }) => Some(Err(text.clone())),
			_ => None,
		});
		if let Err(text) = done {
			panic!("export failed: {text}");
		}
		let imported = fx_io::import_file(&path, &self.store, &mut |_| true).unwrap();
		let (w, h) = (imported.width, imported.height);
		let mut px = vec![0u16; (w * h * 4) as usize];
		let image = &imported.image;
		for ty in 0..h.div_ceil(256) {
			for tx in 0..w.div_ceil(256) {
				let tile: Option<Vec<u16>> = match image.slot(0, tx, ty) {
					TileSlot::Empty => None,
					TileSlot::Solid(v) => Some(v.0.repeat(256 * 256)),
					TileSlot::Data(handle) => Some(self.store.get(handle).unwrap().as_u16().to_vec()),
				};
				let Some(tile) = tile else { continue };
				for y in 0..256u32 {
					for x in 0..256u32 {
						let (gx, gy) = (tx * 256 + x, ty * 256 + y);
						if gx < w && gy < h {
							let s = ((y * 256 + x) * 4) as usize;
							let d = ((gy * w + gx) * 4) as usize;
							px[d..d + 4].copy_from_slice(&tile[s..s + 4]);
						}
					}
				}
			}
		}
		Image { w, h, px }
	}

	fn take_error(&self) -> Option<String> {
		let mut seen = self.h.seen.lock().unwrap();
		let i = seen.iter().position(|s| matches!(s, Seen::Ui(EngineToUi::Error { .. })))?;
		match seen.remove(i) {
			Seen::Ui(EngineToUi::Error { text }) => Some(text),
			_ => None,
		}
	}

	fn wait_toast(&self, what: &str) -> String {
		self.h.wait(what, |s| match s {
			Seen::Ui(EngineToUi::Toast { text }) if text != "Engine connected" => Some(text.clone()),
			Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
			_ => None,
		})
	}

	/// The dirty flag in the next DocumentChanged for `doc`.
	fn dirty(&self, doc: DocId) -> bool {
		self.h.wait("document changed", |s| match s {
			Seen::Ui(EngineToUi::DocumentChanged { info }) if info.doc == doc => Some(info.dirty),
			_ => None,
		})
	}

	/// Everything said so far, then forget it.
	fn drain(&self) -> Vec<String> {
		std::thread::sleep(Duration::from_millis(300));
		let seen = std::mem::take(&mut *self.h.seen.lock().unwrap());
		seen.iter()
			.filter_map(|s| match s {
				Seen::Ui(EngineToUi::Toast { text }) => Some(format!("toast: {text}")),
				Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
				Seen::Ui(EngineToUi::CloseDirtyDocument { doc, name }) => Some(format!("close-dirty prompt {doc:?} {name}")),
				Seen::Ui(EngineToUi::DocumentClosed { doc }) => Some(format!("closed {doc:?}")),
				Seen::Ui(EngineToUi::DocumentOpened { info }) => Some(format!("opened {:?} {}", info.doc, info.name)),
				Seen::Ui(EngineToUi::DocumentChanged { info }) => Some(format!("changed {:?} dirty={}", info.doc, info.dirty)),
				Seen::NeedSavePath(doc) => Some(format!("NEED SAVE PATH {doc:?}")),
				Seen::Ui(EngineToUi::History { doc, labels, current, .. }) => {
					Some(format!("history {doc:?} → {:?}", labels.get(current.wrapping_sub(1))))
				}
				_ => None,
			})
			.collect()
	}

	fn opened_doc(&self) -> DocId {
		common::opened(&self.h)
	}
}

#[derive(Clone)]
struct Image {
	w: u32,
	h: u32,
	px: Vec<u16>,
}

/// (max abs difference, mean abs difference, PSNR dB) over RGBA16.
fn compare(a: &Image, b: &Image) -> (u16, f64, f64) {
	assert_eq!((a.w, a.h), (b.w, b.h), "sizes differ");
	let mut max = 0u16;
	let mut sum = 0f64;
	let mut sq = 0f64;
	for (x, y) in a.px.iter().zip(&b.px) {
		let d = x.abs_diff(*y);
		max = max.max(d);
		sum += f64::from(d);
		sq += f64::from(d) * f64::from(d);
	}
	let n = a.px.len() as f64;
	let mse = sq / n;
	let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (65535.0f64 * 65535.0 / mse).log10() };
	(max, sum / n, psnr)
}

fn crop(image: &Image, x0: u32, y0: u32, w: u32, h: u32) -> Image {
	let mut px = Vec::with_capacity((w * h * 4) as usize);
	for y in y0..y0 + h {
		let s = ((y * image.w + x0) * 4) as usize;
		px.extend_from_slice(&image.px[s..s + (w * 4) as usize]);
	}
	Image { w, h, px }
}

/// The noise as an opaque RGBA16 image (what the TIFF holds).
fn noise_image(x0: u32, y0: u32, w: u32, h: u32) -> Image {
	let mut px = Vec::with_capacity((w * h * 4) as usize);
	for y in y0..y0 + h {
		for x in x0..x0 + w {
			px.extend_from_slice(&[hash(x, y, 0), hash(x, y, 1), hash(x, y, 2), 65535]);
		}
	}
	Image { w, h, px }
}

fn scale_about(s: f64, cx: f64, cy: f64) -> Mapping {
	Mapping::Affine([s, 0.0, 0.0, s, (1.0 - s) * cx, (1.0 - s) * cy])
}

fn rotate_about(deg: f64, cx: f64, cy: f64) -> Mapping {
	let (sin, cos) = deg.to_radians().sin_cos();
	// x' = cos x − sin y + e, y' = sin x + cos y + f, fixed point (cx, cy).
	Mapping::Affine([cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy])
}

fn transform(layer: LayerRef, mapping: Mapping) -> Command {
	Command::Transform {
		layer,
		mapping: Box::new(mapping),
		filter: Filter::Bicubic,
	}
}

fn print_cmp(what: &str, r: (u16, f64, f64)) {
	println!("AUDIT {what}: max |Δ| {} / 65535, mean |Δ| {:.2}, PSNR {:.1} dB", r.0, r.1, r.2);
}

// ---------------------------------------------- transforms: lossless or not

#[test]
#[ignore = "audit"]
fn smart_object_transforms_do_not_degrade() {
	let Some(mut a) = Audit::start("so-transforms") else { return };
	let tif = noise_tiff(&a.dir, "noise.tif", 512, 512);
	let (cx, cy) = (256.0, 256.0);

	for smart in [true, false] {
		let doc = a.open(&tif);
		if smart {
			println!("AUDIT convert → {}", a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] }));
		} else {
			// A plain pixel layer (the import's background may be locked).
			println!("AUDIT layer via copy → {}", a.command(doc, Command::DuplicateLayers { layers: vec![LayerRef::Active] }));
			println!(
				"AUDIT unlock → {}",
				a.command(
					doc,
					Command::SetLayerProps {
						layer: LayerRef::Active,
						props: LayerPropsPatch {
							locked_position: Some(false),
							locked_pixels: Some(false),
							..Default::default()
						},
					}
				)
			);
		}
		let base = a.export(doc);
		print_cmp(&format!("{} identity export vs source noise", if smart { "SO" } else { "pixel" }), compare(&base, &noise_image(0, 0, 512, 512)));
		for (label, there, back, cycles) in [
			("scale 10% → 1000%", scale_about(0.1, cx, cy), scale_about(10.0, cx, cy), 5),
			("rotate 7 × 51.43°", rotate_about(360.0 / 7.0, cx, cy), rotate_about(0.0, cx, cy), 1),
		] {
			let t = Instant::now();
			let mut cycles_done = 0;
			for _ in 0..cycles {
				if label.starts_with("rotate") {
					for _ in 0..7 {
						let s = a.command(doc, transform(LayerRef::Active, there));
						if !s.contains("Transform") {
							println!("AUDIT rotate step answered: {s}");
						}
					}
				} else {
					let s1 = a.command(doc, transform(LayerRef::Active, there));
					if cycles_done == 0 {
						let mid = a.export(doc);
						print_cmp(&format!("{} mid-way (scaled to 10 %) vs identity: must differ", if smart { "SO" } else { "pixel" }), compare(&base, &mid));
					}
					cycles_done += 1;
					let s2 = a.command(doc, transform(LayerRef::Active, back));
					if s1.starts_with("error") || s2.starts_with("error") || s1.starts_with("toast") || s2.starts_with("toast") {
						println!("AUDIT transform refused: {s1} / {s2}");
					}
				}
			}
			let after = a.export(doc);
			let r = compare(&base, &after);
			print_cmp(
				&format!("{} after {cycles} × {label} ({:.1} s)", if smart { "SO" } else { "pixel layer" }, t.elapsed().as_secs_f64()),
				r,
			);
			if smart {
				assert!(r.2 > 60.0, "a Smart Object lost quality through transforms that cancel out");
			}
		}
		a.h.ui(UiToEngine::CloseDocument { doc });
		a.h.ui(UiToEngine::CloseDocumentAnswer {
			doc,
			answer: CloseAnswer::DontSave,
		});
		let _ = a.drain();
	}
	a.h.engine.shutdown();
}

/// Place a picture bigger than the canvas: does the Smart Object keep the
/// picture's own pixels, or the canvas-sized crop?
#[test]
#[ignore = "audit"]
fn placed_picture_keeps_its_resolution() {
	let Some(mut a) = Audit::start("so-place") else { return };
	let canvas = noise_tiff(&a.dir, "canvas.tif", 1000, 800);
	let big = noise_tiff(&a.dir, "big.tif", 3000, 2400);
	let doc = a.open(&canvas);
	a.h.engine.send(EngineInput::Place(vec![big.clone()]));
	// Paste, name, convert, then the Free Transform box (fit to the canvas).
	for _ in 0..3 {
		println!("AUDIT place step → {}", a.next_step(doc));
	}
	std::thread::sleep(Duration::from_millis(500));
	a.h.ui(UiToEngine::Key { key: "Enter".into() });
	println!("AUDIT place commit → {}", a.next_step(doc));
	let fitted = a.export(doc);
	// What a correct place shows: the whole picture scaled by 1/3 into the
	// canvas. Compare its centre column of tiles loosely (a downscale) by
	// checking that the corners of the picture are inside the canvas: the
	// pixels right inside the fitted rectangle's left edge are not the
	// canvas noise.
	let canvas_noise = noise_image(0, 0, 1000, 800);
	let row = 400u32;
	let changed_cols = (0..1000u32)
		.filter(|&x| {
			let i = ((row * 1000 + x) * 4) as usize;
			fitted.px[i..i + 3] != canvas_noise.px[i..i + 3]
		})
		.count();
	println!("AUDIT placed SO covers {changed_cols} of 1000 columns on row {row} (a 3000 px picture fitted to 1000 px covers ~1000)");

	// Scale it back to 100 % about the canvas centre: the centre of the
	// picture must come back pixel for pixel if its source was kept.
	println!("AUDIT scale ×3 → {}", a.command(doc, transform(LayerRef::Active, scale_about(3.0, 500.0, 400.0))));
	let full = a.export(doc);
	let expected = noise_image(1000, 800, 1000, 800);
	let r = compare(&full, &expected);
	print_cmp("placed 3000×2400 picture, fitted then scaled back to 100 %, vs the picture's centre crop", r);
	a.h.engine.shutdown();
	assert!(r.2 > 40.0, "the placed Smart Object did not keep the picture's resolution");
}

// --------------------------------------------- instances and Edit Contents

/// Two instances (Duplicate Layer shares the source); edit the contents of
/// one; do both update?
#[test]
#[ignore = "audit"]
fn editing_contents_updates_every_instance() {
	let Some(mut a) = Audit::start("so-instances") else { return };
	let tif = noise_tiff(&a.dir, "noise.tif", 512, 512);
	let doc = a.open(&tif);
	println!("AUDIT convert → {}", a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] }));
	println!("AUDIT duplicate → {}", a.command(doc, Command::DuplicateLayers { layers: vec![LayerRef::Active] }));
	a.h.ui(UiToEngine::RequestLayers { doc });
	let layers = a.h.wait("layers", |s| match s {
		Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == doc && layers.len() >= 2 => Some(layers.clone()),
		_ => None,
	});
	let (top, bottom) = (layers[0].id, layers[1].id);
	let before = a.export(doc);

	// Edit Contents of the top instance: invert inside, save back.
	a.action("smart:edit", serde_json::Value::Null);
	let child = a.opened_doc();
	println!("AUDIT child invert → {}", a.command(child, Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None }));
	a.action("doc:save", serde_json::Value::Null);
	println!("AUDIT save contents → {}", a.next_step(doc));

	let hide = |layer, visible| Command::SetLayerProps {
		layer: LayerRef::Id(layer),
		props: LayerPropsPatch {
			visible: Some(visible),
			..Default::default()
		},
	};
	let top_only = {
		a.command(doc, hide(bottom, false));
		a.export(doc)
	};
	let bottom_only = {
		a.command(doc, hide(bottom, true));
		a.command(doc, hide(top, false));
		a.export(doc)
	};
	let inverted = Image {
		w: before.w,
		h: before.h,
		px: before.px.chunks(4).flat_map(|p| [65535 - p[0], 65535 - p[1], 65535 - p[2], p[3]]).collect(),
	};
	let rt = compare(&top_only, &inverted);
	let rb = compare(&bottom_only, &inverted);
	print_cmp("edited instance vs inverted original", rt);
	print_cmp("other instance (same source) vs inverted original", rb);
	print_cmp("other instance vs the unedited original", compare(&bottom_only, &before));
	a.h.engine.shutdown();
	assert!(rb.2 > 60.0, "the second instance of the Smart Object was not updated");
}

/// Edit Contents tab whose parent is closed, or whose layer is deleted,
/// before the contents are saved back.
#[test]
#[ignore = "audit"]
fn edit_contents_when_the_parent_goes_away() {
	let Some(a) = Audit::start("so-orphan") else { return };
	let tif = noise_tiff(&a.dir, "noise.tif", 256, 256);

	// Case 1: the parent document is closed.
	let doc = a.open(&tif);
	a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
	a.action("smart:edit", serde_json::Value::Null);
	let child = a.opened_doc();
	a.command(child, Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None });
	let _ = a.drain();
	a.h.ui(UiToEngine::CloseDocument { doc });
	a.h.ui(UiToEngine::CloseDocumentAnswer {
		doc,
		answer: CloseAnswer::DontSave,
	});
	println!("AUDIT orphan/closed: closing the parent → {:?}", a.drain());
	a.h.ui(UiToEngine::ActivateDocument { doc: child });
	a.action("doc:save", serde_json::Value::Null);
	println!("AUDIT orphan/closed: saving the contents → {:?}", a.drain());
	a.h.ui(UiToEngine::CloseDocument { doc: child });
	let closing = a.drain();
	println!("AUDIT orphan/closed: closing the edited contents tab → {closing:?}");
	let prompt1 = closing.iter().any(|s| s.starts_with("close-dirty"));

	// Case 2: the Smart Object layer is deleted in the parent.
	let doc = a.open(&tif);
	a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
	a.action("smart:edit", serde_json::Value::Null);
	let child = a.opened_doc();
	a.command(child, Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None });
	a.h.ui(UiToEngine::ActivateDocument { doc });
	let _ = a.drain();
	println!("AUDIT orphan/deleted: delete the SO → {}", a.command(doc, Command::DeleteLayers { layers: vec![LayerRef::Active] }));
	a.h.ui(UiToEngine::ActivateDocument { doc: child });
	a.action("doc:save", serde_json::Value::Null);
	println!("AUDIT orphan/deleted: saving the contents → {:?}", a.drain());
	a.h.ui(UiToEngine::CloseDocument { doc: child });
	let closing = a.drain();
	println!("AUDIT orphan/deleted: closing the edited contents tab → {closing:?}");
	let prompt2 = closing.iter().any(|s| s.starts_with("close-dirty"));

	// Case 3: the contents tab is closed with "Save" in the prompt.
	let doc = a.open(&tif);
	a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
	a.action("smart:edit", serde_json::Value::Null);
	let child = a.opened_doc();
	a.command(child, Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None });
	let _ = a.drain();
	a.h.ui(UiToEngine::CloseDocument { doc: child });
	println!("AUDIT contents-close: close → {:?}", a.drain());
	a.h.ui(UiToEngine::CloseDocumentAnswer {
		doc: child,
		answer: CloseAnswer::Save,
	});
	let answer = a.drain();
	println!("AUDIT contents-close: answer Save → {answer:?}");
	let parent_updated = answer.iter().any(|s| s.contains("Edit Contents"));
	a.h.engine.shutdown();
	assert!(prompt1 && prompt2, "an Edit Contents tab whose edits went nowhere closed without asking");
	assert!(parent_updated, "\"Save\" in the close prompt of an Edit Contents tab did not update the Smart Object");
}

/// Smart Object in a Smart Object: an inner edit reaches the top document
/// once each level is saved back.
#[test]
#[ignore = "audit"]
fn nested_smart_objects_propagate() {
	let Some(mut a) = Audit::start("so-nested") else { return };
	let tif = noise_tiff(&a.dir, "noise.tif", 256, 256);
	let doc = a.open(&tif);
	a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
	a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
	let before = a.export(doc);
	a.action("smart:edit", serde_json::Value::Null);
	let outer = a.opened_doc();
	a.action("smart:edit", serde_json::Value::Null);
	let inner = a.opened_doc();
	println!("AUDIT nested: inner invert → {}", a.command(inner, Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None }));
	a.action("doc:save", serde_json::Value::Null);
	println!("AUDIT nested: inner saved → {}", a.next_step(outer));
	a.h.ui(UiToEngine::ActivateDocument { doc: outer });
	std::thread::sleep(Duration::from_millis(200));
	a.action("doc:save", serde_json::Value::Null);
	println!("AUDIT nested: outer saved → {}", a.next_step(doc));
	let after = a.export(doc);
	let inverted = Image {
		w: before.w,
		h: before.h,
		px: before.px.chunks(4).flat_map(|p| [65535 - p[0], 65535 - p[1], 65535 - p[2], p[3]]).collect(),
	};
	let r = compare(&after, &inverted);
	print_cmp("nested SO after inner edit vs inverted", r);
	// Undo in the top document restores the old contents.
	a.h.ui(UiToEngine::Undo { doc });
	println!("AUDIT nested: undo → {}", a.next_step(doc));
	let undone = a.export(doc);
	print_cmp("nested SO after Undo vs before", compare(&undone, &before));
	a.h.engine.shutdown();
	assert!(r.2 > 60.0);
}

// ------------------------------------------------------------ smart filters

#[test]
#[ignore = "audit"]
fn smart_filters_are_reeditable() {
	let Some(mut a) = Audit::start("so-filters") else { return };
	let tif = noise_tiff(&a.dir, "noise.tif", 512, 384);
	let doc = a.open(&tif);
	a.command(doc, Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
	let plain = a.export(doc);
	let apply = |f: FilterParams| Command::ApplyFilter { layer: LayerRef::Active, filter: f };
	println!("AUDIT filter 1 → {}", a.command(doc, apply(FilterParams::Sharpen { edges: false })));
	println!("AUDIT filter 2 → {}", a.command(doc, apply(FilterParams::Despeckle)));
	let both = a.export(doc);
	print_cmp("two smart filters vs none (must differ)", compare(&both, &plain));

	let set = |a: &Audit, enabled: bool, rows: serde_json::Value| {
		a.action("smart:filters-set", serde_json::json!({ "enabled": enabled, "filters": rows }));
		a.next_step(doc)
	};
	println!("AUDIT stack eye off → {}", set(&a, false, serde_json::json!([{}, {}])));
	let off = a.export(doc);
	let r_off = compare(&off, &plain);
	print_cmp("filters off vs no filters", r_off);
	println!("AUDIT stack eye on → {}", set(&a, true, serde_json::json!([{}, {}])));
	let on = a.export(doc);
	let r_on = compare(&on, &both);
	print_cmp("filters back on vs before", r_on);
	println!("AUDIT reorder → {}", set(&a, true, serde_json::json!([{ "order": 1 }, { "order": 0 }])));
	let reordered = a.export(doc);
	print_cmp("reordered vs original order (differs if order matters)", compare(&reordered, &both));
	println!("AUDIT opacity 50 % on the first → {}", set(&a, true, serde_json::json!([{ "opacity": 50 }, {}])));
	let half = a.export(doc);
	print_cmp("filter opacity 50 % vs 100 %", compare(&half, &reordered));
	a.h.ui(UiToEngine::Undo { doc });
	a.next_step(doc);
	a.h.ui(UiToEngine::Undo { doc });
	a.next_step(doc);
	let undone = a.export(doc);
	let r_undo = compare(&undone, &both);
	print_cmp("two undos vs the original order", r_undo);
	a.h.ui(UiToEngine::Redo { doc });
	a.next_step(doc);
	let redone = a.export(doc);
	print_cmp("redo vs reordered", compare(&redone, &reordered));

	// Save, reopen, compare.
	let path = a.dir.join("filters.fxd");
	a.h.engine.send(EngineInput::SaveAs { doc, path: path.clone() });
	a.dirty(doc);
	let reopened = a.open(&path);
	let back = a.export(reopened);
	let r_reopen = compare(&back, &redone);
	print_cmp("reopened .fxd vs before saving", r_reopen);
	a.h.ui(UiToEngine::RequestLayers { doc: reopened });
	let layers = a.h.wait("layers", |s| match s {
		Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == reopened => Some(layers.clone()),
		_ => None,
	});
	println!("AUDIT reopened smart info: {:?}", layers.iter().map(|l| l.smart.clone()).collect::<Vec<_>>());

	// Rasterize: the pixels stay what the Smart Object showed.
	println!("AUDIT rasterize → {}", a.command(reopened, Command::Rasterize { layers: vec![LayerRef::Active] }));
	let raster = a.export(reopened);
	let r_raster = compare(&raster, &redone);
	print_cmp("rasterized vs the Smart Object", r_raster);
	a.h.engine.shutdown();
	assert_eq!(r_off.0, 0, "turning the filter stack off must show the unfiltered object");
	assert_eq!(r_on.0, 0);
	assert_eq!(r_undo.0, 0);
	assert_eq!(r_reopen.0, 0);
	assert_eq!(r_raster.0, 0);
}

// ------------------------------------------------- saving while editing

/// Edits made while a save runs are not in the file and keep the document
/// dirty; a failed save says so and keeps it dirty.
#[test]
#[ignore = "audit"]
fn edits_during_a_save_and_failed_saves() {
	let Some(a) = Audit::start("save-during-edit") else { return };
	let side: u32 = std::env::var("FOTOX_AUDIT_SAVE_SIDE").ok().and_then(|v| v.parse().ok()).unwrap_or(6000);
	let tif = noise_tiff(&a.dir, "noise.tif", side, side);
	let doc = a.open(&tif);
	let path = a.dir.join("during.fxd");
	let t = Instant::now();
	a.h.engine.send(EngineInput::SaveAs { doc, path: path.clone() });
	// Edits while the worker writes.
	std::thread::sleep(Duration::from_millis(30));
	let s1 = a.command(
		doc,
		Command::SetLayerProps {
			layer: LayerRef::Active,
			props: LayerPropsPatch {
				opacity: Some(0.25),
				..Default::default()
			},
		},
	);
	let s2 = a.command(doc, Command::AddLayer { layer: NewLayer::Pixel, name: Some("during save".into()) });
	let edits_done = t.elapsed();
	let after_save = a.h.wait("the document after the save", |s| match s {
		Seen::Ui(EngineToUi::DocumentChanged { info }) if info.doc == doc && info.name.ends_with(".fxd") => Some(Ok(info.dirty)),
		Seen::Ui(EngineToUi::Error { text }) => Some(Err(text.clone())),
		_ => None,
	});
	let saved_at = t.elapsed();
	let toast = a.wait_toast("the Saved toast");
	println!(
		"AUDIT save-during-edit ({side}² 16-bit noise): edits answered after {:.0} ms ({s1}, {s2}), save finished after {:.1} s ({toast}); dirty right after the save: {after_save:?}",
		edits_done.as_secs_f64() * 1000.0,
		saved_at.as_secs_f64()
	);
	let dirty: Vec<String> = if after_save == Ok(true) { vec!["dirty=true".into()] } else { Vec::new() };
	assert!(edits_done < saved_at, "the edits waited for the save");
	assert!(dirty.iter().any(|s| s.contains("dirty=true")), "edits made during the save must keep the document dirty");
	let reopened = a.open(&path);
	let layers = a.h.wait("layers", |s| match s {
		Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == reopened => Some(layers.clone()),
		_ => None,
	});
	println!(
		"AUDIT the file holds the state at save start: {} layer(s), opacity {:?}",
		layers.len(),
		layers.iter().map(|l| l.opacity).collect::<Vec<_>>()
	);
	assert_eq!(layers.len(), 1);
	assert_eq!(layers[0].opacity, 1.0);

	// A save that cannot write: a folder that does not exist.
	let bad = a.dir.join("no-such-folder").join("x.fxd");
	a.h.engine.send(EngineInput::SaveAs { doc, path: bad });
	let failed = a.drain();
	println!("AUDIT save to a missing folder → {failed:?}");
	// A save whose target is held open without delete sharing.
	use std::os::windows::fs::OpenOptionsExt;
	let held = a.dir.join("held.fxd");
	std::fs::write(&held, b"someone else's file").unwrap();
	let guard = std::fs::OpenOptions::new().read(true).share_mode(0x1).open(&held).unwrap();
	a.h.engine.send(EngineInput::SaveAs { doc, path: held.clone() });
	std::thread::sleep(Duration::from_secs(8));
	let failed = a.drain();
	drop(guard);
	let parts: Vec<String> = std::fs::read_dir(&a.dir)
		.unwrap()
		.filter_map(Result::ok)
		.map(|e| e.file_name().to_string_lossy().into_owned())
		.filter(|n| n.ends_with(".part"))
		.collect();
	println!(
		"AUDIT save over a file held open elsewhere → {failed:?}; target now {:?}; .part left {parts:?}",
		std::fs::read(&held).map(|b| String::from_utf8_lossy(&b[..b.len().min(24)]).into_owned())
	);
	a.h.engine.shutdown();
}
