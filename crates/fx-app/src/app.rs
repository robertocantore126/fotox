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
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::run_on_demand::EventLoopExtRunOnDemand;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use fx_engine::{CursorShape, EngineHandle, EngineInput, EngineOutput};

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
			Routed::Engine(message) => self.engine.send(EngineInput::Ui(message)),
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
			WindowEvent::CloseRequested => self.exit(ExitReason::Shutdown),
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
