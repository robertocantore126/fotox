//! Viewport tools (M5-T01, recipe R5 in `docs/tasks/HOWTO.md`).
//!
//! A tool consumes the pointer events the *view* did not (pan/zoom keeps
//! priority: Space, the Hand tool and the middle button never reach a tool),
//! works in document coordinates ([`DocPointer`], the inverse of the view
//! transform) and answers with a [`ToolResult`]: a command to execute (R1), a
//! cursor change, a status line, or an engine-side effect (the eyedropper's
//! colour).
//!
//! Overlays (marching ants, the brush outline, selection handles) are drawn by
//! [`Tool::overlay`] in document coordinates and handed to the render thread
//! (M5-T02), which tessellates them (`fx_render::overlay`).

use std::collections::HashMap;

use fx_core::{Command, Document, SelectMode};
use fx_render::{Overlay, ViewTransform};
use fx_tiles::TileStore;

use crate::ops::EngineOps;
use crate::{CursorShape, Modifiers, PointerKind};

pub mod bucket;
pub mod crop;
pub mod eyedropper;
pub mod gradient;
pub mod kinds;
pub mod lasso;
pub mod marquee;
pub mod move_tool;
pub mod paint;
pub mod path_select;
pub mod shape;
pub mod transform;
pub mod type_tool;
pub mod wand;

pub use eyedropper::sample_pixel;

/// A pointer event in document pixel coordinates (M5-T01): [`PointerInput`]
/// mapped through the inverse of the view transform.
///
/// [`PointerInput`]: crate::PointerInput
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocPointer {
	pub kind: PointerKind,
	/// Document pixels (f64, so sub-pixel positions survive zoom).
	pub x: f64,
	pub y: f64,
	pub pressure: f32,
	pub tilt_x: f32,
	pub tilt_y: f32,
	pub buttons: u8,
	pub modifiers: Modifiers,
	pub time_us: u64,
}

/// Which swatch a picked colour goes to (Alt with the eyedropper = background).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTarget {
	Foreground,
	Background,
}

impl ColorTarget {
	/// The `target` field of `EngineToUi::ColorPicked`.
	pub fn as_str(self) -> &'static str {
		match self {
			ColorTarget::Foreground => "fg",
			ColorTarget::Background => "bg",
		}
	}
}

/// The colours and option-bar values the tools read (M5-T01).
#[derive(Clone, Debug)]
pub struct ToolSettings {
	/// Foreground colour, 16-bit RGBA (straight alpha).
	pub fg: [u16; 4],
	/// Background colour, 16-bit RGBA (straight alpha).
	pub bg: [u16; 4],
	/// The last `UiToEngine::ToolOptions` per tool id, keyed by the option
	/// bar's field text without the colon.
	pub options: HashMap<String, serde_json::Value>,
}

impl Default for ToolSettings {
	fn default() -> Self {
		Self {
			fg: [0, 0, 0, u16::MAX],
			bg: [u16::MAX, u16::MAX, u16::MAX, u16::MAX],
			options: HashMap::new(),
		}
	}
}

impl ToolSettings {
	/// The value of option `key` of `tool` as a number (integers read as f64).
	pub fn number(&self, tool: &str, key: &str) -> Option<f64> {
		self.options.get(tool)?.get(key)?.as_f64()
	}

	/// The value of option `key` of `tool` as a string.
	pub fn string(&self, tool: &str, key: &str) -> Option<String> {
		self.options.get(tool)?.get(key)?.as_str().map(str::to_owned)
	}

	/// The value of option `key` of `tool` as a bool.
	pub fn bool(&self, tool: &str, key: &str) -> Option<bool> {
		self.options.get(tool)?.get(key)?.as_bool()
	}
}

// ---------------------------------------------------------------------------
// The selection tools' shared option-bar readings (M5-T04)
// ---------------------------------------------------------------------------

/// The Selection Mode the option bar is set to. The bar's Mode control is a
/// button group, which reports its index (0 New, 1 Add, 2 Subtract,
/// 3 Intersect); a wording is accepted too, so the value survives the bar
/// being rebuilt as a drop-down.
fn selection_mode(ctx: &ToolContext<'_>, tool: &str) -> SelectMode {
	match ctx.settings.number(tool, "Mode") {
		Some(0.0) => SelectMode::Replace,
		Some(1.0) => SelectMode::Add,
		Some(2.0) => SelectMode::Subtract,
		Some(3.0) => SelectMode::Intersect,
		_ => match ctx.settings.string(tool, "Mode").as_deref() {
			Some("Add to selection") => SelectMode::Add,
			Some("Subtract from selection") => SelectMode::Subtract,
			Some("Intersect with selection") => SelectMode::Intersect,
			_ => SelectMode::Replace,
		},
	}
}

/// The mode a press asks for: Shift adds, Alt subtracts, Shift+Alt intersects
/// (Photoshop's modifiers), anything else is the option bar's Mode.
fn mode_at_press(modifiers: Modifiers, option: SelectMode) -> SelectMode {
	match (modifiers.shift, modifiers.alt) {
		(true, true) => SelectMode::Intersect,
		(true, false) => SelectMode::Add,
		(false, true) => SelectMode::Subtract,
		(false, false) => option,
	}
}

/// The Feather and Anti-alias the option bar asks for, in the units
/// [`Command::Select`] takes.
fn selection_shape_options(ctx: &ToolContext<'_>, tool: &str) -> (f64, bool) {
	let feather = ctx.settings.number(tool, "Feather").unwrap_or(0.0).max(0.0);
	let anti_alias = ctx.settings.bool(tool, "Anti-alias").unwrap_or(true);
	(feather, anti_alias)
}

/// Dragging the selection outline with a selection tool (M5-T04): a press
/// inside the selection with no modifier (New mode) may grab the outline. It
/// becomes a move once the pointer travels more than [`OUTLINE_SLOP`] screen
/// pixels; released before that it was a click, and the tool does what a
/// click does (the wand selects, the polygonal lasso starts a path). While it
/// moves, the ants follow the pointer; the release is one `OffsetSelection`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutlineDrag {
	start: (f64, f64),
	current: (f64, f64),
	/// The press, for the click the tool falls back to.
	pub press: DocPointer,
	dragged: bool,
}

/// Screen pixels a press inside the selection must travel to move the outline.
pub(crate) const OUTLINE_SLOP: f64 = 3.0;

impl OutlineDrag {
	/// Grab the outline if the press is inside the selection.
	pub(crate) fn begin(ctx: &ToolContext<'_>, event: &DocPointer) -> Option<Self> {
		inside_selection(ctx, event.x, event.y).then_some(Self {
			start: (event.x, event.y),
			current: (event.x, event.y),
			press: *event,
			dragged: false,
		})
	}

	/// Follow the pointer; `zoom` turns the slop into document pixels.
	pub(crate) fn track(&mut self, event: &DocPointer, zoom: f64) {
		self.current = (event.x, event.y);
		let slop = OUTLINE_SLOP / zoom.max(f64::MIN_POSITIVE);
		if (self.current.0 - self.start.0).abs() > slop || (self.current.1 - self.start.1).abs() > slop {
			self.dragged = true;
		}
	}

	/// Whether the press became a move.
	pub(crate) fn dragged(&self) -> bool {
		self.dragged
	}

	/// The whole-pixel move so far (0 until the press becomes a move).
	pub(crate) fn delta(&self) -> (i32, i32) {
		if !self.dragged {
			return (0, 0);
		}
		((self.current.0 - self.start.0).round() as i32, (self.current.1 - self.start.1).round() as i32)
	}

	/// The command of the release: `None` for no movement.
	pub(crate) fn command(&self) -> Option<Command> {
		let (dx, dy) = self.delta();
		(dx != 0 || dy != 0).then_some(Command::OffsetSelection { dx, dy })
	}
}

/// Whether the document point is inside the pixel selection (coverage at
/// least 50 %).
pub(crate) fn inside_selection(ctx: &ToolContext<'_>, x: f64, y: f64) -> bool {
	let Some(selection) = &ctx.doc.selection else {
		return false;
	};
	if x < 0.0 || y < 0.0 || x >= f64::from(ctx.doc.width) || y >= f64::from(ctx.doc.height) {
		return false;
	}
	let (px, py) = (x as u32, y as u32);
	let tile = fx_tiles::TILE_SIZE;
	selection
		.tile_coverage(ctx.store, px / tile, py / tile)
		.is_ok_and(|c| c.at(px % tile, py % tile) >= 0.5)
}

/// The arrow keys move the selection outline by 1 px, Shift+arrow by 10 px
/// (Photoshop, with a selection tool active). `None` for any other key or
/// without a selection.
pub(crate) fn nudge_outline(ctx: &ToolContext<'_>, key: &str) -> Option<Command> {
	ctx.doc.selection.as_ref()?;
	let (step, arrow) = match key.strip_prefix("Shift+") {
		Some(arrow) => (10, arrow),
		None => (1, key),
	};
	let (dx, dy) = match arrow {
		"ArrowLeft" => (-step, 0),
		"ArrowRight" => (step, 0),
		"ArrowUp" => (0, -step),
		"ArrowDown" => (0, step),
		_ => return None,
	};
	Some(Command::OffsetSelection { dx, dy })
}

/// What a tool produced for one event.
#[derive(Clone, Debug, Default)]
pub struct ToolResult {
	/// A command the engine should execute through `History` (R1).
	pub command: Option<Command>,
	/// A colour the tool picked (the eyedropper), for the UI swatch.
	pub picked: Option<([u16; 4], ColorTarget)>,
	/// The cursor to show now.
	pub cursor: Option<CursorShape>,
	/// A message for a toast (a refused click, a missing clone source).
	pub info: Option<String>,
	/// A status line for the Info panel / status bar (M5-T10: the marquee's
	/// size while it is dragged).
	pub status: Option<String>,
	/// The tool's overlay changed (M5-T04): redraw the viewport. The composited
	/// tiles are reused, so this is the cheap path a rubber band needs.
	pub redraw: bool,
	/// Brush stroke events for the engine to paint (M5-T07), in order.
	pub strokes: Vec<StrokeEvent>,
	/// The tool changed the document outside the history (the Type tool's
	/// live edit, M6-T07): the engine refreshes the snapshot and the panels.
	pub doc_changed: bool,
	/// The Type tool's session changed (M6-T07): the UI shows or hides its
	/// textarea.
	pub text_session: Option<type_tool::TextSession>,
	/// Commands to execute after `command`, in order (M7-T02: Alt+drag's
	/// duplicate, then the move).
	pub then: Vec<Command>,
}

/// What a painting tool asks the engine to do with its stroke (M5-T07).
#[derive(Clone, Debug, PartialEq)]
pub enum StrokeEvent {
	/// Pen down: start a stroke on the active layer (or its mask).
	Begin {
		target: fx_core::stroke::StrokeTarget,
		tool: fx_core::stroke::StrokeTool,
		brush: fx_core::stroke::BrushParams,
		color: [u16; 4],
		samples: Vec<fx_core::stroke::StrokeSample>,
	},
	/// More samples (document pixels, after smoothing).
	Add(Vec<fx_core::stroke::StrokeSample>),
	/// Pen up: record the stroke as one History step.
	End,
}

/// The document and services a tool works with.
pub struct ToolContext<'a> {
	pub doc: &'a mut Document,
	pub store: &'a TileStore,
	pub ops: &'a EngineOps,
	pub settings: &'a ToolSettings,
	/// The view the pointer arrived through (M5-T04). Tools need the zoom to
	/// work in screen units: a lasso drops a sample every 0.5 *screen* pixels,
	/// a click is a drag under 3 *screen* pixels, whatever the document size.
	pub view: ViewTransform,
	/// Painting goes to the active layer's mask (the mask thumbnail was
	/// clicked in the Layers panel, M5-T09).
	pub mask_target: bool,
}

/// A viewport tool (M5-T01).
pub trait Tool {
	/// The tool became the active one on `doc` (M6-T03): the crop box starts as
	/// the whole canvas, Free Transform (M6-T04) takes the layer's bounds. The
	/// only hook a tool gets before its first event.
	fn activate(&mut self, _doc: &Document) {}

	/// Handle one pointer event in document coordinates.
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult;

	/// Handle a key the UI's shortcut map did not consume (M5-T04). The names
	/// are the DOM's (`"Escape"`, `"Enter"`, `"Backspace"`, `"ArrowUp"`…).
	fn key(&mut self, _ctx: &mut ToolContext<'_>, _key: &str) -> ToolResult {
		ToolResult::default()
	}

	/// The overlay to draw over the viewport, in document coordinates
	/// (M5-T02): marching ants, the brush outline, handles. `None` = nothing.
	fn overlay(&self) -> Option<Overlay> {
		None
	}

	/// The cursor to show while this tool is active and the pointer is idle.
	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Default
	}

	/// While the tool drags the selection outline (M5-T04), how far the
	/// marching ants are shifted, in whole document pixels.
	fn selection_nudge(&self) -> Option<(i32, i32)> {
		None
	}

	/// The UI's text input for the Type tool (M6-T07): the whole text and the
	/// selection as byte offsets.
	fn text_input(&mut self, _ctx: &mut ToolContext<'_>, _text: &str, _selection: (usize, usize)) -> ToolResult {
		ToolResult::default()
	}

	/// The tool's option bar changed.
	fn options_changed(&mut self, _ctx: &mut ToolContext<'_>) -> ToolResult {
		ToolResult::default()
	}

	/// Start editing `layer` (the Type tool, from the Layers panel).
	fn edit_layer(&mut self, _ctx: &mut ToolContext<'_>, _layer: fx_core::LayerId) -> ToolResult {
		ToolResult::default()
	}

	/// Another tool is being picked: finish what is under way.
	fn deactivate(&mut self, _ctx: &mut ToolContext<'_>) -> ToolResult {
		ToolResult::default()
	}
}

/// The tool registry: one boxed tool per id, created on first use, so a tool's
/// state (a drag in progress, the last clone source) lives as long as the
/// engine.
#[derive(Default)]
pub struct Tools {
	active: HashMap<String, Box<dyn Tool>>,
}

impl Tools {
	/// The tool for `id`, created on first use. `None` for a tool id M5 does
	/// not implement yet: the view still pans and zooms, the pointer is just
	/// ignored.
	pub fn get(&mut self, id: &str) -> Option<&mut Box<dyn Tool>> {
		if !self.active.contains_key(id) {
			self.active.insert(id.to_owned(), new_tool(id)?);
		}
		self.active.get_mut(id)
	}
}

/// The tool a UI tool id maps to, or `None` for an id Fotox does not know at
/// all (M5-T01). A tool the milestone has not reached yet (the magic wand,
/// quick selection, T06+ painting) gets [`NotYet`], which says so in a toast
/// instead of silently ignoring the pointer.
fn new_tool(id: &str) -> Option<Box<dyn Tool>> {
	match id {
		"eyedropper" => Some(Box::new(eyedropper::Eyedropper)),
		"crop" => Some(Box::new(crop::Crop::new("crop"))),
		// The tool keeps its own id: it reads that tool's option-bar values.
		"marquee" => Some(Box::new(marquee::Marquee::new("marquee", marquee::Shape::Rect))),
		"marquee-ellipse" => Some(Box::new(marquee::Marquee::new("marquee-ellipse", marquee::Shape::Ellipse))),
		"marquee-row" => Some(Box::new(marquee::Marquee::new("marquee-row", marquee::Shape::Row))),
		"marquee-col" => Some(Box::new(marquee::Marquee::new("marquee-col", marquee::Shape::Column))),
		"lasso" => Some(Box::new(lasso::Lasso::new("lasso", lasso::Kind::Freehand))),
		"lasso-poly" => Some(Box::new(lasso::Lasso::new("lasso-poly", lasso::Kind::Polygonal))),
		// The card puts these out of scope for M5-T04 (the wand is a later
		// step of it, the rest belongs to a milestone that is not planned).
		"magic-wand" => Some(Box::new(wand::MagicWand::default())),
		"brush" => Some(Box::new(paint::Paint::new("brush", paint::Kind::Brush))),
		"pencil" => Some(Box::new(paint::Paint::new("pencil", paint::Kind::Pencil))),
		"eraser" => Some(Box::new(paint::Paint::new("eraser", paint::Kind::Eraser))),
		"clone" => Some(Box::new(paint::Paint::new("clone", paint::Kind::Clone))),
		"heal-brush" => Some(Box::new(paint::Paint::new("heal-brush", paint::Kind::Heal))),
		"heal" => Some(Box::new(paint::Paint::new("heal", paint::Kind::SpotHeal))),
		"eraser-bg" => Some(Box::new(paint::Paint::new("eraser-bg", paint::Kind::BgEraser))),
		"dodge" => Some(Box::new(paint::Paint::new("dodge", paint::Kind::Dodge))),
		"burn" => Some(Box::new(paint::Paint::new("burn", paint::Kind::Burn))),
		"sponge" => Some(Box::new(paint::Paint::new("sponge", paint::Kind::Sponge))),
		"blur" => Some(Box::new(paint::Paint::new("blur", paint::Kind::Blur))),
		"sharpen" => Some(Box::new(paint::Paint::new("sharpen", paint::Kind::Sharpen))),
		"smudge" => Some(Box::new(paint::Paint::new("smudge", paint::Kind::Smudge))),
		"pattern-stamp" => Some(Box::new(paint::Paint::new("pattern-stamp", paint::Kind::PatternStamp))),
		"history-brush" => Some(Box::new(paint::Paint::new("history-brush", paint::Kind::HistoryBrush))),
		"art-history" => Some(Box::new(paint::Paint::new("art-history", paint::Kind::ArtHistory))),
		"color-replace" => Some(Box::new(paint::Paint::new("color-replace", paint::Kind::ColorReplace))),
		// The shape tools (M6-T06) and the Path Selection tool that moves a
		// shape by its transform.
		"shape" => Some(Box::new(shape::Shape::new("shape", shape::Kind::Rect))),
		"shape-rounded" => Some(Box::new(shape::Shape::new("shape-rounded", shape::Kind::Rounded))),
		"shape-ellipse" => Some(Box::new(shape::Shape::new("shape-ellipse", shape::Kind::Ellipse))),
		"shape-polygon" => Some(Box::new(shape::Shape::new("shape-polygon", shape::Kind::Polygon))),
		"shape-line" => Some(Box::new(shape::Shape::new("shape-line", shape::Kind::Line))),
		"path-select" => Some(Box::new(path_select::PathSelect::default())),
		"type" => Some(Box::new(type_tool::TypeTool::default())),
		"move" => Some(Box::new(move_tool::MoveTool::default())),
		"quick-select" | "object-select" | "lasso-magnet" | "shape-custom" | "shape-3d" => Some(Box::new(NotYet { name: not_yet_name(id) })),
		// Tools built from a kind (M7-T08, HOWTO R11).
		other => kinds::registered(other),
	}
}

/// The Photoshop name of a tool that is not implemented, for the toast.
fn not_yet_name(id: &str) -> &'static str {
	match id {
		"quick-select" => "Quick Selection",
		"object-select" => "Object Selection",
		"lasso-magnet" => "Magnetic Lasso",
		"shape-custom" => "Custom Shape",
		"shape-3d" => "3D Object",
		_ => "This tool",
	}
}

/// A tool that is not implemented yet: it says so once per click, so the user
/// is not left wondering why nothing happens.
struct NotYet {
	name: &'static str,
}

impl Tool for NotYet {
	fn pointer(&mut self, _ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		if event.kind != PointerKind::Down {
			return ToolResult::default();
		}
		ToolResult {
			info: Some(format!("{} is not implemented yet", self.name)),
			..Default::default()
		}
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

#[cfg(test)]
pub(crate) mod testing {
	//! A document, a scratch store and the services a tool reads, so the tool
	//! tests drive a tool with synthetic pointer and key events (M5-T04).

	use fx_core::{BitDepth, ColorProfile, DocumentColor, SelectionShape};
	use fx_render::{OverlayItem, OverlayStyle};
	use fx_tiles::{TileStore, TileStoreConfig};

	use super::*;

	pub(crate) struct Fixture {
		pub doc: Document,
		pub store: TileStore,
		pub ops: EngineOps,
		pub settings: ToolSettings,
		pub view: ViewTransform,
		/// The clock the events carry, advanced by the tests that need it.
		pub time_us: u64,
	}

	impl Fixture {
		pub(crate) fn new(name: &str, size: (u32, u32), zoom: f64) -> Self {
			let dir = std::env::temp_dir().join(format!("fx-engine-{name}-tests"));
			std::fs::create_dir_all(&dir).unwrap();
			Self {
				doc: Document::new(
					size.0,
					size.1,
					DocumentColor {
						depth: BitDepth::U8,
						profile: ColorProfile::Srgb,
					},
					72.0,
				),
				store: TileStore::new(TileStoreConfig::for_tests(dir)).unwrap(),
				ops: EngineOps::default(),
				settings: ToolSettings::default(),
				view: ViewTransform {
					zoom,
					center_x: f64::from(size.0) / 2.0,
					center_y: f64::from(size.1) / 2.0,
					rotation: 0.0,
				},
				time_us: 0,
			}
		}

		/// The tool's option bar, as `UiToEngine::ToolOptions` delivers it.
		pub(crate) fn options(&mut self, tool: &str, options: serde_json::Value) {
			self.settings.options.insert(tool.to_owned(), options);
		}

		/// Send one pointer event to `tool`; the clock advances a little, so two
		/// presses are never a double-click unless a test says so.
		pub(crate) fn pointer(&mut self, tool: &mut dyn Tool, kind: PointerKind, x: f64, y: f64, modifiers: Modifiers) -> ToolResult {
			self.time_us += 1_000_000;
			self.send(tool, kind, x, y, modifiers)
		}

		/// Like [`pointer`](Self::pointer) but without moving the clock: for a
		/// double-click, which is two presses within a few hundred ms.
		pub(crate) fn pointer_now(&mut self, tool: &mut dyn Tool, kind: PointerKind, x: f64, y: f64, modifiers: Modifiers) -> ToolResult {
			self.send(tool, kind, x, y, modifiers)
		}

		fn send(&mut self, tool: &mut dyn Tool, kind: PointerKind, x: f64, y: f64, modifiers: Modifiers) -> ToolResult {
			let event = DocPointer {
				kind,
				x,
				y,
				pressure: 1.0,
				tilt_x: 0.0,
				tilt_y: 0.0,
				buttons: u8::from(kind != PointerKind::Up),
				modifiers,
				time_us: self.time_us,
			};
			let mut ctx = ToolContext {
				doc: &mut self.doc,
				store: &self.store,
				ops: &self.ops,
				settings: &self.settings,
				view: self.view,
				mask_target: false,
			};
			tool.pointer(&mut ctx, &event)
		}

		/// Make `tool` the active one, the way the engine does when the UI picks a
		/// tool from the toolbar (M6-T03).
		pub(crate) fn activate(&mut self, tool: &mut dyn Tool) {
			tool.activate(&self.doc);
		}

		/// Send one key to `tool`.
		pub(crate) fn key(&mut self, tool: &mut dyn Tool, key: &str) -> ToolResult {
			let mut ctx = ToolContext {
				doc: &mut self.doc,
				store: &self.store,
				ops: &self.ops,
				settings: &self.settings,
				view: self.view,
				mask_target: false,
			};
			tool.key(&mut ctx, key)
		}

		/// Make a selection the way the engine would (a `Select` command).
		pub(crate) fn select(&mut self, shape: SelectionShape) {
			let mut ctx = fx_core::CommandContext {
				tiles: &self.store,
				ops: Some(&self.ops),
			};
			Command::Select {
				shape,
				mode: SelectMode::Replace,
				feather: 0.0,
				anti_alias: true,
			}
			.apply(&mut self.doc, &mut ctx)
			.expect("the selection applies");
		}

		/// A press, a drag through `points` and a release, as one gesture.
		pub(crate) fn drag(&mut self, tool: &mut dyn Tool, points: &[(f64, f64)], modifiers: Modifiers) -> ToolResult {
			let (start, rest) = points.split_first().expect("a drag has a start");
			self.pointer(tool, PointerKind::Down, start.0, start.1, modifiers);
			for &(x, y) in rest {
				self.pointer(tool, PointerKind::Move, x, y, modifiers);
			}
			let end = points.last().expect("a drag has an end");
			self.pointer(tool, PointerKind::Up, end.0, end.1, modifiers)
		}
	}

	/// The polyline of a tool's overlay, for the rubber-band tests.
	pub(crate) fn polyline(overlay: &Overlay) -> (Vec<(f64, f64)>, bool, OverlayStyle) {
		match overlay.items.first() {
			Some(OverlayItem::Polyline { points, closed, style }) => (points.clone(), *closed, *style),
			other => panic!("expected a polyline, got {other:?}"),
		}
	}

	/// The `Select` command a result carries, split up for assertions.
	pub(crate) fn selection(result: ToolResult) -> (SelectionShape, SelectMode, f64, bool) {
		match result.command.expect("a selection command") {
			Command::Select {
				shape,
				mode,
				feather,
				anti_alias,
			} => (shape, mode, feather, anti_alias),
			other => panic!("expected Select, got {other:?}"),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::testing::Fixture;
	use super::*;

	#[test]
	fn unknown_tools_have_no_implementation() {
		let mut tools = Tools::default();
		assert!(tools.get("mixer-brush").is_none(), "not implemented");
		assert!(tools.get("brush").is_some());
		assert!(tools.get("eyedropper").is_some());
		for id in [
			"marquee",
			"marquee-ellipse",
			"marquee-row",
			"marquee-col",
			"lasso",
			"lasso-poly",
			"magic-wand",
			"crop",
		] {
			assert!(tools.get(id).is_some(), "{id}");
		}
	}

	#[test]
	fn a_tool_that_is_not_implemented_says_so_once_per_click() {
		let mut tools = Tools::default();
		let mut fixture = Fixture::new("tools", (10, 10), 1.0);
		let tool = tools.get("quick-select").expect("quick selection has a placeholder");
		let result = fixture.pointer(&mut **tool, PointerKind::Down, 0.0, 0.0, Modifiers::default());
		assert!(result.info.unwrap().contains("Quick Selection"));
		assert!(result.command.is_none(), "nothing to undo");
		// A move after the click stays quiet, so a drag does not spam toasts.
		assert!(fixture.pointer(&mut **tool, PointerKind::Move, 5.0, 5.0, Modifiers::default()).info.is_none());
	}

	#[test]
	fn the_selection_mode_comes_from_the_option_bar_or_the_modifiers() {
		let mut fixture = Fixture::new("tools", (100, 100), 1.0);
		fixture.options("marquee", serde_json::json!({"Mode": 2}));
		let settings = &fixture.settings;
		let store = fx_tiles::TileStore::new(fx_tiles::TileStoreConfig::for_tests(std::env::temp_dir())).unwrap();
		let ops = EngineOps::default();
		let mut doc = Document::new(
			10,
			10,
			fx_core::DocumentColor {
				depth: fx_core::BitDepth::U8,
				profile: fx_core::ColorProfile::Srgb,
			},
			72.0,
		);
		let view = fixture.view;
		let ctx = ToolContext {
			doc: &mut doc,
			store: &store,
			ops: &ops,
			settings,
			view,
			mask_target: false,
		};
		assert_eq!(selection_mode(&ctx, "marquee"), SelectMode::Subtract, "the button group's index");
		assert_eq!(selection_mode(&ctx, "lasso"), SelectMode::Replace, "no options: New selection");
		let plain = Modifiers::default();
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		let alt = Modifiers {
			alt: true,
			..Default::default()
		};
		let both = Modifiers {
			shift: true,
			alt: true,
			..Default::default()
		};
		assert_eq!(mode_at_press(plain, SelectMode::Replace), SelectMode::Replace);
		assert_eq!(mode_at_press(shift, SelectMode::Replace), SelectMode::Add);
		assert_eq!(mode_at_press(alt, SelectMode::Replace), SelectMode::Subtract);
		assert_eq!(mode_at_press(both, SelectMode::Replace), SelectMode::Intersect);
		// A wording works too (a drop-down instead of the button group).
		fixture.options("lasso", serde_json::json!({"Mode": "Add to selection"}));
		let settings = &fixture.settings;
		let ctx = ToolContext {
			doc: &mut doc,
			store: &store,
			ops: &ops,
			settings,
			view,
			mask_target: false,
		};
		assert_eq!(selection_mode(&ctx, "lasso"), SelectMode::Add);
	}

	#[test]
	fn settings_read_typed_options() {
		let mut settings = ToolSettings::default();
		settings.options.insert(
			"eyedropper".into(),
			serde_json::json!({"Sample Size": "3 by 3 Average", "Sample": "Current Layer", "Show Sampling Ring": true, "Area": 3}),
		);
		assert_eq!(settings.string("eyedropper", "Sample Size").as_deref(), Some("3 by 3 Average"));
		assert_eq!(settings.string("eyedropper", "Sample").as_deref(), Some("Current Layer"));
		assert!(settings.bool("eyedropper", "Show Sampling Ring").unwrap());
		assert_eq!(settings.number("eyedropper", "Area"), Some(3.0));
		assert_eq!(settings.string("eyedropper", "Missing"), None);
		assert_eq!(settings.string("brush", "Size"), None);
	}
}
