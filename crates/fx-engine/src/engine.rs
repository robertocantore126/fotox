//! The engine thread: processes [`EngineInput`] strictly in order, owns the
//! open documents and their views, runs imports as jobs, asks the render
//! thread for frames and keeps the UI informed.
//!
//! With no document open, the view navigates a virtual 30 000² document
//! ([`crate::view::VIRTUAL_DOC`]) drawn as a test pattern (M0).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select_biased};
use fx_core::command::{LayerPropsPatch, NewLayer};
use fx_core::{Command, CommandContext, LayerId, LayerRef};
use fx_io::{ImportedImage, IoError};
use fx_protocol::{DocId, EngineToUi, MemoryStats, UI_LOCAL_ACTION_PREFIXES, UiToEngine};
use fx_tiles::{TileError, TileStore};

use crate::documents::{Documents, OpenDoc};
use crate::render::{Frame, MipWork, RenderRequest};
use crate::stats::RenderStats;
use crate::thumbs::{self, ThumbSource, Thumbnail};
use crate::view::{Changed, VIRTUAL_DOC, ViewState};
use crate::{EngineInput, EngineOutput, OutputSink, PointerKind, layers, mips};

/// `view` messages to the UI are throttled to this interval (60 Hz).
const VIEW_MESSAGE_INTERVAL: Duration = Duration::from_micros(16_667);

/// How often the `status` message (memory, frame statistics) goes out.
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

/// A repeat of the same edit within this interval replaces the previous
/// history step instead of adding one (slider drags, live dialogs).
const MERGE_EDITS_WITHIN: Duration = Duration::from_secs(1);

/// A layer's thumbnail is re-rendered at most this often while it changes (M2-T07).
const THUMBNAIL_INTERVAL: Duration = Duration::from_millis(500);

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
	/// An export finished or failed (M3).
	Exported { task: u64, path: PathBuf, result: Result<(), IoError> },
	/// The B3 layers are built (M2-T08).
	B3Built { task: u64, doc: DocId, layers: Vec<Arc<fx_core::Layer>> },
	/// A layer thumbnail finished rendering.
	Thumbnail {
		doc: DocId,
		layer: LayerId,
		revision: u64,
		result: Result<Thumbnail, TileError>,
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
	/// Thumbnails the Layers panel asked for (doc, layer) → size in px, when
	/// each was last rendered, and refreshes waiting for the throttle.
	thumbs_wanted: HashMap<(DocId, LayerId), u32>,
	thumbs_last: HashMap<(DocId, LayerId), Instant>,
	thumbs_due: HashMap<(DocId, LayerId), Instant>,
	/// The last mergeable edit: what it was, when, and how many undo steps
	/// the document had right after it (so an intervening undo or edit
	/// breaks the merge).
	last_edit: Option<(DocId, EditKey, Instant, usize)>,
}

/// What makes two consecutive edits "the same" for history merging.
#[derive(Clone, Debug, PartialEq)]
enum EditKey {
	/// `set_layer_props` of the same layer touching the same fields.
	Props(LayerRef, [bool; 8]),
	/// `set_adjustment` of the same layer.
	Adjustment(LayerRef),
}

impl EditKey {
	fn of(command: &Command) -> Option<Self> {
		match command {
			Command::SetLayerProps { layer, props } => {
				let LayerPropsPatch {
					name,
					visible,
					opacity,
					fill,
					blend,
					clipped,
					locked_pixels,
					locked_position,
				} = props;
				Some(Self::Props(
					layer.clone(),
					[
						name.is_some(),
						visible.is_some(),
						opacity.is_some(),
						fill.is_some(),
						blend.is_some(),
						clipped.is_some(),
						locked_pixels.is_some(),
						locked_position.is_some(),
					],
				))
			}
			Command::SetAdjustment { layer, .. } => Some(Self::Adjustment(layer.clone())),
			_ => None,
		}
	}
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
		thumbs_wanted: HashMap::new(),
		thumbs_last: HashMap::new(),
		thumbs_due: HashMap::new(),
		last_edit: None,
	};

	loop {
		let deadline = [engine.view_message_deadline(), engine.hot_expiry(), engine.thumbs_due.values().min().copied()]
			.into_iter()
			.flatten()
			.fold(engine.next_status, Instant::min);
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
		engine.cool_hot_layer();
		engine.render_due_thumbnails();
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
			EngineInput::Export(path) => {
				self.export(path);
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
			UiToEngine::Command { doc, command } => {
				self.command(doc, command);
				Changed::default()
			}
			UiToEngine::Undo { doc } => {
				self.step_history(doc, false);
				Changed::default()
			}
			UiToEngine::Redo { doc } => {
				self.step_history(doc, true);
				Changed::default()
			}
			UiToEngine::RequestThumbnails { doc, layers, size } => {
				for layer in layers {
					self.thumbs_wanted.insert((doc, layer), size);
					self.render_thumbnail(doc, layer);
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
			id if id.starts_with("layer:") && self.layer_action(id) => return Changed::default(),
			// Build the B3 benchmark on top of the active document (M2-T08).
			"debug:load-b3" => {
				self.load_b3();
				return Changed::default();
			}
			"tab:close-all" => {
				for doc in self.docs.ids() {
					self.close(doc);
				}
				return Changed::default();
			}
			// Edit ▸ Undo / Redo and Ctrl+Z / Ctrl+Shift+Z; Ctrl+Alt+Z toggles the
			// last step (redo if something was just undone, else undo).
			"hist:undo" | "hist:redo" | "hist:toggle" => {
				if let Some(active) = self.docs.active_mut() {
					let redo = match id {
						"hist:redo" => true,
						"hist:toggle" => active.history.can_redo(),
						_ => false,
					};
					let doc = active.id;
					self.step_history(doc, redo);
				}
				return Changed::default();
			}
			// The shell answers dlg:open with the native file dialog and sends
			// the chosen files as `EngineInput::Open`.
			"dlg:open" => return Changed::default(),
			// Likewise export: the save dialog, then `EngineInput::Export`.
			"export:png" | "export:tiff" => return Changed::default(),
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

	/// Export the active document as a job on a worker thread, with progress.
	fn export(&mut self, path: PathBuf) {
		let Some(open) = self.docs.active_mut() else {
			self.to_ui(&EngineToUi::Toast {
				text: "Open a document to export it".into(),
			});
			return;
		};
		// A snapshot: editing may go on while the export runs.
		let doc = open.doc.clone();
		let options = match crate::export::options_for(&doc, &path) {
			Ok(options) => options,
			Err(error) => {
				self.to_ui(&EngineToUi::Error {
					text: format!("Cannot export {}: {error}", path.display()),
				});
				return;
			}
		};
		self.next_task += 1;
		let task = self.next_task;
		let (store, internal) = (self.store.clone(), self.internal.clone());
		let label = format!(
			"Exporting {}",
			path.file_name()
				.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
		);
		self.to_ui(&EngineToUi::Progress {
			task,
			label: label.clone(),
			fraction: 0.0,
		});
		let spawned = std::thread::Builder::new().name(format!("export-{task}")).spawn(move || {
			let mut report = |fraction: f32| {
				let _ = internal.send(Internal::Progress {
					task,
					label: label.clone(),
					fraction,
				});
				true
			};
			let result = crate::export::export_document(&doc, &store, &path, options, &mut report);
			let _ = internal.send(Internal::Exported { task, path, result });
		});
		if let Err(error) = spawned {
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot start the export: {error}"),
			});
		}
	}

	fn internal(&mut self, message: Internal) {
		match message {
			Internal::Exported { task, path, result } => {
				self.to_ui(&EngineToUi::ProgressDone { task });
				match result {
					Ok(()) => {
						tracing::info!("exported {}", path.display());
						self.to_ui(&EngineToUi::Toast {
							text: format!("Exported {}", path.display()),
						});
					}
					Err(IoError::Cancelled) => {}
					Err(error) => {
						tracing::warn!("cannot export {}: {error}", path.display());
						self.to_ui(&EngineToUi::Error {
							text: format!("Could not export {}: {error}", path.display()),
						});
					}
				}
			}
			Internal::B3Built { task, doc, layers } => {
				self.to_ui(&EngineToUi::ProgressDone { task });
				let Some(open) = self.docs.get_mut(doc) else { return };
				// Built outside the history on purpose (a benchmark setup, not
				// an edit): the history restarts from here.
				open.doc.layers.extend(layers);
				open.doc.revision += 1;
				open.history = fx_core::History::default();
				open.dirty = true;
				open.changed();
				tracing::info!("B3 loaded into {doc:?}: {} layers", open.doc.layers.len());
				self.after_edit(doc, true);
			}
			Internal::Thumbnail { doc, layer, revision, result } => match result {
				Ok(thumb) => {
					let header = EngineToUi::Thumbnail {
						doc,
						layer,
						revision,
						width: thumb.width,
						height: thumb.height,
					};
					(self.output)(EngineOutput::ToUi(fx_protocol::encode_binary(&header, &thumb.pixels)));
				}
				Err(error) => tracing::warn!("thumbnail of {layer:?} failed: {error}"),
			},
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
			self.thumbs_wanted.retain(|(d, _), _| *d != id);
			self.thumbs_last.retain(|(d, _), _| *d != id);
			self.thumbs_due.retain(|(d, _), _| *d != id);
			// Dropping the document drops its tile handles: memory is freed.
			self.to_ui(&EngineToUi::DocumentClosed { doc: id });
			self.after_active_change();
		}
	}

	// ------------------------------------------------------------ editing

	/// Apply a document command through its history (M2).
	fn command(&mut self, id: DocId, command: Command) {
		let store = self.store.clone();
		let Some(doc) = self.docs.get_mut(id) else {
			tracing::warn!("command for unknown document {id:?}");
			return;
		};
		let now = Instant::now();
		let key = EditKey::of(&command);
		// Merge a repeat of the previous edit into its history step: undo it,
		// then apply the new value on the state before it.
		let merge = matches!((&self.last_edit, &key), (Some((d, k, at, steps)), Some(new))
			if *d == id && k == new && now.saturating_duration_since(*at) <= MERGE_EDITS_WITHIN
				&& *steps == doc.history.labels().count() && !doc.history.can_redo());
		let before_merge = merge.then(|| doc.doc.clone());
		if merge {
			doc.history.undo(&mut doc.doc);
		}
		let mut ctx = CommandContext { tiles: &store };
		let result = doc.history.execute(&mut doc.doc, command, &mut ctx);
		if let (Err(_), Some(before)) = (&result, before_merge) {
			// The new value was refused: put the merged step back as it was.
			doc.history.redo(&mut doc.doc);
			doc.doc = before;
		}
		self.last_edit = match (&result, key) {
			(Ok(_), Some(key)) => Some((id, key, now, doc.history.labels().count())),
			_ => None,
		};
		match result {
			Ok(effect) => {
				if !effect.selection_only {
					doc.dirty = true;
					doc.changed();
					let edited: Vec<LayerId> = effect.props_changed.iter().chain(&effect.pixels_changed).copied().collect();
					doc.note_edits(&edited, Instant::now());
				}
				let pixels = effect.pixels_changed.clone();
				self.after_edit(id, !effect.selection_only);
				for layer in pixels {
					self.refresh_thumbnail(id, layer);
				}
			}
			Err(error) => {
				tracing::debug!("command refused: {error}");
				self.to_ui(&EngineToUi::Error { text: error.to_string() });
			}
		}
	}

	/// Undo (`redo == false`) or redo one step.
	fn step_history(&mut self, id: DocId, redo: bool) {
		let Some(doc) = self.docs.get_mut(id) else { return };
		let stepped = if redo {
			doc.history.redo(&mut doc.doc)
		} else {
			doc.history.undo(&mut doc.doc)
		};
		if !stepped {
			return;
		}
		doc.dirty = true;
		doc.changed();
		self.after_edit(id, true);
		// Any layer may have changed: refresh every thumbnail on show.
		let wanted: Vec<LayerId> = self.thumbs_wanted.keys().filter(|(d, _)| *d == id).map(|(_, l)| *l).collect();
		for layer in wanted {
			self.refresh_thumbnail(id, layer);
		}
	}

	/// Tell the UI what an edit changed and redraw.
	fn after_edit(&mut self, id: DocId, content: bool) {
		let Some(doc) = self.docs.get_mut(id) else { return };
		let layers = EngineToUi::Layers {
			doc: id,
			revision: doc.doc.revision,
			layers: layers::layer_infos(&doc.doc),
		};
		let history = EngineToUi::History {
			doc: id,
			labels: doc.history.labels().chain(doc.history.redo_labels()).map(str::to_owned).collect(),
			current: doc.history.labels().count(),
			can_undo: doc.history.can_undo(),
			can_redo: doc.history.can_redo(),
		};
		let info = doc.info();
		self.to_ui(&layers);
		self.to_ui(&history);
		if content {
			self.to_ui(&EngineToUi::DocumentChanged { info });
			if self.docs.active_id() == Some(id) {
				self.request_frame();
			}
		}
	}

	fn hot_expiry(&mut self) -> Option<Instant> {
		self.docs.active_mut().and_then(|doc| doc.hot_expiry())
	}

	/// Let the hot layer cool down after 2 s without edits (M2-T05).
	fn cool_hot_layer(&mut self) {
		let now = Instant::now();
		if self.docs.active_mut().is_some_and(|doc| doc.expire_hot(now)) {
			self.request_frame();
		}
	}

	// ------------------------------------------------------------ thumbnails

	/// A layer changed: re-render its thumbnail if the panel shows it, at most
	/// every [`THUMBNAIL_INTERVAL`].
	fn refresh_thumbnail(&mut self, doc: DocId, layer: LayerId) {
		let key = (doc, layer);
		if !self.thumbs_wanted.contains_key(&key) {
			return;
		}
		let now = Instant::now();
		match self.thumbs_last.get(&key) {
			Some(&last) if now.saturating_duration_since(last) < THUMBNAIL_INTERVAL => {
				self.thumbs_due.insert(key, last + THUMBNAIL_INTERVAL);
			}
			_ => self.render_thumbnail(doc, layer),
		}
	}

	fn render_due_thumbnails(&mut self) {
		let now = Instant::now();
		let due: Vec<(DocId, LayerId)> = self.thumbs_due.iter().filter(|&(_, &at)| at <= now).map(|(k, _)| *k).collect();
		for (doc, layer) in due {
			self.thumbs_due.remove(&(doc, layer));
			self.render_thumbnail(doc, layer);
		}
	}

	/// Render a thumbnail on the rayon pool; the result comes back as
	/// `Internal::Thumbnail`.
	fn render_thumbnail(&mut self, id: DocId, layer_id: LayerId) {
		let Some(&size) = self.thumbs_wanted.get(&(id, layer_id)) else { return };
		let Some(doc) = self.docs.get_mut(id) else { return };
		let Some(layer) = doc.doc.layer(layer_id) else { return };
		let Some(source) = ThumbSource::of(&layer.kind) else { return };
		let (w, h, revision) = (doc.doc.width, doc.doc.height, doc.doc.revision);
		self.thumbs_last.insert((id, layer_id), Instant::now());
		let (store, internal) = (self.store.clone(), self.internal.clone());
		rayon::spawn(move || {
			let result = thumbs::render(source, w, h, size, &store);
			let _ = internal.send(Internal::Thumbnail {
				doc: id,
				layer: layer_id,
				revision,
				result,
			});
		});
	}

	/// Layer menu actions on the active document's selection, as commands.
	/// Returns false for the ones not implemented yet (merge, flatten, …).
	fn layer_action(&mut self, id: &str) -> bool {
		let Some(doc) = self.docs.active_mut() else { return false };
		let doc_id = doc.id;
		let selected: Vec<LayerRef> = doc.doc.selected.iter().map(|&l| LayerRef::Id(l)).collect();
		let active = doc.doc.active_layer();
		let hidden: Vec<LayerRef> = {
			let mut out = Vec::new();
			doc.doc.walk(|layer, _| {
				if !layer.visible {
					out.push(LayerRef::Id(layer.id));
				}
			});
			out
		};
		let clipped = active.and_then(|l| doc.doc.layer(l)).is_some_and(|l| l.clipped);
		let props = |patch: LayerPropsPatch| {
			selected
				.iter()
				.map(|l| Command::SetLayerProps {
					layer: l.clone(),
					props: patch.clone(),
				})
				.collect::<Vec<_>>()
		};
		let commands: Vec<Command> = match id {
			"layer:new" => vec![Command::AddLayer {
				layer: NewLayer::Pixel,
				name: None,
			}],
			"layer:new-group" => vec![Command::AddLayer {
				layer: NewLayer::Group,
				name: None,
			}],
			"layer:group" | "layer:group-from" if !selected.is_empty() => vec![Command::GroupLayers {
				layers: selected.clone(),
				name: None,
			}],
			// Without a pixel selection (M5), Ctrl+J duplicates the layer, like Photoshop.
			"layer:duplicate" | "layer:via-copy" if !selected.is_empty() => vec![Command::DuplicateLayers { layers: selected.clone() }],
			"layer:delete" if !selected.is_empty() => vec![Command::DeleteLayers { layers: selected.clone() }],
			"layer:delete-hidden" if !hidden.is_empty() => vec![Command::DeleteLayers { layers: hidden }],
			// Alt+Ctrl+G toggles the clipping mask of the active layer.
			"layer:clip" => active.map_or_else(Vec::new, |l| {
				vec![Command::SetLayerProps {
					layer: LayerRef::Id(l),
					props: LayerPropsPatch {
						clipped: Some(!clipped),
						..Default::default()
					},
				}]
			}),
			"layer:release-clip" => props(LayerPropsPatch {
				clipped: Some(false),
				..Default::default()
			}),
			"layer:hide" => props(LayerPropsPatch {
				visible: Some(false),
				..Default::default()
			}),
			"layer:lock" => props(LayerPropsPatch {
				locked_pixels: Some(true),
				locked_position: Some(true),
				..Default::default()
			}),
			"layer:group" | "layer:group-from" | "layer:duplicate" | "layer:via-copy" | "layer:delete" | "layer:delete-hidden" => {
				// Nothing selected / nothing hidden: nothing to do, and no toast.
				return true;
			}
			_ => return false,
		};
		for command in commands {
			self.command(doc_id, command);
		}
		true
	}

	fn load_b3(&mut self) {
		let Some(doc) = self.docs.active_mut() else {
			self.to_ui(&EngineToUi::Error {
				text: "Open B1 (or any document) first: B3 is built on top of it".into(),
			});
			return;
		};
		let ids: Vec<LayerId> = (0..crate::b3::PIXEL_LAYERS + crate::b3::ADJUSTMENT_LAYERS)
			.map(|_| doc.doc.allocate_layer_id())
			.collect();
		let (id, width, height, format) = (doc.id, doc.doc.width, doc.doc.height, doc.doc.color.depth.rgba_format());
		self.next_task += 1;
		let task = self.next_task;
		self.to_ui(&EngineToUi::Progress {
			task,
			label: "Building B3 (219 layers)".into(),
			fraction: 0.0,
		});
		let (store, internal) = (self.store.clone(), self.internal.clone());
		let spawned = std::thread::Builder::new().name("b3".into()).spawn(move || {
			let layers = crate::b3::build(width, height, format, &ids, 3, &store);
			let _ = internal.send(Internal::B3Built { task, doc: id, layers });
		});
		if let Err(error) = spawned {
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot build B3: {error}"),
			});
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
					generation: doc.generation,
					hot_layer: doc.hot_layer(),
					virtual_doc: VIRTUAL_DOC,
				}
			}
			None => {
				let Some(viewport) = virtual_view.viewport else { return };
				Frame {
					view: virtual_view.view,
					viewport,
					doc: None,
					generation: 0,
					hot_layer: None,
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
