//! The flight recorder: one JSON-lines file per session in
//! `%LOCALAPPDATA%\Fotox\logs` (`FOTOX_LOGS` overrides), so a bug seen in the
//! app can be read back afterwards — what was done, in what order, how long
//! each step took, and where a freeze happened.
//!
//! Each line is `{"t": ms since start, "k": kind, ...}`. Kinds:
//! * `start` — version, file, command line;
//! * `in` — an input to the engine (UI message, open, paste…; pointer moves
//!   are counted, not written);
//! * `cmd` / `job` — a command or a background job, with its duration;
//! * `ui_error`, `toast`, `warn`, `error`, `panic` — what went wrong;
//! * `frame_slow` — a render frame over [`SLOW_FRAME_MS`];
//! * `freeze` / `unfreeze` — a thread busy with one thing for longer than
//!   [`FREEZE_MS`] (the watchdog), and when it came back.
//!
//! Threads mark what they are doing with [`busy`] / [`idle`]; waiting for
//! work is idle, so only a stuck *task* is a freeze. Everything is a no-op
//! until [`init`] runs (tests do not write files).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A task longer than this is a freeze.
pub const FREEZE_MS: u64 = 2000;
/// A frame longer than this is written down.
pub const SLOW_FRAME_MS: u64 = 100;
/// Session files kept in the logs folder.
const KEEP_FILES: usize = 20;

/// The threads the watchdog looks after.
#[derive(Clone, Copy, Debug)]
pub enum Thread {
	/// The shell's event loop (window, CEF, input).
	Ui = 0,
	Engine = 1,
	Render = 2,
}

const THREAD_NAMES: [&str; 3] = ["ui", "engine", "render"];

struct Beat {
	/// Milliseconds since start when the current task began, + 1; 0 = idle.
	since: AtomicU64,
	what: Mutex<String>,
	/// The current task was reported as a freeze.
	reported: AtomicBool,
}

struct Recorder {
	start: Instant,
	tx: crossbeam_channel::Sender<String>,
	beats: [Beat; 3],
	path: PathBuf,
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

fn now_ms(r: &Recorder) -> u64 {
	r.start.elapsed().as_millis() as u64
}

/// The logs folder.
pub fn logs_dir() -> PathBuf {
	if let Some(p) = std::env::var_os("FOTOX_LOGS") {
		return PathBuf::from(p);
	}
	let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
	base.join("Fotox").join("logs")
}

/// Start recording (once per process); returns the session file.
pub fn init() -> Option<PathBuf> {
	if let Some(r) = RECORDER.get() {
		return Some(r.path.clone());
	}
	let dir = logs_dir();
	std::fs::create_dir_all(&dir).ok()?;
	prune(&dir);
	let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
	let path = dir.join(format!("fotox-{stamp}-{}.jsonl", std::process::id()));
	let file = std::fs::File::create(&path).ok()?;
	let (tx, rx) = crossbeam_channel::unbounded::<String>();
	let beat = || Beat {
		since: AtomicU64::new(0),
		what: Mutex::new(String::new()),
		reported: AtomicBool::new(false),
	};
	let recorder = Recorder {
		start: Instant::now(),
		tx,
		beats: [beat(), beat(), beat()],
		path: path.clone(),
	};
	if RECORDER.set(recorder).is_err() {
		return RECORDER.get().map(|r| r.path.clone());
	}
	// The writer: lines as they come, flushed at least every 200 ms.
	let _ = std::thread::Builder::new().name("trace-writer".into()).spawn(move || {
		let mut out = std::io::BufWriter::new(file);
		loop {
			match rx.recv_timeout(Duration::from_millis(200)) {
				Ok(line) => {
					let _ = out.write_all(line.as_bytes());
					let _ = out.write_all(b"\n");
					for line in rx.try_iter() {
						let _ = out.write_all(line.as_bytes());
						let _ = out.write_all(b"\n");
					}
					let _ = out.flush();
				}
				Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
				Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
			}
		}
	});
	// The watchdog.
	let _ = std::thread::Builder::new().name("trace-watchdog".into()).spawn(|| {
		loop {
			std::thread::sleep(Duration::from_millis(250));
			let Some(r) = RECORDER.get() else { continue };
			let now = now_ms(r);
			for (i, beat) in r.beats.iter().enumerate() {
				let since = beat.since.load(Ordering::Relaxed);
				if since == 0 || beat.reported.load(Ordering::Relaxed) {
					continue;
				}
				let busy = now.saturating_sub(since - 1);
				if busy >= FREEZE_MS {
					beat.reported.store(true, Ordering::Relaxed);
					let what = beat.what.lock().map(|w| w.clone()).unwrap_or_default();
					event("freeze", json!({ "thread": THREAD_NAMES[i], "what": what, "ms": busy }));
				}
			}
		}
	});
	std::panic::set_hook(Box::new(|info| {
		let thread = std::thread::current().name().unwrap_or("?").to_owned();
		let backtrace = std::backtrace::Backtrace::force_capture().to_string();
		event("panic", json!({ "thread": thread, "message": info.to_string(), "backtrace": backtrace }));
		// Give the writer a moment: the process may be about to die.
		std::thread::sleep(Duration::from_millis(300));
		eprintln!("panic in {thread}: {info}");
	}));
	event(
		"start",
		json!({
			"version": env!("CARGO_PKG_VERSION"),
			"release": !cfg!(debug_assertions),
			"args": std::env::args().collect::<Vec<_>>(),
			"file": path.display().to_string(),
		}),
	);
	Some(path)
}

/// Keep the newest [`KEEP_FILES`] session files.
fn prune(dir: &std::path::Path) {
	let Ok(entries) = std::fs::read_dir(dir) else { return };
	let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
		.flatten()
		.filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
		.filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
		.collect();
	files.sort();
	let excess = files.len().saturating_sub(KEEP_FILES - 1);
	for (_, path) in files.into_iter().take(excess) {
		let _ = std::fs::remove_file(path);
	}
}

/// Write one event. `fields` should be an object; long strings are cut.
pub fn event(kind: &str, fields: Value) {
	let Some(r) = RECORDER.get() else { return };
	let mut line = match fields {
		Value::Object(map) => map,
		other => {
			let mut map = serde_json::Map::new();
			map.insert("v".into(), other);
			map
		}
	};
	line.insert("t".into(), json!(now_ms(r)));
	line.insert("k".into(), json!(kind));
	let text = Value::Object(line).to_string();
	let _ = r.tx.send(text);
}

/// Whether recording is on (skip building expensive fields otherwise).
pub fn enabled() -> bool {
	RECORDER.get().is_some()
}

/// `thread` starts a task; the watchdog reports it if it lasts too long.
pub fn busy(thread: Thread, what: &str) {
	let Some(r) = RECORDER.get() else { return };
	let beat = &r.beats[thread as usize];
	if let Ok(mut w) = beat.what.lock() {
		w.clear();
		w.push_str(what);
	}
	beat.reported.store(false, Ordering::Relaxed);
	beat.since.store(now_ms(r) + 1, Ordering::Relaxed);
}

/// `thread` finished its task (and is waiting for work).
pub fn idle(thread: Thread) {
	let Some(r) = RECORDER.get() else { return };
	let beat = &r.beats[thread as usize];
	let since = beat.since.swap(0, Ordering::Relaxed);
	if since != 0 && beat.reported.swap(false, Ordering::Relaxed) {
		let what = beat.what.lock().map(|w| w.clone()).unwrap_or_default();
		event(
			"unfreeze",
			json!({ "thread": THREAD_NAMES[thread as usize], "what": what, "ms": now_ms(r).saturating_sub(since - 1) }),
		);
	}
}

/// A string cut to `max` characters (payloads can be large).
pub fn cut(text: &str, max: usize) -> String {
	if text.chars().count() <= max {
		text.to_owned()
	} else {
		format!("{}…", text.chars().take(max).collect::<String>())
	}
}

/// A `tracing` layer copying warnings and errors into the recorder.
pub struct TraceLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TraceLayer {
	fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
		let level = *event.metadata().level();
		if level > tracing::Level::WARN || !enabled() {
			return;
		}
		struct Message(String);
		impl tracing::field::Visit for Message {
			fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
				if !self.0.is_empty() {
					self.0.push(' ');
				}
				if field.name() == "message" {
					self.0.push_str(&format!("{value:?}"));
				} else {
					self.0.push_str(&format!("{}={value:?}", field.name()));
				}
			}
		}
		let mut message = Message(String::new());
		event.record(&mut message);
		let kind = if level == tracing::Level::ERROR { "error" } else { "warn" };
		self::event(kind, json!({ "target": event.metadata().target(), "msg": cut(&message.0, 2000) }));
	}
}
