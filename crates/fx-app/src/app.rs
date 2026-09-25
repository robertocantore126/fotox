//! The winit `ApplicationHandler`: owns the window, the render state and the
//! UI instance, and is the only place that touches all three.
//!
//! Ported from `reference/graphite-desktop/src/app.rs`. Everything driven by
//! Graphite's editor protocol — file dialogs, persistence, clipboard, menus,
//! pointer lock, drag and drop, the node-graph render thread — is gone; the
//! Fotox equivalents arrive with the `fx-protocol` bridge in M0-T05 and the
//! engine in M0-T06.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::data_transfer::TypeHint;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::run_on_demand::EventLoopExtRunOnDemand;
use winit::event_loop::{ActiveEventLoop, AsyncRequestSerial, ControlFlow, DndAction, EventLoop};
use winit::window::WindowId;

use fx_engine::{CursorShape, EngineHandle, EngineInput, EngineOutput};
use fx_protocol::UiToEngine;

use crate::bridge::{self, Routed};
use crate::event::{AppEvent, AppEventScheduler};
use crate::gpu::Gpu;
use crate::input::InputState;
use crate::preferences::Preferences;
use crate::render::{RenderError, RenderState};
use crate::ui::{Cursor, UiCommand, UiInstance};
use crate::window::Window;

/// How long the accelerated UI may go without presenting a single frame before
/// the app concludes the GPU sharing path does not work here.
const UI_ACCELERATION_GRACE: Duration = Duration::from_secs(3);

/// How long the event loop sleeps when nothing else wakes it.
const IDLE_WAIT: Duration = Duration::from_millis(10);

/// Why the event loop ended.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExitReason {
	/// The user closed the window, or the UI died.
	Shutdown,
	/// The accelerated UI never presented a frame; restart with it disabled.
	UiAccelerationFailure,
}

/// Everything the app owns while it runs.
pub(crate) struct App {
	gpu: Gpu,
	ui: UiInstance,
	engine: EngineHandle,
	preferences: Preferences,
	render_state: Option<RenderState>,
	window: Option<Window>,
	window_size: PhysicalSize<u32>,
	window_scale: f64,
	input_state: InputState,
	app_event_receiver: Receiver<AppEvent>,
	app_event_scheduler: AppEventScheduler,
	ui_frame_received: bool,
	web_communication_initialized: bool,
	/// The cursor the UI asked for last, and the engine's for the viewport;
	/// whichever owns the pointer shows.
	ui_cursor: Option<Cursor>,
	engine_cursor: CursorShape,
	/// A drop whose file list is being fetched.
	pending_drop: Option<AsyncRequestSerial>,
	startup_time: Option<Instant>,
	exiting: Arc<AtomicBool>,
	exit_reason: ExitReason,
}

impl App {
	/// Process-wide initialisation that has to run before the event loop starts.
	pub(crate) fn init() {
		Window::init();
	}

	/// Assemble the app.
	pub(crate) fn new(
		ui: UiInstance,
		engine: EngineHandle,
		gpu: Gpu,
		app_event_receiver: Receiver<AppEvent>,
		app_event_scheduler: AppEventScheduler,
		preferences: Preferences,
	) -> Self {
		Self {
			gpu,
			ui,
			engine,
			preferences,
			render_state: None,
			window: None,
			window_size: PhysicalSize { width: 0, height: 0 },
			window_scale: 1.0,
			input_state: InputState::new(),
			app_event_receiver,
			app_event_scheduler,
			ui_frame_received: false,
			web_communication_initialized: false,
			ui_cursor: None,
			engine_cursor: CursorShape::Default,
			pending_drop: None,
			startup_time: None,
			exiting: Arc::new(AtomicBool::new(false)),
			exit_reason: ExitReason::Shutdown,
		}
	}

	/// Run the event loop to completion, stop the engine and report why it
	/// stopped.
	pub(crate) fn run(mut self, mut event_loop: EventLoop) -> ExitReason {
		if let Err(error) = event_loop.run_app_on_demand(&mut self) {
			tracing::error!("the event loop failed: {error}");
		}
		let reason = self.exit_reason;
		self.engine.shutdown();
		reason
	}

	/// Leave the event loop, recording why. Idempotent.
	fn exit(&mut self, reason: ExitReason) {
		if self.exiting.swap(true, Ordering::Relaxed) {
			return;
		}
		self.exit_reason = reason;
		self.app_event_scheduler.schedule(AppEvent::Exit);
	}

	/// Tell the UI and the renderer about the current window size and scale.
	fn resize(&mut self) {
		let Some(window) = &self.window else {
			tracing::error!("cannot handle a resize without a window");
			return;
		};

		let size = window.surface_size();
		let scale = window.scale_factor();
		let is_new_size = size != self.window_size;
		let is_new_scale = scale != self.window_scale;
		if !is_new_size && !is_new_scale {
			return;
		}

		if is_new_size {
			self.ui.send(UiCommand::Resized {
				width: size.width,
				height: size.height,
			});
		}
		if is_new_scale {
			self.ui.send(UiCommand::ScaleChanged(scale));
		}
		self.ui.send(UiCommand::Refresh);

		if let Some(render_state) = &mut self.render_state {
			render_state.resize(size.width, size.height);
		}
		window.request_redraw();

		self.window_size = size;
		self.window_scale = scale;
	}

	/// Draw one frame, then check whether the accelerated UI is working at all.
	fn redraw(&mut self) {
		if !self.window.as_ref().is_some_and(Window::can_render) {
			return;
		}

		if let (Some(render_state), Some(window)) = (&mut self.render_state, &self.window) {
			match render_state.render(window) {
				Ok(()) => {}
				// The UI texture did not match the window this frame: it was
				// presented stretched once and the UI is already re-rendering.
				Err(RenderError::OutdatedUiTexture) => self.ui.send(UiCommand::Refresh),
				Err(RenderError::SurfaceLost) => tracing::warn!("lost the window surface"),
				Err(error) => tracing::error!("render error: {error:?}"),
			}
		}

		// If the accelerated path is in use and the UI has never presented a
		// frame, it is not going to: restart with software paint.
		if !self.ui_frame_received
			&& !self.preferences.disable_ui_acceleration
			&& self.web_communication_initialized
			&& let Some(startup_time) = self.startup_time
			&& startup_time.elapsed() > UI_ACCELERATION_GRACE
		{
			tracing::error!("the accelerated UI never presented a frame; restarting with acceleration disabled");
			self.exit(ExitReason::UiAccelerationFailure);
		}
	}

	/// Handle one `fx-protocol` frame from the UI (docs/PROTOCOL.md). Messages
	/// for the shell are handled here; the rest go to the engine once it
	/// exists (M0-T05/T06).
	fn ui_message(&mut self, frame: &[u8]) {
		let routed = match bridge::route(frame) {
			Ok(routed) => routed,
			Err(error) => {
				tracing::warn!("dropping a malformed UI message ({} bytes): {error}", frame.len());
				return;
			}
		};
		match routed {
			Routed::ViewportBounds(bounds) => {
				tracing::debug!("viewport bounds: {bounds:?}");
				self.input_state.set_viewport(bounds);
				self.engine.send(EngineInput::ViewportResized {
					width: bounds.width,
					height: bounds.height,
				});
				if let Some(render_state) = &mut self.render_state {
					render_state.set_viewport_bounds(bounds);
				}
				if let Some(window) = &self.window {
					window.request_redraw();
				}
			}
			Routed::DirectInput(enabled) => {
				tracing::debug!("direct input {enabled}");
				self.input_state.set_direct_input(enabled);
			}
			Routed::Engine(message) => {
				// The shell owns native dialogs: the chosen files go to the
				// engine as `Open` (docs/tasks/M1.md, M1-T08).
				if let UiToEngine::Action { id, args } = &message {
					match id.as_str() {
						"dlg:open" => self.open_file_dialog(),
						"export:png" => self.export_file_dialog("PNG", "png", None),
						"export:tiff" => self.export_file_dialog("TIFF", "tif", None),
						"export:jpg" => self.export_file_dialog("JPEG", "jpg", None),
						// The Export As dialog (M3-T07): its options, then the save dialog.
						"export:as" => {
							let (name, extension, choice) = export_choice(args);
							self.export_file_dialog(name, extension, Some(choice));
						}
						_ => {}
					}
				}
				self.engine.send(EngineInput::Ui(message));
			}
		}
	}

	/// Show the native save dialog for an export (helper thread, like
	/// [`open_file_dialog`](Self::open_file_dialog)); the chosen file comes
	/// back as `AppEvent::ExportTo`.
	fn export_file_dialog(&self, name: &'static str, extension: &'static str, choice: Option<fx_engine::ExportChoice>) {
		let scheduler = self.app_event_scheduler.clone();
		let spawned = std::thread::Builder::new().name("export-dialog".into()).spawn(move || {
			let dialog = rfd::AsyncFileDialog::new()
				.set_title(format!("Export As {name}"))
				.set_file_name(format!("Untitled.{extension}"))
				.add_filter(name, &[extension]);
			if let Some(file) = futures::executor::block_on(dialog.save_file()) {
				let mut path = file.path().to_path_buf();
				// A name typed without the extension still gets the chosen format.
				if path.extension().is_none() {
					path.set_extension(extension);
				}
				scheduler.schedule(AppEvent::ExportTo(path, choice));
			}
		});
		if let Err(error) = spawned {
			tracing::error!("cannot show the export dialog: {error}");
		}
	}

	/// Show the native open dialog on a helper thread (it is modal and would
	/// otherwise block the event loop); the result comes back as
	/// `AppEvent::OpenFiles`.
	fn open_file_dialog(&self) {
		let scheduler = self.app_event_scheduler.clone();
		let spawned = std::thread::Builder::new().name("open-dialog".into()).spawn(move || {
			let dialog = rfd::AsyncFileDialog::new()
				.set_title("Open")
				.add_filter("Fotox documents and images", &["fxd", "tif", "tiff", "png", "jpg", "jpeg"])
				.add_filter("Fotox document", &["fxd"])
				.add_filter("TIFF", &["tif", "tiff"])
				.add_filter("PNG", &["png"])
				.add_filter("JPEG", &["jpg", "jpeg"])
				.add_filter("All files", &["*"]);
			let files = futures::executor::block_on(dialog.pick_files()).unwrap_or_default();
			if !files.is_empty() {
				scheduler.schedule(AppEvent::OpenFiles(files.iter().map(|f| f.path().to_path_buf()).collect()));
			}
		});
		if let Err(error) = spawned {
			tracing::error!("cannot show the open dialog: {error}");
		}
	}

	/// Handle one output of the engine or render thread.
	fn engine_output(&mut self, event_loop: &dyn ActiveEventLoop, output: EngineOutput) {
		match output {
			EngineOutput::ToUi(frame) => self.ui.send(UiCommand::Message(frame)),
			EngineOutput::ViewportFrame(texture) => {
				if let Some(render_state) = &mut self.render_state {
					render_state.bind_viewport_texture(texture);
				}
				if let Some(window) = &self.window {
					window.request_redraw();
				}
			}
			EngineOutput::RedrawViewport => {
				if let Some(window) = &self.window {
					window.request_redraw();
				}
			}
			EngineOutput::Cursor(shape) => {
				self.engine_cursor = shape;
				self.apply_cursor(event_loop);
			}
			EngineOutput::NeedSavePath { doc, suggested_name } => self.save_fxd_dialog(doc, suggested_name),
			// The engine asked about every unsaved document (M3-T06).
			EngineOutput::MayClose(true) => self.exit(ExitReason::Shutdown),
			EngineOutput::MayClose(false) => {}
		}
	}

	/// Show the native "Save As" dialog for a `.fxd` (helper thread, like
	/// [`open_file_dialog`](Self::open_file_dialog)); the answer comes back as
	/// `AppEvent::SaveAs` or `AppEvent::SaveCancelled`.
	fn save_fxd_dialog(&self, doc: fx_protocol::DocId, suggested_name: String) {
		let scheduler = self.app_event_scheduler.clone();
		let spawned = std::thread::Builder::new().name("save-dialog".into()).spawn(move || {
			let dialog = rfd::AsyncFileDialog::new()
				.set_title("Save As")
				.set_file_name(suggested_name)
				.add_filter("Fotox document", &["fxd"]);
			match futures::executor::block_on(dialog.save_file()) {
				Some(file) => {
					let mut path = file.path().to_path_buf();
					if path.extension().is_none() {
						path.set_extension("fxd");
					}
					scheduler.schedule(AppEvent::SaveAs { doc, path });
				}
				None => scheduler.schedule(AppEvent::SaveCancelled(doc)),
			}
		});
		if let Err(error) = spawned {
			tracing::error!("cannot show the save dialog: {error}");
		}
	}

	/// Show the engine's cursor while it owns the pointer, the UI's otherwise.
	fn apply_cursor(&mut self, event_loop: &dyn ActiveEventLoop) {
		let cursor = if self.input_state.pointer_on_engine() {
			engine_cursor(self.engine_cursor)
		} else {
			match &self.ui_cursor {
				Some(cursor) => cursor.clone(),
				None => return,
			}
		};
		if let Some(window) = &mut self.window {
			window.set_cursor(event_loop, cursor);
		}
	}

	/// Handle one event that arrived from another thread.
	fn user_event(&mut self, event_loop: &dyn ActiveEventLoop, event: AppEvent) {
		match event {
			AppEvent::WebCommunicationInitialized => {
				tracing::info!("the UI is ready");
				self.web_communication_initialized = true;
			}
			AppEvent::UiUpdate(texture) => {
				if !self.ui_frame_received {
					tracing::info!("first UI frame received: {}x{}", texture.width(), texture.height());
				}
				if let Some(render_state) = &mut self.render_state {
					render_state.bind_ui_texture(texture);
				}
				if let Some(window) = &self.window {
					window.request_redraw();
				}
				self.ui_frame_received = true;
			}
			AppEvent::CursorChange(cursor) => {
				self.ui_cursor = Some(cursor);
				self.apply_cursor(event_loop);
			}
			AppEvent::Engine(output) => self.engine_output(event_loop, output),
			AppEvent::OpenFiles(paths) => self.engine.send(EngineInput::Open(paths)),
			AppEvent::ExportTo(path, choice) => self.engine.send(EngineInput::Export { path, choice }),
			AppEvent::SaveAs { doc, path } => self.engine.send(EngineInput::SaveAs { doc, path }),
			AppEvent::SaveCancelled(doc) => self.engine.send(EngineInput::SaveCancelled { doc }),
			AppEvent::UiMessage(frame) => self.ui_message(&frame),
			AppEvent::UiCrashed => {
				tracing::error!("the UI crashed, exiting");
				self.exit(ExitReason::Shutdown);
			}
			AppEvent::Exit => {
				tracing::info!("leaving the event loop");
				event_loop.exit();
			}
		}
	}
}

impl ApplicationHandler for App {
	fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
		let window = match Window::new(event_loop) {
			Ok(window) => window,
			Err(error) => {
				tracing::error!("failed to create the window: {error:#}");
				self.exit(ExitReason::Shutdown);
				return;
			}
		};
		let render_state = match RenderState::new(&window, &self.gpu) {
			Ok(render_state) => render_state,
			Err(error) => {
				tracing::error!("failed to create the render state: {error:#}");
				self.exit(ExitReason::Shutdown);
				return;
			}
		};

		window.show();
		self.window = Some(window);
		self.render_state = Some(render_state);

		self.resize();
		self.startup_time = Some(Instant::now());
	}

	fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
		while let Ok(event) = self.app_event_receiver.try_recv() {
			self.user_event(event_loop, event);
		}
	}

	fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
		// Pointer input over the viewport goes to the engine, everything else
		// to the UI (`docs/ARCHITECTURE.md` §2.2, `input.rs`).
		let was_on_engine = self.input_state.pointer_on_engine();
		let ui = self.ui.clone();
		let engine = &self.engine;
		self.input_state
			.process(&event, |input| ui.send(UiCommand::Input(input)), |input| engine.send(input));
		if self.input_state.pointer_on_engine() != was_on_engine {
			self.apply_cursor(event_loop);
		}

		match event {
			// Drag and drop of files onto the window opens them.
			WindowEvent::DragEntered { id, .. } => {
				let accepts = event_loop.data_transfer(id).is_ok_and(|data| data.has_type(&TypeHint::UriList));
				let actions: &[DndAction] = if accepts { &[DndAction::Copy] } else { &[] };
				if let Err(error) = event_loop.set_valid_dnd_actions(id, actions) {
					tracing::error!("cannot accept the drag: {error}");
				}
			}
			WindowEvent::DragDropped { id, .. } => match event_loop.fetch_data_transfer(id, &TypeHint::UriList) {
				Ok(serial) => self.pending_drop = Some(serial),
				Err(error) => tracing::error!("cannot read the dropped files: {error}"),
			},
			WindowEvent::DataTransferReceived { serial, ref value, .. } if self.pending_drop == Some(serial) => match value.try_as_uris() {
				Ok(uris) => {
					self.pending_drop = None;
					let paths: Vec<_> = uris.iter().filter_map(|uri| file_uri_to_path(uri)).collect();
					if !paths.is_empty() {
						self.engine.send(EngineInput::Open(paths));
					}
				}
				Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
				Err(error) => {
					self.pending_drop = None;
					tracing::error!("cannot read the dropped files: {error}");
				}
			},
			// The engine asks about unsaved documents first and answers
			// `MayClose` (M3-T06).
			WindowEvent::CloseRequested => self.engine.send(EngineInput::CloseRequested),
			WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. } => self.resize(),
			WindowEvent::RedrawRequested => self.redraw(),
			_ => {}
		}
	}

	fn new_events(&mut self, _event_loop: &dyn ActiveEventLoop, cause: winit::event::StartCause) {
		if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. })
			&& let Some(window) = &self.window
		{
			window.request_redraw();
		}
	}

	fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
		event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + IDLE_WAIT));
	}
}

/// The OS cursor for an engine cursor shape.
fn engine_cursor(shape: CursorShape) -> Cursor {
	use winit::cursor::CursorIcon;
	match shape {
		CursorShape::Default => Cursor::Icon(CursorIcon::Default),
		CursorShape::Crosshair => Cursor::Icon(CursorIcon::Crosshair),
		CursorShape::Grab => Cursor::Icon(CursorIcon::Grab),
		CursorShape::Grabbing => Cursor::Icon(CursorIcon::Grabbing),
		CursorShape::ZoomIn => Cursor::Icon(CursorIcon::ZoomIn),
		CursorShape::ZoomOut => Cursor::Icon(CursorIcon::ZoomOut),
		CursorShape::None => Cursor::None,
	}
}

/// `file:///C:/My%20Pictures/a.tif` → `C:\\My Pictures\\a.tif`. Other schemes → `None`.
fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
	let rest = uri.trim().strip_prefix("file://")?;
	// `file:///C:/…` (local) or `file://server/share/…` (UNC).
	let rest = match rest.strip_prefix('/') {
		Some(local) if local.as_bytes().get(1) == Some(&b':') => local.to_owned(),
		Some(local) => format!("/{local}"),
		None => format!("//{rest}"),
	};
	let bytes = rest.as_bytes();
	let mut decoded = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%'
			&& let Some(hex) = rest.get(i + 1..i + 3)
			&& let Ok(byte) = u8::from_str_radix(hex, 16)
		{
			decoded.push(byte);
			i += 3;
			continue;
		}
		decoded.push(bytes[i]);
		i += 1;
	}
	let path = String::from_utf8(decoded).ok()?;
	Some(std::path::PathBuf::from(path.replace('/', "\\")))
}

/// The Export As dialog's `export:as` arguments (see `ui/js/actions.js`):
/// format name, extension and the engine's options. Unknown values fall back
/// to the defaults (PNG, automatic transparency, quality 90, 4:4:4).
fn export_choice(args: &serde_json::Value) -> (&'static str, &'static str, fx_engine::ExportChoice) {
	let text = |key: &str| args.get(key).and_then(serde_json::Value::as_str).unwrap_or("");
	let (name, extension) = match text("format") {
		"jpg" => ("JPEG", "jpg"),
		"tif" => ("TIFF", "tif"),
		_ => ("PNG", "png"),
	};
	let choice = fx_engine::ExportChoice {
		eight_bit: args.get("eight_bit").and_then(serde_json::Value::as_bool).unwrap_or(false),
		transparency: match text("transparency") {
			"on" => Some(true),
			"off" => Some(false),
			_ => None,
		},
		quality: args.get("quality").and_then(serde_json::Value::as_u64).map_or(90, |q| q.min(100) as u8),
		chroma_half: text("chroma") == "420",
	};
	(name, extension, choice)
}

#[cfg(test)]
mod tests {
	use super::file_uri_to_path;

	#[test]
	fn file_uris_become_windows_paths() {
		assert_eq!(
			file_uri_to_path("file:///C:/My%20Pictures/a.tif").unwrap().to_str(),
			Some("C:\\My Pictures\\a.tif")
		);
		assert_eq!(
			file_uri_to_path("file://server/share/b.png").unwrap().to_str(),
			Some("\\\\server\\share\\b.png")
		);
		assert_eq!(file_uri_to_path("https://example.com/c.jpg"), None);
	}
}
