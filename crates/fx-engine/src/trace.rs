//! The flight recorder: one JSON-lines file per session in
//! `%LOCALAPPDATA%\Fotox\logs` (`FOTOX_LOGS` overrides), so a bug seen in the
//! app can be read back afterwards — what was done, in what order, how long
//! each step took, and where a freeze happened.
//!
//! Each JSONL line is `{"t": ms since start, "k": kind, ...}`. The
//! downloadable JSON wraps the complete flushed session as a versioned `events`
//! array. Kinds:
//! * `start` — version, file, command line;
//! * `in` — every input to the engine, including each viewport pointer move;
//! * `cmd` / `job` — a command or a background job, with its duration;
//! * `ui_error`, `toast`, `warn`, `error`, `panic` — what went wrong;
//! * `crash` — a thread the app needs died ([`guard`]); the shell then shows
//!   an error and restarts;
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

enum WriterMessage {
	Event(String),
	Snapshot(crossbeam_channel::Sender<Result<Vec<u8>, String>>),
}

struct Recorder {
	start: Instant,
	tx: crossbeam_channel::Sender<WriterMessage>,
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
	let (tx, rx) = crossbeam_channel::unbounded::<WriterMessage>();
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
	let writer_path = path.clone();
	let _ = std::thread::Builder::new().name("trace-writer".into()).spawn(move || {
		let mut out = std::io::BufWriter::new(file);
		loop {
			match rx.recv_timeout(Duration::from_millis(200)) {
				Ok(message) => {
					write_message(&mut out, &writer_path, message);
					for message in rx.try_iter() {
						write_message(&mut out, &writer_path, message);
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
		LAST_PANIC.with(|last| *last.borrow_mut() = Some(info.to_string()));
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

fn write_message(out: &mut std::io::BufWriter<std::fs::File>, path: &std::path::Path, message: WriterMessage) {
	match message {
		WriterMessage::Event(line) => {
			let _ = out.write_all(line.as_bytes());
			let _ = out.write_all(b"\n");
		}
		WriterMessage::Snapshot(reply) => {
			let snapshot = out
				.flush()
				.map_err(|error| error.to_string())
				.and_then(|()| std::fs::read(path).map_err(|error| error.to_string()));
			let _ = reply.send(snapshot);
		}
	}
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

// ---------------------------------------------------------------- crashes

/// What stopped the app: the thread that died and why.
#[derive(Clone, Debug)]
pub struct Crash {
	pub thread: String,
	pub message: String,
}

static LAST_CRASH: Mutex<Option<Crash>> = Mutex::new(None);
type CrashHandler = Box<dyn Fn() + Send + Sync>;
static CRASH_HANDLER: OnceLock<CrashHandler> = OnceLock::new();

thread_local! {
	/// The message of this thread's last panic, as the hook formatted it
	/// (with its location), for [`guard`].
	static LAST_PANIC: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Call `handler` when a thread the app cannot run without dies (once: the
/// first crash is the one reported). The shell uses it to show an error and
/// restart.
pub fn on_crash(handler: impl Fn() + Send + Sync + 'static) {
	let _ = CRASH_HANDLER.set(Box::new(handler));
}

/// Record that `thread` died of `message` and tell the crash handler.
pub fn crashed(thread: &str, message: &str) {
	event("crash", json!({ "thread": thread, "message": message }));
	let first = {
		let mut last = LAST_CRASH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		let first = last.is_none();
		if first {
			*last = Some(Crash {
				thread: thread.to_owned(),
				message: message.to_owned(),
			});
		}
		first
	};
	tracing::error!("{thread} crashed: {message}");
	if first && let Some(handler) = CRASH_HANDLER.get() {
		handler();
	}
}

/// The first crash of this process, if any.
pub fn last_crash() -> Option<Crash> {
	LAST_CRASH.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// This session's recorder file.
pub fn session_file() -> Option<PathBuf> {
	RECORDER.get().map(|r| r.path.clone())
}

/// Run a thread's body; a panic that escapes it is a crash of `thread`
/// ([`crashed`]) instead of a thread that silently stops (the render thread
/// dying left a frozen view, flight recorder 2026-09-26).
pub fn guard<T>(thread: &str, body: impl FnOnce() -> T) -> Option<T> {
	match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
		Ok(value) => Some(value),
		Err(payload) => {
			let message = LAST_PANIC.with(|last| last.borrow_mut().take()).unwrap_or_else(|| {
				payload
					.downcast_ref::<&str>()
					.map(|s| (*s).to_owned())
					.or_else(|| payload.downcast_ref::<String>().cloned())
					.unwrap_or_else(|| "unknown panic".into())
			});
			crashed(thread, &message);
			None
		}
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
	let _ = r.tx.send(WriterMessage::Event(text));
}

/// Return the flushed session as a versioned JSON document and a download name.
pub fn export_json() -> Result<(String, String), String> {
	let Some(r) = RECORDER.get() else {
		return Err("The flight recorder is not available".into());
	};
	let (reply, result) = crossbeam_channel::bounded(1);
	r.tx.send(WriterMessage::Snapshot(reply)).map_err(|error| error.to_string())?;
	let bytes = result
		.recv_timeout(Duration::from_secs(15))
		.map_err(|error| format!("The flight recorder did not flush: {error}"))??;
	let json = json_document(&bytes)?;
	let stem = r.path.file_stem().and_then(|name| name.to_str()).unwrap_or("fotox-trace");
	Ok((format!("{stem}.json"), json))
}

fn json_document(bytes: &[u8]) -> Result<String, String> {
	let mut events = Vec::new();
	for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
		let line = line.strip_suffix(b"\r").unwrap_or(line);
		if line.is_empty() {
			continue;
		}
		let event: Value = serde_json::from_slice(line).map_err(|error| format!("Invalid trace event on line {}: {error}", index + 1))?;
		events.push(event);
	}
	serde_json::to_string_pretty(&json!({
		"format": "fotox-flight-recorder",
		"schema_version": 1,
		"events": events,
	}))
	.map_err(|error| error.to_string())
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn export_wraps_every_jsonl_event_in_a_versioned_document() {
		let json = json_document(b"{\"t\":1,\"k\":\"start\"}\n{\"t\":2,\"k\":\"in\",\"action\":\"doc:save\"}\n").unwrap();
		let value: Value = serde_json::from_str(&json).unwrap();
		assert_eq!(value["format"], "fotox-flight-recorder");
		assert_eq!(value["schema_version"], 1);
		assert_eq!(value["events"].as_array().unwrap().len(), 2);
		assert_eq!(value["events"][1]["action"], "doc:save");
	}

	#[test]
	fn export_rejects_a_malformed_event_instead_of_silently_dropping_it() {
		assert!(json_document(b"{not json}\n").unwrap_err().contains("line 1"));
	}

	#[test]
	fn snapshot_flushes_queued_events_before_reading_the_session_file() {
		let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
		let path = std::env::temp_dir().join(format!("fotox-trace-snapshot-{}-{stamp}.jsonl", std::process::id()));
		let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
		write_message(&mut out, &path, WriterMessage::Event(r#"{"k":"in","action":"doc:save"}"#.into()));
		let (reply, result) = crossbeam_channel::bounded(1);
		write_message(&mut out, &path, WriterMessage::Snapshot(reply));
		let snapshot = result.recv().unwrap().unwrap();
		assert!(String::from_utf8(snapshot).unwrap().contains("doc:save"));
		drop(out);
		std::fs::remove_file(path).unwrap();
	}
}
