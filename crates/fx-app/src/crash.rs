//! What the user sees when Fotox crashes: an error box naming the thread and
//! the error, with the flight recorder's file, then a fresh process.
//!
//! Crashes come from three places:
//! * a panic that kills the engine or render thread (`fx_engine::trace::guard`,
//!   which calls the handler `main` registers: the event loop exits with
//!   `ExitReason::Crashed`);
//! * a panic on the main thread (`main` guards the event loop the same way),
//!   or the UI process (CEF) stopping;
//! * a native exception nobody handles (an access violation in a driver or
//!   in ONNX Runtime): [`install_native_handler`] reports it from the
//!   crashing thread, the process being past saving.
//!
//! A process started after a crash that crashes again within [`CRASH_LOOP`]
//! is not restarted: the box says so instead.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fx_engine::trace::Crash;

/// A crash this soon after a crash restart does not restart again.
const CRASH_LOOP: Duration = Duration::from_secs(30);
/// Characters of the error shown in the box (the log has all of it).
const MESSAGE_CHARS: usize = 600;

/// When this process started, and whether it was started after a crash.
static STARTED: OnceLock<(Instant, bool)> = OnceLock::new();
/// A crash was reported (the box is up or was shown): one box per process.
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Note the start of this process; `after_crash` when it is a crash restart.
pub(crate) fn started(after_crash: bool) {
	let _ = STARTED.set((Instant::now(), after_crash));
}

/// Whether a crash now should restart the app.
fn should_restart() -> bool {
	STARTED.get().is_none_or(|(at, after_crash)| !after_crash || at.elapsed() >= CRASH_LOOP)
}

/// Show the error box for `crash`. Returns whether to restart. A second
/// report (another thread crashing meanwhile) waits for the first to end the
/// process instead of showing a second box.
pub(crate) fn report(crash: &Crash) -> bool {
	if REPORTED.swap(true, Ordering::SeqCst) {
		loop {
			std::thread::park();
		}
	}
	// AUDIT-FIX(D2): a surviving engine gets a bounded best-effort recovery request before exiting.
	if crash.thread != "engine" {
		fx_engine::recovery::emergency(Duration::from_secs(5));
	}
	let restart = should_restart();
	let log = fx_engine::trace::session_file().map_or_else(|| "(no log was recorded)".to_owned(), |path| path.display().to_string());
	let text = format!(
		"Fotox stopped because of an internal error.\n\n\
		 Where: the {} thread\n\
		 Error: {}\n\n\
		 Unsaved changes may be available from recovery snapshots after restart.\n\
		 What happened is in the session log:\n{}\n\n{}",
		crash.thread,
		fx_engine::trace::cut(crash.message.trim(), MESSAGE_CHARS),
		log,
		if restart {
			"Fotox will now restart."
		} else {
			"Fotox had just restarted after a crash, so it was not restarted again."
		}
	);
	message_box(&text);
	restart
}

/// Start a fresh Fotox that waits for this process to let go of the
/// instance lock (`--after-crash`).
pub(crate) fn restart() {
	let exe = match std::env::current_exe() {
		Ok(exe) => exe,
		Err(error) => {
			tracing::error!("cannot find the executable to restart: {error}");
			return;
		}
	};
	tracing::info!("restarting {} after a crash", exe.display());
	let spawned = std::process::Command::new(exe).arg("--after-crash").arg(std::process::id().to_string()).spawn();
	if let Err(error) = spawned {
		tracing::error!("failed to restart: {error}");
	}
}

/// Wait (up to `limit`) until the instance lock at `path` is free: the
/// crashed process holds it until it has exited.
pub(crate) fn wait_for_instance_lock(path: &std::path::Path, limit: Duration) {
	let deadline = Instant::now() + limit;
	while Instant::now() < deadline {
		let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path) else {
			return;
		};
		let mut lock = fd_lock::RwLock::new(file);
		match lock.try_write() {
			Ok(_) => return,
			Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
			Err(_) => return,
		}
		std::thread::sleep(Duration::from_millis(100));
	}
	tracing::warn!("the crashed Fotox still holds the instance lock");
}

#[cfg(windows)]
fn message_box(text: &str) {
	use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW};
	use windows::core::HSTRING;
	// SAFETY: plain Win32 call with owned, NUL-terminated strings.
	unsafe {
		MessageBoxW(
			None,
			&HSTRING::from(text),
			&HSTRING::from("Fotox"),
			MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
		);
	}
}

#[cfg(not(windows))]
fn message_box(text: &str) {
	eprintln!("{text}");
}

/// Report native exceptions nobody handles (access violations and the like)
/// and restart, like a panic. Best effort: the process is already broken.
#[cfg(windows)]
pub(crate) fn install_native_handler() {
	use windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
	// SAFETY: installs a process-wide filter; `native_crash` is 'static.
	unsafe {
		SetUnhandledExceptionFilter(Some(native_crash));
	}
}

#[cfg(not(windows))]
pub(crate) fn install_native_handler() {}

#[cfg(windows)]
unsafe extern "system" fn native_crash(info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
	/// Terminate the process once the filter returns.
	const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
	// SAFETY: the system passes valid exception pointers (or null).
	let code = unsafe { info.as_ref().and_then(|info| info.ExceptionRecord.as_ref()) }.map_or(0, |record| record.ExceptionCode.0 as u32);
	let thread = std::thread::current().name().unwrap_or("native").to_owned();
	let what = match code {
		0xC000_0005 => " (access violation)",
		0xC000_00FD => " (stack overflow)",
		0xC000_001D => " (illegal instruction)",
		0xC000_0094 => " (integer division by zero)",
		_ => "",
	};
	fx_engine::trace::crashed(&thread, &format!("native exception 0x{code:08X}{what}"));
	// Let the recorder's writer flush the crash.
	std::thread::sleep(Duration::from_millis(300));
	if let Some(crash) = fx_engine::trace::last_crash()
		&& report(&crash)
	{
		restart();
	}
	EXCEPTION_EXECUTE_HANDLER
}
