//! The engine thread: processes [`EngineInput`] strictly in order, owns the
//! open documents and their views, runs imports as jobs, asks the render
//! thread for frames and keeps the UI informed.
//!
//! With no document open, the view navigates a virtual 30 000² document
//! ([`crate::view::VIRTUAL_DOC`]) drawn as a test pattern (M0).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select_biased};
use fx_io::{ImportedImage, IoError};
use fx_protocol::{DocId, EngineToUi, MemoryStats, UI_LOCAL_ACTION_PREFIXES, UiToEngine};
use fx_tiles::TileStore;

use crate::documents::{Documents, OpenDoc};
use crate::render::{Frame, MipWork, RenderRequest};
use crate::stats::RenderStats;
use crate::view::{Changed, VIRTUAL_DOC, ViewState};
use crate::{EngineInput, EngineOutput, OutputSink, PointerKind, layers, mips};

/// `view` messages to the UI are throttled to this interval (60 Hz).
const VIEW_MESSAGE_INTERVAL: Duration = Duration::from_micros(16_667);

/// How often the `status` message (memory, frame statistics) goes out.
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

/// The virtual M0 document's id in `view` messages.
const VIRTUAL_DOC_ID: DocId = DocId(0);

/// Work finished on other threads, reported back to the engine thread.
pub(crate) enum Internal {
	/// Import progress, 0..=1.
	Progress { task: u64, label: String, fraction: f32 },
	/// An import finished (mips included) or failed.
	Imported {
		task: u64,
		path: PathBuf,
		result: Result<ImportedImage, IoError>,
	},
}

struct Engine {
	output: OutputSink,
	render: Sender<RenderRequest>,
	internal: Sender<Internal>,
	store: Arc<TileStore>,
	stats: Arc<Mutex<RenderStats>>,
	next_status: Instant,
	docs: Documents,
	/// View of the virtual document, used while no document is open.
	virtual_view: ViewState,
	next_task: u64,
	last_view_message: Option<Instant>,
	view_message_pending: bool,
}

/// Channels and shared state the engine thread works with.
pub(crate) struct EngineContext {
	pub inputs: Receiver<EngineInput>,
	pub internal_rx: Receiver<Internal>,
	/// Handed to import jobs so they can report back.
	pub internal: Sender<Internal>,
	pub mips_rx: Receiver<MipWork>,
	pub render: Sender<RenderRequest>,
	pub store: Arc<TileStore>,
	pub stats: Arc<Mutex<RenderStats>>,
	pub output: OutputSink,
}

/// Body of the engine thread.
pub(crate) fn run(ctx: EngineContext) {
	let EngineContext {
		inputs,
		internal_rx,
		internal,
		mips_rx,
		render,
		store,
		stats,
		output,
	} = ctx;
	let mut engine = Engine {
		output,
		render,
		internal,
		store,
		stats,
		next_status: Instant::now() + STATUS_INTERVAL,
		docs: Documents::default(),
		virtual_view: ViewState::new(VIRTUAL_DOC),
		next_task: 0,
		last_view_message: None,
		view_message_pending: false,
	};

	loop {
		let deadline = engine.view_message_deadline().map_or(engine.next_status, |d| d.min(engine.next_status));
		let timeout = deadline.saturating_duration_since(Instant::now());
		select_biased! {
			recv(inputs) -> input => match input {
				Ok(EngineInput::Shutdown) | Err(_) => break,
				Ok(input) => engine.handle(input),
			},
			recv(internal_rx) -> msg => if let Ok(msg) = msg { engine.internal(msg) },
			recv(mips_rx) -> work => if let Ok(work) = work { engine.compute_mips(work) },
			default(timeout) => {}
		}
		engine.flush_view_message();
		engine.send_status_if_due();
	}

	let _ = engine.render.send(RenderRequest::Stop);
	tracing::debug!("engine thread finished");
}

impl Engine {
	/// The view of the active document, or of the virtual one.
	fn view_mut(&mut self) -> &mut ViewState {
		match self.docs.active_mut() {
			Some(doc) => &mut doc.view,
			None => &mut self.virtual_view,
		}
	}

	fn handle(&mut self, input: EngineInput) {
		let changed = match input {
			EngineInput::Ui(message) => self.ui_message(message),
			EngineInput::Pointer(pointer) => {
				if pointer.kind != PointerKind::Move || self.view_mut().dragging() {
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
				self.view_mut().pointer(&pointer)
			}
			EngineInput::Wheel { x, y, dx, dy, modifiers } => self.view_mut().wheel(x, y, dx, dy, modifiers),
			EngineInput::ViewportResized { width, height } => {
				// Every document shares the one viewport.
				let mut changed = self.virtual_view.resize(width, height);
				for doc in self.docs.iter_mut() {
					changed.view |= doc.view.resize(width, height).view;
				}
				changed
			}
			EngineInput::Open(paths) => {
				for path in paths {
					self.open(path);
				}
				Changed::default()
			}
			EngineInput::Shutdown => Changed::default(),
		};
		self.apply(changed);
	}

	fn apply(&mut self, changed: Changed) {
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
				// A reloaded page needs the documents and the view again.
				let ids = self.docs.ids();
				for id in ids {
					if let Some(doc) = self.docs.get_mut(id) {
						let info = doc.info();
						self.to_ui(&EngineToUi::DocumentOpened { info });
					}
				}
				self.to_ui(&EngineToUi::ActiveDocument { doc: self.docs.active_id() });
				self.send_layers();
				self.view_message_pending = true;
				Changed::default()
			}
			UiToEngine::Action { id, .. } => self.action(&id),
			UiToEngine::SetZoom { doc, zoom } => match self.docs.get_mut(doc) {
				Some(open) => open.view.set_zoom(zoom),
				None if doc == VIRTUAL_DOC_ID => self.virtual_view.set_zoom(zoom),
				None => Changed::default(),
			},
			UiToEngine::ActivateDocument { doc } => {
				if self.docs.activate(doc) {
					self.after_active_change();
				}
				Changed::default()
			}
			UiToEngine::CloseDocument { doc } => {
				self.close(doc);
				Changed::default()
			}
			// Shell messages never reach the engine; documents commands, undo
			// and thumbnails arrive with M2.
			other => {
				tracing::debug!("not handled yet: {other:?}");
				Changed::default()
			}
		}
	}

	fn action(&mut self, id: &str) -> Changed {
		match id {
			"tab:close" => {
				if let Some(active) = self.docs.active_id() {
					self.close(active);
				}
				return Changed::default();
			}
			"tab:close-all" => {
				for doc in self.docs.ids() {
					self.close(doc);
				}
				return Changed::default();
			}
			// The shell answers dlg:open with the native file dialog and sends
			// the chosen files as `EngineInput::Open`.
			"dlg:open" => return Changed::default(),
			_ => {}
		}
		if let Some(changed) = self.view_mut().action(id) {
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

	// ------------------------------------------------------------ documents

	/// Import `path` as a job: decode + mip pyramid on worker threads, with
	/// `progress` messages; the document appears when it is complete.
	fn open(&mut self, path: PathBuf) {
		self.next_task += 1;
		let task = self.next_task;
		let (store, internal) = (self.store.clone(), self.internal.clone());
		let label = format!(
			"Opening {}",
			path.file_name()
				.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
		);
		self.to_ui(&EngineToUi::Progress {
			task,
			label: label.clone(),
			fraction: 0.0,
		});
		let spawned = std::thread::Builder::new().name(format!("import-{task}")).spawn(move || {
			let mut last = 0.0f32;
			let mut report = |fraction: f32| {
				// ~1 % steps are plenty for a progress bar.
				if fraction - last >= 0.01 || fraction >= 1.0 {
					last = fraction;
					let _ = internal.send(Internal::Progress {
						task,
						label: label.clone(),
						fraction: fraction * 0.9,
					});
				}
				true
			};
			let result = fx_io::import_file(&path, &store, &mut report).and_then(|mut imported| {
				let _ = internal.send(Internal::Progress {
					task,
					label: format!("{label}: building previews"),
					fraction: 0.9,
				});
				mips::ensure_all_mips(&mut imported.image, &store)?;
				Ok(imported)
			});
			let _ = internal.send(Internal::Imported { task, path, result });
		});
		if let Err(error) = spawned {
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot start the import: {error}"),
			});
		}
	}

	fn internal(&mut self, message: Internal) {
		match message {
			Internal::Progress { task, label, fraction } => self.to_ui(&EngineToUi::Progress { task, label, fraction }),
			Internal::Imported { task, path, result } => {
				self.to_ui(&EngineToUi::ProgressDone { task });
				match result {
					Ok(imported) => {
						let id = self.docs.allocate_id();
						let mut doc = OpenDoc::from_import(id, &path, imported);
						if let Some(viewport) = self.virtual_view.viewport {
							doc.view.resize(viewport.width, viewport.height);
						}
						tracing::info!("opened {} as {id:?} ({} × {})", path.display(), doc.doc.width, doc.doc.height);
						let info = doc.info();
						self.docs.add(doc);
						self.to_ui(&EngineToUi::DocumentOpened { info });
						self.after_active_change();
					}
					Err(IoError::Cancelled) => {}
					Err(error) => {
						tracing::warn!("cannot open {}: {error}", path.display());
						self.to_ui(&EngineToUi::Error {
							text: format!("Could not open {}: {error}", path.display()),
						});
					}
				}
			}
		}
	}

	fn close(&mut self, id: DocId) {
		if self.docs.close(id).is_some() {
			// Dropping the document drops its tile handles: memory is freed.
			self.to_ui(&EngineToUi::DocumentClosed { doc: id });
			self.after_active_change();
		}
	}

	fn after_active_change(&mut self) {
		self.to_ui(&EngineToUi::ActiveDocument { doc: self.docs.active_id() });
		self.send_layers();
		self.request_frame();
		self.view_message_pending = true;
	}

	fn send_layers(&mut self) {
		if let Some(doc) = self.docs.active_mut() {
			let message = EngineToUi::Layers {
				doc: doc.id,
				revision: doc.doc.revision,
				layers: layers::layer_infos(&doc.doc),
			};
			self.to_ui(&message);
		}
	}

	/// Compute the dirty mip tiles a frame asked for, then redraw. Mips are
	/// derived data: no undo step, no revision bump (only a new snapshot).
	fn compute_mips(&mut self, work: MipWork) {
		let store = self.store.clone();
		let Some(doc) = self.docs.get_mut(work.doc) else { return };
		if doc.doc.revision != work.revision {
			// The document changed meanwhile; the next frame asks again.
			return;
		}
		for request in &work.requests {
			let Some(layer) = doc.doc.layer_mut(request.layer) else { continue };
			let image = if request.mask {
				match layer.mask.as_mut() {
					Some(mask) => &mut mask.image,
					None => continue,
				}
			} else {
				match &mut layer.kind {
					fx_core::LayerKind::Pixel { image, .. } => image,
					_ => continue,
				}
			};
			if let Err(error) = mips::ensure_mip(image, &store, request.level, request.x, request.y) {
				tracing::warn!("mip {request:?} failed: {error}");
			}
		}
		doc.invalidate_snapshot();
		if self.docs.active_id() == Some(work.doc) {
			self.request_frame();
		}
	}

	// ------------------------------------------------------------ output

	fn request_frame(&mut self) {
		let virtual_view = self.virtual_view.clone();
		let frame = match self.docs.active_mut() {
			Some(doc) => {
				let Some(viewport) = doc.view.viewport else { return };
				Frame {
					view: doc.view.view,
					viewport,
					doc: Some((doc.id, doc.snapshot())),
					virtual_doc: VIRTUAL_DOC,
				}
			}
			None => {
				let Some(viewport) = virtual_view.viewport else { return };
				Frame {
					view: virtual_view.view,
					viewport,
					doc: None,
					virtual_doc: VIRTUAL_DOC,
				}
			}
		};
		let _ = self.render.send(RenderRequest::Frame(frame));
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
		let (doc, view) = match self.docs.active_mut() {
			Some(doc) => (doc.id, doc.view.view),
			None => (VIRTUAL_DOC_ID, self.virtual_view.view),
		};
		self.to_ui(&EngineToUi::View {
			doc,
			zoom: view.zoom,
			center_x: view.center_x,
			center_y: view.center_y,
			rotation_deg: 0.0,
		});
		self.last_view_message = Some(Instant::now());
		self.view_message_pending = false;
	}

	/// Memory and frame statistics, twice per second (M1-T11).
	fn send_status_if_due(&mut self) {
		let now = Instant::now();
		if now < self.next_status {
			return;
		}
		self.next_status = now + STATUS_INTERVAL;
		let tiles = self.store.stats();
		let (frames, uploads, pending_loads, gpu_bytes) = {
			let mut stats = self.stats.lock().expect("render stats poisoned");
			(stats.summary(now), stats.uploads, stats.pending_loads, stats.gpu_bytes)
		};
		self.to_ui(&EngineToUi::Status {
			memory: MemoryStats {
				hot_bytes: tiles.hot_bytes,
				warm_bytes: tiles.warm_bytes,
				scratch_bytes: tiles.cold_bytes,
				gpu_bytes,
			},
			fps: frames.fps,
			frame_ms_p50: frames.p50_ms,
			frame_ms_p99: frames.p99_ms,
			uploads,
			pending_loads,
		});
	}

	fn to_ui(&self, message: &EngineToUi) {
		(self.output)(EngineOutput::ToUi(fx_protocol::encode_json(message)));
	}
}
