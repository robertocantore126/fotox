//! The engine thread: processes [`EngineInput`] strictly in order, owns the
//! view state, asks the render thread for frames and keeps the UI informed.
//!
//! M0 has no document yet: the view navigates a virtual 30 000² document
//! ([`crate::view::VIRTUAL_DOC`]) drawn by the test pattern.

use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use fx_protocol::{DocId, EngineToUi, UI_LOCAL_ACTION_PREFIXES, UiToEngine};

use crate::render::RenderRequest;
use crate::view::{Changed, VIRTUAL_DOC, ViewState};
use crate::{EngineInput, EngineOutput, OutputSink, PointerKind};

/// `view` messages to the UI are throttled to this interval (60 Hz).
const VIEW_MESSAGE_INTERVAL: Duration = Duration::from_micros(16_667);

/// The virtual M0 document's id in `view` messages.
const VIRTUAL_DOC_ID: DocId = DocId(0);

struct Engine {
	output: OutputSink,
	render: Sender<RenderRequest>,
	state: ViewState,
	/// When the last `view` message went out, and whether a newer view is
	/// waiting for the throttle interval to pass.
	last_view_message: Option<Instant>,
	view_message_pending: bool,
}

/// Body of the engine thread.
pub(crate) fn run(inputs: Receiver<EngineInput>, render: Sender<RenderRequest>, output: OutputSink) {
	let mut engine = Engine {
		output,
		render,
		state: ViewState::new(VIRTUAL_DOC),
		last_view_message: None,
		view_message_pending: false,
	};

	loop {
		let received = match engine.view_message_deadline() {
			Some(deadline) => inputs.recv_deadline(deadline),
			None => inputs.recv().map_err(|_| RecvTimeoutError::Disconnected),
		};
		match received {
			Ok(EngineInput::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
			Ok(input) => engine.handle(input),
			Err(RecvTimeoutError::Timeout) => {}
		}
		engine.flush_view_message();
	}

	let _ = engine.render.send(RenderRequest::Stop);
	tracing::debug!("engine thread finished");
}

impl Engine {
	fn handle(&mut self, input: EngineInput) {
		let changed = match input {
			EngineInput::Ui(message) => self.ui_message(message),
			EngineInput::Pointer(pointer) => {
				if pointer.kind != PointerKind::Move || self.state.dragging() {
					tracing::trace!(
						"pointer {:?} at ({:.1}, {:.1}) pressure {:.3} tilt ({}, {}) buttons {:#05b}",
						pointer.kind,
						pointer.x,
						pointer.y,
						pointer.pressure,
						pointer.tilt_x,
						pointer.tilt_y,
						pointer.buttons
					);
				}
				self.state.pointer(&pointer)
			}
			EngineInput::Wheel { x, y, dx, dy, modifiers } => self.state.wheel(x, y, dx, dy, modifiers),
			EngineInput::ViewportResized { width, height } => self.state.resize(width, height),
			EngineInput::Shutdown => Changed::default(),
		};
		if let Some(cursor) = changed.cursor {
			(self.output)(EngineOutput::Cursor(cursor));
		}
		if changed.view {
			self.request_frame();
			self.view_message_pending = true;
		}
	}

	fn ui_message(&mut self, message: UiToEngine) -> Changed {
		match message {
			UiToEngine::Hello { ui_version } => {
				tracing::info!("UI connected (ui_version {ui_version})");
				self.to_ui(&EngineToUi::Toast {
					text: "Engine connected".into(),
				});
				// A reloaded page needs the current view straight away.
				self.view_message_pending = true;
				Changed::default()
			}
			UiToEngine::Action { id, .. } => {
				if let Some(changed) = self.state.action(&id) {
					return changed;
				}
				if UI_LOCAL_ACTION_PREFIXES.iter().any(|prefix| id.starts_with(prefix)) {
					tracing::debug!("UI-local action {id}");
				} else {
					tracing::debug!("action {id} is not implemented yet");
					self.to_ui(&EngineToUi::Toast {
						text: format!("{id}: not implemented yet"),
					});
				}
				Changed::default()
			}
			UiToEngine::SetZoom { zoom, .. } => self.state.set_zoom(zoom),
			// Shell messages never reach the engine; everything else needs
			// documents (M1+).
			other => {
				tracing::debug!("not handled yet: {other:?}");
				Changed::default()
			}
		}
	}

	fn request_frame(&self) {
		let Some(viewport) = self.state.viewport else {
			return;
		};
		let _ = self.render.send(RenderRequest::Frame {
			view: self.state.view,
			viewport,
			doc: self.state.doc,
		});
	}

	fn view_message_deadline(&self) -> Option<Instant> {
		if !self.view_message_pending {
			return None;
		}
		Some(self.last_view_message.map_or_else(Instant::now, |last| last + VIEW_MESSAGE_INTERVAL))
	}

	/// Send the pending `view` message if the throttle interval has passed.
	fn flush_view_message(&mut self) {
		if !self.view_message_pending || self.view_message_deadline().is_some_and(|deadline| Instant::now() < deadline) {
			return;
		}
		let view = self.state.view;
		self.to_ui(&EngineToUi::View {
			doc: VIRTUAL_DOC_ID,
			zoom: view.zoom,
			center_x: view.center_x,
			center_y: view.center_y,
			rotation_deg: 0.0,
		});
		self.last_view_message = Some(Instant::now());
		self.view_message_pending = false;
	}

	fn to_ui(&self, message: &EngineToUi) {
		(self.output)(EngineOutput::ToUi(fx_protocol::encode_json(message)));
	}
}
