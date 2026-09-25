//! The engine thread: processes [`EngineInput`] strictly in order, owns the
//! open documents and their views, runs imports as jobs, asks the render
//! thread for frames and keeps the UI informed.
//!
//! With no document open, the view navigates a virtual 30 000² document
//! ([`crate::view::VIRTUAL_DOC`]) drawn as a test pattern (M0).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select_biased};
use fx_core::command::{LayerPropsPatch, MaskFill, NewLayer};
use fx_core::pixels::{Content, Placed, content_bounds};
use fx_core::{
	ColorProfile, Command, CommandContext, CommandEffect, CommandError, Document, FilterParams, LayerId, LayerKind, LayerRef, Permutation, PixelOps,
};
use fx_io::fxd::{self, FxdFile, OpenedFxd, SaveRequest, SaveTarget};
use fx_io::{ImportedImage, IoError};
use fx_protocol::{CloseAnswer, DocId, EngineToUi, MemoryStats, UI_LOCAL_ACTION_PREFIXES, UiToEngine};
use fx_tiles::{PixelFormat, PixelValue, TILE_SIZE, TileError, TileSlot, TileStore, TiledImage};

use crate::documents::{Documents, OpenDoc};
use crate::filters::{FilterPreview, PreviewJob};
use crate::ops::EngineOps;
use crate::render::{Frame, MipWork, RenderRequest};
use crate::stats::RenderStats;
use crate::thumbs::{self, ThumbSource, Thumbnail};
use crate::tools::transform::{self as free_transform, Mode as TransformMode, Update as TransformUpdate};
use crate::tools::{ColorTarget, DocPointer, ToolContext, ToolResult, ToolSettings, Tools};
use crate::transform_preview::{Prepared, PreviewJob as TransformJob, TransformPreview};
use crate::view::{Changed, VIRTUAL_DOC, ViewState};
use crate::{CursorShape, EngineInput, EngineOutput, Modifiers, OutputSink, PointerKind, filters, layers, mips};

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

/// True when `path` starts with the `.fxd` magic (M3).
fn is_fxd(path: &std::path::Path) -> bool {
	use std::io::Read;
	let Ok(mut file) = std::fs::File::open(path) else { return false };
	let mut header = [0u8; 8];
	let mut n = 0;
	while n < header.len() {
		match file.read(&mut header[n..]) {
			Ok(0) => break,
			Ok(k) => n += k,
			Err(_) => return false,
		}
	}
	matches!(fx_io::sniff(&header[..n]), Some(fx_io::Sniffed::Fxd))
}

/// True when `a` and `b` name the same file (case-insensitive on Windows,
/// resolved through `canonicalize` when both exist).
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
	match (a.canonicalize(), b.canonicalize()) {
		(Ok(a), Ok(b)) => a == b,
		_ => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
	}
}

/// A `.fxd` file name for a document called `name` (`"Untitled"`, `"sky"` →
/// `"sky.fxd"`, `"sky.tif"` → `"sky.fxd"`).
fn suggested_fxd_name(name: &str) -> String {
	match name.rsplit_once('.') {
		Some((stem, _)) if !stem.is_empty() => format!("{stem}.fxd"),
		_ => format!("{name}.fxd"),
	}
}
/// Display LUTs kept alive for reuse (M4-T02): the active document, plus a
/// couple of recently used ones, so switching tabs does not rebuild 35 937
/// samples each time.
const DISPLAY_LUT_CACHE: usize = 4;

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
	/// A `.fxd` was opened lazily (M3-T05).
	OpenedFxd {
		task: u64,
		path: PathBuf,
		result: Result<Box<OpenedFxd>, IoError>,
	},
	/// A save finished; `Ok` carries the reopened file (M3-T06).
	Saved {
		task: u64,
		doc: DocId,
		path: PathBuf,
		/// The document's generation when the snapshot was taken: edits made
		/// during the save keep it dirty.
		generation: u64,
		result: Result<Arc<FxdFile>, IoError>,
	},
	/// Copy Merged finished (M5-T05).
	Copied {
		task: u64,
		result: Result<Option<fx_core::pixels::ClipboardImage>, String>,
	},
	/// The B3 layers are built (M2-T08).
	B3Built { task: u64, doc: DocId, layers: Vec<Arc<fx_core::Layer>> },
	/// Free Transform's source is cut out and its mips are valid (M6-T04).
	TransformPrepared {
		doc: DocId,
		layer: LayerId,
		result: Result<Option<Prepared>, TileError>,
	},
	/// A Free Transform preview update (M6-T04).
	TransformShown {
		doc: DocId,
		request: u64,
		result: Result<Option<crate::transform_preview::Shown>, TileError>,
	},
	/// A batch of filter-preview tiles (M4-T05).
	PreviewTiles {
		doc: DocId,
		request: u64,
		tiles: Vec<((u32, u32), fx_tiles::TileBuffer)>,
	},
	/// A preview job failed (a tile could not be read).
	PreviewFailed { doc: DocId, request: u64, error: TileError },
	/// A pixel job (filter, merge, flatten) finished: the new document, or why not.
	PixelJobDone {
		task: u64,
		doc: DocId,
		command: Box<Command>,
		result: Result<(Box<Document>, CommandEffect), CommandError>,
	},
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
	/// Pixel operations for commands applied on the engine thread.
	ops: EngineOps,
	/// Per document, the latest filter-preview request (jobs compare with it
	/// and stop when superseded).
	preview_latest: HashMap<DocId, Arc<AtomicU64>>,
	/// The last filter applied, for Filter ▸ Last Filter (Ctrl+F).
	last_filter: Option<FilterParams>,
	/// A document waiting to be closed once its save finishes (M3-T06).
	pending_close: Option<DocId>,
	/// The window is closing: after each dirty document is answered, ask about
	/// the next one (M3-T06).
	window_close_pending: bool,
	/// ICC bytes of the monitor the window is on (M4-T02): `None` = assume
	/// sRGB (no profile reported, or the shell could not read one).
	display_profile: Option<Vec<u8>>,
	/// Display LUTs built so far, most recent last: `(key, table)`.
	/// `None` = that transform is (nearly) the identity: no LUT.
	display_luts: Vec<(u64, Option<Arc<fx_color::Lut3d>>)>,
	/// The viewport tools, created on first use (M5-T01).
	tools: Tools,
	/// Colours and option-bar values the tools read (M5-T01).
	settings: ToolSettings,
	/// The marching-ants overlay of the selection, cached per document,
	/// generation, view level and visible rectangle (M5-T03).
	selection_overlay: Option<(SelectionOverlayKey, Arc<fx_render::Overlay>)>,
	/// The brush stroke being painted (M5-T07).
	stroke: Option<crate::stroke::Session>,
	/// The Free Transform box that is up, and on which document (M6-T04).
	transform: Option<(DocId, free_transform::Session)>,
	/// The latest transform-preview request (jobs compare with it).
	transform_latest: Arc<AtomicU64>,
	/// When a drag's coarse preview is refined at the view level.
	transform_refine: Option<Instant>,
}

/// How long the pointer must rest before a dragged transform is previewed at
/// full view resolution (M6-T04).
const TRANSFORM_REFINE_AFTER: Duration = Duration::from_millis(150);

/// Cache key of the selection contour (M5-T03).
type SelectionOverlayKey = (DocId, u64, usize, (i64, i64, i64, i64));

/// What makes two consecutive edits "the same" for history merging.
#[derive(Clone, Debug, PartialEq)]
enum EditKey {
	/// `set_layer_props` of the same layer touching the same fields.
	Props(LayerRef, [bool; 9]),
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
					locked_transparency,
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
						locked_transparency.is_some(),
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
		ops: EngineOps::default(),
		preview_latest: HashMap::new(),
		last_filter: None,
		pending_close: None,
		window_close_pending: false,
		display_profile: None,
		display_luts: Vec::new(),
		tools: Tools::default(),
		settings: ToolSettings::default(),
		selection_overlay: None,
		stroke: None,
		transform: None,
		transform_latest: Arc::new(AtomicU64::new(0)),
		transform_refine: None,
	};

	loop {
		let deadline = [
			engine.view_message_deadline(),
			engine.hot_expiry(),
			engine.thumbs_due.values().min().copied(),
			engine.transform_refine,
		]
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
		engine.refine_transform_if_due();
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
				let outcome = self.view_mut().pointer(&pointer);
				let mut changed = outcome.changed;
				if !outcome.consumed {
					changed = self.tool_pointer(&pointer, changed);
				}
				changed
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
			EngineInput::Export { path, choice } => {
				self.export(path, choice);
				Changed::default()
			}
			EngineInput::Save { doc } => {
				self.save(doc);
				Changed::default()
			}
			EngineInput::SaveAs { doc, path } => {
				self.save_as(doc, path);
				Changed::default()
			}
			EngineInput::SaveCancelled { doc } => {
				if self.pending_close == Some(doc) {
					self.pending_close = None;
					self.window_close_pending = false;
				}
				Changed::default()
			}
			EngineInput::CloseRequested => {
				self.close_requested();
				Changed::default()
			}
			EngineInput::DisplayProfile(bytes) => {
				self.set_display_profile(bytes);
				Changed { view: true, cursor: None }
			}
			EngineInput::PasteImage { width, height, rgba8 } => {
				self.paste_image(width, height, &rgba8);
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
			self.refresh_preview();
			if self.transform.is_some() {
				self.restart_transform_preview(false);
			}
			self.request_frame();
			self.view_message_pending = true;
		}
	}

	/// Route a pointer event the view did not consume to the active tool
	/// (M5-T01): the event is mapped into document coordinates, the tool's
	/// answer is applied here (a command, a cursor, a picked colour).
	fn tool_pointer(&mut self, input: &crate::PointerInput, changed: Changed) -> Changed {
		let mut changed = changed;
		let Some(doc_id) = self.docs.active_id() else {
			return changed;
		};
		// Map the event through the inverse of the view transform.
		let (tool_id, event) = {
			let Some(open) = self.docs.get_mut(doc_id) else {
				return changed;
			};
			let Some(viewport) = open.view.viewport else {
				return changed;
			};
			let (x, y) = open.view.view.screen_to_doc(viewport, input.x, input.y);
			(
				open.view.tool.clone(),
				DocPointer {
					kind: input.kind,
					x,
					y,
					pressure: input.pressure,
					tilt_x: input.tilt_x,
					tilt_y: input.tilt_y,
					buttons: input.buttons,
					modifiers: input.modifiers,
					time_us: input.time_us,
				},
			)
		};
		// A Free Transform box takes every pointer event while it is up.
		if let Some((doc, session)) = &mut self.transform
			&& *doc == doc_id
		{
			let zoom = self.docs.get(doc_id).map_or(1.0, |open| open.view.view.zoom);
			let update = session.pointer(&event, zoom);
			changed.cursor = Some(session.cursor());
			self.transform_update(doc_id, update);
			return changed;
		}
		let store = self.store.clone();
		let (result, idle_cursor) = {
			let Some(tool) = self.tools.get(&tool_id) else {
				return changed;
			};
			let idle = tool.cursor(event.modifiers);
			let Some(open) = self.docs.get_mut(doc_id) else {
				return changed;
			};
			let mask_target = open.paints_mask();
			let mut ctx = ToolContext {
				doc: &mut open.doc,
				store: &store,
				ops: &self.ops,
				settings: &self.settings,
				view: open.view.view,
				mask_target,
			};
			(tool.pointer(&mut ctx, &event), idle)
		};
		// The view puts `Default` in for a plain hover (no gesture owns the
		// cursor). A tool that has an idle cursor — the marquee's crosshair —
		// takes that slot instead; a pan's Grab/Grabbing keeps it.
		if changed.cursor == Some(CursorShape::Default) {
			changed.cursor = Some(idle_cursor);
		}
		self.apply_tool_result(doc_id, result, &mut changed);
		changed
	}

	/// Route a key the UI's shortcut map did not consume to the active tool
	/// (M5-T04): Escape cancels the operation under way, Enter closes a
	/// polygonal lasso, Backspace drops its last point.
	fn tool_key(&mut self, key: &str) -> Changed {
		let mut changed = Changed::default();
		let Some(doc_id) = self.docs.active_id() else {
			return changed;
		};
		if let Some((doc, session)) = &mut self.transform
			&& *doc == doc_id
		{
			let update = session.key(key);
			self.transform_update(doc_id, update);
			return changed;
		}
		let tool_id = self.docs.get(doc_id).map_or_else(String::new, |open| open.view.tool.clone());
		let store = self.store.clone();
		let result = match (self.tools.get(&tool_id), self.docs.get_mut(doc_id)) {
			(Some(tool), Some(open)) => {
				let mask_target = open.paints_mask();
				let mut ctx = ToolContext {
					doc: &mut open.doc,
					store: &store,
					ops: &self.ops,
					settings: &self.settings,
					view: open.view.view,
					mask_target,
				};
				tool.key(&mut ctx, key)
			}
			// A tool Fotox does not implement uses no key.
			_ => ToolResult::default(),
		};
		// Delete / Backspace that no tool used: Edit ▸ Clear (M5-T05).
		let unused = result.command.is_none() && !result.redraw && result.info.is_none() && result.cursor.is_none();
		if unused && matches!(key, "Delete" | "Backspace") {
			self.edit_action("clip:clear", &serde_json::Value::Null);
			return changed;
		}
		self.apply_tool_result(doc_id, result, &mut changed);
		changed
	}

	/// Apply what a tool answered (M5-T01): the cursor, a status line, a picked
	/// colour for the UI, a command to execute (R1), or an overlay redraw
	/// (M5-T04: a marquee or lasso rubber band, which needs no re-composite).
	fn apply_tool_result(&mut self, doc_id: DocId, result: ToolResult, changed: &mut Changed) {
		if let Some(cursor) = result.cursor {
			changed.cursor = Some(cursor);
		}
		if let Some(info) = result.info {
			self.to_ui(&EngineToUi::Toast { text: info });
		}
		if let Some(text) = result.status {
			self.to_ui(&EngineToUi::ToolInfo { text });
		}
		if let Some((rgba, target)) = result.picked {
			match target {
				ColorTarget::Foreground => self.settings.fg = rgba,
				ColorTarget::Background => self.settings.bg = rgba,
			}
			self.to_ui(&EngineToUi::ColorPicked {
				rgba,
				target: target.as_str().into(),
			});
		}
		for event in result.strokes {
			self.stroke_event(doc_id, event);
		}
		if result.redraw {
			self.request_frame();
		}
		if let Some(command) = result.command {
			// A command that changes the canvas hands the tool the new document
			// when it lands (`after_edit`), which for a job is later than now.
			self.command(doc_id, command);
		}
	}

	/// Hand the active tool the active document again (M6-T03): a crop box
	/// starts as the canvas, so a new canvas size, an undo across a crop or
	/// another document starts it over. `activate` is the only hook a tool gets
	/// before its first event; the tools that keep no box ignore it.
	fn reactivate_tool(&mut self) {
		let Some(open) = self.docs.active_id().and_then(|id| self.docs.get(id)) else {
			return;
		};
		if let Some(tool) = self.tools.get(&open.view.tool) {
			tool.activate(&open.doc);
		}
	}

	/// A painting tool's stroke event (M5-T07): start, paint, record.
	fn stroke_event(&mut self, doc_id: DocId, event: crate::tools::StrokeEvent) {
		use crate::tools::StrokeEvent;
		match event {
			StrokeEvent::Begin {
				target,
				tool,
				brush,
				color,
				samples,
			} => {
				self.end_stroke();
				let store = self.store.clone();
				let Some(open) = self.docs.get_mut(doc_id) else { return };
				if let Some(job) = &open.busy {
					let text = format!("Wait until {job} is finished");
					self.to_ui(&EngineToUi::Toast { text });
					return;
				}
				let Some(layer) = open.doc.active_layer() else {
					self.to_ui(&EngineToUi::Toast {
						text: "Select a layer to paint on".into(),
					});
					return;
				};
				if open.doc.layer(layer).is_some_and(|l| l.locked_pixels) {
					self.to_ui(&EngineToUi::Toast {
						text: "Could not paint: the layer's pixels are locked".into(),
					});
					return;
				}
				let before = open.doc.clone();
				let prepared = match crate::stroke::prepare(&before, layer, target, &tool, &store) {
					Ok(prepared) => prepared,
					Err(error) => {
						self.to_ui(&EngineToUi::Toast { text: error.to_string() });
						return;
					}
				};
				let stroke = match fx_ops::brush::Stroke::begin(crate::stroke::setup(&prepared, &before, tool, brush, color), &store) {
					Ok(stroke) => stroke,
					Err(error) => {
						self.to_ui(&EngineToUi::Toast { text: error.to_string() });
						return;
					}
				};
				// The layer holds the (possibly grown) image while the stroke runs.
				let (image, offset) = stroke.start();
				set_stroke_image(&mut open.doc, layer, target, image.clone(), offset);
				self.stroke = Some(crate::stroke::Session {
					doc: doc_id,
					layer,
					target,
					tool,
					brush,
					color,
					before,
					stroke,
					pending_input: None,
				});
				self.paint_samples(&samples);
			}
			StrokeEvent::Add(samples) => self.paint_samples(&samples),
			StrokeEvent::End => self.end_stroke(),
		}
	}

	/// Paint samples of the live stroke and show them.
	fn paint_samples(&mut self, samples: &[fx_core::stroke::StrokeSample]) {
		let Some(session) = &mut self.stroke else { return };
		if samples.is_empty() {
			return;
		}
		let tiles = match session.stroke.add(samples) {
			Ok(tiles) => tiles,
			Err(error) => {
				tracing::warn!("a stroke could not paint: {error}");
				return;
			}
		};
		// The input arrived now (the shell forwards pointer events at once).
		session.pending_input.get_or_insert_with(Instant::now);
		let (doc_id, layer, target) = (session.doc, session.layer, session.target);
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		let level = open.view.view.mip_level(usize::MAX);
		if let Some(image) = stroke_image(&mut open.doc, layer, target) {
			for ((tx, ty), buffer) in tiles.iter().cloned() {
				image.put_buffer(&store, tx, ty, buffer);
			}
			// Visible at fit right away (S15): the touched tiles' mips up to the
			// view level, only those.
			let level = level.min(image.level_count().saturating_sub(1));
			if level > 0 {
				let mut ancestors: Vec<(u32, u32)> = tiles.iter().map(|((tx, ty), _)| (tx >> level, ty >> level)).collect();
				ancestors.sort_unstable();
				ancestors.dedup();
				for (ax, ay) in ancestors {
					if let Err(error) = crate::mips::ensure_mip(image, &store, level, ax, ay) {
						tracing::debug!("stroke mip: {error}");
					}
				}
			}
		}
		open.doc.revision += 1;
		open.changed();
		open.note_edits(&[layer], Instant::now());
		self.request_frame();
	}

	/// Finish the live stroke, if any: its final pixels, one History step.
	fn end_stroke(&mut self) {
		let Some(session) = self.stroke.take() else { return };
		let crate::stroke::Session {
			doc: doc_id,
			layer,
			target,
			tool,
			brush,
			color,
			before,
			stroke,
			..
		} = session;
		let samples = stroke.samples().to_vec();
		let finished = stroke.finish();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		match finished {
			Ok((image, offset)) => {
				set_stroke_image(&mut open.doc, layer, target, image, offset);
				open.doc.revision += 1;
				let command = Command::Stroke {
					layer: LayerRef::Id(layer),
					target,
					tool,
					brush,
					color,
					samples,
				};
				open.history.record(before, command, tool.label().to_owned());
				open.dirty = true;
				open.changed();
				open.note_edits(&[layer], Instant::now());
				self.last_edit = None;
				self.after_edit(doc_id, true);
				self.refresh_thumbnail(doc_id, layer);
			}
			Err(error) => {
				// Put the document back as it was before the stroke.
				open.doc = before;
				open.changed();
				self.to_ui(&EngineToUi::Error {
					text: format!("The stroke failed: {error}"),
				});
				self.request_frame();
			}
		}
	}

	fn ui_message(&mut self, message: UiToEngine) -> Changed {
		match message {
			UiToEngine::Hello { ui_version } => {
				tracing::info!("UI connected (ui_version {ui_version})");
				let profiles = cmyk_profile_files()
					.into_iter()
					.map(|p| fx_protocol::CmykProfileInfo {
						name: p.name,
						path: p.path.display().to_string(),
					})
					.collect();
				self.to_ui(&EngineToUi::CmykProfiles { profiles });
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
			UiToEngine::Action { id, args } => {
				if self.edit_action(&id, &args) {
					Changed::default()
				} else {
					self.action(&id)
				}
			}
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
			UiToEngine::CloseDocumentAnswer { doc, answer } => {
				self.close_answer(doc, answer);
				Changed::default()
			}
			UiToEngine::FilterPreview { doc, layer, filter } => {
				self.start_preview(doc, layer, filter);
				Changed::default()
			}
			UiToEngine::FilterPreviewCancel { doc } => {
				self.cancel_preview(doc);
				Changed::default()
			}
			UiToEngine::ToolOptions { tool, options } => {
				let transform = tool == "_transform";
				self.settings.options.insert(tool, options);
				// The transform bar's Interpolation applies to the box that is up.
				if transform && self.transform.is_some() {
					let filter = self.transform_filter();
					if let Some((_, session)) = &mut self.transform {
						session.filter = filter;
					}
					self.restart_transform_preview(false);
				}
				Changed::default()
			}
			UiToEngine::SetColors { fg, bg } => {
				self.settings.fg = fg;
				self.settings.bg = bg;
				Changed::default()
			}
			UiToEngine::Key { key } => self.tool_key(&key),
			UiToEngine::ProofSetup {
				doc,
				path,
				intent,
				bpc,
				simulate_paper,
			} => {
				self.proof_setup(doc, PathBuf::from(path), intent, bpc, simulate_paper);
				Changed { view: true, cursor: None }
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
			// View ▸ Proof Colors (Ctrl+Y) / Gamut Warning (Shift+Ctrl+Y), M4-T04.
			"view:proof-colors" | "view:gamut-warning" => {
				self.toggle_proof(id == "view:gamut-warning");
				return Changed { view: true, cursor: None };
			}
			// Filter ▸ Last Filter (Ctrl+F): the last filter, same parameters.
			"filter:last" => {
				match (self.docs.active_id(), self.last_filter.clone()) {
					(Some(doc), Some(filter)) => self.command(
						doc,
						Command::ApplyFilter {
							layer: LayerRef::Active,
							filter,
						},
					),
					_ => self.to_ui(&EngineToUi::Toast {
						text: "No filter applied yet".into(),
					}),
				}
				return Changed::default();
			}
			// Image ▸ Rotate / Flip (M6-T02): the canvas-level permutations. One
			// action = one command, so History shows them like Photoshop does.
			"img:rot90cw" | "img:rot90ccw" | "img:rot180" | "img:flip-h" | "img:flip-v" => {
				if let Some(doc) = self.docs.active_id() {
					let command = match id {
						"img:rot90cw" => Command::RotateCanvas { quarter_turns: 1 },
						"img:rot90ccw" => Command::RotateCanvas { quarter_turns: 3 },
						"img:rot180" => Command::RotateCanvas { quarter_turns: 2 },
						"img:flip-h" => Command::FlipCanvas { horizontal: true },
						_ => Command::FlipCanvas { horizontal: false },
					};
					self.command(doc, command);
				}
				return Changed::default();
			}
			// Edit ▸ Free Transform (Ctrl+T) and Edit ▸ Transform ▸ … (M6-T04).
			id if id.starts_with("xf:") => {
				self.transform_action(id);
				return Changed::default();
			}
			// Image ▸ Crop (M6-T03): the canvas becomes the pixels the selection
			// covers. Without a selection there is nothing to crop to, exactly
			// like the greyed-out item in Photoshop.
			"img:crop" => {
				self.crop_to_selection();
				return Changed::default();
			}
			// A tool's ✓ and ✗ in the option bar (M6-T03): the same keys Enter
			// and Escape send to the active tool, so the crop box commits and
			// cancels exactly as it does from the keyboard. Free Transform
			// (M6-T04) reuses them.
			"tool:commit" => return self.tool_key("Enter"),
			"tool:cancel" => return self.tool_key("Escape"),
			// Select ▸ All / Deselect / Reselect / Inverse (M5-T04): the four
			// selection commands the M5-T03 core added. Deselect and Reselect
			// are quiet when there is nothing to do: Photoshop greys the items
			// out, and the UI cannot know the selection state yet (T05/T10).
			"sel:all" | "sel:none" | "sel:reselect" | "sel:inverse" => {
				if let Some(doc) = self.docs.active_id() {
					let command = match id {
						"sel:all" => Some(Command::SelectAll),
						"sel:none" => self.docs.get(doc).filter(|open| open.doc.selection.is_some()).map(|_| Command::Deselect),
						"sel:reselect" => self.docs.get(doc).filter(|open| open.doc.reselect.is_some()).map(|_| Command::Reselect),
						_ => Some(Command::InvertSelection),
					};
					if let Some(command) = command {
						self.command(doc, command);
					}
				}
				return Changed::default();
			}
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
			// File ▸ Save / Save As (M3-T06). Save As always asks the shell for a
			// path; Save only does so when the document has no file yet.
			"doc:save" => {
				if let Some(doc) = self.docs.active_id() {
					self.save(doc);
				}
				return Changed::default();
			}
			"doc:save-as" => {
				if let Some(doc) = self.docs.active_id() {
					let name = self.docs.get_mut(doc).map_or_else(String::new, |open| open.name.clone());
					self.ask_save_path(doc, &name);
				}
				return Changed::default();
			}
			// Likewise export: the save dialog, then `EngineInput::Export`.
			"export:png" | "export:tiff" | "export:jpg" | "export:as" => return Changed::default(),
			_ => {}
		}
		if let Some(changed) = self.view_mut().action(id) {
			let mut changed = changed;
			// A tool change also sets the tool's cursor (M5-T01) and hands the
			// tool the document, so a crop box starts as the canvas and the view
			// draws it at once (M6-T03).
			if let Some(tool) = id.strip_prefix("tool:")
				&& let Some(t) = self.tools.get(tool)
			{
				changed.cursor = Some(t.cursor(Modifiers::default()));
				if let Some(open) = self.docs.active_id().and_then(|id| self.docs.get(id)) {
					t.activate(&open.doc);
				}
				changed.view = true;
			}
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
			// A native `.fxd` opens lazily, reading only the manifest (M3-T05);
			// every other file imports band by band.
			if is_fxd(&path) {
				let _ = internal.send(Internal::Progress {
					task,
					label: format!("{label}: reading the manifest"),
					fraction: 0.5,
				});
				let result = fxd::open(&path, &store).map(Box::new);
				let _ = internal.send(Internal::OpenedFxd { task, path, result });
				return;
			}
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
	fn export(&mut self, path: PathBuf, choice: Option<crate::ExportChoice>) {
		let Some(open) = self.docs.active_mut() else {
			self.to_ui(&EngineToUi::Toast {
				text: "Open a document to export it".into(),
			});
			return;
		};
		// A snapshot: editing may go on while the export runs.
		let doc = open.doc.clone();
		if let Err(error) = crate::export::options_for(&doc, &path, false) {
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot export {}: {error}", path.display()),
			});
			return;
		}
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
			// An opaque document is written without alpha (a quarter smaller for RGB).
			let opaque = crate::export::opaque_background(&doc, &store);
			let result = crate::export::options_for(&doc, &path, opaque)
				.and_then(|options| crate::export::apply_choice(options, choice, opaque, &doc.color.profile))
				.and_then(|options| crate::export::export_document(&doc, &store, &path, options, &mut report));
			let _ = internal.send(Internal::Exported { task, path, result });
		});
		if let Err(error) = spawned {
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot start the export: {error}"),
			});
		}
	}

	// ------------------------------------------------------------ free transform (M6-T04)

	/// `xf:free` (Ctrl+T) and the Transform submenu: start a box, or switch the
	/// box's gesture; the turns and flips act on the box when one is up, else
	/// they transform the active layer at once.
	fn transform_action(&mut self, id: &str) {
		let Some(doc_id) = self.docs.active_id() else { return };
		let up = matches!(self.transform, Some((doc, _)) if doc == doc_id);
		if let Some(mode) = TransformMode::of_action(id) {
			if up {
				if let Some((_, session)) = &mut self.transform {
					session.set_mode(mode);
				}
				self.transform_update(doc_id, TransformUpdate::Changed { dragging: false });
			} else {
				self.start_transform(doc_id, mode);
			}
			return;
		}
		let Some(linear) = free_transform::instant_turn(id) else {
			return;
		};
		if up {
			if let Some((_, session)) = &mut self.transform {
				session.turn(linear);
			}
			self.transform_update(doc_id, TransformUpdate::Changed { dragging: false });
			return;
		}
		// No box: the whole layer (or the selection) at once, about its centre.
		let Some(open) = self.docs.get(doc_id) else { return };
		let Some(layer) = open.doc.active_layer() else { return };
		match crate::transform_preview::start_rect(&open.doc, layer, &self.store) {
			Ok(Some(rect)) => {
				let mapping = free_transform::turn_mapping(linear, rect);
				self.command(
					doc_id,
					Command::Transform {
						layer: LayerRef::Id(layer),
						mapping: Box::new(mapping),
						filter: fx_core::Filter::Bicubic,
					},
				);
			}
			Ok(None) => self.to_ui(&EngineToUi::Toast {
				text: "Nothing to transform: the layer is empty".into(),
			}),
			Err(error) => self.to_ui(&EngineToUi::Error { text: error.to_string() }),
		}
	}

	/// Put a Free Transform box over the active layer (or the selection).
	fn start_transform(&mut self, doc_id: DocId, mode: TransformMode) {
		let filter = self.transform_filter();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		if let Some(job) = &open.busy {
			let text = format!("Wait until {job} is finished");
			self.to_ui(&EngineToUi::Toast { text });
			return;
		}
		let Some(layer_id) = open.doc.active_layer() else { return };
		let refusal = match open.doc.layer(layer_id) {
			Some(layer) if !matches!(layer.kind, LayerKind::Pixel { .. }) => Some("Free Transform works on a pixel layer"),
			Some(layer) if layer.locked_position || layer.locked_pixels => Some("The layer is locked"),
			None => Some("Select a layer to transform"),
			_ => None,
		};
		if let Some(text) = refusal {
			self.to_ui(&EngineToUi::Toast { text: text.into() });
			return;
		}
		let rect = match crate::transform_preview::start_rect(&open.doc, layer_id, &self.store) {
			Ok(Some(rect)) => rect,
			Ok(None) => {
				self.to_ui(&EngineToUi::Toast {
					text: "Nothing to transform: the layer is empty".into(),
				});
				return;
			}
			Err(error) => {
				self.to_ui(&EngineToUi::Error { text: error.to_string() });
				return;
			}
		};
		self.end_stroke();
		let session = free_transform::Session::new(layer_id, rect, mode, filter);
		let status = session.status();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		open.transform_preview = Some(TransformPreview {
			layer: layer_id,
			prepared: None,
			shown: None,
			request: 0,
		});
		// Cut the source out and make its mips valid, off the engine thread.
		let doc = open.doc.clone();
		let (store, internal) = (self.store.clone(), self.internal.clone());
		rayon::spawn(move || {
			let result = Prepared::new(&doc, layer_id, &store);
			let _ = internal.send(Internal::TransformPrepared {
				doc: doc_id,
				layer: layer_id,
				result,
			});
		});
		self.transform = Some((doc_id, session));
		self.to_ui(&EngineToUi::TransformBox { up: true });
		self.to_ui(&EngineToUi::ToolInfo { text: status });
		self.request_frame();
	}

	/// The option bar's Interpolation for Free Transform (Bicubic by default,
	/// as in Photoshop).
	fn transform_filter(&self) -> fx_core::Filter {
		use fx_core::Filter;
		match self.settings.string("_transform", "Interpolation").as_deref() {
			Some("Nearest Neighbor") => Filter::Nearest,
			Some("Bilinear") => Filter::Bilinear,
			Some("Bicubic Smoother") => Filter::BicubicSmoother,
			Some("Bicubic Sharper") => Filter::BicubicSharper,
			Some("Bicubic Automatic") => Filter::BicubicAutomatic,
			Some("Lanczos 3") => Filter::Lanczos3,
			_ => Filter::Bicubic,
		}
	}

	/// Act on what an event did to the box.
	fn transform_update(&mut self, doc_id: DocId, update: TransformUpdate) {
		match update {
			TransformUpdate::None => {}
			TransformUpdate::Redraw => self.request_frame(),
			TransformUpdate::Changed { dragging } => {
				if let Some((_, session)) = &self.transform {
					let text = session.status();
					self.to_ui(&EngineToUi::ToolInfo { text });
				}
				self.restart_transform_preview(dragging);
				self.transform_refine = dragging.then(|| Instant::now() + TRANSFORM_REFINE_AFTER);
				self.request_frame();
			}
			TransformUpdate::Commit(command) => {
				// The preview stays on screen until the job's pixels land.
				self.end_transform(true);
				self.command(doc_id, command);
				let started = self.docs.get(doc_id).is_some_and(|open| open.busy.is_some());
				if !started && let Some(open) = self.docs.get_mut(doc_id) {
					open.transform_preview = None;
					open.preview_rev += 1;
					self.request_frame();
				}
			}
			TransformUpdate::Cancel => self.end_transform(false),
		}
	}

	/// Take the box down; `keep_preview` leaves the transformed pixels on
	/// screen (a commit, until its job is done).
	fn end_transform(&mut self, keep_preview: bool) {
		let Some((doc_id, _)) = self.transform.take() else { return };
		self.transform_refine = None;
		// Whatever preview job is running is now stale.
		self.transform_latest.fetch_add(1, Ordering::Relaxed);
		if !keep_preview
			&& let Some(open) = self.docs.get_mut(doc_id)
			&& open.transform_preview.take().is_some()
		{
			open.preview_rev += 1;
		}
		self.to_ui(&EngineToUi::TransformBox { up: false });
		self.to_ui(&EngineToUi::ToolInfo { text: String::new() });
		self.request_frame();
	}

	/// The source is ready: show the first preview.
	fn transform_prepared(&mut self, doc_id: DocId, layer: LayerId, result: Result<Option<Prepared>, TileError>) {
		let current = matches!(&self.transform, Some((doc, session)) if *doc == doc_id && session.layer == layer);
		if !current {
			return;
		}
		match result {
			Ok(Some(prepared)) => {
				if let Some(preview) = self.docs.get_mut(doc_id).and_then(|open| open.transform_preview.as_mut()) {
					preview.prepared = Some(Arc::new(prepared));
				}
				self.restart_transform_preview(false);
			}
			Ok(None) => {
				self.end_transform(false);
				self.to_ui(&EngineToUi::Toast {
					text: "Nothing to transform".into(),
				});
			}
			Err(error) => {
				self.end_transform(false);
				self.to_ui(&EngineToUi::Error {
					text: format!("Free Transform: {error}"),
				});
			}
		}
	}

	/// Start a preview job for the box as it is now (latest request wins).
	fn restart_transform_preview(&mut self, coarser: bool) {
		let Some((doc_id, session)) = &self.transform else { return };
		let (doc_id, mapping, filter) = (*doc_id, session.mapping(), session.filter);
		let Some(mapping) = mapping else { return };
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		let Some(viewport) = open.view.viewport else { return };
		let (view, canvas) = (open.view.view, (open.doc.width, open.doc.height));
		let Some(preview) = open.transform_preview.as_mut() else { return };
		let Some(prepared) = preview.prepared.clone() else { return };
		let request = self.transform_latest.fetch_add(1, Ordering::Relaxed) + 1;
		preview.request = request;
		let job = TransformJob {
			request,
			latest: self.transform_latest.clone(),
			prepared,
			mapping,
			filter,
			coarser,
			view,
			viewport,
			canvas,
		};
		let (store, internal) = (self.store.clone(), self.internal.clone());
		rayon::spawn(move || {
			let result = job.run(&store);
			let _ = internal.send(Internal::TransformShown { doc: doc_id, request, result });
		});
	}

	/// A preview update arrived: show it if it is the newest.
	fn transform_shown(&mut self, doc_id: DocId, request: u64, result: Result<Option<crate::transform_preview::Shown>, TileError>) {
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		let Some(preview) = open.transform_preview.as_mut() else { return };
		if preview.request != request {
			return;
		}
		match result {
			Ok(Some(shown)) => {
				preview.shown = Some(shown);
				open.preview_rev += 1;
				if self.docs.active_id() == Some(doc_id) {
					self.request_frame();
				}
			}
			Ok(None) => {}
			// A derived tile went while the job ran: the next update retries.
			Err(TileError::Evicted) => {}
			Err(error) => tracing::warn!("transform preview failed: {error}"),
		}
	}

	/// The pointer rested after a drag: preview at the view's own level.
	fn refine_transform_if_due(&mut self) {
		if self.transform_refine.is_some_and(|at| Instant::now() >= at) {
			self.transform_refine = None;
			self.restart_transform_preview(false);
		}
	}

	// ------------------------------------------------------------ filters (M4-T05)

	/// Show `layer` filtered with `filter` on the visible area, live.
	fn start_preview(&mut self, id: DocId, layer: LayerId, filter: FilterParams) {
		if let Err(error) = filter.validate() {
			self.to_ui(&EngineToUi::Error { text: error.to_string() });
			return;
		}
		let Some(open) = self.docs.get_mut(id) else { return };
		let base = match open.doc.layer(layer).map(|l| &l.kind) {
			Some(LayerKind::Pixel { image, .. }) => image.clone(),
			_ => {
				self.to_ui(&EngineToUi::Toast {
					text: "Select a pixel layer to filter it".into(),
				});
				return;
			}
		};
		// Same layer: keep what is displayed until the new tiles arrive.
		let image = match open.preview.take() {
			Some(old) if old.layer == layer => old.image,
			_ => base.clone(),
		};
		open.preview = Some(FilterPreview {
			layer,
			params: filter,
			base,
			image,
			level: usize::MAX,
			region: (0, 0, 0, 0),
			request: 0,
		});
		self.restart_preview(id);
	}

	/// Start (or restart) the preview job of `id` for the current view.
	fn restart_preview(&mut self, id: DocId) {
		let latest = self.preview_latest.entry(id).or_insert_with(|| Arc::new(AtomicU64::new(0))).clone();
		let request = latest.fetch_add(1, Ordering::Relaxed) + 1;
		let Some(open) = self.docs.get_mut(id) else { return };
		let Some(viewport) = open.view.viewport else { return };
		let (doc_w, doc_h) = (open.doc.width, open.doc.height);
		let view = open.view.view;
		let Some(preview) = &mut open.preview else { return };
		let Some(layer) = open.doc.layer(preview.layer) else { return };
		let LayerKind::Pixel { offset, .. } = layer.kind else { return };
		let level = view.mip_level(preview.base.level_count());
		if level != preview.level {
			// Another level: start from the unfiltered pixels.
			preview.image = preview.base.clone();
		}
		preview.level = level;
		preview.request = request;
		let Some((region, tiles)) = filters::visible_tiles(&view, viewport, (doc_w, doc_h), &preview.base, offset, level) else {
			return;
		};
		preview.region = region;
		let job = PreviewJob {
			request,
			latest,
			params: preview.params.clone(),
			base: preview.base.clone(),
			geometry: fx_ops::filter::Geometry {
				offset,
				canvas: (doc_w, doc_h),
				image: (preview.base.width(), preview.base.height()),
			},
			level,
			region,
			tiles,
		};
		let (store, internal) = (self.store.clone(), self.internal.clone());
		rayon::spawn(move || {
			let send = |tiles| {
				let _ = internal.send(Internal::PreviewTiles { doc: id, request, tiles });
			};
			if let Err(error) = job.run(&store, send) {
				let _ = internal.send(Internal::PreviewFailed { doc: id, request, error });
			}
		});
	}

	/// The view moved: extend or recompute the active document's preview when
	/// it no longer covers what is on screen.
	fn refresh_preview(&mut self) {
		let Some(id) = self.docs.active_id() else { return };
		let Some(open) = self.docs.get_mut(id) else { return };
		let (Some(preview), Some(viewport)) = (&open.preview, open.view.viewport) else {
			return;
		};
		let Some(LayerKind::Pixel { offset, .. }) = open.doc.layer(preview.layer).map(|l| &l.kind) else {
			return;
		};
		let level = open.view.view.mip_level(preview.base.level_count());
		let visible = filters::visible_tiles(&open.view.view, viewport, (open.doc.width, open.doc.height), &preview.base, *offset, level);
		let covered = level == preview.level
			&& visible.is_some_and(|((x0, y0, x1, y1), _)| {
				let (a, b, c, d) = preview.region;
				x0 >= a && y0 >= b && x1 <= c && y1 <= d
			});
		if !covered {
			self.restart_preview(id);
		}
	}

	/// Drop the preview of `id` (Cancel, Preview off).
	fn cancel_preview(&mut self, id: DocId) {
		if let Some(latest) = self.preview_latest.get(&id) {
			latest.fetch_add(1, Ordering::Relaxed);
		}
		if let Some(open) = self.docs.get_mut(id)
			&& open.preview.take().is_some()
		{
			open.preview_rev += 1;
			self.request_frame();
		}
	}

	/// Run a heavy pixel command on a worker thread (recipe R2) and record
	/// it when done (R1b). The document is busy meanwhile.
	fn start_pixel_job(&mut self, id: DocId, command: Command) {
		let Some(open) = self.docs.get_mut(id) else { return };
		let label = pixel_job_label(&command);
		open.busy = Some(label.clone());
		let before = open.doc.clone();
		self.next_task += 1;
		let task = self.next_task;
		self.to_ui(&EngineToUi::Progress {
			task,
			label: label.clone(),
			fraction: 0.0,
		});
		let (store, internal) = (self.store.clone(), self.internal.clone());
		let clipboard = self.ops.clipboard.clone();
		let spawned = std::thread::Builder::new().name(format!("pixel-job-{task}")).spawn(move || {
			let progress_internal = internal.clone();
			let progress_label = label.clone();
			let ops = EngineOps {
				progress: Some(Arc::new(move |fraction| {
					let _ = progress_internal.send(Internal::Progress {
						task,
						label: progress_label.clone(),
						fraction,
					});
				})),
				clipboard,
			};
			let mut after = before;
			let mut ctx = CommandContext {
				tiles: &store,
				ops: Some(&ops),
			};
			let result = command.apply(&mut after, &mut ctx).map(|effect| (Box::new(after), effect));
			let _ = internal.send(Internal::PixelJobDone {
				task,
				doc: id,
				command: Box::new(command),
				result,
			});
		});
		if let Err(error) = spawned {
			if let Some(open) = self.docs.get_mut(id) {
				open.busy = None;
			}
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot start the job: {error}"),
			});
		}
	}

	/// A pixel job finished: install its document as a history step.
	fn pixel_job_done(&mut self, task: u64, id: DocId, command: Command, result: Result<(Box<Document>, CommandEffect), CommandError>) {
		self.to_ui(&EngineToUi::ProgressDone { task });
		if let Some(latest) = self.preview_latest.get(&id) {
			latest.fetch_add(1, Ordering::Relaxed);
		}
		let Some(open) = self.docs.get_mut(id) else { return };
		open.busy = None;
		if open.preview.take().is_some() {
			open.preview_rev += 1;
		}
		// A committed Free Transform stayed on screen until its pixels landed.
		if open.transform_preview.take().is_some() {
			open.preview_rev += 1;
		}
		match result {
			Ok((after, effect)) => {
				if let Command::ApplyFilter { filter, .. } = &command {
					self.last_filter = Some(filter.clone());
				}
				let Some(open) = self.docs.get_mut(id) else { return };
				let before = std::mem::replace(&mut open.doc, *after);
				open.history.record(before, command, effect.label.clone());
				// A selection reshape (M5) is a history step, not a content change.
				let content = !effect.history_only;
				if content {
					open.dirty = true;
					open.changed();
					open.note_edits(&effect.pixels_changed, Instant::now());
				}
				self.last_edit = None;
				self.after_edit(id, content);
				if !content {
					self.selection_overlay = None;
					self.request_frame();
				}
				for layer in effect.pixels_changed {
					self.refresh_thumbnail(id, layer);
				}
			}
			Err(error) => {
				self.to_ui(&EngineToUi::Error { text: error.to_string() });
				self.request_frame();
			}
		}
	}

	fn internal(&mut self, message: Internal) {
		match message {
			Internal::PreviewTiles { doc, request, tiles } => {
				let store = self.store.clone();
				if let Some(open) = self.docs.get_mut(doc)
					&& let Some(preview) = &mut open.preview
					&& preview.request == request
				{
					filters::install(preview, &store, tiles);
					open.preview_rev += 1;
					if self.docs.active_id() == Some(doc) {
						self.request_frame();
					}
				}
			}
			Internal::PreviewFailed { doc, request, error } => {
				let current = self
					.docs
					.get_mut(doc)
					.and_then(|open| open.preview.as_ref())
					.is_some_and(|p| p.request == request);
				if current {
					tracing::warn!("filter preview failed: {error}");
					self.to_ui(&EngineToUi::Error {
						text: format!("The preview failed: {error}"),
					});
				}
			}
			Internal::PixelJobDone { task, doc, command, result } => self.pixel_job_done(task, doc, *command, result),
			Internal::TransformPrepared { doc, layer, result } => self.transform_prepared(doc, layer, result),
			Internal::TransformShown { doc, request, result } => self.transform_shown(doc, request, result),
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
			Internal::Copied { task, result } => {
				self.to_ui(&EngineToUi::ProgressDone { task });
				match result {
					Ok(Some(clip)) => self.set_clipboard(clip),
					Ok(None) => self.to_ui(&EngineToUi::Toast {
						text: "Could not copy: the selected area is empty".into(),
					}),
					Err(text) => self.to_ui(&EngineToUi::Error { text }),
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
			Internal::OpenedFxd { task, path, result } => {
				self.to_ui(&EngineToUi::ProgressDone { task });
				match result {
					Ok(opened) => {
						let id = self.docs.allocate_id();
						let mut doc = OpenDoc::from_fxd(id, &path, *opened);
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
			Internal::Saved {
				task,
				doc,
				path,
				generation,
				result,
			} => {
				self.to_ui(&EngineToUi::ProgressDone { task });
				match result {
					Ok(file) => {
						let info = self.docs.get_mut(doc).map(|open| {
							open.file = Some(file);
							open.path = Some(path.clone());
							open.dirty = open.generation != generation;
							open.name = path
								.file_name()
								.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
							open.info()
						});
						if let Some(info) = info {
							self.to_ui(&EngineToUi::DocumentChanged { info });
						}
						tracing::info!("saved {}", path.display());
						self.to_ui(&EngineToUi::Toast {
							text: format!("Saved {}", path.display()),
						});
						if self.pending_close == Some(doc) {
							self.pending_close = None;
							self.force_close(doc);
							self.continue_window_close();
						}
					}
					Err(IoError::Cancelled) => {
						self.pending_close = None;
						self.window_close_pending = false;
					}
					Err(error) => {
						tracing::warn!("cannot save {}: {error}", path.display());
						self.pending_close = None;
						self.window_close_pending = false;
						self.to_ui(&EngineToUi::Error {
							text: format!("Could not save {}: {error}", path.display()),
						});
					}
				}
			}
		}
	}

	/// Close `id`, asking the UI first when it is dirty (M3-T06).
	fn close(&mut self, id: DocId) {
		let dirty = self.docs.get_mut(id).is_some_and(|open| open.dirty);
		if dirty {
			let name = self.docs.get_mut(id).map_or_else(String::new, |open| open.name.clone());
			self.to_ui(&EngineToUi::CloseDirtyDocument { doc: id, name });
			return; // no answer yet: the document stays open
		}
		self.force_close(id);
	}

	/// The user's answer to the "save changes?" prompt.
	fn close_answer(&mut self, id: DocId, answer: CloseAnswer) {
		match answer {
			CloseAnswer::Cancel => self.window_close_pending = false,
			CloseAnswer::DontSave => {
				if let Some(open) = self.docs.get_mut(id) {
					open.dirty = false;
				}
				self.force_close(id);
				self.continue_window_close();
			}
			CloseAnswer::Save => {
				self.pending_close = Some(id);
				self.save(id);
			}
		}
	}

	/// After a dirty document was dealt with while the window is closing: ask
	/// about the next one, or let the shell close.
	fn continue_window_close(&mut self) {
		if self.window_close_pending {
			self.close_requested();
		}
	}

	/// The user asked to close the window: allowed only when nothing is dirty.
	/// Each dirty document is asked about in turn; Cancel stops the close.
	fn close_requested(&mut self) {
		self.window_close_pending = true;
		let dirty = self.docs.iter_mut().find(|open| open.dirty).map(|open| (open.id, open.name.clone()));
		match dirty {
			Some((id, name)) => {
				self.to_ui(&EngineToUi::CloseDirtyDocument { doc: id, name });
				(self.output)(EngineOutput::MayClose(false));
			}
			None => {
				self.window_close_pending = false;
				(self.output)(EngineOutput::MayClose(true));
			}
		}
	}

	/// Close without asking (the document must already be clean).
	fn force_close(&mut self, id: DocId) {
		if matches!(self.transform, Some((doc, _)) if doc == id) {
			self.end_transform(false);
		}
		if self.docs.close(id).is_some() {
			self.thumbs_wanted.retain(|(d, _), _| *d != id);
			self.thumbs_last.retain(|(d, _), _| *d != id);
			self.thumbs_due.retain(|(d, _), _| *d != id);
			// Dropping the document drops its tile handles: memory is freed.
			self.to_ui(&EngineToUi::DocumentClosed { doc: id });
			self.after_active_change();
		}
	}

	// ------------------------------------------------------------ saving

	/// Save in place; a document without a file asks the shell for a path.
	fn save(&mut self, id: DocId) {
		let (file, name) = match self.docs.get_mut(id) {
			Some(open) => (open.file.clone(), open.name.clone()),
			None => return,
		};
		match file {
			Some(file) => self.start_save(id, SaveTarget::Incremental(file)),
			None => self.ask_save_path(id, &name),
		}
	}

	/// Ask the shell for a path (Save As, or Save of a document with no file).
	fn ask_save_path(&self, id: DocId, name: &str) {
		(self.output)(EngineOutput::NeedSavePath {
			doc: id,
			suggested_name: suggested_fxd_name(name),
		});
	}

	/// Save to `path` (the shell chose it). Choosing the document's own file
	/// is an incremental save: Windows refuses to replace a file that is open,
	/// and the document's backed tiles keep it open (D-027).
	fn save_as(&mut self, id: DocId, path: PathBuf) {
		let own = self.docs.get_mut(id).and_then(|open| {
			let same = open.path.as_ref().is_some_and(|p| same_file(p, &path));
			if same { open.file.clone() } else { None }
		});
		match own {
			Some(file) => self.start_save(id, SaveTarget::Incremental(file)),
			None => self.start_save(id, SaveTarget::Fresh(path)),
		}
	}

	/// Snapshot the document and save it on a worker thread (recipe R2).
	fn start_save(&mut self, id: DocId, target: SaveTarget) {
		let Some(open) = self.docs.get_mut(id) else { return };
		let snapshot = open.doc.clone();
		let generation = open.generation;
		let path = match &target {
			SaveTarget::Incremental(file) => file.path().to_path_buf(),
			SaveTarget::Fresh(path) => path.clone(),
		};
		self.next_task += 1;
		let task = self.next_task;
		let (store, internal) = (self.store.clone(), self.internal.clone());
		let label = format!(
			"Saving {}",
			path.file_name()
				.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
		);
		self.to_ui(&EngineToUi::Progress {
			task,
			label: label.clone(),
			fraction: 0.0,
		});
		let spawned = std::thread::Builder::new().name(format!("save-{task}")).spawn(move || {
			let mut report = |fraction: f32| {
				let _ = internal.send(Internal::Progress {
					task,
					label: label.clone(),
					fraction,
				});
				true
			};
			// The composite preview (D-026) needs the engine's compositor; a
			// later card can render it and pass it here.
			let result = fxd::save(
				SaveRequest {
					doc: &snapshot,
					store: &store,
					preview: None,
				},
				target,
				&mut report,
			)
			.map(|saved| saved.file);
			let _ = internal.send(Internal::Saved {
				task,
				doc: id,
				path,
				generation,
				result,
			});
		});
		if let Err(error) = spawned {
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.pending_close = None;
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot start the save: {error}"),
			});
		}
	}

	// ------------------------------------------------------------ editing

	/// Apply a document command through its history (M2).
	fn command(&mut self, id: DocId, command: Command) {
		self.end_stroke();
		// Any other edit while a transform box is up drops the box (Photoshop
		// greys everything else out; here the edit wins).
		if matches!(self.transform, Some((doc, _)) if doc == id) && !matches!(command, Command::Transform { .. }) {
			self.end_transform(false);
		}
		let store = self.store.clone();
		let Some(doc) = self.docs.get_mut(id) else {
			tracing::warn!("command for unknown document {id:?}");
			return;
		};
		if let Some(job) = &doc.busy {
			let text = format!("Wait until {job} is finished");
			self.to_ui(&EngineToUi::Toast { text });
			return;
		}
		// Heavy pixel commands run as jobs (recipe R2): the UI stays live.
		if is_pixel_job(&command) {
			self.start_pixel_job(id, command);
			return;
		}
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
		let mut ctx = CommandContext {
			tiles: &store,
			ops: Some(&self.ops),
		};
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
				// A pixel-selection command is a history step but not a document
				// change (M5-T03): it must not mark the document dirty (D-028),
				// but the ants have to be redrawn.
				let content = !effect.selection_only && !effect.history_only;
				if content {
					doc.dirty = true;
					doc.changed();
					let edited: Vec<LayerId> = effect.props_changed.iter().chain(&effect.pixels_changed).copied().collect();
					doc.note_edits(&edited, Instant::now());
				}
				let pixels = effect.pixels_changed.clone();
				self.after_edit(id, content);
				if effect.history_only {
					// The selection is not part of the content generation (it is
					// not saved, D-028), so the cached ants are stale now. Drop
					// them rather than bump the generation: a bump would make the
					// render thread recomposite the frame.
					self.selection_overlay = None;
					if self.docs.active_id() == Some(id) {
						self.request_frame();
					}
				}
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
		self.end_stroke();
		if matches!(self.transform, Some((doc, _)) if doc == id) {
			// Undo while the box is up cancels the box, as in Photoshop.
			self.end_transform(false);
			return;
		}
		let Some(doc) = self.docs.get_mut(id) else { return };
		if let Some(job) = &doc.busy {
			let text = format!("Wait until {job} is finished");
			self.to_ui(&EngineToUi::Toast { text });
			return;
		}
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
		// A command may have changed the document's size (Canvas Size, Image Size,
		// an arbitrary rotation in M6): the view and the viewport maths read this
		// copy of it, and `zoom:fit` uses it.
		let resized = doc.view.doc != (doc.doc.width, doc.doc.height);
		doc.view.doc = (doc.doc.width, doc.doc.height);
		let layers = EngineToUi::Layers {
			doc: id,
			revision: doc.doc.revision,
			layers: layer_list(doc),
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
		if resized && self.docs.active_id() == Some(id) {
			self.reactivate_tool();
		}
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
		// The sibling directly below the active layer (Merge Down).
		let below: Option<LayerId> = active.and_then(|id| {
			let path = doc.doc.path_of(id)?;
			let (&index, parents) = path.split_last()?;
			let siblings = if parents.is_empty() {
				&doc.doc.layers[..]
			} else {
				let mut layers = &doc.doc.layers[..];
				for &i in parents {
					layers = layers.get(i)?.children()?;
				}
				layers
			};
			index.checked_sub(1).and_then(|i| siblings.get(i)).map(|l| l.id)
		});
		let visible_roots: Vec<LayerRef> = doc.doc.layers.iter().filter(|l| l.visible).map(|l| LayerRef::Id(l.id)).collect();
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
			// Merge (Ctrl+E): the selected layers, or the active one into the layer below.
			"layer:merge" if selected.len() > 1 => vec![Command::MergeLayers { layers: selected.clone() }],
			"layer:merge" => match (active, below) {
				(Some(a), Some(b)) => vec![Command::MergeLayers {
					layers: vec![LayerRef::Id(a), LayerRef::Id(b)],
				}],
				_ => {
					self.to_ui(&EngineToUi::Toast {
						text: "There is no layer below to merge with".into(),
					});
					return true;
				}
			},
			"layer:merge-visible" if visible_roots.len() > 1 => vec![Command::MergeLayers { layers: visible_roots }],
			"layer:flatten" => vec![Command::Flatten],
			"layer:stamp-visible" => vec![Command::StampVisible],
			"layer:merge-visible" => return true,
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

	/// Image ▸ Crop (M6-T03): one `Command::Crop` around the pixels the
	/// selection covers, measured exactly on the coverage image (the Crop tool's
	/// box does the same for a dragged rectangle).
	fn crop_to_selection(&mut self) {
		let Some(doc_id) = self.docs.active_id() else {
			return;
		};
		let store = self.store.clone();
		let bounds = self.docs.get(doc_id).map(|open| {
			open.doc.selection.as_ref().map_or(Ok(None), |selection| {
				content_bounds(
					Placed {
						image: &selection.image,
						offset: selection.offset,
					},
					Content::Opaque,
					&store,
				)
			})
		});
		match bounds {
			Some(Ok(Some((x0, y0, x1, y1)))) => self.command(
				doc_id,
				Command::Crop {
					rect: (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32),
					angle_deg: 0.0,
					// D-056: Delete Cropped Pixels is off by default, so the menu
					// item keeps what the new canvas does not show.
					delete_cropped: false,
				},
			),
			Some(Ok(None)) => self.to_ui(&EngineToUi::Toast {
				text: "Nothing is selected to crop to".into(),
			}),
			Some(Err(error)) => self.to_ui(&EngineToUi::Toast { text: error.to_string() }),
			None => {}
		}
	}

	/// Image ▸ Trim (M6-T03): the borders the dialog names are cut off as one
	/// `Command::Crop`. The content is the **active layer's own pixels** — the
	/// "Based On" choice picks what counts as content (anything not transparent,
	/// or anything that is not the corner's colour) — and "Trim Away" picks the
	/// edges that may go.
	fn trim(&mut self, doc_id: DocId, args: &serde_json::Value) {
		let store = self.store.clone();
		let away = trim_edges(args);
		let Some(based_on) = args.get("based_on").and_then(|v| v.as_str()) else {
			return;
		};
		let trimmed = {
			let Some(open) = self.docs.get(doc_id) else {
				return;
			};
			let Some(layer) = open.doc.active_layer().and_then(|id| open.doc.layer(id)) else {
				self.to_ui(&EngineToUi::Toast {
					text: "Nothing to trim: the active layer has no pixels".into(),
				});
				return;
			};
			let LayerKind::Pixel { image, offset } = &layer.kind else {
				self.to_ui(&EngineToUi::Toast {
					text: "Nothing to trim: rasterise the layer first".into(),
				});
				return;
			};
			// "Based On: Top Left / Bottom Right Pixel Colour" counts the pixels
			// that differ from the corner the user named.
			let content = match based_on {
				"Top Left Pixel Colour" => stored_pixel(image, (0, 0), &store).map_or(Content::Opaque, Content::DifferentFrom),
				"Bottom Right Pixel Colour" => {
					let corner = (i64::from(image.width()) - 1, i64::from(image.height()) - 1);
					stored_pixel(image, corner, &store).map_or(Content::Opaque, Content::DifferentFrom)
				}
				_ => Content::Opaque,
			};
			match content_bounds(Placed { image, offset: *offset }, content, &store) {
				Ok(Some(bounds)) => bounds,
				Ok(None) => {
					self.to_ui(&EngineToUi::Toast {
						text: "Nothing to trim from that layer".into(),
					});
					return;
				}
				Err(error) => {
					self.to_ui(&EngineToUi::Toast { text: error.to_string() });
					return;
				}
			}
		};
		let (width, height) = self.docs.get(doc_id).map_or((0, 0), |open| (open.doc.width, open.doc.height));
		// Only the edges the dialog ticked are pulled in, and the pixels are
		// clipped to the canvas they live on.
		let (x0, y0, x1, y1) = trimmed;
		let left = if away.0 { x0.clamp(0, width as i32) } else { 0 };
		let top = if away.1 { y0.clamp(0, height as i32) } else { 0 };
		let right = if away.2 { x1.clamp(left, width as i32) } else { width as i32 };
		let bottom = if away.3 { y1.clamp(top, height as i32) } else { height as i32 };
		let rect = (left, top, (right - left) as u32, (bottom - top) as u32);
		if rect == (0, 0, width, height) {
			self.to_ui(&EngineToUi::Toast {
				text: "Nothing to trim: the layer fills the canvas".into(),
			});
			return;
		}
		self.command(
			doc_id,
			Command::Crop {
				rect,
				angle_deg: 0.0,
				delete_cropped: false,
			},
		);
	}

	/// Edit and Layer menu actions that use the selection or the clipboard
	/// (M5-T05). `true` when `id` was one of them.
	fn edit_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		let Some(doc_id) = self.docs.active_id() else {
			return matches!(
				id,
				"clip:copy" | "clip:copy-merged" | "clip:cut" | "clip:paste" | "clip:paste-special" | "clip:clear" | "edit:fill" | "edit:trim"
			);
		};
		if id == "edit:trim" {
			self.trim(doc_id, args);
			return true;
		}
		let has_selection = self.docs.get(doc_id).is_some_and(|open| open.doc.selection.is_some());
		match id {
			"clip:copy" | "clip:cut" => {
				if id == "clip:cut" && !has_selection {
					self.to_ui(&EngineToUi::Toast {
						text: "Could not cut: nothing is selected".into(),
					});
					return true;
				}
				if self.copy_layer(doc_id) && id == "clip:cut" {
					self.command(
						doc_id,
						Command::Clear {
							layer: LayerRef::Active,
							cut: true,
						},
					);
				}
			}
			"clip:copy-merged" => self.copy_merged(doc_id),
			"clip:paste" | "clip:paste-special" => {
				let in_place = id == "clip:paste-special";
				let Some(clip) = PixelOps::clipboard(&self.ops) else {
					self.to_ui(&EngineToUi::Toast {
						text: "The clipboard is empty".into(),
					});
					return true;
				};
				let center = if in_place { None } else { self.paste_center(doc_id, clip.bounds) };
				self.command(doc_id, Command::Paste { in_place, center });
			}
			"clip:clear" if has_selection => self.command(
				doc_id,
				Command::Clear {
					layer: LayerRef::Active,
					cut: false,
				},
			),
			"clip:clear" => {}
			// Edit ▸ Fill (Shift+F5) with the dialog's values, and the shortcuts:
			// Alt+Backspace foreground, Ctrl+Backspace background, Shift keeps
			// transparency.
			"edit:fill" | "edit:fill-fg" | "edit:fill-bg" | "edit:fill-fg-preserve" | "edit:fill-bg-preserve" => {
				let command = self.fill_command(id, args);
				self.command(doc_id, command);
			}
			"layer:via-copy" | "layer:via-cut" if has_selection => self.command(doc_id, Command::LayerViaCopy { cut: id == "layer:via-cut" }),
			"layer:via-cut" => self.to_ui(&EngineToUi::Toast {
				text: "Layer via Cut needs a selection".into(),
			}),
			// A click on a layer's thumbnail (pixels) or its mask's (M5-T09).
			"layer:edit-mask" => {
				let layer = args.get("layer").and_then(serde_json::Value::as_u64).map(LayerId);
				let mask = args.get("mask").and_then(serde_json::Value::as_bool).unwrap_or(false);
				if let Some(open) = self.docs.get_mut(doc_id) {
					open.mask_target = if mask { layer } else { None };
				}
				if let Some(layer) = layer
					&& self.docs.get(doc_id).is_some_and(|open| open.doc.active_layer() != Some(layer))
				{
					self.command(
						doc_id,
						Command::SelectLayers {
							layers: vec![LayerRef::Id(layer)],
						},
					);
				} else {
					self.send_layers();
				}
			}
			// The Layers panel's mask button: from the selection when there is
			// one, Alt hides (Photoshop).
			"mask:add" => {
				let alt = args.get("alt").and_then(serde_json::Value::as_bool).unwrap_or(false);
				let fill = match (has_selection, alt) {
					(true, false) => MaskFill::RevealSelection,
					(true, true) => MaskFill::HideSelection,
					(false, false) => MaskFill::RevealAll,
					(false, true) => MaskFill::HideAll,
				};
				self.command(doc_id, Command::AddMask { layer: LayerRef::Active, fill });
			}
			"mask:reveal-sel" | "mask:hide-sel" => {
				let fill = if id == "mask:hide-sel" {
					MaskFill::HideSelection
				} else {
					MaskFill::RevealSelection
				};
				self.command(doc_id, Command::AddMask { layer: LayerRef::Active, fill });
			}
			_ => return false,
		}
		true
	}

	/// The Fill command an action asks for; colours come from the swatches.
	fn fill_command(&self, id: &str, args: &serde_json::Value) -> Command {
		let text = |key: &str| args.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
		let (color, preserve) = match id {
			"edit:fill-fg" => (self.settings.fg, false),
			"edit:fill-bg" => (self.settings.bg, false),
			"edit:fill-fg-preserve" => (self.settings.fg, true),
			"edit:fill-bg-preserve" => (self.settings.bg, true),
			_ => {
				let color = match text("use").as_str() {
					"Background Colour" => self.settings.bg,
					"Black" => [0, 0, 0, u16::MAX],
					"White" => [u16::MAX; 4],
					"50% Grey" => [32768, 32768, 32768, u16::MAX],
					_ => self.settings.fg,
				};
				(color, args.get("preserve").and_then(serde_json::Value::as_bool).unwrap_or(false))
			}
		};
		let mode =
			serde_json::from_value::<fx_core::BlendMode>(serde_json::Value::String(text("mode").to_lowercase().replace([' ', '-'], "_"))).unwrap_or_default();
		let opacity = args
			.get("opacity")
			.and_then(serde_json::Value::as_f64)
			.map_or(1.0, |v| (v / 100.0).clamp(0.0, 1.0));
		Command::Fill {
			layer: LayerRef::Active,
			color,
			mode,
			opacity,
			preserve_transparency: preserve,
		}
	}

	/// Copy the active layer's selected pixels (M5-T05). `false` when there is
	/// nothing to copy (a toast says why).
	fn copy_layer(&mut self, doc_id: DocId) -> bool {
		let store = self.store.clone();
		let result = {
			let Some(open) = self.docs.get(doc_id) else { return false };
			let Some((image, offset)) = crate::clipboard::active_pixels(&open.doc) else {
				self.to_ui(&EngineToUi::Toast {
					text: "Could not copy: the layer has no pixels".into(),
				});
				return false;
			};
			crate::clipboard::copy_layer(image, offset, open.doc.selection.as_ref(), (open.doc.width, open.doc.height), &store)
		};
		match result {
			Ok(Some(clip)) => {
				self.set_clipboard(clip);
				true
			}
			Ok(None) => {
				self.to_ui(&EngineToUi::Toast {
					text: "Could not copy: the selected area is empty".into(),
				});
				false
			}
			Err(error) => {
				self.to_ui(&EngineToUi::Error { text: error.to_string() });
				false
			}
		}
	}

	/// Edit ▸ Copy Merged (M5-T05): the composite of the visible layers under
	/// the selection. The composite runs on a helper thread.
	fn copy_merged(&mut self, doc_id: DocId) {
		let Some(open) = self.docs.get(doc_id) else { return };
		let doc = open.doc.clone();
		let (store, internal) = (self.store.clone(), self.internal.clone());
		self.next_task += 1;
		let task = self.next_task;
		self.to_ui(&EngineToUi::Progress {
			task,
			label: "Copy Merged".into(),
			fraction: 0.0,
		});
		let spawned = std::thread::Builder::new().name("copy-merged".into()).spawn(move || {
			let ids: Vec<LayerId> = doc.layers.iter().map(|l| l.id).collect();
			let result = crate::export::composite_layers(&doc, &ids, None, &store, None)
				.map_err(|e| e.to_string())
				.and_then(|merged| {
					crate::clipboard::copy_layer(&merged, (0, 0), doc.selection.as_ref(), (doc.width, doc.height), &store).map_err(|e| e.to_string())
				});
			let _ = internal.send(Internal::Copied { task, result });
		});
		if let Err(error) = spawned {
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot copy: {error}"),
			});
		}
	}

	/// Keep `clip` as the clipboard, and share a small one with Windows.
	fn set_clipboard(&mut self, clip: fx_core::pixels::ClipboardImage) {
		let image = crate::clipboard::os_pixels(&clip, &self.store).unwrap_or_else(|error| {
			tracing::warn!("the clipboard copy for Windows failed: {error}");
			None
		});
		(self.output)(EngineOutput::ClipboardCopied { image });
		*self.ops.clipboard.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(clip);
	}

	/// Where Paste centres the clipboard: `None` (keep the source position)
	/// when that position is in view, else the view's centre (Photoshop).
	fn paste_center(&self, doc_id: DocId, bounds: (i32, i32, i32, i32)) -> Option<(f64, f64)> {
		let open = self.docs.get(doc_id)?;
		let view = open.view.view;
		let visible = open
			.view
			.viewport
			.and_then(|viewport| view.visible_doc_rect(viewport, open.doc.width, open.doc.height));
		let (x0, y0, x1, y1) = (f64::from(bounds.0), f64::from(bounds.1), f64::from(bounds.2), f64::from(bounds.3));
		let in_view = visible.is_some_and(|(vx0, vy0, vx1, vy1)| x0 < vx1 && x1 > vx0 && y0 < vy1 && y1 > vy0);
		let on_canvas = x0 < f64::from(open.doc.width) && y0 < f64::from(open.doc.height) && x1 > 0.0 && y1 > 0.0;
		if in_view && on_canvas { None } else { Some((view.center_x, view.center_y)) }
	}

	/// An image from the Windows clipboard: it becomes the clipboard, then a
	/// new layer (M5-T05).
	fn paste_image(&mut self, width: u32, height: u32, rgba8: &[u8]) {
		let Some(doc_id) = self.docs.active_id() else { return };
		if rgba8.len() != (width as usize) * (height as usize) * 4 || width == 0 || height == 0 {
			tracing::warn!("a clipboard image of the wrong size was ignored");
			return;
		}
		let format = self
			.docs
			.get(doc_id)
			.map_or(fx_tiles::PixelFormat::Rgba8, |open| open.doc.color.depth.rgba_format());
		let clip = crate::clipboard::from_os(width, height, rgba8, format, &self.store);
		let center = self.docs.get(doc_id).map(|open| (open.view.view.center_x, open.view.view.center_y));
		*self.ops.clipboard.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(clip);
		self.command(doc_id, Command::Paste { in_place: false, center });
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
			let mut layers = crate::b3::build(width, height, format, &ids, 3, &store);
			// The full mip pyramid of every new layer, like an import: the first
			// frame at fit, and a saved `.fxd` (which stores mips ≥ 3), need them.
			for layer in &mut layers {
				if let fx_core::LayerKind::Pixel { image, .. } = &mut Arc::make_mut(layer).kind
					&& let Err(error) = mips::ensure_all_mips(image, &store)
				{
					tracing::warn!("B3 mips of {:?}: {error}", layer.id);
				}
			}
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
		if self.transform.as_ref().is_some_and(|(doc, _)| Some(*doc) != self.docs.active_id()) {
			self.end_transform(false);
		}
		self.reactivate_tool();
		self.send_layers();
		self.request_frame();
		self.view_message_pending = true;
	}

	fn send_layers(&mut self) {
		if let Some(doc) = self.docs.active_mut() {
			let message = EngineToUi::Layers {
				doc: doc.id,
				revision: doc.doc.revision,
				layers: layer_list(doc),
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
		// The stroke input this frame will show (M5-T11).
		let input_since = self.stroke.as_mut().and_then(|s| s.pending_input.take());
		let overlay = self.active_overlay();
		// The display transform needs the cache on `self`, so build it before
		// borrowing the active document mutably (and only for a real document:
		// the virtual test pattern has no profile).
		let (display_lut, gamut_warning) = match self.docs.active_mut() {
			Some(doc) => {
				let profile = doc.doc.color.profile.clone();
				let proof = doc.proof_colors.then(|| doc.proof.clone()).flatten();
				let gamut = doc.gamut_warning && proof.is_some();
				match proof {
					Some(proof) => (self.proof_lut_for(&profile, &proof), gamut),
					None => (self.display_lut_for(&profile), false),
				}
			}
			None => (None, false),
		};
		let frame = match self.docs.active_mut() {
			Some(doc) => {
				let Some(viewport) = doc.view.viewport else { return };
				Frame {
					view: doc.view.view,
					viewport,
					doc: Some((doc.id, doc.snapshot())),
					generation: doc.render_generation(),
					hot_layer: doc.hot_layer(),
					virtual_doc: VIRTUAL_DOC,
					display_lut,
					gamut_warning,
					overlay,
					input_since,
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
					display_lut: None,
					gamut_warning: false,
					overlay,
					input_since: None,
				}
			}
		};
		let _ = self.render.send(RenderRequest::Frame(frame));
	}

	/// Everything to draw over the image: the active tool's overlay plus the
	/// selection's marching ants (M5-T02/T03). `None` when there is nothing.
	fn active_overlay(&mut self) -> Option<Arc<fx_render::Overlay>> {
		// A Free Transform box replaces the tool's overlay and the ants.
		if let (Some((doc, session)), Some(active)) = (&self.transform, self.docs.active_id())
			&& *doc == active
		{
			return Some(Arc::new(session.overlay()));
		}
		let mut items = Vec::new();
		let mut nudge = None;
		if let Some(tool_id) = self.docs.active_mut().map(|doc| doc.view.tool.clone())
			&& let Some(tool) = self.tools.get(&tool_id)
		{
			if let Some(overlay) = tool.overlay() {
				items.extend(overlay.items);
			}
			nudge = tool.selection_nudge();
		}
		if let Some(selection) = self.selection_overlay() {
			match nudge {
				// The outline is being dragged (M5-T04): the ants follow.
				Some((dx, dy)) => items.extend(selection.items.iter().map(|item| item.translated(f64::from(dx), f64::from(dy)))),
				None => items.extend(selection.items.iter().cloned()),
			}
		}
		(!items.is_empty()).then(|| Arc::new(fx_render::Overlay { items }))
	}

	/// The selection's marching-ants contour, cached (M5-T03): recomputed only
	/// when the document, its content generation, the view level or the visible
	/// rectangle changes — panning is the only common invalidator, and a
	/// selection command clears the cache explicitly (`command`), because it
	/// does not move the generation.
	fn selection_overlay(&mut self) -> Option<Arc<fx_render::Overlay>> {
		let store = self.store.clone();
		let (key, selection) = {
			let doc = self.docs.active_mut()?;
			let viewport = doc.view.viewport?;
			let selection = doc.doc.selection.clone()?;
			let level = doc.view.view.mip_level(selection.image.level_count());
			let visible = doc.view.view.visible_doc_rect(viewport, doc.doc.width, doc.doc.height)?;
			let rect = (
				visible.0.floor() as i64,
				visible.1.floor() as i64,
				visible.2.ceil() as i64,
				visible.3.ceil() as i64,
			);
			((doc.id, doc.render_generation(), level, rect), selection)
		};
		if let Some((cached, overlay)) = &self.selection_overlay
			&& *cached == key
		{
			return Some(overlay.clone());
		}
		let overlay = match crate::selection::contour(&selection, &store, key.2, key.3) {
			Ok(overlay) => Arc::new(overlay),
			Err(error) => {
				tracing::warn!("the selection contour failed: {error}");
				return None;
			}
		};
		self.selection_overlay = Some((key, overlay.clone()));
		Some(overlay)
	}

	/// The monitor profile the shell reported (M4-T02), or `None` while it has
	/// not reported one: Fotox then assumes an sRGB display.
	fn monitor_profile(&self) -> Option<ColorProfile> {
		self.display_profile.as_ref().map(|bytes| ColorProfile::Icc(Arc::from(bytes.as_slice())))
	}

	/// Tell the engine which display profile to transform into (M4-T02).
	///
	/// Called by the shell at start-up and whenever the window moves to another
	/// monitor. An unreadable profile is reported as `None`; the LUT cache is
	/// dropped because every entry depended on the old profile.
	fn set_display_profile(&mut self, bytes: Option<Vec<u8>>) {
		if self.display_profile == bytes {
			return;
		}
		let name = match &bytes {
			Some(bytes) => format!("{} ICC bytes", bytes.len()),
			None => "none (assuming sRGB)".to_owned(),
		};
		tracing::info!("display profile: {name}");
		self.display_profile = bytes;
		self.display_luts.clear();
	}

	/// The display LUT for a document with `profile`, or `None` when the
	/// document is already in the monitor's space (criterion C1: the viewport
	/// then shows the document's values untouched).
	///
	/// Built here, on the engine thread: the render thread only uploads it
	/// (M4-T02). The result is cached because a moving window, a tab switch or
	/// a slider drag must not rebuild a 35 937-sample transform per frame.
	fn proof_lut_for(&mut self, profile: &ColorProfile, proof: &crate::documents::ProofSettings) -> Option<Arc<fx_color::Lut3d>> {
		let monitor = self.monitor_profile().unwrap_or(ColorProfile::Srgb);
		let intent = fx_color::lcms_intent(proof.intent);
		let key = {
			use std::hash::{Hash, Hasher};
			let mut h = std::collections::hash_map::DefaultHasher::new();
			fx_color::display_lut_key(profile, &monitor, intent, proof.bpc).hash(&mut h);
			proof.icc.hash(&mut h);
			proof.simulate_paper.hash(&mut h);
			h.finish()
		};
		if let Some((_, lut)) = self.display_luts.iter().find(|(k, _)| *k == key) {
			return lut.clone();
		}
		let built = match fx_color::proof_lut(profile, &proof.icc, &monitor, intent, proof.bpc, proof.simulate_paper) {
			Ok(lut) => Some(Arc::new(lut)),
			Err(error) => {
				tracing::error!("cannot build the proof transform: {error}");
				None
			}
		};
		self.display_luts.push((key, built.clone()));
		if self.display_luts.len() > DISPLAY_LUT_CACHE {
			self.display_luts.remove(0);
		}
		built
	}

	/// View ▸ Proof Setup (M4-T04): the press to simulate; turns Proof Colors on.
	fn proof_setup(&mut self, id: DocId, path: PathBuf, intent: fx_core::RenderingIntent, bpc: bool, simulate_paper: bool) {
		let icc = match std::fs::read(&path) {
			Ok(bytes) if fx_color::cmyk_profile(&bytes).is_ok() => bytes,
			_ => {
				self.to_ui(&EngineToUi::Error {
					text: format!("{} is not a readable CMYK profile", path.display()),
				});
				return;
			}
		};
		let Some(open) = self.docs.get_mut(id) else { return };
		open.proof = Some(crate::documents::ProofSettings {
			path,
			icc: Arc::from(icc),
			intent,
			bpc,
			simulate_paper,
		});
		open.proof_colors = true;
		self.send_proof_state(id);
	}

	/// Ctrl+Y / Shift+Ctrl+Y on the active document. Without a Proof Setup yet,
	/// the first CMYK profile found is used (Windows ships RSWOP.icm).
	fn toggle_proof(&mut self, gamut: bool) {
		let Some(id) = self.docs.active_id() else { return };
		let needs_setup = self.docs.get_mut(id).is_some_and(|open| open.proof.is_none());
		if needs_setup {
			match cmyk_profile_files().into_iter().next() {
				Some(first) => self.proof_setup(id, first.path, fx_core::RenderingIntent::RelativeColorimetric, true, false),
				None => {
					self.to_ui(&EngineToUi::Toast {
						text: "No CMYK profile found: add one to %APPDATA%\\Fotox\\profiles".into(),
					});
					return;
				}
			}
			if !gamut {
				return; // proof_setup already turned Proof Colors on
			}
		}
		let Some(open) = self.docs.get_mut(id) else { return };
		if gamut {
			open.gamut_warning = !open.gamut_warning;
		} else {
			open.proof_colors = !open.proof_colors;
		}
		self.send_proof_state(id);
	}

	fn send_proof_state(&mut self, id: DocId) {
		let Some(open) = self.docs.get_mut(id) else { return };
		let message = EngineToUi::ProofState {
			doc: id,
			proof_colors: open.proof_colors,
			gamut_warning: open.gamut_warning,
			profile: open
				.proof
				.as_ref()
				.map(|p| fx_color::icc_description(&p.icc).unwrap_or_else(|| p.path.display().to_string())),
		};
		self.to_ui(&message);
	}

	fn display_lut_for(&mut self, profile: &ColorProfile) -> Option<Arc<fx_color::Lut3d>> {
		let monitor = self.monitor_profile().unwrap_or(ColorProfile::Srgb);
		if fx_color::same_profile(profile, &monitor) {
			return None;
		}
		let key = fx_color::display_lut_key(profile, &monitor, fx_color::DEFAULT_INTENT, fx_color::DEFAULT_BPC);
		if let Some((_, lut)) = self.display_luts.iter().find(|(k, _)| *k == key) {
			return lut.clone();
		}
		let built = match display_transform(profile, self.display_profile.as_deref()) {
			Ok(lut) => lut.map(Arc::new),
			Err(error) => {
				// A monitor profile we cannot read is the shell's problem, not the
				// user's: forget it (so this is logged once, not every frame) and
				// show the document as if the monitor were sRGB.
				if self.display_profile.take().is_some() {
					tracing::error!("cannot use the display profile: {error}; assuming an sRGB monitor");
					self.display_luts.clear();
					return self.display_lut_for(profile);
				}
				tracing::error!("cannot build the display transform: {error}");
				return None;
			}
		};
		self.display_luts.push((key, built.clone()));
		if self.display_luts.len() > DISPLAY_LUT_CACHE {
			self.display_luts.remove(0);
		}
		built
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
		let (frames, uploads, pending_loads, gpu_bytes, latency) = {
			let mut stats = self.stats.lock().expect("render stats poisoned");
			(
				stats.summary(now),
				stats.uploads,
				stats.pending_loads,
				stats.gpu_bytes,
				stats.input_latency(now),
			)
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
			input_latency_ms_p50: latency.0,
			input_latency_ms_p99: latency.1,
		});
	}

	fn to_ui(&self, message: &EngineToUi) {
		(self.output)(EngineOutput::ToUi(fx_protocol::encode_json(message)));
	}
}

/// The display transform for `profile` on a monitor whose ICC profile is
/// `monitor` (M4-T02).
///
/// * `monitor = None` means the shell reported no profile: the display is
///   assumed to be sRGB, which is also what makes an sRGB document on an sRGB
///   monitor cost nothing (criterion C1).
/// * The same profile on both sides returns `Ok(None)`: no LUT, no change.
/// * The result is a plain table ([`fx_color::Lut3d`]), so it can travel to
///   the render thread; building it needs a few milliseconds of lcms2 work
///   and is why this is not called per frame.
fn display_transform(profile: &ColorProfile, monitor: Option<&[u8]>) -> Result<Option<fx_color::Lut3d>, fx_color::ColorError> {
	let monitor = match monitor {
		Some(bytes) => ColorProfile::Icc(Arc::from(bytes)),
		None => ColorProfile::Srgb,
	};
	if fx_color::same_profile(profile, &monitor) {
		return Ok(None);
	}
	let lut = fx_color::display_lut(profile, &monitor, fx_color::DEFAULT_INTENT, fx_color::DEFAULT_BPC)?;
	// Two encodings of the same space (the named sRGB and Windows' sRGB ICC
	// file differ by at most 0.54/255, at the saturated green corner): within
	// one 8-bit step everywhere, show the values as they are (criterion C1).
	Ok((!lut.is_identity(1.0 / 255.0)).then_some(lut))
}

/// The Layers panel's list, with the mask painting goes to marked (M5-T09).
fn layer_list(doc: &crate::documents::OpenDoc) -> Vec<fx_protocol::LayerInfo> {
	let mut list = layers::layer_infos(&doc.doc);
	if let Some(target) = doc.mask_target {
		for info in &mut list {
			info.edit_mask = info.id == target && info.has_mask;
		}
	}
	list
}

/// The image a stroke paints on: the layer's pixels or its mask.
fn stroke_image(doc: &mut Document, layer: LayerId, target: fx_core::stroke::StrokeTarget) -> Option<&mut fx_tiles::TiledImage> {
	let layer = doc.layer_mut(layer)?;
	match target {
		fx_core::stroke::StrokeTarget::Pixels => match &mut layer.kind {
			LayerKind::Pixel { image, .. } => Some(image),
			_ => None,
		},
		fx_core::stroke::StrokeTarget::Mask => layer.mask.as_mut().map(|m| &mut m.image),
	}
}

/// Put a stroke's image (and, for pixels, its offset) into the layer.
fn set_stroke_image(doc: &mut Document, layer: LayerId, target: fx_core::stroke::StrokeTarget, new_image: fx_tiles::TiledImage, new_offset: (i32, i32)) {
	let Some(layer) = doc.layer_mut(layer) else { return };
	match target {
		fx_core::stroke::StrokeTarget::Pixels => {
			if let LayerKind::Pixel { image, offset } = &mut layer.kind {
				*image = new_image;
				*offset = new_offset;
			}
		}
		fx_core::stroke::StrokeTarget::Mask => {
			if let Some(mask) = &mut layer.mask {
				mask.image = new_image;
			}
		}
	}
}

/// The CMYK profiles for proofing and export (D-032): Windows' colour folder
/// and `%APPDATA%/Fotox/profiles`.
fn cmyk_profile_files() -> Vec<fx_color::CmykProfileFile> {
	let windows = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join(r"System32\spool\drivers\color"));
	let user = std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join(r"Fotox\profiles"));
	let dirs: Vec<PathBuf> = [windows, user].into_iter().flatten().collect();
	fx_color::cmyk_profiles(&dirs.iter().map(PathBuf::as_path).collect::<Vec<_>>())
}

/// Which edges Image ▸ Trim may cut, from the dialog's "Trim Away" list
/// (M6-T03), as `(left, top, right, bottom)`.
fn trim_edges(args: &serde_json::Value) -> (bool, bool, bool, bool) {
	let has = |name: &str| {
		args.get("away")
			.and_then(|v| v.as_array())
			.is_some_and(|list| list.iter().any(|v| v.as_str() == Some(name)))
	};
	(has("Left"), has("Top"), has("Right"), has("Bottom"))
}

/// The stored value of one pixel of a placed image, in the document's 16-bit
/// scale (Trim's corner colour, M6-T03): `None` outside the image or when the
/// tile cannot be read.
fn stored_pixel(image: &TiledImage, corner: (i64, i64), store: &TileStore) -> Option<[u16; 4]> {
	if corner.0 < 0 || corner.1 < 0 || corner.0 >= i64::from(image.width()) || corner.1 >= i64::from(image.height()) {
		return None;
	}
	let tile = i64::from(TILE_SIZE);
	let (tx, ty) = ((corner.0 / tile) as u32, (corner.1 / tile) as u32);
	let format = image.format();
	let value = match image.slot(0, tx, ty) {
		TileSlot::Empty => return Some(PixelValue::TRANSPARENT.0),
		TileSlot::Solid(value) => *value,
		TileSlot::Data(handle) => {
			let buffer = store.get(handle).ok()?;
			let (x, y) = ((corner.0 % tile) as u32, (corner.1 % tile) as u32);
			let bpp = format.bytes_per_pixel();
			let index = ((y * TILE_SIZE + x) * bpp as u32) as usize;
			let bytes = buffer.bytes();
			let channel = |c: usize| -> u16 {
				match format {
					// 16-bit formats keep two bytes per channel.
					PixelFormat::Rgba16 | PixelFormat::Gray16 => u16::from_ne_bytes([bytes[index + c * 2], bytes[index + c * 2 + 1]]),
					_ => u16::from(bytes[index + c]) * 257,
				}
			};
			match format {
				PixelFormat::Gray8 | PixelFormat::Gray16 => PixelValue::gray16(channel(0)),
				_ => PixelValue::rgba16(channel(0), channel(1), channel(2), channel(3)),
			}
		}
	};
	Some(value.0)
}

/// Commands whose pixel work is too heavy for the engine thread (M4).
fn is_pixel_job(command: &Command) -> bool {
	matches!(
		command,
		Command::ApplyFilter { .. }
			| Command::MergeLayers { .. }
			| Command::Flatten
			| Command::StampVisible
			| Command::ConvertProfile { .. }
			| Command::ModifySelection { .. }
			| Command::MagicWand { .. }
			// Rotating a big canvas is tile I/O, resampling is a full pass over
			// every layer (M6-T02): both would freeze the engine thread.
			| Command::RotateCanvas { .. }
			| Command::FlipCanvas { .. }
			| Command::RotateCanvasArbitrary { .. }
			// Crop clips (and a straighten resamples) every layer (M6-T03).
			| Command::Crop { .. }
	)
	// Image Size without resampling only changes the print resolution: instant.
	|| matches!(command, Command::ImageSize { resample: Some(_), .. })
}

/// The progress label of a pixel job, as Photoshop names the operation.
fn pixel_job_label(command: &Command) -> String {
	match command {
		Command::ApplyFilter { filter, .. } => filter.label().to_owned(),
		Command::MergeLayers { .. } => "Merge Layers".to_owned(),
		Command::Flatten => "Flatten Image".to_owned(),
		Command::StampVisible => "Stamp Visible".to_owned(),
		Command::ConvertProfile { .. } => "Convert to Profile".to_owned(),
		Command::ModifySelection { .. } => "Modify Selection".to_owned(),
		Command::MagicWand { .. } => "Magic Wand".to_owned(),
		Command::RotateCanvas { quarter_turns } => {
			Permutation::from_quarter_turns(*quarter_turns).map_or_else(|| "Rotate Canvas".to_owned(), |op| op.label().to_owned())
		}
		Command::FlipCanvas { horizontal } => if *horizontal { "Flip Canvas Horizontal" } else { "Flip Canvas Vertical" }.to_owned(),
		Command::RotateCanvasArbitrary { .. } => "Rotate Image".to_owned(),
		Command::ImageSize { .. } => "Image Size".to_owned(),
		Command::Crop { .. } => "Crop".to_owned(),
		_ => "Working".to_owned(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn an_srgb_document_on_an_srgb_monitor_needs_no_transform() {
		assert!(
			display_transform(&ColorProfile::Srgb, None).unwrap().is_none(),
			"no profile reported = assume sRGB"
		);
		let icc: Arc<[u8]> = Arc::from(&b"the very same sRGB profile"[..]);
		let same = ColorProfile::Icc(icc.clone());
		assert!(
			display_transform(&same, Some(&icc)).unwrap().is_none(),
			"the document's own profile on the monitor"
		);
	}

	#[test]
	fn another_space_than_the_monitor_gets_a_lut() {
		let lut = display_transform(&ColorProfile::AdobeRgb1998, None)
			.unwrap()
			.expect("adobe rgb → sRGB needs a transform");
		assert_eq!(lut.grid(), fx_color::LUT_GRID);
		// And it is not the identity: a saturated Adobe RGB colour moves.
		let source = [0.9, 0.25, 0.4];
		let out = lut.sample(source);
		assert!(
			out.iter().zip(source).any(|(mapped, original)| (mapped - original).abs() > 0.01),
			"{out:?} for {source:?}"
		);
	}

	#[test]
	fn windows_srgb_profile_counts_as_srgb() {
		// The monitor profile Windows reports for an sRGB display is an ICC
		// file, not lcms2's built-in sRGB: the two must still give no LUT (C1).
		let path = std::path::Path::new(r"C:\Windows\System32\spool\drivers\color\sRGB Color Space Profile.icm");
		let Ok(bytes) = std::fs::read(path) else {
			eprintln!("no Windows sRGB profile on this machine: test skipped");
			return;
		};
		assert!(display_transform(&ColorProfile::Srgb, Some(&bytes)).unwrap().is_none());
	}

	#[test]
	fn an_unreadable_monitor_profile_is_an_error() {
		assert!(display_transform(&ColorProfile::Srgb, Some(b"not an ICC profile")).is_err());
	}
}
