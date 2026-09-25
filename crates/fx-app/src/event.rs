//! Events that have to be handled on the main thread.
//!
//! The UI bridge thread and anything else that cannot touch the window or the
//! GPU sends an [`AppEvent`] through an [`AppEventScheduler`], which also wakes
//! the winit event loop so the event is picked up promptly instead of at the
//! next unrelated wake-up.

use crate::ui::Cursor;

/// Something that has to run on the main thread.
pub(crate) enum AppEvent {
	/// The UI finished loading and is ready to talk to native code.
	WebCommunicationInitialized,
	/// A freshly rendered frame of the UI, ready to composite and present.
	UiUpdate(wgpu::Texture),
	/// The UI asks for a different mouse cursor.
	CursorChange(Cursor),
	/// An `fx-protocol` frame from the UI (decoded by `bridge.rs`).
	UiMessage(Vec<u8>),
	/// Something the engine or render thread produced.
	Engine(fx_engine::EngineOutput),
	/// Files chosen in the native open dialog.
	OpenFiles(Vec<std::path::PathBuf>),
	/// File chosen in the native export dialog.
	ExportTo(std::path::PathBuf),
	/// The UI failed or crashed; the app cannot continue.
	UiCrashed,
	/// Leave the event loop and shut down.
	Exit,
}

/// Hands [`AppEvent`]s to the event loop from any thread.
#[derive(Clone)]
pub(crate) struct AppEventScheduler {
	pub(crate) proxy: winit::event_loop::EventLoopProxy,
	pub(crate) sender: std::sync::mpsc::Sender<AppEvent>,
}

impl AppEventScheduler {
	/// Queue an event and wake the event loop to handle it.
	pub(crate) fn schedule(&self, event: AppEvent) {
		let _ = self.sender.send(event);
		self.proxy.wake_up();
	}
}

/// Creates an [`AppEventScheduler`] that wakes a specific event loop.
pub(crate) trait CreateAppEventSchedulerEventLoopExt {
	/// Create a scheduler that wakes this event loop.
	fn create_app_event_scheduler(&self, sender: std::sync::mpsc::Sender<AppEvent>) -> AppEventScheduler;
}

impl CreateAppEventSchedulerEventLoopExt for winit::event_loop::EventLoop {
	fn create_app_event_scheduler(&self, sender: std::sync::mpsc::Sender<AppEvent>) -> AppEventScheduler {
		AppEventScheduler {
			proxy: self.create_proxy(),
			sender,
		}
	}
}
