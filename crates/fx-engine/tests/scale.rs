//! Scale stress tests: how a real engine (engine + render threads on the GPU,
//! driven by the same messages the UI sends) behaves with thousands of
//! layers, deeply nested groups, huge canvases and many open documents.
//!
//! These are measurements more than gates: each scenario prints what every
//! step took (and the memory at checkpoints) and fails only on errors, lost
//! data or a step past a generous ceiling. All are `#[ignore]`d — minutes,
//! gigabytes of scratch. Release build, one at a time:
//!
//!   cargo test --release -p fx-engine --test scale -- --ignored --nocapture --test-threads 1
//!
//! Knobs (environment):
//! * `FOTOX_STRESS_LAYERS` — layers of `thousands_of_layers` (default 2000)
//! * `FOTOX_STRESS_DEPTHS` — nesting depths of `deeply_nested_groups` (default `10,11`: 10 is the
//!   limit, deeper is refused)
//! * `FOTOX_STRESS_SIZE`   — side of `huge_canvas`, 16-bit (default 30000)
//! * `FOTOX_STRESS_DOCS`   — documents of `many_documents` (default 10)
//! * `FOTOX_STRESS_CSV=1`  — also append the rows to `bench/results.csv`
//! * `FOTOX_STRESS_DIR`    — where scratch and saved files go (default: the
//!   temp folder; `huge_canvas` at 30000 needs ~20 GB free there)
//!
//! Per-step timings are split into the first and last tenth: a step that is
//! fast on average but grows with the layer count shows up there, not in
//! the mean.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Harness, Seen, gpu};
use fx_core::command::{LayerPropsPatch, NewLayer};
use fx_core::{BlendMode, Command, LayerId, LayerRef, SelectMode, SelectionShape};
use fx_engine::EngineInput;
use fx_protocol::{DocId, EngineToUi, LayerInfo, MemoryStats, UiToEngine};

// ------------------------------------------------------------------ harness

/// A frame burst has ended when no frame came for this long.
const QUIET: Duration = Duration::from_millis(750);
/// Longest any single step may take before the scenario gives up on it.
const CEILING: Duration = Duration::from_secs(600);

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
	std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// splitmix64: deterministic, so every run builds the same documents.
struct Rng(u64);

impl Rng {
	fn next(&mut self) -> u64 {
		self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
		let mut z = self.0;
		z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
		z ^ (z >> 31)
	}
	fn unit(&mut self) -> f64 {
		(self.next() >> 11) as f64 / (1u64 << 53) as f64
	}
	fn range(&mut self, lo: f64, hi: f64) -> f64 {
		lo + (hi - lo) * self.unit()
	}
}

/// Removes the scenario's directory (scratch, saved files) when the
/// scenario ends, even on a panic. Declared before the [`Probe`], so it
/// drops after the engine has stopped.
struct TempDir(PathBuf);

impl TempDir {
	fn new(name: &str) -> Self {
		let root = std::env::var_os("FOTOX_STRESS_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
		let dir = root.join(format!("fx-scale-{name}-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		Self(dir)
	}
}

impl Drop for TempDir {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.0);
	}
}

/// One measured row.
struct Row {
	document: String,
	metric: String,
	value: f64,
	unit: &'static str,
}

/// The engine plus what the scenario has learnt from its messages.
struct Probe {
	h: Harness,
	scenario: &'static str,
	document: String,
	rows: Vec<Row>,
	/// Messages not consumed yet, oldest first.
	inbox: std::collections::VecDeque<EngineToUi>,
	status: Option<MemoryStats>,
	layers: Option<(DocId, Vec<LayerInfo>)>,
	fit_zoom: Option<f64>,
	/// Thumbnail frames received so far.
	thumbnails: usize,
}

impl Probe {
	fn start(scenario: &'static str, dir: &Path) -> Option<Self> {
		let Some((device, queue)) = gpu() else {
			eprintln!("no GPU adapter: test skipped");
			return None;
		};
		Some(Self {
			h: Harness::start(device, queue, dir),
			scenario,
			document: String::new(),
			rows: Vec::new(),
			inbox: Default::default(),
			status: None,
			layers: None,
			fit_zoom: None,
			thumbnails: 0,
		})
	}

	/// Move what the engine said into the inbox, remembering the latest
	/// status and layer list on the way (they arrive all the time).
	fn pump(&mut self) {
		let seen = std::mem::take(&mut *self.h.seen.lock().unwrap());
		for item in seen {
			if let Seen::Ui(message) = item {
				match &message {
					EngineToUi::Status { memory, .. } => self.status = Some(*memory),
					EngineToUi::Layers { doc, layers, .. } => self.layers = Some((*doc, layers.clone())),
					EngineToUi::Thumbnail { .. } => self.thumbnails += 1,
					_ => {}
				}
				self.inbox.push_back(message);
			}
		}
	}

	/// Forget everything received so far.
	fn drain(&mut self) {
		self.pump();
		self.inbox.clear();
	}

	/// Wait for a message `pick` accepts, consuming everything up to it. An
	/// engine error or toast fails the scenario (with the text).
	fn until<T>(&mut self, what: &str, timeout: Duration, mut pick: impl FnMut(&EngineToUi) -> Option<T>) -> T {
		let deadline = Instant::now() + timeout;
		loop {
			self.pump();
			while let Some(message) = self.inbox.pop_front() {
				if let Some(found) = pick(&message) {
					return found;
				}
				match &message {
					EngineToUi::Error { text } => panic!("{}: engine error while waiting for {what}: {text}", self.scenario),
					EngineToUi::Toast { text } if text != "Engine connected" => {
						eprintln!("  (toast while waiting for {what}: {text})");
					}
					_ => {}
				}
			}
			assert!(Instant::now() < deadline, "{}: timed out after {timeout:?} waiting for {what}", self.scenario);
			std::thread::sleep(Duration::from_millis(1));
		}
	}

	fn ui(&self, message: UiToEngine) {
		self.h.ui(message);
	}

	fn action(&self, id: &str, args: serde_json::Value) {
		self.ui(UiToEngine::Action { id: id.into(), args });
	}

	/// Send `commands` and wait until each has answered with a History
	/// message; returns how long that took.
	fn step(&mut self, doc: DocId, commands: Vec<Command>) -> Duration {
		self.drain();
		let n = commands.len();
		let start = Instant::now();
		for command in commands {
			self.ui(UiToEngine::Command { doc, command });
		}
		for i in 0..n {
			self.until(&format!("history step {}/{n}", i + 1), CEILING, |m| match m {
				EngineToUi::History { doc: d, .. } if *d == doc => Some(()),
				_ => None,
			});
		}
		start.elapsed()
	}

	/// An action answered by one History message (merge, group, undo…).
	fn history_action(&mut self, doc: DocId, id: &str) -> Duration {
		self.drain();
		let start = Instant::now();
		self.action(id, serde_json::Value::Null);
		self.until(id, CEILING, |m| match m {
			EngineToUi::History { doc: d, .. } if *d == doc => Some(()),
			_ => None,
		});
		start.elapsed()
	}

	/// How long after `since` the viewport stopped changing: the time of the
	/// last frame of the burst that started after `since`. `None` when no
	/// frame came within 3 s of the call (nothing to redraw).
	fn settle(&mut self, since: Instant) -> Option<Duration> {
		// A step may itself have taken seconds: "no frame" counts from here.
		let called = Instant::now();
		loop {
			self.pump();
			self.inbox.retain(|m| !matches!(m, EngineToUi::Status { .. }));
			let last = *self.h.last_frame_at.lock().unwrap();
			let now = Instant::now();
			match last {
				// Quiet since the last frame, and for as long since this call: a
				// burst that ended before a long step finished is not the end.
				Some(at) if at > since && now - at >= QUIET && now - called >= QUIET => return Some(at - since),
				Some(at) if at > since => {}
				_ if now - called > Duration::from_secs(3) => return None,
				_ => {}
			}
			if now - since > Duration::from_secs(120) {
				println!("  !! the view was still redrawing after 120 s");
				return Some(now - since);
			}
			std::thread::sleep(Duration::from_millis(2));
		}
	}

	fn new_document(&mut self, width: u32, height: u32, depth: u8) -> DocId {
		self.drain();
		self.action(
			"doc:new",
			serde_json::json!({ "width": width, "height": height, "ppi": 72, "depth": depth, "background": "white" }),
		);
		self.opened()
	}

	/// The next opened document; remembers its fit zoom.
	fn opened(&mut self) -> DocId {
		let doc = self.until("a document", CEILING, |m| match m {
			EngineToUi::DocumentOpened { info } => Some(info.doc),
			_ => None,
		});
		let zoom = self.until("its view", Duration::from_secs(30), |m| match m {
			EngineToUi::View { doc: d, zoom, .. } if *d == doc => Some(*zoom),
			_ => None,
		});
		self.fit_zoom = Some(zoom);
		doc
	}

	/// The document's layer list as the engine last sent it.
	fn layer_list(&mut self, doc: DocId) -> Vec<LayerInfo> {
		self.pump();
		match &self.layers {
			Some((d, layers)) if *d == doc => layers.clone(),
			_ => Vec::new(),
		}
	}

	fn save_as(&mut self, doc: DocId, path: &Path) -> Duration {
		self.drain();
		let start = Instant::now();
		self.h.engine.send(EngineInput::SaveAs { doc, path: path.to_owned() });
		self.until("the saved document", CEILING, |m| match m {
			EngineToUi::DocumentChanged { info } if info.doc == doc && !info.dirty => Some(()),
			_ => None,
		});
		start.elapsed()
	}

	fn close(&mut self, doc: DocId) {
		self.drain();
		self.ui(UiToEngine::CloseDocument { doc });
		self.until("the document closed", CEILING, |m| match m {
			EngineToUi::DocumentClosed { doc: d } if *d == doc => Some(()),
			EngineToUi::CloseDirtyDocument { doc: d, .. } if *d == doc => panic!("closing a saved document asked to save"),
			_ => None,
		});
	}

	fn record(&mut self, metric: impl Into<String>, value: f64, unit: &'static str) {
		let metric = metric.into();
		println!("  {:<48} {:>12.1} {unit}", metric, value);
		self.rows.push(Row {
			document: self.document.clone(),
			metric,
			value,
			unit,
		});
	}

	fn record_time(&mut self, metric: impl Into<String>, d: Duration) {
		self.record(metric, d.as_secs_f64() * 1000.0, "ms");
	}

	fn record_settle(&mut self, metric: &str, since: Instant) {
		match self.settle(since) {
			Some(d) => self.record_time(metric, d),
			None => println!("  {metric:<48} (no frame)"),
		}
	}

	/// Engine tile-store memory (from its Status message) and the process's
	/// working set, private bytes and peak working set.
	fn record_memory(&mut self, label: &str) {
		// Status comes twice a second: wait for a fresh one.
		self.status = None;
		let deadline = Instant::now() + Duration::from_secs(3);
		while self.status.is_none() && Instant::now() < deadline {
			self.pump();
			std::thread::sleep(Duration::from_millis(20));
		}
		self.inbox.retain(|m| !matches!(m, EngineToUi::Status { .. }));
		let mib = |b: u64| b as f64 / (1 << 20) as f64;
		if let Some(m) = self.status {
			self.record(format!("{label}: tiles hot"), mib(m.hot_bytes), "MiB");
			self.record(format!("{label}: tiles warm (lz4)"), mib(m.warm_bytes), "MiB");
			self.record(format!("{label}: tiles scratch"), mib(m.scratch_bytes), "MiB");
		}
		if let Some((working, private, peak)) = process_memory() {
			self.record(format!("{label}: process working set"), mib(working), "MiB");
			self.record(format!("{label}: process private bytes"), mib(private), "MiB");
			self.record(format!("{label}: process peak working set"), mib(peak), "MiB");
		}
	}

	/// Median, 99th percentile and maximum of `samples`, each for the first
	/// and the last tenth (growth with size shows there) and overall.
	fn record_series(&mut self, name: &str, samples: &[Duration]) {
		if samples.is_empty() {
			return;
		}
		let tenth = (samples.len() / 10).max(1);
		for (part, slice) in [
			("first 10%", &samples[..tenth]),
			("last 10%", &samples[samples.len() - tenth..]),
			("all", samples),
		] {
			let mut ms: Vec<f64> = slice.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
			ms.sort_by(f64::total_cmp);
			let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
			self.record(format!("{name}, {part}: median"), at(0.5), "ms");
			self.record(format!("{name}, {part}: p99"), at(0.99), "ms");
			self.record(format!("{name}, {part}: max"), at(1.0), "ms");
		}
		let total: Duration = samples.iter().sum();
		self.record(format!("{name}: total"), total.as_secs_f64(), "s");
	}

	/// Print the summary and, with `FOTOX_STRESS_CSV=1`, append the rows to
	/// `bench/results.csv`.
	fn finish(mut self) {
		self.h.engine.shutdown();
		if std::env::var("FOTOX_STRESS_CSV").as_deref() != Ok("1") {
			return;
		}
		let csv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/results.csv");
		let commit = std::process::Command::new("git")
			.args(["rev-parse", "--short", "HEAD"])
			.output()
			.ok()
			.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
			.unwrap_or_default();
		let date = today();
		let quote = |s: &str| {
			if s.contains([',', '"']) {
				format!("\"{}\"", s.replace('"', "\"\""))
			} else {
				s.to_owned()
			}
		};
		let mut text = String::new();
		for row in std::mem::take(&mut self.rows) {
			text.push_str(&format!(
				"{date},{commit},stress:{},{},{},{:.3},{},\n",
				self.scenario,
				quote(&row.document),
				quote(&row.metric),
				row.value,
				row.unit
			));
		}
		use std::io::Write;
		let mut file = std::fs::OpenOptions::new().append(true).open(&csv).expect("bench/results.csv");
		file.write_all(text.as_bytes()).unwrap();
	}
}

/// Working set, private bytes and peak working set of this process (Windows;
/// `None` elsewhere or when PowerShell is missing).
fn process_memory() -> Option<(u64, u64, u64)> {
	let script = format!(
		"$p = Get-Process -Id {}; \"$($p.WorkingSet64) $($p.PrivateMemorySize64) $($p.PeakWorkingSet64)\"",
		std::process::id()
	);
	let out = std::process::Command::new("powershell")
		.args(["-NoProfile", "-Command", &script])
		.output()
		.ok()?;
	let text = String::from_utf8_lossy(&out.stdout);
	let mut it = text.split_whitespace().map(|v| v.parse::<u64>().ok());
	Some((it.next()??, it.next()??, it.next()??))
}

fn today() -> String {
	let days = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| d.as_secs() / 86_400) as i64;
	// Howard Hinnant's civil_from_days.
	let z = days + 719_468;
	let era = z.div_euclid(146_097);
	let doe = z - era * 146_097;
	let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let d = doy - (153 * mp + 2) / 5 + 1;
	let m = if mp < 10 { mp + 3 } else { mp - 9 };
	let y = yoe + era * 400 + i64::from(m <= 2);
	format!("{y:04}-{m:02}-{d:02}")
}

fn file_mib(path: &Path) -> f64 {
	std::fs::metadata(path).map_or(0.0, |m| m.len() as f64 / (1 << 20) as f64)
}

fn visible(layer: LayerId, on: bool) -> Command {
	Command::SetLayerProps {
		layer: LayerRef::Id(layer),
		props: LayerPropsPatch {
			visible: Some(on),
			..Default::default()
		},
	}
}

/// A random rectangle of 0.5–4 % of the canvas, a fill of it, and Deselect
/// (marching ants redraw the view 8 times a second: it would never settle).
fn rect_fill(rng: &mut Rng, (w, h): (u32, u32)) -> [Command; 3] {
	let (w, h) = (f64::from(w), f64::from(h));
	let area = rng.range(0.005, 0.04) * w * h;
	let aspect = rng.range(0.3, 3.0);
	let rw = (area * aspect).sqrt().min(w);
	let rh = (area / rw).min(h);
	let (x, y) = (rng.range(0.0, w - rw), rng.range(0.0, h - rh));
	let color = [0, 1, 2].map(|_| (rng.range(0.05, 0.95) * 65535.0) as u16);
	[
		Command::Select {
			shape: SelectionShape::Rect { x, y, w: rw, h: rh },
			mode: SelectMode::Replace,
			feather: 0.0,
			anti_alias: true,
		},
		Command::Fill {
			layer: LayerRef::Active,
			color: [color[0], color[1], color[2], 65535],
			mode: BlendMode::Normal,
			opacity: 1.0,
			preserve_transparency: false,
		},
		Command::Deselect,
	]
}

// ---------------------------------------------------------------- scenarios

/// N pixel layers, each holding a filled rectangle, added one by one the way
/// the Layers panel and Edit ▸ Fill would; then what a user does with such a
/// file: toggle and fade layers, zoom, undo, save, reopen, merge.
#[test]
#[ignore = "stress: minutes; run with --ignored --nocapture"]
fn thousands_of_layers() {
	let n: usize = env("FOTOX_STRESS_LAYERS", 2000);
	let (w, h) = (4000, 3000);
	let dir = TempDir::new("layers");
	let Some(mut p) = Probe::start("thousands_of_layers", &dir.0) else { return };
	p.document = format!("{w}x{h} 8-bit, {n} layers");
	println!("thousands_of_layers: {}", p.document);

	let doc = p.new_document(w, h, 8);
	p.record_settle("first frame of the new document", Instant::now() - Duration::from_millis(1));
	let mut rng = Rng(7);
	let mut per_layer = Vec::with_capacity(n);
	let mut layers_bytes = Vec::new();
	let build = Instant::now();
	for i in 0..n {
		let mut commands = vec![Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		}];
		commands.extend(rect_fill(&mut rng, (w, h)));
		per_layer.push(p.step(doc, commands));
		if (i + 1) % (n / 10).max(1) == 0 {
			let list = p.layer_list(doc);
			let bytes = fx_protocol::encode_json(&EngineToUi::Layers {
				doc,
				revision: 0,
				layers: list.clone(),
				seq: 0,
			})
			.len();
			layers_bytes.push((list.len(), bytes));
			println!("  … {} layers, {:.1} s", i + 1, build.elapsed().as_secs_f64());
		}
	}
	p.record_series("add layer + select + fill + deselect (per layer)", &per_layer);
	let list = p.layer_list(doc);
	assert_eq!(list.len(), n + 1, "every layer arrived (plus Background)");
	let sent = p.h.layer_frames.lock().unwrap().last().copied();
	if let Some(&(count, bytes)) = layers_bytes.last() {
		p.record(format!("full layers message at {count} layers"), bytes as f64 / 1024.0, "KiB");
	}
	if let Some(frame) = sent {
		p.record(
			format!("last list frame of the build ({})", if frame.patch { "patch" } else { "full" }),
			frame.bytes as f64 / 1024.0,
			"KiB",
		);
	}
	p.drain();
	p.record_settle("view settles after the build", Instant::now() - Duration::from_millis(1));
	p.record_memory("after the build");

	// The Layers panel asks for every pixel layer's thumbnail when the list
	// arrives (64 px, layers-panel.js); from then on the engine keeps them
	// fresh.
	let ids: Vec<LayerId> = list.iter().map(|l| l.id).collect();
	p.drain();
	let since = Instant::now();
	p.ui(UiToEngine::RequestThumbnails {
		doc,
		layers: ids.clone(),
		size: 64,
	});
	let mut got = std::collections::HashSet::new();
	let want = ids.len();
	let deadline = Instant::now() + Duration::from_secs(120);
	while got.len() < want && Instant::now() < deadline {
		p.pump();
		while let Some(m) = p.inbox.pop_front() {
			if let EngineToUi::Thumbnail { layer, .. } = m {
				got.insert(layer);
			}
		}
		std::thread::sleep(Duration::from_millis(2));
	}
	p.record_time("thumbnails of every layer: all arrived", since.elapsed());
	p.record("thumbnails received", got.len() as f64, "thumbs");
	if got.len() < want {
		println!("  !! {} of {want} thumbnails never arrived", want - got.len());
	}

	// Toggling a layer near the bottom recomposites everything above it.
	let bottom = ids[ids.len() - 2];
	let middle = ids[ids.len() / 2];
	for (name, layer) in [("bottom", bottom), ("middle", middle)] {
		let since = Instant::now();
		let t = p.step(doc, vec![visible(layer, false)]);
		p.record_time(format!("hide {name} layer: command"), t);
		p.record_settle(&format!("hide {name} layer: view settled"), since);
		let since = Instant::now();
		p.step(doc, vec![visible(layer, true)]);
		p.record_settle(&format!("show {name} layer: view settled"), since);
	}

	// Opacity drag: 20 changes in a row, like a slider.
	let since = Instant::now();
	let mut drags = Vec::new();
	for k in 0..20 {
		drags.push(p.step(
			doc,
			vec![Command::SetLayerProps {
				layer: LayerRef::Id(bottom),
				props: LayerPropsPatch {
					opacity: Some(1.0 - k as f32 / 40.0),
					..Default::default()
				},
			}],
		));
	}
	p.record_series("opacity step of the bottom layer", &drags);
	p.record_settle("opacity drag: view settled after the last step", since);

	// 100 % in the middle, then back to fit.
	let since = Instant::now();
	p.ui(UiToEngine::SetZoom { doc, zoom: 1.0 });
	p.record_settle("zoom to 100 %: view settled", since);
	let since = Instant::now();
	let fit = p.fit_zoom.unwrap_or(0.2);
	p.ui(UiToEngine::SetZoom { doc, zoom: fit });
	p.record_settle("zoom back to fit: view settled", since);

	// Undo / redo (every thumbnail is wanted now, as in the app). Counts the
	// thumbnails re-rendered meanwhile; undo refreshes all of them.
	p.drain();
	let thumbs = p.thumbnails;
	let mut undo = Vec::new();
	let since = Instant::now();
	for _ in 0..20 {
		p.drain();
		let t = Instant::now();
		p.ui(UiToEngine::Undo { doc });
		p.until("undo", CEILING, |m| matches!(m, EngineToUi::History { doc: d, .. } if *d == doc).then_some(()));
		undo.push(t.elapsed());
	}
	p.record_series("undo", &undo);
	let mut redo = Vec::new();
	for _ in 0..20 {
		p.drain();
		let t = Instant::now();
		p.ui(UiToEngine::Redo { doc });
		p.until("redo", CEILING, |m| matches!(m, EngineToUi::History { doc: d, .. } if *d == doc).then_some(()));
		redo.push(t.elapsed());
	}
	p.record_series("redo", &redo);
	p.record_settle("undo/redo: view settled", since);
	std::thread::sleep(Duration::from_secs(2));
	p.drain();
	let rerendered = p.thumbnails - thumbs;
	p.record("thumbnails re-rendered by 20 undo + 20 redo", rerendered as f64, "thumbs");

	// Save, close, reopen.
	let fxd = dir.0.join("layers.fxd");
	let t = p.save_as(doc, &fxd);
	p.record_time("save as .fxd", t);
	p.record("saved file size", file_mib(&fxd), "MiB");
	p.close(doc);
	p.drain();
	let since = Instant::now();
	p.h.engine.send(EngineInput::Open(vec![fxd.clone()]));
	let doc = p.opened();
	p.record_time("reopen: document opened", since.elapsed());
	p.record_settle("reopen: view settled", since);
	std::thread::sleep(Duration::from_millis(300));
	let reopened = p.layer_list(doc);
	assert_eq!(reopened.len(), n + 1, "the reopened file has every layer");
	p.record_memory("after the reopen");

	// Merge Visible collapses the stack.
	let since = Instant::now();
	let t = p.history_action(doc, "layer:merge-visible");
	p.record_time("merge visible", t);
	p.record_settle("merge visible: view settled", since);
	std::thread::sleep(Duration::from_millis(300));
	let merged = p.layer_list(doc).len();
	p.record("layers after merge visible", merged as f64, "layers");
	let t = p.history_action(doc, "hist:undo");
	p.record_time("undo merge visible", t);
	p.finish();
}

/// Groups nested `depth` deep around one filled pixel layer (each level made
/// with Layer ▸ Group Layers), for each depth of `FOTOX_STRESS_DEPTHS`: draw,
/// hide and show, save and reopen (the nesting must survive), undo. The
/// render and save paths walk the tree recursively, and the `.fxd` manifest
/// is JSON — deep documents find their limits.
#[test]
#[ignore = "stress: run with --ignored --nocapture"]
fn deeply_nested_groups() {
	let depths: Vec<usize> = std::env::var("FOTOX_STRESS_DEPTHS")
		.unwrap_or_else(|_| "10,11".into())
		.split(',')
		.filter_map(|s| s.trim().parse().ok())
		.collect();
	let (w, h) = (2000, 1500);
	let dir = TempDir::new("nesting");
	let Some(mut p) = Probe::start("deeply_nested_groups", &dir.0) else { return };
	let mut rng = Rng(11);
	for depth in depths {
		p.document = format!("{w}x{h} 8-bit, groups {depth} deep");
		println!("deeply_nested_groups: {}", p.document);
		let doc = p.new_document(w, h, 8);
		let mut commands = vec![Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		}];
		commands.extend(rect_fill(&mut rng, (w, h)));
		p.step(doc, commands);
		let pixel = p.layer_list(doc).iter().find(|l| l.name != "Background").map(|l| l.id).expect("the new layer");

		// Groups nest at most MAX_GROUP_NESTING deep: past it, Group Layers is
		// refused with an error and the document keeps its depth.
		let allowed = depth.min(fx_core::MAX_GROUP_NESTING);
		let mut per_level = Vec::new();
		for _ in 0..allowed {
			per_level.push(p.step(
				doc,
				vec![Command::GroupLayers {
					layers: vec![LayerRef::Id(pixel)],
					name: None,
				}],
			));
		}
		if depth > allowed {
			p.drain();
			p.ui(UiToEngine::Command {
				doc,
				command: Command::GroupLayers {
					layers: vec![LayerRef::Id(pixel)],
					name: None,
				},
			});
			let refused = p.until("the refusal", CEILING, |m| match m {
				EngineToUi::Error { text } => Some(text.clone()),
				EngineToUi::History { .. } => Some(String::new()),
				_ => None,
			});
			assert!(refused.contains("at most"), "group {} levels deep refused: {refused:?}", allowed + 1);
		}
		let depth = allowed;
		p.record_series("group one level deeper", &per_level);
		std::thread::sleep(Duration::from_millis(200));
		let list = p.layer_list(doc);
		let deepest = list.iter().map(|l| l.depth).max().unwrap_or(0);
		p.record("deepest layer depth reported", f64::from(deepest), "levels");
		assert_eq!(deepest as usize, depth, "the pixel layer sits {depth} groups deep");

		p.drain();
		p.record_settle("view settled", Instant::now() - Duration::from_millis(1));
		let since = Instant::now();
		p.step(doc, vec![visible(pixel, false)]);
		p.record_settle("hide the deepest layer: view settled", since);
		p.step(doc, vec![visible(pixel, true)]);

		let fxd = dir.0.join(format!("nested-{depth}.fxd"));
		let t = p.save_as(doc, &fxd);
		p.record_time("save as .fxd", t);
		p.close(doc);
		let since = Instant::now();
		p.h.engine.send(EngineInput::Open(vec![fxd.clone()]));
		// A manifest the reader refuses answers with an error: say which depth.
		let doc = p.until(&format!("the reopened {depth}-deep file"), CEILING, |m| match m {
			EngineToUi::DocumentOpened { info } => Some(Ok(info.doc)),
			EngineToUi::Error { text } => Some(Err(text.clone())),
			_ => None,
		});
		let doc = match doc {
			Ok(doc) => doc,
			Err(text) => panic!("a {depth}-deep document saved but does not reopen: {text}"),
		};
		p.record_time("reopen", since.elapsed());
		std::thread::sleep(Duration::from_millis(300));
		let deepest = p.layer_list(doc).iter().map(|l| l.depth).max().unwrap_or(0);
		assert_eq!(deepest as usize, depth, "the reopened file keeps the nesting");
		p.close(doc);
	}
	p.finish();
}

/// One `FOTOX_STRESS_SIZE`² 16-bit document with a full-canvas gradient
/// layer (every tile holds real, dithered pixels: 8 bytes a pixel), then 50
/// small layers on top: fill time, view at fit and at 100 %, panning,
/// hide/show, undo, save and reopen, and the memory at each point — the
/// tile store's budgets are what keep this within the machine.
#[test]
#[ignore = "stress: many minutes and ~10 GB of scratch; run with --ignored --nocapture"]
fn huge_canvas() {
	let side: u32 = env("FOTOX_STRESS_SIZE", 30_000);
	let dir = TempDir::new("huge");
	let Some(mut p) = Probe::start("huge_canvas", &dir.0) else { return };
	p.document = format!("{side}x{side} 16-bit");
	println!("huge_canvas: {}", p.document);

	let since = Instant::now();
	let doc = p.new_document(side, side, 16);
	p.record_time("new document", since.elapsed());
	p.record_settle("first frame", since);

	let since = Instant::now();
	let t = p.step(
		doc,
		vec![
			Command::AddLayer {
				layer: NewLayer::Pixel,
				name: Some("gradient".into()),
			},
			Command::FillGradient {
				layer: LayerRef::Active,
				fill: fx_core::gradient::GradientFill {
					gradient: fx_core::gradient::Gradient::two([0.9, 0.2, 0.1], [0.1, 0.3, 0.9]),
					kind: Default::default(),
					start: (0.0, 0.0),
					end: (f64::from(side), f64::from(side) * 0.7),
					reverse: false,
					dither: true,
					transparency: true,
					mirror: false,
				},
				mode: BlendMode::Normal,
				opacity: 1.0,
			},
		],
	);
	p.record_time("full-canvas gradient fill", t);
	p.record_settle("gradient: view settled", since);
	p.record_memory("after the gradient");

	let mut rng = Rng(5);
	let mut per_layer = Vec::new();
	for _ in 0..50 {
		let mut commands = vec![Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		}];
		commands.extend(rect_fill(&mut rng, (side, side)));
		per_layer.push(p.step(doc, commands));
	}
	p.record_series("add layer + select + fill + deselect (per layer)", &per_layer);
	p.drain();
	p.record_settle("50 layers: view settled", Instant::now() - Duration::from_millis(1));

	let since = Instant::now();
	p.ui(UiToEngine::SetZoom { doc, zoom: 1.0 });
	p.record_settle("zoom to 100 %: view settled", since);
	// Pan: ten wheel notches, each waited for.
	let mut pans = Vec::new();
	for _ in 0..10 {
		p.drain();
		let since = Instant::now();
		p.h.engine.send(EngineInput::Wheel {
			x: 400.0,
			y: 300.0,
			dx: 0.0,
			dy: -5.0,
			modifiers: Default::default(),
		});
		if let Some(d) = p.settle(since) {
			pans.push(d);
		}
	}
	p.record_series("pan at 100 % (5 notches): view settled", &pans);
	let since = Instant::now();
	let fit = p.fit_zoom.unwrap_or(0.02);
	p.ui(UiToEngine::SetZoom { doc, zoom: fit });
	p.record_settle("back to fit: view settled", since);

	let gradient = p
		.layer_list(doc)
		.iter()
		.find(|l| l.name == "gradient")
		.map(|l| l.id)
		.expect("the gradient layer");
	let since = Instant::now();
	p.step(doc, vec![visible(gradient, false)]);
	p.record_settle("hide the gradient layer: view settled", since);
	let since = Instant::now();
	p.step(doc, vec![visible(gradient, true)]);
	p.record_settle("show the gradient layer: view settled", since);

	p.record_memory("before saving");
	let fxd = dir.0.join("huge.fxd");
	let t = p.save_as(doc, &fxd);
	p.record_time("save as .fxd", t);
	p.record("saved file size", file_mib(&fxd), "MiB");
	let before_close = p.status.map_or(0, |m| m.hot_bytes + m.warm_bytes);
	p.close(doc);
	// Closing the only document gives its tiles back (the render thread held
	// the last frame's snapshot, 2026-09-27).
	std::thread::sleep(Duration::from_secs(2));
	p.record_memory("after closing");
	let after_close = p.status.map_or(0, |m| m.hot_bytes + m.warm_bytes);
	assert!(
		after_close < before_close / 10,
		"closing the only document freed its tiles: {} MiB before, {} MiB after",
		before_close >> 20,
		after_close >> 20
	);
	let since = Instant::now();
	p.h.engine.send(EngineInput::Open(vec![fxd.clone()]));
	let doc = p.opened();
	p.record_time("reopen: document opened", since.elapsed());
	p.record_settle("reopen: view settled", since);
	let since = Instant::now();
	p.ui(UiToEngine::SetZoom { doc, zoom: 1.0 });
	p.record_settle("reopened, zoom to 100 %: view settled", since);
	p.record_memory("after the reopen");
	p.finish();
}

/// `FOTOX_STRESS_DOCS` documents of 8000 × 6000, 16-bit, each with a
/// full-canvas gradient layer: opening, switching tabs, closing — and
/// whether closing gives the memory back.
#[test]
#[ignore = "stress: minutes; run with --ignored --nocapture"]
fn many_documents() {
	let count: usize = env("FOTOX_STRESS_DOCS", 10);
	let (w, h) = (8000, 6000);
	let dir = TempDir::new("docs");
	let Some(mut p) = Probe::start("many_documents", &dir.0) else { return };
	p.document = format!("{count} × {w}x{h} 16-bit");
	println!("many_documents: {}", p.document);
	p.record_memory("empty");

	let mut docs = Vec::new();
	let mut per_doc = Vec::new();
	for i in 0..count {
		let since = Instant::now();
		let doc = p.new_document(w, h, 16);
		p.step(
			doc,
			vec![
				Command::AddLayer {
					layer: NewLayer::Pixel,
					name: None,
				},
				Command::FillGradient {
					layer: LayerRef::Active,
					fill: fx_core::gradient::GradientFill {
						gradient: fx_core::gradient::Gradient::two([0.1 * i as f32 % 1.0, 0.5, 0.2], [0.9, 0.1, 0.5]),
						kind: Default::default(),
						start: (0.0, 0.0),
						end: (f64::from(w), f64::from(h)),
						reverse: false,
						dither: true,
						transparency: true,
						mirror: false,
					},
					mode: BlendMode::Normal,
					opacity: 1.0,
				},
			],
		);
		per_doc.push(since.elapsed());
		docs.push(doc);
	}
	p.record_series("new document + gradient layer", &per_doc);
	p.record_memory(&format!("{count} documents open"));

	let mut switches = Vec::new();
	for round in 0..2 {
		for &doc in &docs {
			p.drain();
			let since = Instant::now();
			p.ui(UiToEngine::ActivateDocument { doc });
			if let Some(d) = p.settle(since) {
				switches.push(d);
			}
			let _ = round;
		}
	}
	p.record_series("switch document: view settled", &switches);

	// Close all but the first; unsaved, so answer Don't Save.
	for &doc in &docs[1..] {
		p.drain();
		p.ui(UiToEngine::CloseDocument { doc });
		p.until("the save prompt", CEILING, |m| match m {
			EngineToUi::CloseDirtyDocument { doc: d, .. } if *d == doc => Some(()),
			_ => None,
		});
		p.ui(UiToEngine::CloseDocumentAnswer {
			doc,
			answer: fx_protocol::CloseAnswer::DontSave,
		});
		p.until("the document closed", CEILING, |m| match m {
			EngineToUi::DocumentClosed { doc: d } if *d == doc => Some(()),
			_ => None,
		});
	}
	std::thread::sleep(Duration::from_secs(2));
	p.record_memory(&format!("{} closed, 1 open", count - 1));
	p.finish();
}

/// Where one edit's time goes as the layer count grows, without the engine
/// around it: the command through `History`, the layer list the engine sends
/// after every edit, its JSON encoding, and decoding it again (what the UI's
/// `JSON.parse` does). A cost that grows with the count shows here.
#[test]
#[ignore = "stress: seconds; run with --ignored --nocapture"]
fn per_edit_costs_by_layer_count() {
	use fx_core::{BitDepth, ColorProfile, CommandContext, Document, DocumentColor, History};
	let dir = TempDir::new("per-edit");
	let mut config = fx_tiles::TileStoreConfig::for_tests(dir.0.clone());
	config.hot_budget = 2 << 30;
	let store = fx_tiles::TileStore::new(config).unwrap();
	let (w, h) = (4000, 3000);
	println!("per_edit_costs_by_layer_count: {w}x{h} 8-bit");
	let mut doc = Document::new(
		w,
		h,
		DocumentColor {
			depth: BitDepth::U8,
			profile: ColorProfile::Srgb,
		},
		72.0,
	);
	let mut history = History::default();
	let mut rng = Rng(3);
	let time = |f: &mut dyn FnMut()| {
		let runs = 5;
		let start = Instant::now();
		for _ in 0..runs {
			f();
		}
		start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs)
	};
	let ops = fx_engine::ops::EngineOps::default();
	let mut ctx = CommandContext {
		tiles: &store,
		ops: Some(&ops),
	};
	for target in [100usize, 1000, 2000, 5000] {
		while doc.layers.len() < target {
			let mut commands = vec![Command::AddLayer {
				layer: NewLayer::Pixel,
				name: None,
			}];
			commands.extend(rect_fill(&mut rng, (w, h)));
			for command in commands {
				history.execute(&mut doc, command, &mut ctx).expect("command");
			}
		}
		let mut step = || {
			let mut commands = vec![Command::AddLayer {
				layer: NewLayer::Pixel,
				name: None,
			}];
			commands.extend(rect_fill(&mut rng, (w, h)));
			for command in commands {
				history.execute(&mut doc, command, &mut ctx).expect("command");
			}
		};
		let execute = time(&mut step);
		let infos = time(&mut || {
			std::hint::black_box(fx_engine::layers::layer_infos(&doc));
		});
		let list = fx_engine::layers::layer_infos(&doc);
		let message = EngineToUi::Layers {
			doc: DocId(1),
			revision: 0,
			layers: list,
			seq: 0,
		};
		let encode = time(&mut || {
			std::hint::black_box(fx_protocol::encode_json(&message));
		});
		let bytes = fx_protocol::encode_json(&message);
		let decode = time(&mut || {
			std::hint::black_box(fx_protocol::decode::<EngineToUi>(&bytes).unwrap());
		});
		let clone = time(&mut || {
			std::hint::black_box(doc.clone());
		});
		// A property edit now sends a patch: diff against the list sent before,
		// encode only the changed row, and the UI applies it.
		let before = fx_engine::layers::layer_infos(&doc);
		let mut after = before.clone();
		let middle = after.len() / 2;
		after[middle].visible = false;
		let diff = time(&mut || {
			std::hint::black_box(fx_protocol::layers_patch(&before, &after));
		});
		let changed = fx_protocol::layers_patch(&before, &after).expect("same tree");
		let patch = fx_protocol::encode_json(&EngineToUi::LayersPatch {
			doc: DocId(1),
			revision: 0,
			seq: 2,
			base: 1,
			changed,
		});
		let apply = time(&mut || {
			let (message, _) = fx_protocol::decode::<EngineToUi>(&patch).unwrap();
			if let EngineToUi::LayersPatch { changed, .. } = message {
				let mut list = before.clone();
				fx_protocol::apply_layers_patch(&mut list, &changed);
				std::hint::black_box(list);
			}
		});
		println!(
			"  {:>5} layers: add+select+fill+deselect {execute:6.2} ms | layer_infos {infos:6.2} ms | encode {encode:6.2} ms | decode {decode:6.2} ms | doc clone {clone:6.3} ms | message {:6.0} KiB",
			doc.layers.len(),
			bytes.len() as f64 / 1024.0
		);
		println!(
			"         property edit as a patch: diff {diff:6.2} ms | patch {:5.2} KiB | decode + apply {apply:6.2} ms",
			patch.len() as f64 / 1024.0
		);
		// A fresh save of the whole document, as Save As does.
		let path = dir.0.join(format!("per-edit-{target}.fxd"));
		let start = Instant::now();
		let saved = fx_io::fxd::save(
			fx_io::fxd::SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			fx_io::fxd::SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.expect("save");
		println!(
			"         save as: {:.2} s ({} tiles, {:.1} MiB)",
			start.elapsed().as_secs_f64(),
			saved.report.tiles_written,
			file_mib(&path)
		);
	}
}
