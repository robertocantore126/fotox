//! Audit 2026-10-01 — large documents through a running engine (engine +
//! render threads on the GPU, the UI's messages). Unlike `scale.rs`, the
//! pixel content is noise (incompressible, every tile real data), and each
//! step measures input → delivered frame, frame intervals and memory.
//!
//!   APPDATA=<scratch> FOTOX_AUDIT_DIR=E:\fotox-audit-run \
//!   cargo test --release -p fx-engine --test audit_scale -- --ignored --nocapture --test-threads 1
//!
//! Knobs: `FOTOX_AUDIT_SIDES` (e.g. `4096,8192`), `FOTOX_AUDIT_DEPTHS`
//! (`8,16`), `FOTOX_AUDIT_LAYERS` (`100,1000`), `FOTOX_AUDIT_EXPORT=1`
//! (validate reopen by exporting before and after: writes a full-size TIFF
//! twice).
//!
//! "Frame" = the render thread delivered a viewport texture (what the shell
//! presents next); the present itself is not measured.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Harness, Seen, gpu};
use fx_core::command::{LayerPropsPatch, MaskFill, NewLayer};
use fx_core::{Adjustment, BlendMode, Command, Filter, FilterParams, LayerRef, Mapping, SelectMode, SelectionShape};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, LayerInfo, MemoryStats, UiToEngine};

const QUIET: Duration = Duration::from_millis(600);
const CEILING: Duration = Duration::from_secs(900);

fn list<T: std::str::FromStr>(name: &str, default: &str) -> Vec<T> {
	std::env::var(name)
		.unwrap_or_else(|_| default.into())
		.split(',')
		.filter_map(|s| s.trim().parse().ok())
		.collect()
}

struct TempDir(PathBuf);

impl TempDir {
	fn new(name: &str) -> Self {
		let root = std::env::var_os("FOTOX_AUDIT_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
		let dir = root.join(format!("fx-audit-scale-{name}-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		Self(dir)
	}
}

impl Drop for TempDir {
	fn drop(&mut self) {
		if std::env::var_os("FOTOX_AUDIT_KEEP").is_none() {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}
}

struct Probe {
	h: Harness,
	tag: String,
	inbox: std::collections::VecDeque<EngineToUi>,
	status: Option<MemoryStats>,
	layers: Option<(DocId, Vec<LayerInfo>)>,
	frames: Vec<Instant>,
	frames_seen: usize,
}

impl Probe {
	fn start(dir: &Path) -> Option<Self> {
		let (device, queue) = gpu()?;
		Some(Self {
			h: Harness::start(device, queue, dir),
			tag: String::new(),
			inbox: Default::default(),
			status: None,
			layers: None,
			frames: Vec::new(),
			frames_seen: 0,
		})
	}

	fn pump(&mut self) {
		let seen = std::mem::take(&mut *self.h.seen.lock().unwrap());
		for item in seen {
			match item {
				Seen::Ui(message) => {
					match &message {
						EngineToUi::Status { memory, .. } => self.status = Some(memory.clone()),
						EngineToUi::Layers { doc, layers, .. } => self.layers = Some((*doc, layers.clone())),
						_ => {}
					}
					self.inbox.push_back(message);
				}
				Seen::Frame => {
					self.frames_seen += 1;
					self.frames.push(Instant::now());
				}
				_ => {}
			}
		}
	}

	fn drain(&mut self) {
		self.pump();
		self.inbox.clear();
	}

	fn until<T>(&mut self, what: &str, timeout: Duration, mut pick: impl FnMut(&EngineToUi) -> Option<T>) -> T {
		let deadline = Instant::now() + timeout;
		loop {
			self.pump();
			while let Some(message) = self.inbox.pop_front() {
				if let Some(found) = pick(&message) {
					return found;
				}
				match &message {
					EngineToUi::Error { text } => panic!("{}: engine error while waiting for {what}: {text}", self.tag),
					EngineToUi::Toast { text } if text != "Engine connected" && !text.starts_with("Saved") => {
						println!("    (toast while waiting for {what}: {text})");
					}
					_ => {}
				}
			}
			assert!(Instant::now() < deadline, "{}: timed out after {timeout:?} waiting for {what}", self.tag);
			std::thread::sleep(Duration::from_millis(1));
		}
	}

	fn ui(&self, message: UiToEngine) {
		self.h.ui(message);
	}

	fn action(&self, id: &str, args: serde_json::Value) {
		self.ui(UiToEngine::Action { id: id.into(), args });
	}

	/// Send `commands`; wait for one History per command. Returns the time.
	fn step(&mut self, doc: DocId, commands: Vec<Command>) -> Duration {
		self.drain();
		let n = commands.len();
		let start = Instant::now();
		for command in commands {
			self.ui(UiToEngine::Command { doc, command });
		}
		for i in 0..n {
			self.until(&format!("history {}/{n}", i + 1), CEILING, |m| match m {
				EngineToUi::History { doc: d, .. } if *d == doc => Some(()),
				_ => None,
			});
		}
		start.elapsed()
	}

	/// A job command (filter, gradient): History arrives when the job ends.
	fn job(&mut self, doc: DocId, command: Command) -> Duration {
		self.step(doc, vec![command])
	}

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

	/// Time from `since` to the last frame of the burst it started; `None`
	/// when no frame came within 3 s.
	fn settle(&mut self, since: Instant) -> Option<Duration> {
		let called = Instant::now();
		loop {
			self.pump();
			self.inbox.retain(|m| !matches!(m, EngineToUi::Status { .. }));
			let last = self.frames.last().copied();
			let now = Instant::now();
			match last {
				Some(at) if at > since && now - at >= QUIET && now - called >= QUIET => return Some(at - since),
				Some(at) if at > since => {}
				_ if now - called > Duration::from_secs(3) => return None,
				_ => {}
			}
			if now - since > Duration::from_secs(300) {
				println!("    !! still redrawing after 300 s");
				return Some(now - since);
			}
			std::thread::sleep(Duration::from_millis(2));
		}
	}

	/// Input → the first delivered frame whose pixels differ from `before`
	/// (GPU readback of each new frame; the readback adds a few ms, so this
	/// is an upper bound). `None` if nothing changed within `timeout`.
	fn changed_frame_after(&mut self, since: Instant, before: &[[u8; 4]], timeout: Duration) -> Option<Duration> {
		let deadline = Instant::now() + timeout;
		let mut seen = self.frames_seen;
		loop {
			self.pump();
			if self.frames_seen != seen {
				seen = self.frames_seen;
				let at = *self.frames.last().unwrap();
				if let Some((px, _)) = self.h.frame_pixels()
					&& px.as_slice() != before
				{
					return Some(at - since);
				}
			}
			if Instant::now() > deadline {
				return None;
			}
			std::thread::sleep(Duration::from_micros(300));
		}
	}

	fn frame_now(&mut self) -> Vec<[u8; 4]> {
		self.pump();
		self.h.frame_pixels().map(|(px, _)| px).unwrap_or_default()
	}

	/// The first frame after `since` (input → frame latency).
	fn first_frame_after(&mut self, since: Instant, timeout: Duration) -> Option<Duration> {
		let deadline = Instant::now() + timeout;
		loop {
			self.pump();
			if let Some(at) = self.frames.iter().find(|t| **t > since) {
				return Some(*at - since);
			}
			if Instant::now() > deadline {
				return None;
			}
			std::thread::sleep(Duration::from_micros(300));
		}
	}

	fn new_document(&mut self, w: u32, h: u32, depth: u8) -> DocId {
		self.drain();
		self.action("doc:new", serde_json::json!({ "width": w, "height": h, "ppi": 72, "depth": depth, "background": "white" }));
		self.opened()
	}

	fn opened(&mut self) -> DocId {
		let doc = self.until("a document", CEILING, |m| match m {
			EngineToUi::DocumentOpened { info } => Some(info.doc),
			_ => None,
		});
		self.until("its view", Duration::from_secs(60), |m| match m {
			EngineToUi::View { doc: d, .. } if *d == doc => Some(()),
			_ => None,
		});
		doc
	}

	fn layer_list(&mut self, doc: DocId) -> Vec<LayerInfo> {
		self.pump();
		match &self.layers {
			Some((d, l)) if *d == doc => l.clone(),
			_ => Vec::new(),
		}
	}

	fn save_as(&mut self, doc: DocId, path: &Path) -> Duration {
		self.drain();
		let start = Instant::now();
		self.h.engine.send(EngineInput::SaveAs { doc, path: path.to_owned() });
		self.until("saved", CEILING, |m| match m {
			EngineToUi::DocumentChanged { info } if info.doc == doc && !info.dirty => Some(()),
			_ => None,
		});
		start.elapsed()
	}

	fn save(&mut self, doc: DocId) -> Duration {
		self.drain();
		let start = Instant::now();
		self.h.engine.send(EngineInput::Save { doc });
		self.until("saved", CEILING, |m| match m {
			EngineToUi::DocumentChanged { info } if info.doc == doc && !info.dirty => Some(()),
			_ => None,
		});
		start.elapsed()
	}

	fn export(&mut self, doc: DocId, path: &Path) -> Duration {
		self.drain();
		let start = Instant::now();
		self.h.engine.send(EngineInput::Export {
			doc,
			path: path.to_owned(),
			choice: None,
		});
		while !path.exists() {
			self.pump();
			if let Some(EngineToUi::Error { text }) = self.inbox.iter().find(|m| matches!(m, EngineToUi::Error { .. })) {
				panic!("export failed: {text}");
			}
			assert!(start.elapsed() < CEILING);
			std::thread::sleep(Duration::from_millis(20));
		}
		start.elapsed()
	}

	fn close(&mut self, doc: DocId) {
		self.drain();
		self.ui(UiToEngine::CloseDocument { doc });
		let dirty = self.until("closed", CEILING, |m| match m {
			EngineToUi::DocumentClosed { doc: d } if *d == doc => Some(false),
			EngineToUi::CloseDirtyDocument { doc: d, .. } if *d == doc => Some(true),
			_ => None,
		});
		if dirty {
			self.ui(UiToEngine::CloseDocumentAnswer {
				doc,
				answer: fx_protocol::CloseAnswer::DontSave,
			});
			self.until("closed", CEILING, |m| match m {
				EngineToUi::DocumentClosed { doc: d } if *d == doc => Some(()),
				_ => None,
			});
		}
	}

	fn memory(&mut self, label: &str) {
		self.status = None;
		let deadline = Instant::now() + Duration::from_secs(3);
		while self.status.is_none() && Instant::now() < deadline {
			self.pump();
			std::thread::sleep(Duration::from_millis(20));
		}
		let mib = |b: u64| b >> 20;
		let s = self.status.clone().unwrap_or_default();
		let p = process_counters().unwrap_or_default();
		println!(
			"AUDIT {} mem {label}: tiles hot {} MiB, warm {} MiB, scratch {} MiB, gpu reserved {} MiB | process WS {} MiB, private {} MiB, peak WS {} MiB, I/O read {} MiB write {} MiB",
			self.tag,
			mib(s.hot_bytes),
			mib(s.warm_bytes),
			mib(s.scratch_bytes),
			mib(s.gpu_bytes),
			mib(p.0),
			mib(p.1),
			mib(p.2),
			mib(p.3),
			mib(p.4)
		);
	}

	fn rec(&self, what: &str, d: Duration) {
		println!("AUDIT {} {what}: {:.1} ms", self.tag, d.as_secs_f64() * 1000.0);
	}

	fn rec_settle(&mut self, what: &str, since: Instant) {
		match self.settle(since) {
			Some(d) => self.rec(&format!("{what} (view settled)"), d),
			None => println!("AUDIT {} {what}: no frame", self.tag),
		}
	}

	fn series(&self, what: &str, samples: &[Duration]) {
		if samples.is_empty() {
			println!("AUDIT {} {what}: no samples", self.tag);
			return;
		}
		let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
		ms.sort_by(f64::total_cmp);
		let q = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
		println!(
			"AUDIT {} {what} (n={}): p50 {:.1} ms, p95 {:.1}, p99 {:.1}, max {:.1}",
			self.tag,
			ms.len(),
			q(0.5),
			q(0.95),
			q(0.99),
			q(1.0)
		);
	}

	/// Frame intervals between `from` and now.
	fn frame_intervals(&mut self, from: Instant) -> Vec<Duration> {
		self.pump();
		let f: Vec<Instant> = self.frames.iter().copied().filter(|t| *t >= from).collect();
		f.windows(2).map(|w| w[1] - w[0]).collect()
	}

	fn zoom(&mut self, doc: DocId, zoom: f64) {
		self.ui(UiToEngine::SetZoom { doc, zoom });
	}
}

/// Working set, private bytes, peak working set, I/O read and write bytes.
fn process_counters() -> Option<(u64, u64, u64, u64, u64)> {
	let script = format!(
		"$p = Get-Process -Id {0}; $c = Get-CimInstance Win32_Process -Filter 'ProcessId={0}'; \"$($p.WorkingSet64) $($p.PrivateMemorySize64) $($p.PeakWorkingSet64) $($c.ReadTransferCount) $($c.WriteTransferCount)\"",
		std::process::id()
	);
	let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().ok()?;
	let text = String::from_utf8_lossy(&out.stdout);
	let mut it = text.split_whitespace().map(|v| v.parse::<u64>().ok());
	Some((it.next()??, it.next()??, it.next()??, it.next()??, it.next()??))
}

fn pointer(kind: PointerKind, x: f64, y: f64, buttons: u8, t: u64) -> PointerInput {
	PointerInput {
		kind,
		x,
		y,
		pressure: 1.0,
		tilt_x: 0.0,
		tilt_y: 0.0,
		buttons,
		modifiers: Modifiers::default(),
		time_us: t,
	}
}

fn file_hash(path: &Path) -> u64 {
	use std::io::Read;
	let mut f = std::fs::File::open(path).unwrap();
	let mut buf = vec![0u8; 8 << 20];
	let mut h: u64 = 0xcbf2_9ce4_8422_2325;
	loop {
		let n = f.read(&mut buf).unwrap();
		if n == 0 {
			break;
		}
		// The embedded ICC profile carries its creation time (12 bytes before
		// the `acsp` signature): mask it, it differs on every export.
		if let Some(at) = buf[..n].windows(4).position(|w| w == b"acsp")
			&& at >= 12
		{
			buf[at - 12..at].fill(0);
		}
		for chunk in buf[..n].chunks(8) {
			let mut v = [0u8; 8];
			v[..chunk.len()].copy_from_slice(chunk);
			h = (h ^ u64::from_le_bytes(v)).wrapping_mul(0x100_0000_01b3);
		}
	}
	h
}

fn mib(path: &Path) -> u64 {
	std::fs::metadata(path).map_or(0, |m| m.len() >> 20)
}

/// Brush stroke in viewport coordinates; returns per-move input → frame
/// latencies and the Up → History time.
fn stroke(p: &mut Probe, doc: DocId, moves: usize) -> (Vec<Duration>, Duration) {
	p.drain();
	p.action("tool:brush", serde_json::Value::Null);
	p.ui(UiToEngine::ToolOptions {
		tool: "brush".into(),
		options: serde_json::json!({ "Size": 60, "Opacity": 100, "Smoothing": 0, "Hardness": 80 }),
	});
	std::thread::sleep(Duration::from_millis(200));
	p.drain();
	let mut latencies = Vec::new();
	p.h.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 200.0, 300.0, 1, 0)));
	for i in 0..moves {
		let x = 200.0 + (i as f64) * 400.0 / moves as f64;
		let y = 300.0 + 80.0 * ((i as f64) / 6.0).sin();
		let before = if i % 5 == 0 { Some(p.frame_now()) } else { None };
		let t = Instant::now();
		p.h.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, x, y, 1, (i as u64 + 1) * 8000)));
		if let Some(before) = before
			&& let Some(d) = p.changed_frame_after(t, &before, Duration::from_secs(10))
		{
			latencies.push(d);
		}
		// ~120 Hz pen.
		let spent = t.elapsed();
		if spent < Duration::from_millis(8) {
			std::thread::sleep(Duration::from_millis(8) - spent);
		}
	}
	let up = Instant::now();
	p.h.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 600.0, 300.0, 0, (moves as u64 + 2) * 8000)));
	p.until("stroke history", CEILING, |m| match m {
		EngineToUi::History { doc: d, .. } if *d == doc => Some(()),
		_ => None,
	});
	(latencies, up.elapsed())
}

/// Wheel-pan `notches` times; per-notch input → frame latencies.
fn pan(p: &mut Probe, notches: usize) -> Vec<Duration> {
	let mut out = Vec::new();
	for i in 0..notches {
		let before = p.frame_now();
		let t = Instant::now();
		p.h.engine.send(EngineInput::Wheel {
			x: 400.0,
			y: 300.0,
			dx: if i % 2 == 0 { 0.0 } else { 1.0 },
			dy: if i % 2 == 0 { -1.0 } else { 0.0 },
			modifiers: Modifiers::default(),
		});
		if let Some(d) = p.changed_frame_after(t, &before, Duration::from_secs(10)) {
			out.push(d);
		}
		std::thread::sleep(Duration::from_millis(16));
	}
	out
}

// ------------------------------------------------------- canvas scaling

#[test]
#[ignore = "audit: GBs of memory and scratch"]
fn canvas_size_scaling() {
	let sides: Vec<u32> = list("FOTOX_AUDIT_SIDES", "4096,8192");
	let depths: Vec<u8> = list("FOTOX_AUDIT_DEPTHS", "8,16");
	let validate = std::env::var("FOTOX_AUDIT_EXPORT").as_deref() == Ok("1");
	let dir = TempDir::new("canvas");
	let Some(mut p) = Probe::start(&dir.0) else { return };
	p.tag = "startup".into();
	p.memory("engine started, no document");
	for &side in &sides {
		for &depth in &depths {
			p.tag = format!("[{side}² {depth}-bit]");
			let t = Instant::now();
			let doc = p.new_document(side, side, depth);
			p.rec("new document", t.elapsed());
			p.rec_settle("first view", t);
			p.memory("new white document (sparse: solid tiles)");

			// Substantial data: a layer of gradient + noise over the whole canvas,
			// or over a centred patch of FOTOX_AUDIT_PATCH px.
			p.step(doc, vec![Command::AddLayer { layer: NewLayer::Pixel, name: Some("noise".into()) }]);
			if let Some(patch) = std::env::var("FOTOX_AUDIT_PATCH").ok().and_then(|v| v.parse::<f64>().ok()) {
				let o = (f64::from(side) - patch) / 2.0;
				p.step(
					doc,
					vec![Command::Select {
						shape: SelectionShape::Rect { x: o, y: o, w: patch, h: patch },
						mode: SelectMode::Replace,
						feather: 0.0,
						anti_alias: false,
					}],
				);
			}
			let t = Instant::now();
			let d = p.job(
				doc,
				Command::FillGradient {
					layer: LayerRef::Active,
					fill: serde_json::from_value(serde_json::json!({
						"gradient": {
							"colors": [
								{ "color": [0.1, 0.2, 0.9], "location": 0.0, "midpoint": 0.5 },
								{ "color": [0.9, 0.6, 0.1], "location": 1.0, "midpoint": 0.5 }
							],
							"method": "classic",
							"opacities": []
						},
						"kind": "linear",
						"start": [0.0, 0.0],
						"end": [f64::from(side), f64::from(side)]
					}))
					.expect("gradient fill json"),
					mode: BlendMode::Normal,
					opacity: 1.0,
				},
			);
			p.rec("full-canvas gradient (job)", d);
			let d = p.job(
				doc,
				Command::ApplyFilter {
					layer: LayerRef::Active,
					filter: FilterParams::AddNoise {
						amount: 25.0,
						gaussian: true,
						monochromatic: false,
						seed: 7,
					},
				},
			);
			p.rec("full-canvas Add Noise 25 % (job)", d);
			if std::env::var_os("FOTOX_AUDIT_PATCH").is_some() {
				p.step(doc, vec![Command::Deselect]);
			}
			p.rec_settle("after noise", t);
			p.memory("one full noise layer");

			// Zoom and pan.
			let t = Instant::now();
			p.zoom(doc, 1.0);
			p.rec_settle("zoom fit → 100 %", t);
			let from = Instant::now();
			let lat = pan(&mut p, 30);
			p.series("pan at 100 %: wheel → frame", &lat);
			let iv = p.frame_intervals(from);
			p.series("pan at 100 %: frame intervals", &iv);
			let t = Instant::now();
			p.zoom(doc, 0.03);
			p.rec_settle("zoom 100 % → 3 %", t);
			let t = Instant::now();
			p.zoom(doc, 1.0);
			p.rec_settle("zoom 3 % → 100 %", t);

			// Paint.
			let (lat, up) = stroke(&mut p, doc, 60);
			p.series("brush stroke 60 moves: pointer → frame", &lat);
			p.rec("brush stroke: pointer up → history", up);

			// Selection + fill (engine thread).
			let q = f64::from(side) / 4.0;
			let d = p.step(
				doc,
				vec![
					Command::Select {
						shape: SelectionShape::Rect { x: q, y: q, w: 2.0 * q, h: 2.0 * q },
						mode: SelectMode::Replace,
						feather: 0.0,
						anti_alias: true,
					},
					Command::Fill {
						layer: LayerRef::Active,
						color: [30000, 20000, 10000, 65535],
						mode: BlendMode::Multiply,
						opacity: 0.5,
						preserve_transparency: false,
					},
					Command::Deselect,
				],
			);
			p.rec("select 50 % rect + fill + deselect", d);

			// Responsiveness during an engine-thread pixel transform.
			p.drain();
			let t = Instant::now();
			p.ui(UiToEngine::Command {
				doc,
				command: Command::Transform {
					layer: LayerRef::Active,
					mapping: Box::new(Mapping::Affine([0.97, 0.02, -0.02, 0.97, 30.0, 10.0])),
					filter: Filter::Bicubic,
				},
			});
			let probe_sent = Instant::now();
			p.zoom(doc, 0.5);
			let (mut view_back, mut hist) = (None, None);
			while view_back.is_none() || hist.is_none() {
				let (v, h) = p.until("view and transform history", CEILING, |m| match m {
					EngineToUi::View { doc: d, .. } if *d == doc => Some((true, false)),
					EngineToUi::History { doc: d, .. } if *d == doc => Some((false, true)),
					_ => None,
				});
				let now = Instant::now();
				if v && view_back.is_none() {
					view_back = Some(now);
				}
				if h && hist.is_none() {
					hist = Some(now);
				}
			}
			let (view_back, hist) = (view_back.unwrap(), hist.unwrap());
			p.rec("free transform rotate+scale of the noise layer", hist.max(view_back) - t);
			p.rec("  a zoom sent during it answered after", view_back - probe_sent);

			let d = p.step(doc, vec![Command::DuplicateLayers { layers: vec![LayerRef::Active] }]);
			p.rec("duplicate layer", d);
			let layers = p.layer_list(doc);
			let bottom_noise = layers.iter().rev().find(|l| l.name == "noise").map(|l| l.id);
			if let Some(id) = bottom_noise {
				let t = Instant::now();
				p.step(
					doc,
					vec![Command::SetLayerProps {
						layer: LayerRef::Id(id),
						props: LayerPropsPatch {
							visible: Some(false),
							..Default::default()
						},
					}],
				);
				p.rec_settle("hide a full layer", t);
			}
			if std::env::var_os("FOTOX_AUDIT_SKIP_BLUR").is_none() {
				let d = p.job(
					doc,
					Command::ApplyFilter {
						layer: LayerRef::Active,
						filter: FilterParams::GaussianBlur { radius: 4.0 },
					},
				);
				p.rec("Gaussian Blur 4 px on a full layer (job)", d);
				p.memory("right after the blur");
			}
			p.memory("after the edits (history holds the old tiles)");
			let mut undo = Vec::new();
			let mut redo = Vec::new();
			for _ in 0..3 {
				undo.push(p.history_action(doc, "hist:undo"));
			}
			for _ in 0..3 {
				redo.push(p.history_action(doc, "hist:redo"));
			}
			p.series("undo", &undo);
			p.series("redo", &redo);

			// Save, export, close, reopen, validate.
			let fxd = dir.0.join(format!("canvas-{side}-{depth}.fxd"));
			let t = Instant::now();
			let d = p.save_as(doc, &fxd);
			p.rec(&format!("Save As .fxd ({} MiB)", mib(&fxd)), d);
			let _ = t;
			p.memory("after saving");
			let before = dir.0.join(format!("before-{side}-{depth}.tif"));
			if validate {
				let d = p.export(doc, &before);
				p.rec(&format!("export TIFF ({} MiB)", mib(&before)), d);
			}
			// Responsiveness during an incremental save of a painted document.
			p.step(doc, vec![Command::AddLayer { layer: NewLayer::Pixel, name: None }]);
			let lat = {
				p.drain();
				p.h.engine.send(EngineInput::Save { doc });
				pan(&mut p, 10)
			};
			p.until("incremental save", CEILING, |m| match m {
				EngineToUi::DocumentChanged { info } if info.doc == doc && !info.dirty => Some(()),
				_ => None,
			});
			p.series("pan during an incremental save: wheel → frame", &lat);
			p.close(doc);
			std::thread::sleep(Duration::from_secs(2));
			p.memory("after closing");
			// VERIFY: how memory settles after the close (recovery snapshot in flight?).
			let polls: u32 = std::env::var("FOTOX_AUDIT_CLOSE_POLLS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
			for k in 0..polls {
				std::thread::sleep(Duration::from_secs(1));
				p.memory(&format!("{}s after closing", k + 3));
			}
			let t = Instant::now();
			p.h.engine.send(EngineInput::Open(vec![fxd.clone()]));
			let doc2 = p.opened();
			p.rec("reopen", t.elapsed());
			p.rec_settle("reopen: first view", t);
			p.memory("reopened (lazy)");
			if validate {
				let after = dir.0.join(format!("after-{side}-{depth}.tif"));
				let d = p.export(doc2, &after);
				p.rec("export after reopen", d);
				let same = file_hash(&before) == file_hash(&after);
				println!("AUDIT {} reopened export identical to the pre-save export: {same}", p.tag);
				assert!(same, "the reopened document renders differently");
				let _ = std::fs::remove_file(&before);
				let _ = std::fs::remove_file(&after);
			}
			let d = p.step(
				doc2,
				vec![Command::SetLayerProps {
					layer: LayerRef::Active,
					props: LayerPropsPatch {
						opacity: Some(0.8),
						..Default::default()
					},
				}],
			);
			p.rec("edit after reopen", d);
			let d = p.save(doc2);
			p.rec(&format!("incremental save after a property edit ({} MiB file)", mib(&fxd)), d);
			p.close(doc2);
			std::thread::sleep(Duration::from_secs(2));
			p.memory("closed again");
			let _ = std::fs::remove_file(&fxd);
		}
	}
	p.h.engine.shutdown();
}

// ------------------------------------------------------- layer scaling

/// `n` layers on a fixed canvas, each with a 256² noise patch (real data),
/// in groups of 10, with masks, styles, adjustments and Smart Objects mixed in.
#[test]
#[ignore = "audit: minutes per thousand layers"]
fn layer_count_scaling() {
	let counts: Vec<usize> = list("FOTOX_AUDIT_LAYERS", "100,1000");
	let (cw, ch) = (4000u32, 3000u32);
	let dir = TempDir::new("layers");
	let Some(mut p) = Probe::start(&dir.0) else { return };
	let mut rng = 0x1234_5678_u64;
	let mut next = move || {
		rng ^= rng << 13;
		rng ^= rng >> 7;
		rng ^= rng << 17;
		rng
	};
	for &n in &counts {
		p.tag = format!("[{n} layers, {cw}×{ch} 8-bit]");
		let doc = p.new_document(cw, ch, 8);
		let mut add = Vec::with_capacity(n);
		let build = Instant::now();
		let mut kinds = (0, 0, 0, 0, 0);
		for i in 0..n {
			let t = Instant::now();
			// A noise patch pasted as a new layer, moved to a random place.
			let mut rgba = vec![0u8; 256 * 256 * 4];
			for px in rgba.chunks_mut(8) {
				px.copy_from_slice(&next().to_le_bytes());
			}
			for a in rgba.iter_mut().skip(3).step_by(4) {
				*a = 255;
			}
			p.drain();
			p.h.engine.send(EngineInput::PasteImage { width: 256, height: 256, rgba8: rgba });
			p.until("paste", CEILING, |m| match m {
				EngineToUi::History { doc: d, .. } if *d == doc => Some(()),
				_ => None,
			});
			let dx = (next() % u64::from(cw - 256)) as i32 - (cw as i32 - 256) / 2;
			let dy = (next() % u64::from(ch - 256)) as i32 - (ch as i32 - 256) / 2;
			let mut cmds = vec![Command::OffsetLayer {
				layer: LayerRef::Active,
				dx,
				dy,
			}];
			if i % 7 == 3 {
				cmds.push(Command::AddMask {
					layer: LayerRef::Active,
					fill: MaskFill::RevealAll,
				});
				kinds.0 += 1;
			}
			if i % 20 == 5 {
				let mut styles = fx_core::styles::LayerStyles::default();
				styles.drop_shadow.push(Default::default());
				cmds.push(Command::SetLayerStyle {
					layer: LayerRef::Active,
					styles: Some(styles),
				});
				kinds.1 += 1;
			}
			if i % 25 == 11 {
				cmds.push(Command::AddLayer {
					layer: NewLayer::Adjustment(Adjustment::Invert),
					name: None,
				});
				kinds.2 += 1;
			}
			if i % 50 == 17 {
				cmds.push(Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
				kinds.3 += 1;
			}
			p.step(doc, cmds);
			if i % 10 == 9 {
				// Group the last ten top-level layers.
				let layers = p.layer_list(doc);
				let top: Vec<LayerRef> = layers.iter().filter(|l| l.depth == 0).take(10).map(|l| LayerRef::Id(l.id)).collect();
				p.step(doc, vec![Command::GroupLayers { layers: top, name: None }]);
				kinds.4 += 1;
			}
			add.push(t.elapsed());
		}
		println!(
			"AUDIT {} built in {:.1} s: {} masks, {} styled, {} adjustments, {} Smart Objects, {} groups",
			p.tag,
			build.elapsed().as_secs_f64(),
			kinds.0,
			kinds.1,
			kinds.2,
			kinds.3,
			kinds.4
		);
		let tenth = (add.len() / 10).max(1);
		p.series("add one layer (paste+move+extras), first 10 %", &add[..tenth]);
		p.series("add one layer, last 10 %", &add[add.len() - tenth..]);
		p.memory("built");
		let t = Instant::now();
		p.rec_settle("view settled after the build", t);

		let layers = p.layer_list(doc);
		let bottom = layers.iter().rev().find(|l| l.depth == 0).map(|l| l.id).unwrap();
		let mut toggles = Vec::new();
		for on in [false, true, false, true] {
			let t = Instant::now();
			p.step(
				doc,
				vec![Command::SetLayerProps {
					layer: LayerRef::Id(bottom),
					props: LayerPropsPatch {
						visible: Some(on),
						..Default::default()
					},
				}],
			);
			if let Some(d) = p.settle(t) {
				toggles.push(d);
			}
		}
		p.series("toggle the bottom group's visibility (settled)", &toggles);

		let t = Instant::now();
		p.zoom(doc, 1.0);
		p.rec_settle("zoom fit → 100 %", t);
		let from = Instant::now();
		let lat = pan(&mut p, 30);
		p.series("pan at 100 %: wheel → frame", &lat);
		let iv = p.frame_intervals(from);
		p.series("pan at 100 %: frame intervals", &iv);
		p.step(doc, vec![Command::AddLayer { layer: NewLayer::Pixel, name: Some("paint".into()) }]);
		let (lat, up) = stroke(&mut p, doc, 40);
		p.series("brush stroke on a new top layer: pointer → frame", &lat);
		p.rec("brush stroke: up → history", up);
		let mut undo = Vec::new();
		for _ in 0..5 {
			undo.push(p.history_action(doc, "hist:undo"));
		}
		p.series("undo", &undo);

		let fxd = dir.0.join(format!("layers-{n}.fxd"));
		let d = p.save_as(doc, &fxd);
		p.rec(&format!("Save As ({} MiB)", mib(&fxd)), d);
		p.close(doc);
		std::thread::sleep(Duration::from_secs(2));
		p.memory("closed");
		let t = Instant::now();
		p.h.engine.send(EngineInput::Open(vec![fxd.clone()]));
		let doc2 = p.opened();
		p.rec("reopen", t.elapsed());
		p.rec_settle("reopen: first view", t);
		let back = p.layer_list(doc2);
		println!("AUDIT {} reopened layer rows {} (before {})", p.tag, back.len(), layers.len());
		p.memory("reopened");
		p.close(doc2);
		std::thread::sleep(Duration::from_secs(2));
		p.memory("closed again");
	}
	p.h.engine.shutdown();
}

/// A tight hot budget (the preference `memory_budget_mb`, set by the runner
/// in its isolated APPDATA): does a frame that needs more source tiles than
/// the budget ever finish?
#[test]
#[ignore = "audit"]
fn frame_larger_than_the_hot_budget() {
	let dir = TempDir::new("tight");
	let Some(mut p) = Probe::start(&dir.0) else { return };
	p.tag = "[tight budget]".into();
	let doc = p.new_document(8192, 8192, 16);
	for k in 0..6 {
		p.memory(&format!("before layer {k}"));
		p.step(doc, vec![Command::AddLayer { layer: NewLayer::Pixel, name: Some(format!("n{k}")) }]);
		let t = Instant::now();
		p.job(
			doc,
			Command::ApplyFilter {
				layer: LayerRef::Active,
				filter: FilterParams::Clouds {
					fg: [1000, 20000, 40000, 65535],
					bg: [60000, 50000, 3000, 65535],
					seed: k,
				},
			},
		);
		p.rec(&format!("clouds on layer {k} (job)"), t.elapsed());
		p.step(
			doc,
			vec![Command::SetLayerProps {
				layer: LayerRef::Active,
				props: LayerPropsPatch {
					opacity: Some(0.5),
					blend: Some(BlendMode::Screen),
					..Default::default()
				},
			}],
		);
	}
	p.memory("6 full 16-bit layers");
	let t = Instant::now();
	p.zoom(doc, 0.25);
	p.rec_settle("zoom to 25 % (whole canvas, every layer at level 2)", t);
	let t = Instant::now();
	p.zoom(doc, 1.0);
	p.rec_settle("zoom to 100 %", t);
	p.memory("after the views");
	p.h.engine.shutdown();
}

// ------------------------------------------------- Smart Objects at scale

/// A large noise Smart Object, `FOTOX_AUDIT_INSTANCES` instances of it
/// (Duplicate Layer: one shared source), a filter stack on one, Edit
/// Contents saved back while the engine is probed for responsiveness.
#[test]
#[ignore = "audit: GBs"]
fn smart_objects_at_scale() {
	let side: u32 = list("FOTOX_AUDIT_SO_SIDE", "8192")[0];
	let instances: usize = list("FOTOX_AUDIT_INSTANCES", "20")[0];
	let dir = TempDir::new("smart");
	let Some(mut p) = Probe::start(&dir.0) else { return };
	p.tag = format!("[SO {side}² 8-bit ×{instances}]");
	let doc = p.new_document(side, side, 8);
	p.step(doc, vec![Command::AddLayer { layer: NewLayer::Pixel, name: Some("noise".into()) }]);
	p.job(
		doc,
		Command::ApplyFilter {
			layer: LayerRef::Active,
			filter: FilterParams::Clouds {
				fg: [5000, 20000, 50000, 65535],
				bg: [60000, 40000, 2000, 65535],
				seed: 3,
			},
		},
	);
	p.job(
		doc,
		Command::ApplyFilter {
			layer: LayerRef::Active,
			filter: FilterParams::AddNoise {
				amount: 20.0,
				gaussian: true,
				monochromatic: false,
				seed: 5,
			},
		},
	);
	p.memory("source layer painted");
	let t = Instant::now();
	p.step(doc, vec![Command::ConvertToSmartObject { layers: vec![LayerRef::Active] }]);
	p.rec("Convert to Smart Object (engine thread)", t.elapsed());
	p.rec_settle("view after convert", t);
	p.memory("one Smart Object");
	let t = Instant::now();
	for i in 0..instances {
		p.step(doc, vec![Command::DuplicateLayers { layers: vec![LayerRef::Active] }]);
		let s = 0.15 + 0.02 * (i % 10) as f64;
		let (dx, dy) = ((i % 5) as f64 * f64::from(side) * 0.18, (i / 5 % 5) as f64 * f64::from(side) * 0.18);
		p.step(
			doc,
			vec![Command::Transform {
				layer: LayerRef::Active,
				mapping: Box::new(Mapping::Affine([s, 0.0, 0.0, s, dx, dy])),
				filter: Filter::Bicubic,
			}],
		);
	}
	p.rec(&format!("{instances} instances duplicated and transformed"), t.elapsed());
	p.rec_settle("view after the instances", t);
	p.memory("instances drawn at fit");
	let t = Instant::now();
	p.zoom(doc, 1.0);
	p.rec_settle("zoom to 100 %", t);
	p.memory("instances drawn at 100 %");
	let from = Instant::now();
	let lat = pan(&mut p, 20);
	p.series("pan at 100 % over instances: wheel → changed frame", &lat);
	let iv = p.frame_intervals(from);
	p.series("pan: frame intervals", &iv);
	// A filter stack on the top instance.
	std::thread::sleep(Duration::from_millis(500));
	let unfiltered = p.frame_now();
	let t = Instant::now();
	for f in [
		FilterParams::GaussianBlur { radius: 6.0 },
		FilterParams::UnsharpMask {
			amount: 120.0,
			radius: 2.0,
			threshold: 2,
		},
		FilterParams::Median { radius: 2.0 },
		FilterParams::FindEdges,
	] {
		p.job(doc, Command::ApplyFilter { layer: LayerRef::Active, filter: f });
	}
	p.rec("4 smart filters added (jobs)", t.elapsed());
	match p.changed_frame_after(t, &unfiltered, Duration::from_secs(120)) {
		Some(d) => p.rec("first frame showing the filter stack", d),
		None => println!("AUDIT {} the filter stack never showed within 120 s", p.tag),
	}
	p.rec_settle("view with the filter stack", t);
	let t = Instant::now();
	p.zoom(doc, 0.05);
	p.rec_settle("zoom to 5 % with the stack", t);
	let t = Instant::now();
	p.zoom(doc, 1.0);
	p.rec_settle("zoom back to 100 %", t);
	p.memory("with the filter stack");
	// Edit Contents, change, save back; probe the engine meanwhile.
	p.drain();
	p.action("smart:edit", serde_json::Value::Null);
	let child = p.opened();
	p.step(child, vec![Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None }]);
	p.drain();
	let t = Instant::now();
	p.action("doc:save", serde_json::Value::Null);
	p.ui(UiToEngine::SetZoom { doc: child, zoom: 0.5 });
	let (mut view_at, mut hist_at) = (None, None);
	while view_at.is_none() || hist_at.is_none() {
		let (v, h) = p.until("save back + probe", CEILING, |m| match m {
			EngineToUi::View { doc: d, .. } if *d == child => Some((true, false)),
			EngineToUi::History { doc: d, .. } if *d == doc => Some((false, true)),
			_ => None,
		});
		if v && view_at.is_none() {
			view_at = Some(t.elapsed());
		}
		if h && hist_at.is_none() {
			hist_at = Some(t.elapsed());
		}
	}
	p.rec("Edit Contents saved back (composite on the engine thread)", hist_at.unwrap());
	p.rec("  a zoom sent right after it answered after", view_at.unwrap());
	p.memory("after the contents edit");
	let fxd = dir.0.join("smart.fxd");
	let d = p.save_as(doc, &fxd);
	p.rec(&format!("Save As ({} MiB, source stored once?)", mib(&fxd)), d);
	p.h.engine.shutdown();
}

/// Why does panning at 100 % stop producing new frames with many layers?
/// Builds `FOTOX_AUDIT_DIAG_LAYERS` noise patches (extras on/off with
/// `FOTOX_AUDIT_EXTRAS`), zooms to 100 %, then logs per wheel notch: the
/// View message, frames delivered, whether the frame changed, and the
/// distinct colours on screen.
#[test]
#[ignore = "audit: diagnostics"]
fn pan_diagnostics() {
	let n: usize = list("FOTOX_AUDIT_DIAG_LAYERS", "1000")[0];
	let which = std::env::var("FOTOX_AUDIT_EXTRAS").unwrap_or_else(|_| "both".into());
	let extras = which != "0";
	let styles_on = which == "both" || which == "styles";
	let smart_on = which == "both" || which == "smart";
	let (cw, ch) = (4000u32, 3000u32);
	let dir = TempDir::new("pan");
	let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).with_max_level(tracing::Level::WARN).try_init();
	let Some(mut p) = Probe::start(&dir.0) else { return };
	p.tag = format!("[pan diag {n} layers, extras {which}]");
	let doc = p.new_document(cw, ch, 8);
	let mut rng = 0x1234_5678_u64;
	for i in 0..n {
		let mut rgba = vec![0u8; 256 * 256 * 4];
		for px in rgba.chunks_mut(8) {
			rng ^= rng << 13;
			rng ^= rng >> 7;
			rng ^= rng << 17;
			px.copy_from_slice(&rng.to_le_bytes());
		}
		for a in rgba.iter_mut().skip(3).step_by(4) {
			*a = 255;
		}
		p.drain();
		p.h.engine.send(EngineInput::PasteImage { width: 256, height: 256, rgba8: rgba });
		p.until("paste", CEILING, |m| matches!(m, EngineToUi::History { doc: d, .. } if *d == doc).then_some(()));
		let dx = (rng % u64::from(cw - 256)) as i32 - (cw as i32 - 256) / 2;
		let dy = ((rng >> 20) % u64::from(ch - 256)) as i32 - (ch as i32 - 256) / 2;
		let mut cmds = vec![Command::OffsetLayer { layer: LayerRef::Active, dx, dy }];
		if extras && styles_on && i % 20 == 5 {
			let mut styles = fx_core::styles::LayerStyles::default();
			styles.drop_shadow.push(Default::default());
			cmds.push(Command::SetLayerStyle { layer: LayerRef::Active, styles: Some(styles) });
		}
		if extras && smart_on && i % 50 == 17 {
			cmds.push(Command::ConvertToSmartObject { layers: vec![LayerRef::Active] });
		}
		p.step(doc, cmds);
	}
	let t = Instant::now();
	p.rec_settle("built; view settled", t);
	p.memory("built");
	let fit = p.frame_now();
	let mut fc: Vec<[u8; 4]> = fit.iter().step_by(97).copied().collect();
	fc.sort_unstable();
	fc.dedup();
	println!("AUDIT {} at fit: distinct colours sampled {} (first {:?})", p.tag, fc.len(), &fc[..fc.len().min(3)]);
	p.drain();
	p.zoom(doc, 1.0);
	let since = Instant::now();
	let settle = p.settle(since);
	println!("AUDIT {} zoom to 100 %: settled {settle:?}, frames so far {}", p.tag, p.frames_seen);
	for notch in 0..6 {
		let before = p.frame_now();
		let frames_before = p.frames_seen;
		let t = Instant::now();
		p.h.engine.send(EngineInput::Wheel {
			x: 400.0,
			y: 300.0,
			dx: 0.0,
			dy: -1.0,
			modifiers: Modifiers::default(),
		});
		let view = p.until("view", Duration::from_secs(10), |m| match m {
			EngineToUi::View { doc: d, zoom, .. } if *d == doc => Some(format!("{m:?}").chars().take(160).collect::<String>() + &format!(" zoom {zoom}")),
			_ => None,
		});
		let changed = p.changed_frame_after(t, &before, Duration::from_secs(5));
		let now = p.frame_now();
		let mut colours: Vec<[u8; 4]> = now.iter().step_by(97).copied().collect();
		colours.sort_unstable();
		colours.dedup();
		println!(
			"AUDIT {} notch {notch}: view after {:.1} ms ({view}); frames +{}; changed frame {:?}; distinct colours sampled {} {:?}",
			p.tag,
			t.elapsed().as_secs_f64() * 1000.0,
			p.frames_seen - frames_before,
			changed,
			colours.len(),
			&colours[..colours.len().min(2)]
		);
	}
	p.close(doc);
	for k in 0..6 {
		std::thread::sleep(Duration::from_secs(5));
		p.memory(&format!("{}s after close", (k + 1) * 5));
	}
	// Does anything derived still render in this session? A new document
	// whose fit view needs mip tiles (computed by the derived-tile worker).
	let fresh = p.new_document(6000, 4000, 8);
	p.step(fresh, vec![Command::AddLayer { layer: NewLayer::Pixel, name: None }]);
	p.job(
		fresh,
		Command::ApplyFilter {
			layer: LayerRef::Active,
			filter: FilterParams::Clouds {
				fg: [5000, 20000, 50000, 65535],
				bg: [60000, 40000, 2000, 65535],
				seed: 9,
			},
		},
	);
	std::thread::sleep(Duration::from_secs(8));
	let now = p.frame_now();
	let mut c: Vec<[u8; 4]> = now.iter().step_by(97).copied().collect();
	c.sort_unstable();
	c.dedup();
	println!("AUDIT {} a NEW document with a clouds layer, 8 s later at fit: distinct colours sampled {} {:?}", p.tag, c.len(), &c[..c.len().min(3)]);
	p.h.engine.shutdown();
}

/// VERIFY(COMPCACHE, FXREGION): the composite cache around the active layer
/// and region-limited effect invalidation must not change a single pixel.
/// Run once per configuration (the switches are read at engine start):
/// default, FOTOX_NO_COMPOSITE_CACHE=1, FOTOX_NO_REGION_EFFECTS=1, and compare
/// the printed hashes. Deterministic content (fixed seed).
#[test]
#[ignore = "audit: compare runs with and without the cache switches"]
fn cache_switch_pixels() {
	use std::hash::{Hash, Hasher};
	let dir = TempDir::new("cache-switch");
	let Some(mut p) = Probe::start(&dir.0) else { return };
	p.tag = "[cache-switch]".into();
	let (cw, ch) = (2048u32, 1536u32);
	let doc = p.new_document(cw, ch, 8);
	let mut rng = 0x9e37_79b9_u64;
	let mut next = move || {
		rng ^= rng << 13;
		rng ^= rng >> 7;
		rng ^= rng << 17;
		rng
	};
	let modes = [
		BlendMode::Normal,
		BlendMode::Multiply,
		BlendMode::Screen,
		BlendMode::Overlay,
		BlendMode::Normal,
		BlendMode::SoftLight,
		BlendMode::Difference,
		BlendMode::Normal,
		BlendMode::ColorDodge,
		BlendMode::Normal,
	];
	for i in 0..40usize {
		// A smooth gradient patch with a little noise: compressible, like art.
		let (pw, ph) = (512usize, 384usize);
		let mut rgba = vec![0u8; pw * ph * 4];
		let (r0, g0, b0) = ((next() % 200) as u8, (next() % 200) as u8, (next() % 200) as u8);
		for y in 0..ph {
			for x in 0..pw {
				let n = (next() % 16) as u8;
				let i4 = (y * pw + x) * 4;
				rgba[i4] = r0.wrapping_add((x / 4) as u8).wrapping_add(n);
				rgba[i4 + 1] = g0.wrapping_add((y / 3) as u8);
				rgba[i4 + 2] = b0.wrapping_add(((x + y) / 6) as u8);
				rgba[i4 + 3] = if (x / 32 + y / 32) % 7 == 0 { 128 } else { 255 };
			}
		}
		p.drain();
		p.h.engine.send(EngineInput::PasteImage { width: pw as u32, height: ph as u32, rgba8: rgba });
		p.until("paste", CEILING, |m| match m {
			EngineToUi::History { doc: d, .. } if *d == doc => Some(()),
			_ => None,
		});
		let dx = (next() % u64::from(cw - 512)) as i32 - (cw as i32 - 512) / 2;
		let dy = (next() % u64::from(ch - 384)) as i32 - (ch as i32 - 384) / 2;
		let mut cmds = vec![
			Command::OffsetLayer { layer: LayerRef::Active, dx, dy },
			Command::SetLayerProps {
				layer: LayerRef::Active,
				props: LayerPropsPatch {
					blend: Some(modes[i % modes.len()]),
					opacity: Some(if i % 3 == 0 { 0.7 } else { 1.0 }),
					clipped: Some(i % 6 == 4),
					..Default::default()
				},
			},
		];
		if i % 5 == 2 {
			let mut styles = fx_core::styles::LayerStyles::default();
			styles.drop_shadow.push(Default::default());
			cmds.push(Command::SetLayerStyle { layer: LayerRef::Active, styles: Some(styles) });
		}
		if i % 13 == 6 {
			cmds.push(Command::AddLayer { layer: NewLayer::Adjustment(Adjustment::Invert), name: None });
		}
		p.step(doc, cmds);
		if i % 10 == 9 {
			let layers = p.layer_list(doc);
			let top: Vec<LayerRef> = layers.iter().filter(|l| l.depth == 0).take(10).map(|l| LayerRef::Id(l.id)).collect();
			p.step(doc, vec![Command::GroupLayers { layers: top, name: None }]);
		}
	}
	let frame_hash = |p: &mut Probe, what: &str| {
		std::thread::sleep(Duration::from_secs(3));
		let t = Instant::now();
		let _ = p.settle(t);
		let px = p.frame_now();
		let mut h = std::collections::hash_map::DefaultHasher::new();
		px.hash(&mut h);
		println!("AUDIT cache-switch {what}: frame {} px, hash {:016x}", px.len(), h.finish());
		if let Ok(label) = std::env::var("FOTOX_AUDIT_LABEL") {
			let name = what.replace([' ', ','], "_");
			let out = std::path::Path::new(&std::env::var("FOTOX_AUDIT_DIR").unwrap_or_else(|_| ".".into())).join(format!("frame-{label}-{name}.raw"));
			std::fs::write(out, px.iter().flatten().copied().collect::<Vec<u8>>()).unwrap();
		}
	};
	frame_hash(&mut p, "built");
	// Paint on a pixel layer in the middle of the stack, twice.
	let layers = p.layer_list(doc);
	let pixels: Vec<&LayerInfo> = layers.iter().filter(|l| matches!(l.kind, fx_protocol::LayerInfoKind::Pixel)).collect();
	let middle = pixels[pixels.len() / 2].id;
	p.step(doc, vec![Command::SelectLayers { layers: vec![LayerRef::Id(middle)] }]);
	stroke(&mut p, doc, 30);
	frame_hash(&mut p, "middle layer, stroke 1");
	stroke(&mut p, doc, 30);
	frame_hash(&mut p, "middle layer, stroke 2");
	// Paint on a styled layer: its effect must follow the new pixels.
	let styled = layers.iter().find(|l| l.styles.is_some() && matches!(l.kind, fx_protocol::LayerInfoKind::Pixel)).map(|l| l.id).unwrap();
	p.step(doc, vec![Command::SelectLayers { layers: vec![LayerRef::Id(styled)] }]);
	stroke(&mut p, doc, 30);
	frame_hash(&mut p, "styled layer, stroke");
	// Hide and show a layer under the middle one.
	let below = pixels[pixels.len() * 3 / 4].id;
	for on in [false, true] {
		p.step(doc, vec![Command::SetLayerProps { layer: LayerRef::Id(below), props: LayerPropsPatch { visible: Some(on), ..Default::default() } }]);
	}
	frame_hash(&mut p, "after hide + show below");
	let out = dir.0.join("cache-switch.tif");
	p.export(doc, &out);
	println!("AUDIT cache-switch export hash {:016x}", file_hash(&out));
}

/// VERIFY-FIX(D8): on exFAT a document's file cannot be compacted while it is
/// open, so the engine compacts it after the document closes. Repaint a 4K
/// layer and save, 10 times (past the 256 MB threshold), close, and watch the file shrink; it must reopen
/// with the same pixels. `FOTOX_AUDIT_DIR` must be on an exFAT volume.
#[test]
#[ignore = "audit: needs FOTOX_AUDIT_DIR on exFAT"]
fn exfat_compaction_after_close() {
	let dir = TempDir::new("exfat-compact");
	let Some(mut p) = Probe::start(&dir.0) else { return };
	p.tag = "[exfat compaction]".into();
	let path = dir.0.join("doc.fxd");
	let doc = p.new_document(4096, 4096, 8);
	for i in 0..10u32 {
		p.job(
			doc,
			Command::ApplyFilter {
				layer: LayerRef::Active,
				filter: FilterParams::AddNoise {
					amount: 25.0,
					gaussian: true,
					monochromatic: false,
					seed: 100 + i,
				},
			},
		);
		if i == 0 {
			p.save_as(doc, &path);
		} else {
			p.save(doc);
		}
	}
	let before = std::fs::metadata(&path).unwrap().len();
	let reference = dir.0.join("before.tif");
	p.export(doc, &reference);
	p.close(doc);
	let t = Instant::now();
	let mut after = before;
	while t.elapsed() < Duration::from_secs(60) {
		std::thread::sleep(Duration::from_millis(250));
		after = std::fs::metadata(&path).unwrap().len();
		if after * 2 < before {
			break;
		}
	}
	println!(
		"AUDIT exfat compaction: file {} MiB before close, {} MiB {:.1} s after",
		before >> 20,
		after >> 20,
		t.elapsed().as_secs_f64()
	);
	p.h.engine.send(EngineInput::Open(vec![path.clone()]));
	let reopened = p.until("reopen", CEILING, |m| match m {
		EngineToUi::DocumentOpened { info } => Some(info.doc),
		_ => None,
	});
	let back = dir.0.join("after.tif");
	p.export(reopened, &back);
	let same = file_hash(&reference) == file_hash(&back);
	println!("AUDIT exfat compaction: reopened export identical: {same}");
	assert!(same, "the compacted file does not hold the saved pixels");
	assert!(after * 2 < before, "the closed file was not compacted");
}
