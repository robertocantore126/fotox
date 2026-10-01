//! Audit 2026-10-01 — save integrity of the native `.fxd`, driven through the
//! real `fx_io::fxd::{save, open}` paths (no engine, no GPU).
//!
//! Every test is `#[ignore]`d: they write hundreds of MB and spawn child
//! processes. Run one volume at a time:
//!
//!   FOTOX_AUDIT_DIR=E:\fotox-audit-run cargo test --release -p fx-io --test audit_save -- --ignored --nocapture --test-threads 1
//!
//! `FOTOX_AUDIT_DIR` picks the volume (NTFS vs exFAT behave differently on
//! replace); default is the temp folder. Each test prints one `AUDIT` line
//! per observation; the tests fail only where the observed behaviour loses
//! or corrupts data silently, so a red test is a finding, not a harness bug.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fx_core::{BitDepth, ColorProfile, Document, DocumentColor, Layer, LayerKind};
use fx_io::IoError;
use fx_io::fxd::{self, FxdFile, SaveRequest, SaveTarget};
use fx_tiles::{PixelFormat, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

// ------------------------------------------------------------------ fixtures

fn root() -> PathBuf {
	std::env::var_os("FOTOX_AUDIT_DIR")
		.map_or_else(std::env::temp_dir, PathBuf::from)
		.join("fx-audit-save")
}

/// A fresh directory for one test (removed first, kept afterwards for
/// inspection unless `FOTOX_AUDIT_KEEP` is unset — the runner script deletes
/// the whole root at the end).
fn fresh_dir(name: &str) -> PathBuf {
	let dir = root().join(format!("{name}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

fn make_store(dir: &Path) -> TileStore {
	let mut config = TileStoreConfig::for_tests(dir.join("scratch"));
	config.hot_budget = 2 << 30;
	config.warm_budget = 1 << 30;
	TileStore::new(config).unwrap()
}

struct Rng(u64);

impl Rng {
	fn next(&mut self) -> u64 {
		self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
		let mut z = self.0;
		z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
		z ^ (z >> 31)
	}
}

/// A tile of noise, fully determined by (layer, version, tx, ty). The first
/// 8 bytes are the version, so a mixed-version layer is detectable even
/// without regenerating the noise.
fn noise_tile(format: PixelFormat, layer: usize, version: u64, tx: u32, ty: u32) -> TileBuffer {
	let mut rng = Rng((layer as u64) << 48 ^ version << 20 ^ u64::from(tx) << 10 ^ u64::from(ty));
	let mut buffer = TileBuffer::zeroed(format);
	for chunk in buffer.bytes_mut().chunks_mut(8) {
		let v = rng.next().to_le_bytes();
		chunk.copy_from_slice(&v[..chunk.len()]);
	}
	buffer.bytes_mut()[..8].copy_from_slice(&version.to_le_bytes());
	buffer
}

/// Canvas side for the versioned documents (a multiple of 256: no edge tiles).
fn side() -> u32 {
	std::env::var("FOTOX_AUDIT_SIDE").ok().and_then(|v| v.parse().ok()).unwrap_or(2048)
}

const LAYERS: usize = 4;
const FORMAT: PixelFormat = PixelFormat::Rgba16;

fn pixel_layer(store: &TileStore, doc: &mut Document, index: usize, version: u64) -> Arc<Layer> {
	let side = doc.width;
	let mut image = TiledImage::new(side, side, FORMAT);
	for ty in 0..side / 256 {
		for tx in 0..side / 256 {
			image.put_buffer(store, tx, ty, noise_tile(FORMAT, index, version, tx, ty));
		}
	}
	let id = fx_core::LayerId(index as u64 + 1);
	Arc::new(Layer::new(id, format!("L{index} v{version}"), LayerKind::Pixel { image, offset: (0, 0) }))
}

/// `LAYERS` noise layers, layer i at `versions[i]`.
fn versioned_document(store: &TileStore, versions: &[u64]) -> Document {
	let mut doc = Document::new(
		side(),
		side(),
		DocumentColor {
			depth: BitDepth::U16,
			profile: ColorProfile::Srgb,
		},
		72.0,
	);
	for _ in 0..versions.len() {
		doc.allocate_layer_id();
	}
	doc.layers = versions.iter().enumerate().map(|(i, v)| pixel_layer(store, &mut doc.clone(), i, *v)).collect();
	doc
}

/// What a reopened versioned document holds.
#[derive(Debug, PartialEq, Eq, Clone)]
enum Verdict {
	/// Every tile regenerated and compared exactly: these versions.
	Exact(Vec<u64>),
	/// A tile could not be read (checksum, decode): an error, not garbage.
	ReadError(String),
	/// A tile read fine but holds the wrong pixels: silent corruption.
	Garbage(String),
}

fn verify(doc: &Document, store: &TileStore) -> Verdict {
	let mut versions = Vec::new();
	for (i, layer) in doc.layers.iter().enumerate() {
		let Some(v) = layer.name.rsplit_once(" v").and_then(|(_, v)| v.parse::<u64>().ok()) else {
			return Verdict::Garbage(format!("layer {i} has name {:?}", layer.name));
		};
		let LayerKind::Pixel { image, .. } = &layer.kind else {
			return Verdict::Garbage(format!("layer {i} is not a pixel layer"));
		};
		for ty in 0..doc.height / 256 {
			for tx in 0..doc.width / 256 {
				let TileSlot::Data(handle) = image.slot(0, tx, ty) else {
					return Verdict::Garbage(format!("layer {i} tile ({tx},{ty}) is not data"));
				};
				let pixels = match store.get(handle) {
					Ok(p) => p,
					Err(e) => return Verdict::ReadError(format!("layer {i} tile ({tx},{ty}): {e}")),
				};
				let expected = noise_tile(FORMAT, i, v, tx, ty);
				if pixels.bytes() != expected.bytes() {
					let stamp = u64::from_le_bytes(pixels.bytes()[..8].try_into().unwrap());
					return Verdict::Garbage(format!("layer {i} (v{v}) tile ({tx},{ty}) differs; its stamp says v{stamp}"));
				}
			}
		}
		versions.push(v);
	}
	Verdict::Exact(versions)
}

/// Open `path` with a fresh store and verify it.
fn reopen(path: &Path, scratch: &Path) -> Result<Verdict, IoError> {
	let store = make_store(scratch);
	let opened = fxd::open(path, &store)?;
	Ok(verify(&opened.document, &store))
}

fn save_doc(doc: &Document, store: &TileStore, target: SaveTarget) -> Result<fxd::SavedFxd, IoError> {
	fxd::save(
		SaveRequest {
			doc,
			store,
			preview: None,
		},
		target,
		&mut |_| true,
	)
}

fn part_files(dir: &Path) -> Vec<String> {
	std::fs::read_dir(dir)
		.map(|entries| {
			entries
				.filter_map(Result::ok)
				.map(|e| e.file_name().to_string_lossy().into_owned())
				.filter(|n| n.ends_with(".part"))
				.collect()
		})
		.unwrap_or_default()
}

fn copy_prefix(src: &Path, dst: &Path, len: u64) {
	std::fs::copy(src, dst).unwrap();
	OpenOptions::new().write(true).open(dst).unwrap().set_len(len).unwrap();
}

fn fs_name(dir: &Path) -> String {
	// Volume of the directory, for the log lines.
	dir.components().next().map_or_else(String::new, |c| c.as_os_str().to_string_lossy().into_owned())
}

// ----------------------------------------------- process-kill (child mode)

/// Child process body: keep saving the versioned document at
/// `FOTOX_AUDIT_CHILD` until killed. Logs `attempt v…` before and `ok v…`
/// after each save into `FOTOX_AUDIT_LOG`. Not a test on its own.
#[test]
#[ignore = "child process of kill_during_saves_*"]
fn child_saver() {
	let Some(path) = std::env::var_os("FOTOX_AUDIT_CHILD").map(PathBuf::from) else {
		return;
	};
	let fresh = std::env::var("FOTOX_AUDIT_MODE").as_deref() == Ok("fresh");
	let log_path = PathBuf::from(std::env::var_os("FOTOX_AUDIT_LOG").unwrap());
	let mut log = OpenOptions::new().create(true).append(true).open(&log_path).unwrap();
	let scratch = path.parent().unwrap().join(format!("child-scratch-{}", std::process::id()));
	let store = make_store(&scratch);

	let (mut doc, mut file, mut versions) = if path.exists() {
		let opened = fxd::open(&path, &store).expect("child: the file must open");
		let Verdict::Exact(versions) = verify(&opened.document, &store) else {
			panic!("child: the file did not verify")
		};
		(opened.document, Some(opened.file), versions)
	} else {
		let versions = vec![1; LAYERS];
		(versioned_document(&store, &versions), None, versions)
	};
	let mut k = versions.iter().sum::<u64>() as usize;
	loop {
		// The first save of a new file writes version 1 as is; every later
		// one repaints one layer at a new version first.
		if file.is_some() {
			let j = k % LAYERS;
			versions[j] += 1;
			let layer = pixel_layer(&store, &mut doc.clone(), j, versions[j]);
			doc.layers[j] = layer;
		}
		writeln!(log, "attempt {versions:?}").unwrap();
		log.flush().unwrap();
		let target = match (&file, fresh) {
			(Some(f), false) => SaveTarget::Incremental(f.clone()),
			_ => SaveTarget::Fresh(path.clone()),
		};
		let saved = save_doc(&doc, &store, target).expect("child: save failed");
		file = Some(saved.file);
		writeln!(log, "ok {versions:?}").unwrap();
		log.flush().unwrap();
		k += 1;
	}
}

fn parse_versions(line: &str) -> Vec<u64> {
	line.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '[')
		.trim_matches(|c| c == '[' || c == ']')
		.split(',')
		.filter_map(|s| s.trim().parse().ok())
		.collect()
}

fn kill_loop(mode: &str, rounds: usize) {
	let dir = fresh_dir(&format!("kill-{mode}"));
	let path = dir.join("doc.fxd");
	let log = dir.join("log.txt");
	let exe = std::env::current_exe().unwrap();
	let mut rng = Rng(0x5eed ^ rounds as u64);
	let (mut exact_last, mut exact_attempt, mut stale, mut errors, mut garbage, mut no_file) = (0, 0, 0, 0, 0, 0);
	let started = Instant::now();
	for round in 0..rounds {
		let _ = std::fs::remove_file(&log);
		let mut child = std::process::Command::new(&exe)
			.args(["child_saver", "--exact", "--ignored", "--nocapture", "--test-threads", "1"])
			.env("FOTOX_AUDIT_CHILD", &path)
			.env("FOTOX_AUDIT_MODE", mode)
			.env("FOTOX_AUDIT_LOG", &log)
			.stdout(std::process::Stdio::null())
			.stderr(std::process::Stdio::null())
			.spawn()
			.unwrap();
		// Wait for the first `attempt` (the child has opened and verified),
		// then a random time into the save loop.
		let deadline = Instant::now() + Duration::from_secs(60);
		while !std::fs::read_to_string(&log).unwrap_or_default().contains("attempt") {
			if let Ok(Some(status)) = child.try_wait() {
				panic!("round {round}: the child exited early ({status}): the previous kill left a file it cannot open");
			}
			assert!(Instant::now() < deadline, "child never started saving");
			std::thread::sleep(Duration::from_millis(5));
		}
		std::thread::sleep(Duration::from_millis(rng.next() % 1500));
		child.kill().unwrap();
		child.wait().unwrap();

		let text = std::fs::read_to_string(&log).unwrap_or_default();
		let last_ok = text.lines().rev().find(|l| l.starts_with("ok")).map(parse_versions);
		let last_attempt = text.lines().rev().find(|l| l.starts_with("attempt")).map(parse_versions);
		if !path.exists() {
			no_file += 1;
			println!("AUDIT kill-{mode} round {round}: NO FILE at the target after the kill");
			continue;
		}
		match reopen(&path, &dir.join("verify-scratch")) {
			Ok(Verdict::Exact(v)) => {
				if Some(&v) == last_attempt.as_ref() {
					exact_attempt += 1;
				} else if Some(&v) == last_ok.as_ref() || (last_ok.is_none() && round > 0) {
					exact_last += 1;
				} else {
					stale += 1;
					println!("AUDIT kill-{mode} round {round}: reopened {v:?}, last ok {last_ok:?}, attempt {last_attempt:?}");
				}
			}
			Ok(Verdict::ReadError(e)) => {
				errors += 1;
				println!("AUDIT kill-{mode} round {round}: READ ERROR {e}");
			}
			Ok(Verdict::Garbage(e)) => {
				garbage += 1;
				println!("AUDIT kill-{mode} round {round}: GARBAGE {e}");
			}
			Err(e) => {
				errors += 1;
				println!("AUDIT kill-{mode} round {round}: OPEN FAILED {e}");
			}
		}
	}
	let parts = part_files(&dir);
	let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
	println!(
		"AUDIT kill-{mode} on {}: {rounds} kills in {:.0} s → previous version {exact_last}, the interrupted save {exact_attempt}, older than the last confirmed save {stale}, read/open errors {errors}, garbage {garbage}, missing file {no_file}; .part left: {parts:?}; file {} MiB",
		fs_name(&dir),
		started.elapsed().as_secs_f64(),
		len >> 20
	);
	assert_eq!((stale, errors, garbage, no_file), (0, 0, 0, 0), "a process kill lost or corrupted a confirmed save");
}

#[test]
#[ignore = "audit: spawns child processes"]
fn kill_during_incremental_saves() {
	kill_loop("incremental", std::env::var("FOTOX_AUDIT_KILLS").ok().and_then(|v| v.parse().ok()).unwrap_or(30));
}

#[test]
#[ignore = "audit: spawns child processes"]
fn kill_during_fresh_saves() {
	kill_loop("fresh", std::env::var("FOTOX_AUDIT_KILLS").ok().and_then(|v| v.parse().ok()).unwrap_or(30));
}

// ------------------------------------------------- concurrency, replacement

/// Doc B is open from X (its tiles backed by X). Doc A is Saved As over X.
/// Then B is saved (incremental, as the engine does for a document with a
/// file). Where do B's bytes go, and what does X hold afterwards?
#[test]
#[ignore = "audit"]
fn save_as_over_a_file_another_document_has_open() {
	let dir = fresh_dir("replace-open");
	let store = make_store(&dir.join("scratch"));
	let x = dir.join("x.fxd");

	let b = versioned_document(&store, &[1, 1, 1, 1]);
	save_doc(&b, &store, SaveTarget::Fresh(x.clone())).unwrap();
	let opened_b = fxd::open(&x, &store).unwrap();
	let mut doc_b = opened_b.document;
	let file_b = opened_b.file;

	let a = versioned_document(&store, &[500, 500, 500, 500]);
	let replace = save_doc(&a, &store, SaveTarget::Fresh(x.clone()));
	println!("AUDIT replace-open on {}: Save As A over X while B has X open → {:?}", fs_name(&dir), replace.as_ref().map(|_| "ok"));
	println!("AUDIT replace-open: .part files now {:?}", part_files(&dir));

	// B paints layer 0 and saves to "its file".
	doc_b.layers[0] = pixel_layer(&store, &mut doc_b.clone(), 0, 2);
	let saved_b = save_doc(&doc_b, &store, SaveTarget::Incremental(file_b.clone()));
	println!("AUDIT replace-open: B's incremental save → {:?}", saved_b.as_ref().map(|s| s.report.tiles_written));

	let on_disk = reopen(&x, &dir.join("v")).unwrap();
	println!("AUDIT replace-open: X now holds {on_disk:?}");
	drop(saved_b);
	drop(file_b);
	drop(doc_b);
	let after_close = reopen(&x, &dir.join("v2")).unwrap();
	println!("AUDIT replace-open: after B closed, X holds {after_close:?}");
	if replace.is_ok() {
		assert_eq!(
			after_close,
			Verdict::Exact(vec![2, 1, 1, 1]),
			"B's save reported success but B's content is not in the file B was saved to"
		);
	}
}

/// Two handles on one file (two documents opened from the same `.fxd`),
/// saved from two threads in turn. The path lease serialises them; the file
/// must always open to one complete version.
#[test]
#[ignore = "audit"]
fn two_documents_from_one_file_saved_from_two_threads() {
	let dir = fresh_dir("two-threads");
	let store = make_store(&dir.join("scratch"));
	let x = dir.join("x.fxd");
	save_doc(&versioned_document(&store, &[1, 1, 1, 1]), &store, SaveTarget::Fresh(x.clone())).unwrap();
	let first = fxd::open(&x, &store).unwrap();
	let second = fxd::open(&x, &store).unwrap();
	let run = |opened: fx_io::fxd::OpenedFxd, base: u64| {
		let store = store.clone();
		std::thread::spawn(move || {
			let mut doc = opened.document;
			let mut file = opened.file;
			let mut last = Vec::new();
			for k in 0..8u64 {
				let j = (k as usize) % LAYERS;
				doc.layers[j] = pixel_layer(&store, &mut doc.clone(), j, base + k);
				file = save_doc(&doc, &store, SaveTarget::Incremental(file)).unwrap().file;
				last = doc.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
			}
			last
		})
	};
	let a = run(first, 100);
	let b = run(second, 200);
	let (a, b) = (a.join().unwrap(), b.join().unwrap());
	let verdict = reopen(&x, &dir.join("v")).unwrap();
	println!("AUDIT two-threads on {}: A ended {a:?}, B ended {b:?}, X holds {verdict:?}", fs_name(&dir));
	assert!(matches!(verdict, Verdict::Exact(_)), "concurrent saves left a corrupt file");
}

/// Two processes saving one file (no cross-process lock in fx-io; the app's
/// single-instance lock is the only guard).
#[test]
#[ignore = "audit: spawns child processes"]
fn two_processes_saving_one_file() {
	let dir = fresh_dir("two-processes");
	let path = dir.join("doc.fxd");
	{
		let store = make_store(&dir.join("seed"));
		save_doc(&versioned_document(&store, &[1, 1, 1, 1]), &store, SaveTarget::Fresh(path.clone())).unwrap();
	}
	let exe = std::env::current_exe().unwrap();
	let spawn = |log: &str| {
		std::process::Command::new(&exe)
			.args(["child_saver", "--exact", "--ignored", "--nocapture", "--test-threads", "1"])
			.env("FOTOX_AUDIT_CHILD", &path)
			.env("FOTOX_AUDIT_MODE", "incremental")
			.env("FOTOX_AUDIT_LOG", dir.join(log))
			.stdout(std::process::Stdio::null())
			.stderr(std::process::Stdio::null())
			.spawn()
			.unwrap()
	};
	let mut outcomes = Vec::new();
	for round in 0..5 {
		let (mut c1, mut c2) = (spawn(&format!("a{round}.txt")), spawn(&format!("b{round}.txt")));
		std::thread::sleep(Duration::from_millis(4000));
		let s1 = c1.try_wait().unwrap();
		let s2 = c2.try_wait().unwrap();
		let _ = c1.kill();
		let _ = c2.kill();
		let _ = c1.wait();
		let _ = c2.wait();
		let oks = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default().lines().filter(|l| l.starts_with("ok")).count();
		let verdict = reopen(&path, &dir.join(format!("v{round}")));
		println!(
			"AUDIT two-processes on {} round {round}: saves confirmed A {} B {}, A exited early {:?}, B exited early {:?}, file → {:?}",
			fs_name(&dir),
			oks(&format!("a{round}.txt")),
			oks(&format!("b{round}.txt")),
			s1,
			s2,
			verdict
		);
		outcomes.push(verdict.map(|v| matches!(v, Verdict::Exact(_))).unwrap_or(false));
		if !outcomes.last().unwrap() {
			break;
		}
	}
}

// ------------------------------------------------------- failure injection

/// Cancel (the only abort hook a save has) at every batch boundary of an
/// incremental and of a fresh save. The file must keep the previous version.
#[test]
#[ignore = "audit"]
fn cancel_at_every_batch_boundary() {
	let dir = fresh_dir("cancel");
	let store = make_store(&dir.join("scratch"));
	let x = dir.join("x.fxd");
	let mut doc = versioned_document(&store, &[1, 1, 1, 1]);
	let mut file = save_doc(&doc, &store, SaveTarget::Fresh(x.clone())).unwrap().file;
	let mut expected = vec![1u64, 1, 1, 1];
	let mut bad = 0;
	for (mode, fresh) in [("incremental", false), ("fresh", true)] {
		for stop_at in 1..12usize {
			// A new version of every layer, so the save has every batch to write.
			let next: Vec<u64> = expected.iter().map(|v| v + 1).collect();
			for (j, v) in next.iter().enumerate() {
				doc.layers[j] = pixel_layer(&store, &mut doc.clone(), j, *v);
			}
			let mut calls = 0usize;
			let target = if fresh { SaveTarget::Fresh(x.clone()) } else { SaveTarget::Incremental(file.clone()) };
			let result = fxd::save(
				SaveRequest {
					doc: &doc,
					store: &store,
					preview: None,
				},
				target,
				&mut |_| {
					calls += 1;
					calls < stop_at
				},
			);
			let len = std::fs::metadata(&x).unwrap().len();
			let verdict = reopen(&x, &dir.join(format!("v-{mode}-{stop_at}"))).unwrap();
			let ok = match &result {
				Err(IoError::Cancelled) => verdict == Verdict::Exact(expected.clone()),
				Ok(saved) => {
					file = saved.file.clone();
					expected = next.clone();
					verdict == Verdict::Exact(expected.clone())
				}
				Err(_) => false,
			};
			if !ok {
				bad += 1;
			}
			println!(
				"AUDIT cancel {mode} after {stop_at} progress calls: {} → file {len} B, reopens {verdict:?} {}; .part {:?}",
				match &result {
					Ok(_) => "completed".to_owned(),
					Err(e) => e.to_string(),
				},
				if ok { "(ok)" } else { "(WRONG)" },
				part_files(&dir)
			);
		}
	}
	// After the cancellations a normal incremental save still works.
	let next: Vec<u64> = expected.iter().map(|v| v + 1).collect();
	for (j, v) in next.iter().enumerate() {
		doc.layers[j] = pixel_layer(&store, &mut doc.clone(), j, *v);
	}
	let fresh_file = fxd::open(&x, &store).unwrap().file;
	save_doc(&doc, &store, SaveTarget::Incremental(fresh_file)).unwrap();
	let verdict = reopen(&x, &dir.join("v-final")).unwrap();
	println!("AUDIT cancel: a save after the cancellations → {verdict:?}");
	assert_eq!(bad, 0);
	assert_eq!(verdict, Verdict::Exact(next));
}

/// A tile that cannot be read mid-save (its backing file is corrupt): the
/// save must fail and the target keep its previous version.
#[test]
#[ignore = "audit"]
fn unreadable_tile_mid_save() {
	let dir = fresh_dir("unreadable");
	let store = make_store(&dir.join("scratch"));
	let y = dir.join("y.fxd");
	let x = dir.join("x.fxd");
	save_doc(&versioned_document(&store, &[7, 7, 7, 7]), &store, SaveTarget::Fresh(y.clone())).unwrap();
	save_doc(&versioned_document(&store, &[1, 1, 1, 1]), &store, SaveTarget::Fresh(x.clone())).unwrap();
	let opened = fxd::open(&y, &store).unwrap();
	// Corrupt the last tile chunk of Y (layer 3's last tile, written last)
	// before anything reads it.
	{
		let file = OpenOptions::new().write(true).read(true).open(&y).unwrap();
		let len = file.metadata().unwrap().len();
		let footer = opened.file.footer();
		let target = footer.manifest_offset - 4096;
		use std::os::windows::fs::FileExt;
		file.seek_write(&[0xAB; 16], target).unwrap();
		let _ = len;
	}
	let x_file = fxd::open(&x, &store).unwrap().file;
	for (mode, target) in [("incremental", SaveTarget::Incremental(x_file)), ("fresh", SaveTarget::Fresh(x.clone()))] {
		let result = save_doc(&opened.document, &store, target);
		let verdict = reopen(&x, &dir.join(format!("v-{mode}"))).unwrap();
		println!(
			"AUDIT unreadable-tile {mode}: save → {:?}; X reopens {verdict:?}; .part {:?}",
			result.as_ref().map(|_| "ok").map_err(ToString::to_string),
			part_files(&dir)
		);
		assert!(result.is_err());
		assert_eq!(verdict, Verdict::Exact(vec![1, 1, 1, 1]));
	}
}

/// Replacement refused by the OS: the target is open elsewhere without
/// delete sharing, or it is read-only.
#[test]
#[ignore = "audit"]
fn replace_refused_by_the_os() {
	use std::os::windows::fs::OpenOptionsExt;
	let dir = fresh_dir("replace-refused");
	let store = make_store(&dir.join("scratch"));
	let x = dir.join("x.fxd");
	save_doc(&versioned_document(&store, &[1, 1, 1, 1]), &store, SaveTarget::Fresh(x.clone())).unwrap();
	let doc2 = versioned_document(&store, &[2, 2, 2, 2]);

	// 1. Another program holds X open with FILE_SHARE_READ only.
	{
		let _held = OpenOptions::new().read(true).share_mode(0x1).open(&x).unwrap();
		let result = save_doc(&doc2, &store, SaveTarget::Fresh(x.clone()));
		println!(
			"AUDIT replace-refused (held open, share read) on {}: {:?}; .part {:?}",
			fs_name(&dir),
			result.as_ref().map(|_| "ok").map_err(ToString::to_string),
			part_files(&dir)
		);
	}
	let verdict = reopen(&x, &dir.join("v1")).unwrap();
	println!("AUDIT replace-refused: X reopens {verdict:?}");
	assert!(matches!(verdict, Verdict::Exact(_)));

	// 2. X is read-only.
	let mut perms = std::fs::metadata(&x).unwrap().permissions();
	perms.set_readonly(true);
	std::fs::set_permissions(&x, perms.clone()).unwrap();
	let result = save_doc(&doc2, &store, SaveTarget::Fresh(x.clone()));
	println!(
		"AUDIT replace-refused (read-only target): {:?}; .part {:?}",
		result.as_ref().map(|_| "ok").map_err(ToString::to_string),
		part_files(&dir)
	);
	// 3. Opening a read-only .fxd at all.
	let open_ro = fxd::open(&x, &store).map(|_| ());
	println!("AUDIT read-only .fxd: open → {:?}", open_ro.map_err(|e| e.to_string()));
	perms.set_readonly(false);
	std::fs::set_permissions(&x, perms).unwrap();
}

// --------------------------------------------- truncation and bit flips

/// A small file with several incremental versions; the end offset of each.
fn versions_file(dir: &Path, side_override: u32) -> (PathBuf, Vec<(u64, Vec<u64>)>) {
	// SAFETY of the env override: tests here run with --test-threads 1.
	unsafe { std::env::set_var("FOTOX_AUDIT_SIDE", side_override.to_string()) };
	let store = make_store(&dir.join("scratch"));
	let x = dir.join("versions.fxd");
	let mut versions = vec![1u64; LAYERS];
	let mut doc = versioned_document(&store, &versions);
	let mut file = save_doc(&doc, &store, SaveTarget::Fresh(x.clone())).unwrap().file;
	let mut ends = vec![(std::fs::metadata(&x).unwrap().len(), versions.clone())];
	for k in 0..5usize {
		let j = k % LAYERS;
		versions[j] += 1;
		doc.layers[j] = pixel_layer(&store, &mut doc.clone(), j, versions[j]);
		file = save_doc(&doc, &store, SaveTarget::Incremental(file)).unwrap().file;
		ends.push((std::fs::metadata(&x).unwrap().len(), versions.clone()));
	}
	unsafe { std::env::remove_var("FOTOX_AUDIT_SIDE") };
	(x, ends)
}

#[test]
#[ignore = "audit"]
fn truncation_at_every_offset_class() {
	let dir = fresh_dir("truncate");
	let (x, ends) = versions_file(&dir, 512);
	let total = ends.last().unwrap().0;
	let mut cuts: Vec<u64> = (64..total).step_by(16 * 1024).collect();
	for (end, _) in &ends {
		for d in [-65i64, -64, -63, -1, 0, 1, 16, 17] {
			let c = *end as i64 + d;
			if c > 0 && (c as u64) <= total {
				cuts.push(c as u64);
			}
		}
	}
	cuts.extend([0, 1, 63, 64, 65, 80]);
	cuts.sort_unstable();
	cuts.dedup();
	let (mut right, mut wrong) = (0, 0);
	let probe = dir.join("probe.fxd");
	for &cut in &cuts {
		copy_prefix(&x, &probe, cut);
		let expected = ends.iter().rev().find(|(end, _)| *end <= cut).map(|(_, v)| v.clone());
		let got = reopen(&probe, &dir.join("probe-scratch"));
		let ok = match (&got, &expected) {
			(Ok(Verdict::Exact(v)), Some(e)) => v == e,
			(Err(_), None) => true,
			_ => false,
		};
		if ok {
			right += 1;
		} else {
			wrong += 1;
			println!("AUDIT truncate at {cut}: expected {expected:?}, got {got:?}");
		}
	}
	println!(
		"AUDIT truncation: {} cut points over a {} KiB file with {} versions → {right} recovered the newest complete version, {wrong} did not",
		cuts.len(),
		total >> 10,
		ends.len()
	);
	assert_eq!(wrong, 0);
}

/// One flipped byte at sampled offsets of each region. Classify what a user
/// gets: the newest version, a silent rollback, an error, or wrong pixels.
#[test]
#[ignore = "audit"]
fn single_byte_corruption_by_region() {
	let dir = fresh_dir("bitflip");
	let (x, ends) = versions_file(&dir, 512);
	let newest = ends.last().unwrap().1.clone();
	let bytes = std::fs::read(&x).unwrap();
	let (_, footer) = FxdFile::open(&x).unwrap();
	let total = bytes.len() as u64;
	let prev_end = ends[ends.len() - 2].0;
	let regions: Vec<(&str, u64, u64)> = vec![
		("header", 0, 64),
		("older chunks", 64, prev_end - 64),
		("older footer", prev_end - 64, prev_end),
		("newest tile chunks", prev_end, footer.manifest_offset),
		("newest manifest", footer.manifest_offset, footer.manifest_offset + footer.manifest_len),
		("newest footer", total - 64, total),
	];
	let probe = dir.join("probe.fxd");
	let mut rng = Rng(42);
	for (name, lo, hi) in regions {
		let (mut newest_ok, mut rollback, mut error, mut garbage, mut open_failed) = (0, 0, 0, 0, 0);
		let samples = 24.min((hi - lo) as usize);
		for _ in 0..samples {
			let at = lo + rng.next() % (hi - lo);
			let mut copy = bytes.clone();
			copy[at as usize] ^= 1 << (rng.next() % 8);
			std::fs::write(&probe, &copy).unwrap();
			match reopen(&probe, &dir.join("probe-scratch")) {
				Ok(Verdict::Exact(v)) if v == newest => newest_ok += 1,
				Ok(Verdict::Exact(_)) => rollback += 1,
				Ok(Verdict::ReadError(_)) => error += 1,
				Ok(Verdict::Garbage(g)) => {
					garbage += 1;
					println!("AUDIT bitflip {name} at {at}: GARBAGE {g}");
				}
				Err(_) => open_failed += 1,
			}
		}
		println!(
			"AUDIT bitflip {name} ({samples} samples): newest intact {newest_ok}, silent rollback to an older version {rollback}, tile read error {error}, file does not open {open_failed}, wrong pixels {garbage}"
		);
		assert_eq!(garbage, 0, "a flipped byte produced wrong pixels without an error");
	}
}

/// The newest save's chunks lost (zeroed) while its footer survived — what
/// a reordering of the data and footer writes would leave if `sync_data`
/// did not hold. Shows what the ordering protects.
#[test]
#[ignore = "audit"]
fn chunks_lost_footer_kept() {
	let dir = fresh_dir("zeroed");
	let (x, ends) = versions_file(&dir, 512);
	let mut bytes = std::fs::read(&x).unwrap();
	let prev_end = ends[ends.len() - 2].0 as usize;
	let footer_at = bytes.len() - 64;
	for b in &mut bytes[prev_end..footer_at] {
		*b = 0;
	}
	let probe = dir.join("probe.fxd");
	std::fs::write(&probe, &bytes).unwrap();
	let got = reopen(&probe, &dir.join("s"));
	println!("AUDIT zeroed newest chunks, footer kept: {got:?} (an older complete version is still in the file: {:?})", ends[ends.len() - 2].1);
}

// --------------------------------------------------------- exact round trip

/// Structure + pixels of a document as one comparable value: the manifest
/// JSON with every tile reference replaced by a hash of the tile's bytes.
fn fingerprint(doc: &Document, store: &TileStore) -> serde_json::Value {
	let manifest = fxd::manifest::to_manifest(doc, |handle| {
		let pixels = store.get(handle).ok()?;
		let mut h: u64 = 0xcbf2_9ce4_8422_2325;
		for b in pixels.bytes() {
			h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
		}
		Some(fxd::ChunkRef { offset: h >> 32, len: h & 0xffff_ffff })
	});
	serde_json::from_slice(&fxd::manifest::manifest_to_json(&manifest).unwrap()).unwrap()
}

fn rich_document(store: &TileStore, depth: BitDepth, side: u32) -> Document {
	let format = depth.rgba_format();
	let gray = fx_core::selection::gray_format(depth);
	let mut doc = Document::new(
		side,
		side,
		DocumentColor {
			depth,
			profile: ColorProfile::Srgb,
		},
		300.0,
	);
	let mut rng = Rng(depth as u64 + 7);
	let mut noisy = |w: u32, h: u32, f: PixelFormat, rng: &mut Rng| {
		let mut image = TiledImage::new(w, h, f);
		for ty in 0..h.div_ceil(256) {
			for tx in 0..w.div_ceil(256) {
				if rng.next() % 3 == 0 {
					image.set_slot(tx, ty, TileSlot::Solid(fx_tiles::PixelValue::rgba16(1000, 2000, 3000, 65535)));
					continue;
				}
				let mut b = TileBuffer::zeroed(f);
				// Only pixels inside the image: edge tiles stay transparent outside.
				let bpp = f.bytes_per_pixel();
				for y in 0..256u32 {
					for x in 0..256u32 {
						if tx * 256 + x < w && ty * 256 + y < h {
							let o = ((y * 256 + x) as usize) * bpp;
							let v = rng.next().to_le_bytes();
							b.bytes_mut()[o..o + bpp].copy_from_slice(&v[..bpp]);
						}
					}
				}
				image.put_buffer(store, tx, ty, b);
			}
		}
		image
	};
	let mut layers = Vec::new();
	let id = doc.allocate_layer_id();
	let mut background = Layer::new(id, "Background", LayerKind::Pixel { image: noisy(side, side, format, &mut rng), offset: (0, 0) });
	background.mask = Some(fx_core::Mask {
		image: noisy(side, side, gray, &mut rng),
		enabled: true,
		linked: true,
		outside_value: 65535,
	});
	layers.push(Arc::new(background));
	let id = doc.allocate_layer_id();
	let mut offset_layer = Layer::new(id, "Offset", LayerKind::Pixel { image: noisy(side / 2 + 37, side / 3 + 11, format, &mut rng), offset: (-41, 97) });
	offset_layer.opacity = 0.37;
	offset_layer.blend = fx_core::BlendMode::Multiply;
	layers.push(Arc::new(offset_layer));
	let id = doc.allocate_layer_id();
	layers.push(Arc::new(Layer::new(id, "Solid", LayerKind::SolidFill { rgba: [1, 2, 3, 40000] })));
	let id = doc.allocate_layer_id();
	layers.push(Arc::new(Layer::new(
		id,
		"Levels",
		LayerKind::Adjustment(fx_core::Adjustment::Invert),
	)));
	// A group with a nested group and a pixel child.
	let id_inner = doc.allocate_layer_id();
	let inner_child_id = doc.allocate_layer_id();
	let inner_child = Arc::new(Layer::new(inner_child_id, "Inner pixels", LayerKind::Pixel { image: noisy(side, side, format, &mut rng), offset: (0, 0) }));
	let inner = Arc::new(Layer::new(id_inner, "Inner group", LayerKind::Group { children: vec![inner_child], expanded: false }));
	let id_outer = doc.allocate_layer_id();
	layers.push(Arc::new(Layer::new(id_outer, "Outer group", LayerKind::Group { children: vec![inner], expanded: true })));
	// A Smart Object with an embedded document of its own size.
	let (sw, sh) = (side / 2 + 3, side / 2 + 129);
	let mut nested = Document::new(sw, sh, doc.color.clone(), 72.0);
	let nid = nested.allocate_layer_id();
	nested.layers = vec![Arc::new(Layer::new(nid, "Nested", LayerKind::Pixel { image: noisy(sw, sh, format, &mut rng), offset: (0, 0) }))];
	let composite = noisy(sw, sh, format, &mut rng);
	let smart = fx_core::smart::SmartObject {
		source: fx_core::smart::SmartSource {
			doc: Arc::new(nested),
			composite,
			linked: None,
			linked_mtime: None,
			uid: 99,
		},
		transform: fx_core::Mapping::Affine([0.5, 0.1, -0.1, 0.5, 120.0, 80.0]),
		filters: vec![fx_core::smart::SmartFilter {
			filter: fx_core::FilterParams::Despeckle,
			enabled: true,
			mode: fx_core::BlendMode::Screen,
			opacity: 0.6,
		}],
		filters_enabled: true,
	};
	let sid = doc.allocate_layer_id();
	layers.push(Arc::new(Layer::new(sid, "Smart", LayerKind::Smart { smart, cache: TiledImage::derived(side, side, format) })));
	doc.layers = layers;
	doc
}

#[test]
#[ignore = "audit"]
fn exact_round_trip_8_and_16_bit() {
	let dir = fresh_dir("roundtrip");
	let store = make_store(&dir.join("scratch"));
	for depth in [BitDepth::U8, BitDepth::U16] {
		for side in [700u32, 2048] {
			let doc = rich_document(&store, depth, side);
			let before = fingerprint(&doc, &store);
			let x = dir.join(format!("rt-{depth:?}-{side}.fxd"));
			let t = Instant::now();
			let saved = save_doc(&doc, &store, SaveTarget::Fresh(x.clone())).unwrap();
			let save_s = t.elapsed().as_secs_f64();
			let other = make_store(&dir.join(format!("s-{depth:?}-{side}")));
			let opened = fxd::open(&x, &other).unwrap();
			let after = fingerprint(&opened.document, &other);
			// Incremental save of the reopened document, reopened again.
			let mut doc2 = opened.document;
			doc2.layer_mut(fx_core::LayerId(2)).unwrap().opacity = 0.5;
			let again = save_doc(&doc2, &other, SaveTarget::Incremental(opened.file.clone())).unwrap();
			let third = make_store(&dir.join(format!("t-{depth:?}-{side}")));
			let reopened = fxd::open(&x, &third).unwrap();
			let equal_after_incremental = fingerprint(&reopened.document, &third) == fingerprint(&doc2, &other);
			println!(
				"AUDIT round-trip {depth:?} {side}²: fresh save {:.2} s ({} tiles), reopened identical {}, incremental ({} written, {} reused) reopened identical {}",
				save_s,
				saved.report.tiles_written,
				before == after,
				again.report.tiles_written,
				again.report.tiles_reused,
				equal_after_incremental
			);
			if before != after {
				let (b, a) = (before.to_string(), after.to_string());
				let i = b.bytes().zip(a.bytes()).position(|(x, y)| x != y).unwrap_or(0);
				println!("  first difference at JSON byte {i}: …{}… vs …{}…", &b[i.saturating_sub(80)..(i + 80).min(b.len())], &a[i.saturating_sub(80)..(i + 80).min(a.len())]);
			}
			assert_eq!(before, after);
			assert!(equal_after_incremental);
		}
	}
}

/// Mips are part of what a save writes: the time and size of a save of a
/// mostly-incompressible 16-bit layer, for the report.
#[test]
#[ignore = "audit"]
fn save_throughput() {
	let dir = fresh_dir("throughput");
	let store = make_store(&dir.join("scratch"));
	unsafe { std::env::set_var("FOTOX_AUDIT_SIDE", "4096") };
	let doc = versioned_document(&store, &[1, 1, 1, 1]);
	unsafe { std::env::remove_var("FOTOX_AUDIT_SIDE") };
	let x = dir.join("big.fxd");
	let t = Instant::now();
	let saved = save_doc(&doc, &store, SaveTarget::Fresh(x.clone())).unwrap();
	let fresh = t.elapsed().as_secs_f64();
	let t = Instant::now();
	let again = save_doc(&doc, &store, SaveTarget::Incremental(saved.file.clone())).unwrap();
	let inc = t.elapsed().as_secs_f64();
	let len = std::fs::metadata(&x).unwrap().len();
	println!(
		"AUDIT throughput on {}: 4 × 4096² 16-bit noise = 512 MiB raw → fresh save {fresh:.2} s ({} MiB/s raw), file {} MiB; unchanged incremental save {inc:.3} s ({} tiles reused)",
		fs_name(&dir),
		(512.0 / fresh) as u64,
		len >> 20,
		again.report.tiles_reused
	);
}
