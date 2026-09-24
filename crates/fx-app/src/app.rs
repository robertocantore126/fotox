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

use crate::event::{AppEvent, AppEventScheduler};
use crate::gpu::Gpu;
use crate::input::InputState;
use crate::preferences::Preferences;
use crate::render::{RenderError, RenderState};
use crate::ui::{UiCommand, UiInstance};
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
		gpu: Gpu,
		app_event_receiver: Receiver<AppEvent>,
		app_event_scheduler: AppEventScheduler,
		preferences: Preferences,
	) -> Self {
		Self {
			gpu,
			ui,
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
			startup_time: None,
			exiting: Arc::new(AtomicBool::new(false)),
			exit_reason: ExitReason::Shutdown,
		}
	}

	/// Run the event loop to completion and report why it stopped.
	pub(crate) fn run(mut self, mut event_loop: EventLoop) -> ExitReason {
		if let Err(error) = event_loop.run_app_on_demand(&mut self) {
			tracing::error!("the event loop failed: {error}");
		}
		self.exit_reason
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
				if let Some(window) = &mut self.window {
					window.set_cursor(event_loop, cursor);
				}
			}
			AppEvent::UiMessage(message) => {
				// The `fx-protocol` bridge that gives this meaning is M0-T05.
				tracing::debug!("received a {}-byte UI message; not routed yet (M0-T05)", message.len());
			}
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

	fn window_event(&mut self, _event_loop: &dyn ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
		// Every input event goes to the UI. M0-T06 splits viewport strokes off
		// to the engine (`docs/ARCHITECTURE.md` §2.2).
		let ui = self.ui.clone();
		self.input_state.process(&event, |input| ui.send(UiCommand::Input(input)));

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
