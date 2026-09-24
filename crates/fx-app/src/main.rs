//! Fotox — the desktop app: a winit window showing the Fotox UI, rendered by
//! CEF and composited over a wgpu viewport.
//!
//! Startup order ported from `reference/graphite-desktop/src/lib.rs`. The whole
//! of Graphite's editor pipeline (`graphite-desktop-wrapper`) is replaced by
//! [`crate::event`]; the `fx-protocol` bridge that fills the gap is M0-T05 and
//! the engine behind it is M0-T06.

mod app;
mod bridge;
mod cli;
mod consts;
mod dirs;
mod event;
mod gpu;
mod input;
mod preferences;
mod render;
mod window;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::app::ExitReason;
use crate::cli::Cli;
use crate::event::{AppEvent, AppEventScheduler, CreateAppEventSchedulerEventLoopExt};
use crate::ui::{Acceleration, Setup, UiConfig, UiContext, UiEvent, UiInstance, UiSetupResult};

pub(crate) use graphite_desktop_ui as ui;

fn main() -> ExitCode {
	// CEF helper processes re-enter `main`. They must be sent on their way
	// before anything else here runs, or they would start a second editor.
	let ui_context = match UiContext::setup() {
		UiSetupResult::Ready(context) => context,
		UiSetupResult::Helper(code) => return code,
		UiSetupResult::Failed => {
			init_logging();
			tracing::error!("failed to set up the UI runtime");
			return ExitCode::FAILURE;
		}
	};

	init_logging();
	run(ui_context)
}

/// Start a second instance with the UI acceleration disabled and exit.
fn init_logging() {
	tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();
}

/// Everything after the CEF helper check.
fn run(ui_context: UiContext<Setup>) -> ExitCode {
	let cli = Cli::parse();
	let mut preferences = preferences::read();

	// One instance only: two would fight over the CEF instance directory, and the
	// second window would be indistinguishable from the first.
	//
	// The lock is owned by the *guard*, not by the `RwLock`, and the guard borrows
	// the `RwLock`, so both have to be locals of this function. Taking the lock in
	// a helper and returning only the `RwLock` would drop the guard (and release
	// the lock) the moment the helper returned, and the second instance would then
	// sail straight through.
	let lock_path = dirs::lock_file_path();
	let lock_file = match std::fs::OpenOptions::new()
		.read(true)
		.write(true)
		.create(true)
		// The pid is written after the lock is taken, never while opening.
		.truncate(false)
		.open(&lock_path)
	{
		Ok(file) => file,
		Err(error) => {
			tracing::error!("failed to open the instance lock {}: {error}", lock_path.display());
			return ExitCode::FAILURE;
		}
	};
	let mut instance_lock = fd_lock::RwLock::new(lock_file);
	// `try_write` rather than `write`: the second instance exits instead of waiting.
	let _instance_guard = match instance_lock.try_write() {
		Ok(mut guard) => {
			let _ = guard.set_len(0);
			let _ = guard.write_all(std::process::id().to_string().as_bytes());
			tracing::info!("acquired the instance lock");
			guard
		}
		Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
			tracing::error!("another Fotox instance is already running");
			return ExitCode::SUCCESS;
		}
		Err(error) => {
			tracing::error!("failed to lock {}: {error}", lock_path.display());
			return ExitCode::FAILURE;
		}
	};

	// Must run before the event loop is created or the native window
	// integrations break.
	app::App::init();

	// The event loop comes first: the wgpu instance needs its display handle to
	// be able to create the window surface (see `gpu::create`).
	let event_loop = match winit::event_loop::EventLoop::new() {
		Ok(event_loop) => event_loop,
		Err(error) => {
			tracing::error!("failed to create the event loop: {error}");
			return ExitCode::FAILURE;
		}
	};

	let gpu = match gpu::create(event_loop.owned_display_handle()) {
		Ok(gpu) => gpu,
		Err(error) => {
			tracing::error!("{error:#}");
			return ExitCode::FAILURE;
		}
	};

	let (app_event_sender, app_event_receiver) = std::sync::mpsc::channel();
	let app_event_scheduler = event_loop.create_app_event_scheduler(app_event_sender);

	if cli.disable_ui_acceleration {
		preferences.disable_ui_acceleration = true;
	}
	if preferences.disable_ui_acceleration {
		tracing::info!("UI acceleration is disabled");
	}

	let acceleration = if preferences.disable_ui_acceleration {
		Acceleration::Disabled
	} else {
		Acceleration::Auto
	};
	let ui_context = match ui_context.start(UiConfig { acceleration }) {
		Ok(context) => context,
		Err(error) => {
			tracing::error!("failed to start the UI runtime: {error:#}");
			return ExitCode::FAILURE;
		}
	};
	let ui = match ui_context.instance(&gpu.device, &gpu.queue) {
		Ok(ui) => ui,
		Err(error) => {
			tracing::error!("failed to start the UI: {error}");
			return ExitCode::FAILURE;
		}
	};
	tracing::info!("UI runtime started");

	if !spawn_ui_bridge_thread(&ui, &app_event_scheduler) {
		return ExitCode::FAILURE;
	}

	let app = app::App::new(ui.clone(), gpu, app_event_receiver, app_event_scheduler, preferences);
	let exit_reason = app.run(event_loop);

	// The UI has to be shut down before a restart, or the next process cannot
	// take the CEF instance directory.
	ui.shutdown();

	if matches!(exit_reason, ExitReason::UiAccelerationFailure) {
		tracing::error!("recording that UI acceleration does not work on this machine");
		preferences::modify(|preferences| preferences.disable_ui_acceleration = true);
	}

	if matches!(exit_reason, ExitReason::UiAccelerationFailure) {
		restart();
	}

	ExitCode::SUCCESS
}

/// Start the thread that turns UI events into app events.
///
/// It runs for the life of the process: `UiInstance::recv` only returns `None`
/// once the UI is gone, which is also when the app is finished.
fn spawn_ui_bridge_thread(ui: &UiInstance, scheduler: &AppEventScheduler) -> bool {
	let ui = ui.clone();
	let scheduler = scheduler.clone();
	let spawned = std::thread::Builder::new().name("ui-events".to_string()).spawn(move || {
		while let Some(event) = ui.recv() {
			match event {
				UiEvent::Ready => scheduler.schedule(AppEvent::WebCommunicationInitialized),
				UiEvent::Frame(texture) => scheduler.schedule(AppEvent::UiUpdate(texture)),
				UiEvent::Cursor(cursor) => scheduler.schedule(AppEvent::CursorChange(cursor)),
				UiEvent::Message(message) => scheduler.schedule(AppEvent::UiMessage(message)),
				UiEvent::Failure(error) => {
					tracing::error!("UI failure: {error}");
					scheduler.schedule(AppEvent::UiCrashed);
				}
				UiEvent::Crashed => scheduler.schedule(AppEvent::UiCrashed),
			}
		}
	});
	match spawned {
		Ok(_) => true,
		Err(error) => {
			tracing::error!("failed to spawn the UI bridge thread: {error}");
			false
		}
	}
}

/// Start a fresh process, with the preferences as just written.
fn restart() {
	let exe = match std::env::current_exe() {
		Ok(exe) => exe,
		Err(error) => {
			tracing::error!("cannot find the executable to restart: {error}");
			return;
		}
	};
	tracing::info!("restarting {}", exe.display());
	if let Err(error) = std::process::Command::new(exe).spawn() {
		tracing::error!("failed to restart: {error}");
	}
}
