//! The Crop tool (C, M6-T03).
//!
//! The tool never touches the document: it draws a box over it (with the area
//! the crop will throw away dimmed, M5-T02's overlay) and, on Enter, sends one
//! [`Command::Crop`] — the whole crop is a single History step, exactly like
//! Photoshop's ✓.
//!
//! The box starts as the canvas ([`Tool::activate`]). Eight handles resize it
//! (Shift keeps the box's ratio, Alt grows it about its centre), the option
//! bar's Ratio / W / H drive the size, a press inside moves the box and a press
//! just outside it turns the box (the straighten). **Deviation**, as the card
//! allows: Photoshop slides and turns the *image* under a box that stays
//! upright; this tool moves and turns the *box* over the image instead (like
//! Photoshop's "Classic Mode"), which is the same crop seen from the other side
//! of the transform: a box turned clockwise by *t* crops an image turned by −*t*.
//! Escape puts the box back on the canvas and upright, so a cancelled crop
//! changes nothing.
//!
//! A turned box keeps its own frame: its rectangle (`x, y, w, h`) is the
//! upright box, and `turn_deg` turns it about its centre. Handles and hit
//! tests work in that frame, so a turned box resizes along its own sides.

use fx_core::{Command, Document};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// Screen pixels a press may miss a handle by and still take it.
const HANDLE_SLOP: f64 = 6.0;
/// A press this close outside the box (screen pixels) starts a straighten.
const ROTATE_RING: f64 = 28.0;
/// The dimming of what the crop will cut away.
const SHADE: [f32; 4] = [0.0, 0.0, 0.0, 0.5];
/// The square handle, on screen.
const HANDLE_PX: f32 = 8.0;
/// The smallest box, in document pixels: a canvas is at least 1 × 1.
const MIN_SIDE: f64 = 1.0;
/// The slops are screen distances; below this zoom they would be huge in
/// document pixels, so the box maths floors the zoom instead.
const MIN_ZOOM: f64 = 0.02;

/// A crop box in document pixels; `w` and `h` are always positive.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
	x: f64,
	y: f64,
	w: f64,
	h: f64,
}

impl Rect {
	/// The whole canvas: where a crop box starts.
	fn canvas(doc: &Document) -> Self {
		Self {
			x: 0.0,
			y: 0.0,
			w: f64::from(doc.width).max(MIN_SIDE),
			h: f64::from(doc.height).max(MIN_SIDE),
		}
	}

	/// The box between two opposite corners, whole pixels or not. The corners
	/// may cross, which flips the box instead of making a negative one.
	fn from_edges(left: f64, top: f64, right: f64, bottom: f64) -> Self {
		Self {
			x: left.min(right),
			y: top.min(bottom),
			w: (right - left).abs().max(MIN_SIDE),
			h: (bottom - top).abs().max(MIN_SIDE),
		}
	}

	fn right(&self) -> f64 {
		self.x + self.w
	}

	fn bottom(&self) -> f64 {
		self.y + self.h
	}

	fn centre(&self) -> (f64, f64) {
		(self.x + self.w / 2.0, self.y + self.h / 2.0)
	}

	/// The document point `grab` names in the box's own (upright) frame: a
	/// corner, an edge's middle, or the centre for a move, a rotation and a
	/// brand-new box.
	fn point(&self, grab: Grab) -> (f64, f64) {
		let (cx, cy) = self.centre();
		match grab {
			Grab::TopLeft => (self.x, self.y),
			Grab::Top => (cx, self.y),
			Grab::TopRight => (self.right(), self.y),
			Grab::Right => (self.right(), cy),
			Grab::BottomRight => (self.right(), self.bottom()),
			Grab::Bottom => (cx, self.bottom()),
			Grab::BottomLeft => (self.x, self.bottom()),
			Grab::Left => (self.x, cy),
			Grab::Move | Grab::Rotate | Grab::New => (cx, cy),
		}
	}

	/// The whole-pixel rectangle [`Command::Crop`] takes. The box may lie
	/// outside the canvas (Photoshop allows that too: such a crop comes out
	/// with empty borders), but never absurdly far away.
	fn whole(&self) -> (i32, i32, u32, u32) {
		let limit = 1_000_000.0;
		(
			self.x.round().clamp(-limit, limit) as i32,
			self.y.round().clamp(-limit, limit) as i32,
			self.w.round().clamp(MIN_SIDE, limit) as u32,
			self.h.round().clamp(MIN_SIDE, limit) as u32,
		)
	}

	/// The four corners on the document, the box turned by `turn_deg` about
	/// its centre: top-left, top-right, bottom-right, bottom-left.
	fn corners(&self, turn_deg: f64) -> [(f64, f64); 4] {
		let centre = self.centre();
		[(self.x, self.y), (self.right(), self.y), (self.right(), self.bottom()), (self.x, self.bottom())].map(|p| turn(p, centre, turn_deg))
	}

	/// What a press at `pointer` (in the box's own frame) takes hold of: a
	/// handle first (they win over the box itself), then the box, then the
	/// straighten ring around it, and beyond that a new box is drawn.
	fn grab_at(&self, pointer: (f64, f64), zoom: f64) -> Grab {
		let zoom = zoom.max(MIN_ZOOM);
		let slop = HANDLE_SLOP / zoom;
		for grab in Grab::HANDLES {
			let (x, y) = self.point(grab);
			if (pointer.0 - x).abs() <= slop && (pointer.1 - y).abs() <= slop {
				return grab;
			}
		}
		let inside = pointer.0 >= self.x && pointer.0 <= self.right() && pointer.1 >= self.y && pointer.1 <= self.bottom();
		if inside {
			return Grab::Move;
		}
		let ring = ROTATE_RING / zoom;
		let near = pointer.0 >= self.x - ring && pointer.0 <= self.right() + ring && pointer.1 >= self.y - ring && pointer.1 <= self.bottom() + ring;
		if near { Grab::Rotate } else { Grab::New }
	}
}

/// What a press took hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
	TopLeft,
	Top,
	TopRight,
	Right,
	BottomRight,
	Bottom,
	BottomLeft,
	Left,
	/// Inside the box: the box follows the pointer.
	Move,
	/// Just outside it: the image is straightened.
	Rotate,
	/// Well outside it: a new box is drawn from the press.
	New,
}

impl Grab {
	/// The eight handles, in the order the overlay draws them.
	const HANDLES: [Grab; 8] = [
		Grab::TopLeft,
		Grab::Top,
		Grab::TopRight,
		Grab::Right,
		Grab::BottomRight,
		Grab::Bottom,
		Grab::BottomLeft,
		Grab::Left,
	];

	fn is_handle(self) -> bool {
		Self::HANDLES.contains(&self)
	}

	fn moves_left(self) -> bool {
		matches!(self, Grab::TopLeft | Grab::Left | Grab::BottomLeft)
	}

	fn moves_right(self) -> bool {
		matches!(self, Grab::TopRight | Grab::Right | Grab::BottomRight)
	}

	fn moves_top(self) -> bool {
		matches!(self, Grab::TopLeft | Grab::Top | Grab::TopRight)
	}

	fn moves_bottom(self) -> bool {
		matches!(self, Grab::BottomLeft | Grab::Bottom | Grab::BottomRight)
	}
}

/// What the option bar asks the box to be.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
	/// Nothing constrains the box: it follows the pointer.
	Free,
	/// The Ratio menu (or Shift) keeps this width ÷ height.
	Ratio(f64),
	/// Width and Height are both filled in: that exact size, in pixels.
	Size(f64, f64),
}

impl Target {
	fn of(ctx: &ToolContext<'_>, id: &str) -> Self {
		let ratio = match ctx.settings.string(id, "Ratio").as_deref() {
			Some("1:1 (Square)") => 1.0,
			Some("5:4") => 5.0 / 4.0,
			Some("4:3") => 4.0 / 3.0,
			Some("3:2") => 3.0 / 2.0,
			Some("16:9") => 16.0 / 9.0,
			Some("Original Ratio") => f64::from(ctx.doc.width) / f64::from(ctx.doc.height.max(1)),
			_ => 0.0,
		};
		let width = ctx.settings.number(id, "W").filter(|v| *v > 0.0);
		let height = ctx.settings.number(id, "H").filter(|v| *v > 0.0);
		match (width, height) {
			(Some(w), Some(h)) => Target::Size(w, h),
			_ if ratio > 0.0 => Target::Ratio(ratio),
			_ => Target::Free,
		}
	}

	/// The ratio the box keeps while it is resized, if any.
	fn ratio(self) -> Option<f64> {
		match self {
			Target::Ratio(ratio) => Some(ratio),
			Target::Size(w, h) => Some(w / h),
			Target::Free => None,
		}
	}
}

/// A drag in progress. It carries the box as it was at the press, because
/// every change is computed from there, not from the previous event.
#[derive(Clone, Copy, Debug)]
struct Drag {
	grab: Grab,
	rect: Rect,
	/// The press, on the document.
	start: (f64, f64),
	/// The box's turn the drag started from, and the pointer's direction from
	/// the box's centre then.
	turn_deg: f64,
	from: f64,
	modifiers: Modifiers,
}

impl Drag {
	/// A document point in the frame of the box as it was at the press.
	fn local(&self, p: (f64, f64)) -> (f64, f64) {
		turn(p, self.rect.centre(), -self.turn_deg)
	}

	/// A box drawn in that frame, back on the document: the frame turns about
	/// the press-time centre, so a box whose centre moved is re-centred where
	/// the turn puts it (its sides stay parallel to the turned frame).
	fn placed(&self, local: Rect) -> Rect {
		let (cx, cy) = turn(local.centre(), self.rect.centre(), self.turn_deg);
		Rect {
			x: cx - local.w / 2.0,
			y: cy - local.h / 2.0,
			w: local.w,
			h: local.h,
		}
	}
}

/// The crop tool.
pub struct Crop {
	id: &'static str,
	/// The box, upright; `None` until the tool is activated.
	rect: Option<Rect>,
	/// The box's turn about its centre, degrees clockwise on screen. The crop
	/// straightens the image by the opposite angle.
	turn_deg: f64,
	drag: Option<Drag>,
	/// The last pointer position and zoom, for the cursor a resize wants.
	hover: Option<(f64, f64)>,
	zoom: f64,
}

impl Crop {
	/// The crop tool, reading option bar `id`.
	pub fn new(id: &'static str) -> Self {
		Self {
			id,
			rect: None,
			turn_deg: 0.0,
			drag: None,
			hover: None,
			zoom: 1.0,
		}
	}

	/// The box the tool is working on, defaulting to the canvas.
	fn rect_of(&self, ctx: &ToolContext<'_>) -> Rect {
		self.rect.unwrap_or_else(|| Rect::canvas(ctx.doc))
	}

	/// The crop this box asks for, or `None` when there is nothing to do (the
	/// box is the whole canvas and the image is not straightened: cropping
	/// would change nothing).
	fn command(&self, ctx: &ToolContext<'_>) -> Option<Command> {
		let rect = self.rect?;
		let whole = rect.whole();
		let unchanged = self.turn_deg == 0.0 && whole == (0, 0, ctx.doc.width, ctx.doc.height);
		(!unchanged).then(|| Command::Crop {
			rect: whole,
			// What the turned box shows, upright: the image turns the other way.
			angle_deg: if self.turn_deg == 0.0 { 0.0 } else { -self.turn_deg },
			delete_cropped: ctx.settings.bool(self.id, "Delete Cropped Pixels").unwrap_or(false),
		})
	}

	/// Fill: Generative Expand (M13-T06): when the upright box reaches past
	/// the canvas, the engine fills the new area through ComfyUI once the crop
	/// has grown the canvas. The old canvas in the new canvas's pixels.
	fn expand_request(&self, ctx: &ToolContext<'_>) -> Option<crate::tools::AiRequest> {
		if ctx.settings.string(self.id, "Fill").as_deref() != Some("Generative Expand") || self.turn_deg != 0.0 {
			return None;
		}
		let (x, y, w, h) = self.rect?.whole();
		let (x, y, w, h) = (i64::from(x), i64::from(y), i64::from(w), i64::from(h));
		let (dw, dh) = (i64::from(ctx.doc.width), i64::from(ctx.doc.height));
		let grows = x < 0 || y < 0 || x + w > dw || y + h > dh;
		grows.then(|| crate::tools::AiRequest::Expand {
			old: (-x, -y, dw - x, dh - y),
			prompt: ctx.settings.string(self.id, "Prompt").unwrap_or_default(),
		})
	}

	/// The box a resize drag asks for, in the press-time frame (`pointer` is
	/// in it too): the grabbed edges follow the pointer, the option bar's ratio
	/// or size has the last word, and Alt grows the box about its centre.
	fn resized(&self, drag: &Drag, ctx: &ToolContext<'_>, pointer: (f64, f64)) -> Rect {
		let (mut left, mut top) = (drag.rect.x, drag.rect.y);
		let (mut right, mut bottom) = (drag.rect.right(), drag.rect.bottom());
		if drag.grab.moves_left() {
			left = pointer.0;
		}
		if drag.grab.moves_right() {
			right = pointer.0;
		}
		if drag.grab.moves_top() {
			top = pointer.1;
		}
		if drag.grab.moves_bottom() {
			bottom = pointer.1;
		}
		let mut rect = Rect::from_edges(left, top, right, bottom);
		let target = Target::of(ctx, self.id);
		// Shift keeps the box's own ratio; the option bar's wins over it.
		let shift_ratio = (drag.modifiers.shift && drag.grab.is_handle()).then(|| drag.rect.w / drag.rect.h);
		if let Some(ratio) = target.ratio().or(shift_ratio).filter(|r| *r > 0.0) {
			// The axis the handle drives sizes the other one.
			let (w, h) = if drag.grab.moves_left() || drag.grab.moves_right() {
				(rect.w, rect.w / ratio)
			} else {
				(rect.h * ratio, rect.h)
			};
			rect = anchored(rect, drag.grab, w, h);
		}
		if let Target::Size(w, h) = target {
			rect = anchored(rect, drag.grab, w, h);
		}
		if drag.modifiers.alt {
			rect = mirrored(rect, drag.rect, drag.grab);
		}
		rect
	}

	/// The turn a rotation drag asks for: the direction from the box's centre
	/// to the pointer, less the direction it started at (the box follows the
	/// pointer round). Shift snaps to 15° steps, like Photoshop's rotation.
	fn turned(&self, drag: &Drag, pointer: (f64, f64)) -> f64 {
		let (cx, cy) = drag.rect.centre();
		let now = (pointer.1 - cy).atan2(pointer.0 - cx);
		let degrees = drag.turn_deg + (now - drag.from).to_degrees();
		// A full turn of the pointer is not a 360° straighten.
		let degrees = (degrees + 180.0).rem_euclid(360.0) - 180.0;
		if drag.modifiers.shift { (degrees / 15.0).round() * 15.0 } else { degrees }
	}

	/// The box after a pointer move (on the document), and the turn a
	/// rotation sets.
	fn track(&mut self, drag: &Drag, ctx: &ToolContext<'_>, pointer: (f64, f64)) -> Rect {
		let (dx, dy) = (pointer.0 - drag.start.0, pointer.1 - drag.start.1);
		match drag.grab {
			Grab::Move => Rect::from_edges(drag.rect.x + dx, drag.rect.y + dy, drag.rect.right() + dx, drag.rect.bottom() + dy),
			Grab::New => {
				let (start, now) = (drag.local(drag.start), drag.local(pointer));
				drag.placed(Rect::from_edges(start.0, start.1, now.0, now.1))
			}
			Grab::Rotate => {
				self.turn_deg = self.turned(drag, pointer);
				drag.rect
			}
			grab if grab.is_handle() => drag.placed(self.resized(drag, ctx, drag.local(pointer))),
			_ => drag.rect,
		}
	}

	/// The status line: the box the user is building, and the straighten angle
	/// once there is one (Photoshop shows the same numbers in the Info panel).
	fn status(&self, rect: Rect) -> String {
		let size = format!("W: {} px  H: {} px", rect.w.round(), rect.h.round());
		if self.turn_deg == 0.0 {
			size
		} else {
			format!("{size}  Angle: {:.1}°", self.turn_deg)
		}
	}
}

impl Tool for Crop {
	fn activate(&mut self, doc: &Document) {
		// A crop starts as the whole canvas, with nothing turned.
		self.rect = Some(Rect::canvas(doc));
		self.turn_deg = 0.0;
		self.drag = None;
	}

	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.zoom = ctx.view.zoom;
		let pointer = (event.x, event.y);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let rect = self.rect_of(ctx);
				let grab = rect.grab_at(turn(pointer, rect.centre(), -self.turn_deg), ctx.view.zoom);
				let (cx, cy) = rect.centre();
				let from = (event.y - cy).atan2(event.x - cx);
				self.drag = Some(Drag {
					grab,
					rect,
					start: pointer,
					turn_deg: self.turn_deg,
					from,
					modifiers: event.modifiers,
				});
				self.rect = Some(rect);
				self.hover = Some(pointer);
				ToolResult {
					cursor: Some(CursorShape::Crosshair),
					status: Some(self.status(rect)),
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move | PointerKind::Down => {
				self.hover = Some(pointer);
				let Some(drag) = self.drag else {
					return ToolResult::default();
				};
				let rect = self.track(&drag, ctx, pointer);
				self.rect = Some(rect);
				ToolResult {
					status: Some(self.status(rect)),
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up => {
				let Some(drag) = self.drag.take() else {
					return ToolResult::default();
				};
				// A press with no travel is still a grab: the box stays where
				// it already was (Photoshop's crop box does not jump on a
				// click either).
				let rect = self.track(&drag, ctx, pointer);
				self.rect = Some(rect);
				ToolResult {
					status: Some(self.status(rect)),
					redraw: true,
					..Default::default()
				}
			}
			// A hover, or a release of a button this tool did not start.
			_ => ToolResult::default(),
		}
	}

	/// Enter crops (one [`Command::Crop`], one History step); Escape puts the
	/// box back on the canvas and drops the straighten angle, so nothing
	/// happens.
	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		match key {
			"Enter" => ToolResult {
				command: self.command(ctx),
				ai: self.expand_request(ctx),
				..Default::default()
			},
			"Escape" => {
				let reset = self.turn_deg != 0.0 || self.rect.is_some_and(|rect| rect.whole() != (0, 0, ctx.doc.width, ctx.doc.height));
				self.rect = Some(Rect::canvas(ctx.doc));
				self.turn_deg = 0.0;
				self.drag = None;
				ToolResult {
					redraw: reset,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let rect = self.rect?;
		let centre = rect.centre();
		let on_doc = |p: (f64, f64)| turn(p, centre, self.turn_deg);
		let mut items = vec![OverlayItem::Shade {
			quad: rect.corners(self.turn_deg),
			color: SHADE,
		}];
		// The rule-of-thirds grid: two lines each way, the box divided in
		// three (Photoshop's default crop guide).
		for t in [1.0 / 3.0, 2.0 / 3.0] {
			let x = rect.x + rect.w * t;
			let y = rect.y + rect.h * t;
			for (a, b) in [((x, rect.y), (x, rect.bottom())), ((rect.x, y), (rect.right(), y))] {
				items.push(OverlayItem::Polyline {
					points: vec![on_doc(a), on_doc(b)],
					closed: false,
					style: OverlayStyle::Xor,
				});
			}
		}
		for grab in Grab::HANDLES {
			items.push(OverlayItem::Handle {
				at: on_doc(rect.point(grab)),
				size_px: HANDLE_PX,
			});
		}
		Some(Overlay { items })
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		// A resize has no cursor of its own in the OS set; a press inside the
		// box moves it, which does.
		let moved = matches!((self.rect, self.hover), (Some(rect), Some(pointer))
			if rect.grab_at(turn(pointer, rect.centre(), -self.turn_deg), self.zoom) == Grab::Move);
		if moved { CursorShape::Move } else { CursorShape::Crosshair }
	}
}

/// `p` turned by `degrees` (clockwise on screen, y down) about `centre`.
fn turn(p: (f64, f64), centre: (f64, f64), degrees: f64) -> (f64, f64) {
	if degrees == 0.0 {
		return p;
	}
	let (sin, cos) = degrees.to_radians().sin_cos();
	let (dx, dy) = (p.0 - centre.0, p.1 - centre.1);
	(centre.0 + dx * cos - dy * sin, centre.1 + dx * sin + dy * cos)
}

/// The box of `w × h` whose opposite corner (or edge) stays where it was: the
/// grabbed side follows the pointer, the other one is pinned.
fn anchored(rect: Rect, grab: Grab, w: f64, h: f64) -> Rect {
	let right = if grab.moves_left() { rect.right() } else { rect.x + w };
	let left = if grab.moves_left() { right - w } else { rect.x };
	let bottom = if grab.moves_top() { rect.bottom() } else { rect.y + h };
	let top = if grab.moves_top() { bottom - h } else { rect.y };
	Rect::from_edges(left, top, right, bottom)
}

/// `rect` mirrored about the centre of `original`, for every axis the grab
/// moves: Alt resizes about the centre instead of about the opposite edge.
fn mirrored(rect: Rect, original: Rect, grab: Grab) -> Rect {
	let (cx, cy) = original.centre();
	let (mut left, mut top) = (rect.x, rect.y);
	let (mut right, mut bottom) = (rect.right(), rect.bottom());
	if grab.moves_left() {
		right = 2.0 * cx - left;
	}
	if grab.moves_right() {
		left = 2.0 * cx - right;
	}
	if grab.moves_top() {
		bottom = 2.0 * cy - top;
	}
	if grab.moves_bottom() {
		top = 2.0 * cy - bottom;
	}
	Rect::from_edges(left, top, right, bottom)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tools::testing::Fixture;

	fn fixture() -> (Fixture, Crop) {
		let mut f = Fixture::new("crop", (400, 300), 1.0);
		let mut tool = Crop::new("crop");
		f.activate(&mut tool);
		(f, tool)
	}

	/// The `Crop` command a result carries: rectangle, angle and option.
	fn crop(result: ToolResult) -> (i32, i32, u32, u32, f64, bool) {
		match result.command.expect("a crop command") {
			Command::Crop {
				rect,
				angle_deg,
				delete_cropped,
			} => (rect.0, rect.1, rect.2, rect.3, angle_deg, delete_cropped),
			other => panic!("expected Crop, got {other:?}"),
		}
	}

	/// The corners of the box the overlay dims around.
	fn corners(overlay: &Overlay) -> [(f64, f64); 4] {
		match overlay.items.first() {
			Some(OverlayItem::Shade { quad, .. }) => *quad,
			other => panic!("expected a shade, got {other:?}"),
		}
	}

	/// The rectangle the box dims around, for an upright box.
	fn shade(overlay: &Overlay) -> (f64, f64, f64, f64) {
		let quad = corners(overlay);
		(quad[0].0, quad[0].1, quad[2].0, quad[2].1)
	}

	#[test]
	fn the_box_starts_as_the_canvas_with_handles_and_a_grid() {
		let (mut f, mut tool) = fixture();
		let overlay = tool.overlay().expect("a box");
		assert_eq!(shade(&overlay), (0.0, 0.0, 400.0, 300.0));
		assert_eq!(
			overlay.items.iter().filter(|item| matches!(item, OverlayItem::Handle { .. })).count(),
			8,
			"eight handles"
		);
		let grid: Vec<_> = overlay
			.items
			.iter()
			.filter_map(|item| match item {
				OverlayItem::Polyline { points, .. } => Some(points.clone()),
				_ => None,
			})
			.collect();
		assert_eq!(grid.len(), 4, "the rule of thirds");
		let (x, y) = grid[0][0];
		assert!((x - 400.0 / 3.0).abs() < 1e-9 && y == 0.0, "{grid:?}");
		assert_eq!(grid[0][1], (x, 300.0));
		// The whole canvas: there is nothing to crop, so Enter is quiet.
		assert!(f.key(&mut tool, "Enter").command.is_none());
	}

	#[test]
	fn dragging_a_handle_resizes_the_box_and_enter_crops_to_it() {
		let (mut f, mut tool) = fixture();
		let status = f
			.pointer(&mut tool, PointerKind::Down, 0.0, 0.0, Modifiers::default())
			.status
			.expect("a status line");
		assert!(status.contains("400") && status.contains("300"), "{status}");
		f.drag(&mut tool, &[(0.0, 0.0), (50.0, 60.0)], Modifiers::default());
		assert_eq!(shade(&tool.overlay().expect("a box")), (50.0, 60.0, 400.0, 300.0));
		assert_eq!(crop(f.key(&mut tool, "Enter")), (50, 60, 350, 240, 0.0, false));
	}

	#[test]
	fn shift_keeps_the_boxes_ratio_and_alt_grows_it_about_the_centre() {
		let (mut f, mut tool) = fixture();
		// The canvas is 400 × 300: Shift keeps 4:3.
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		f.drag(&mut tool, &[(400.0, 300.0), (200.0, 250.0)], shift);
		let (x, y, x1, y1) = shade(&tool.overlay().expect("a box"));
		assert!(((x1 - x) / (y1 - y) - 4.0 / 3.0).abs() < 1e-6, "{} × {}", x1 - x, y1 - y);
		assert_eq!((x, y), (0.0, 0.0), "the opposite corner stays put");

		let (mut f, mut tool) = fixture();
		let alt = Modifiers {
			alt: true,
			..Default::default()
		};
		f.drag(&mut tool, &[(400.0, 300.0), (300.0, 250.0)], alt);
		let (x, y, x1, y1) = shade(&tool.overlay().expect("a box"));
		assert_eq!((x, y, x1, y1), (100.0, 50.0, 300.0, 250.0), "the box mirrors about the centre");
		assert_eq!(((x + x1) / 2.0, (y + y1) / 2.0), (200.0, 150.0));
	}

	#[test]
	fn the_option_bars_ratio_and_size_drive_the_resize() {
		let (mut f, mut tool) = fixture();
		f.options("crop", serde_json::json!({"Ratio": "1:1 (Square)"}));
		f.drag(&mut tool, &[(400.0, 300.0), (200.0, 250.0)], Modifiers::default());
		let (x, y, x1, y1) = shade(&tool.overlay().expect("a box"));
		assert!(((x1 - x) - (y1 - y)).abs() < 1e-6, "{} × {}", x1 - x, y1 - y);

		let (mut f, mut tool) = fixture();
		f.options("crop", serde_json::json!({"W": 120, "H": 80}));
		f.drag(&mut tool, &[(400.0, 300.0), (100.0, 100.0)], Modifiers::default());
		let (x, y, x1, y1) = shade(&tool.overlay().expect("a box"));
		assert_eq!((x1 - x, y1 - y), (120.0, 80.0), "the size the option bar asked for");
		assert_eq!((x, y), (0.0, 0.0), "anchored at the corner the drag did not take");
		assert_eq!(crop(f.key(&mut tool, "Enter")), (0, 0, 120, 80, 0.0, false));
	}

	#[test]
	fn a_press_inside_moves_the_box_and_just_outside_straightens() {
		let (mut f, mut tool) = fixture();
		// A box a little smaller than the canvas, so it has room to move.
		f.drag(&mut tool, &[(0.0, 0.0), (50.0, 50.0)], Modifiers::default());
		f.drag(&mut tool, &[(300.0, 200.0), (310.0, 210.0)], Modifiers::default());
		let (x, y, x1, y1) = shade(&tool.overlay().expect("a box"));
		assert_eq!((x, y, x1, y1), (60.0, 60.0, 410.0, 310.0), "the box moved by the drag");
		// Pressed just outside the box, it turns the box instead.
		let result = f.drag(&mut tool, &[(x1 + 8.0, y1 + 8.0), (x1 + 40.0, y1 - 30.0)], Modifiers::default());
		assert!(result.command.is_none(), "the rotation is only applied on Enter");
		let (x, y, w, h, angle, _) = crop(f.key(&mut tool, "Enter"));
		assert_eq!((x, y, w, h), (60, 60, 350, 250), "the box did not move or resize");
		assert!(angle.abs() > 1.0, "a straighten angle: {angle}");
	}

	#[test]
	fn shift_snaps_the_straighten_to_fifteen_degrees() {
		let (mut f, mut tool) = fixture();
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		// Take the ring just past the box's right edge and swing the pointer.
		f.drag(&mut tool, &[(410.0, 200.0), (440.0, 350.0)], shift);
		let (.., angle, _) = crop(f.key(&mut tool, "Enter"));
		assert_eq!(angle % 15.0, 0.0, "{angle} is a step");
		assert!(angle != 0.0);
	}

	#[test]
	fn enter_carries_delete_cropped_pixels_and_escape_cancels() {
		let (mut f, mut tool) = fixture();
		f.options("crop", serde_json::json!({"Delete Cropped Pixels": true}));
		f.drag(&mut tool, &[(0.0, 0.0), (100.0, 100.0)], Modifiers::default());
		assert_eq!(crop(f.key(&mut tool, "Enter")), (100, 100, 300, 200, 0.0, true));
		// Escape puts the box back on the canvas, so Enter then has nothing to
		// do.
		f.drag(&mut tool, &[(100.0, 100.0), (150.0, 150.0)], Modifiers::default());
		assert!(f.key(&mut tool, "Escape").redraw, "the box was somewhere else");
		assert_eq!(shade(&tool.overlay().expect("a box")), (0.0, 0.0, 400.0, 300.0));
		assert!(f.key(&mut tool, "Enter").command.is_none(), "nothing to crop");
		// Escape with nothing to undo is quiet.
		assert!(!f.key(&mut tool, "Escape").redraw);
		// Any other key is the tool's to ignore.
		assert!(!f.key(&mut tool, "Tab").redraw);
	}

	#[test]
	fn a_turned_box_shows_turned_and_crops_the_image_the_other_way() {
		let (mut f, mut tool) = fixture();
		// Take the ring right of the box's centre and swing the pointer down:
		// the box follows it clockwise.
		f.drag(
			&mut tool,
			&[(410.0, 150.0), (410.0, 150.0 + 210.0 * 10f64.to_radians().tan())],
			Modifiers::default(),
		);
		let quad = corners(&tool.overlay().expect("a box"));
		// The top-left corner turned 10° clockwise about (200, 150).
		let (sin, cos) = 10f64.to_radians().sin_cos();
		let expected = (200.0 - 200.0 * cos + 150.0 * sin, 150.0 - 200.0 * sin - 150.0 * cos);
		assert!((quad[0].0 - expected.0).abs() < 1e-6 && (quad[0].1 - expected.1).abs() < 1e-6, "{quad:?}");
		let (x, y, w, h, angle, _) = crop(f.key(&mut tool, "Enter"));
		assert_eq!((x, y, w, h), (0, 0, 400, 300), "the box itself did not change size");
		assert!((angle + 10.0).abs() < 1e-9, "the image turns back: {angle}");

		// A handle of the turned box drags along the box's own side: pulling
		// the right edge's handle outwards along the turned x axis widens the
		// box, and its height stays.
		let (mut f, mut tool) = fixture();
		f.drag(
			&mut tool,
			&[(410.0, 150.0), (410.0, 150.0 + 210.0 * 10f64.to_radians().tan())],
			Modifiers::default(),
		);
		let right = (200.0 + 200.0 * cos, 150.0 + 200.0 * sin);
		f.drag(&mut tool, &[right, (right.0 - 50.0 * cos, right.1 - 50.0 * sin)], Modifiers::default());
		let (.., w, h, _, _) = crop(f.key(&mut tool, "Enter"));
		assert_eq!((w, h), (350, 300));
	}

	#[test]
	fn a_press_far_outside_draws_a_new_box() {
		let (mut f, mut tool) = fixture();
		f.drag(&mut tool, &[(600.0, 500.0), (700.0, 600.0)], Modifiers::default());
		let (x, y, x1, y1) = shade(&tool.overlay().expect("a box"));
		assert_eq!((x, y, x1, y1), (600.0, 500.0, 700.0, 600.0), "the box the drag drew");
		assert_eq!(
			crop(f.key(&mut tool, "Enter")),
			(600, 500, 100, 100, 0.0, false),
			"outside the canvas is allowed"
		);
	}

	#[test]
	fn the_cursor_follows_what_a_press_would_take() {
		let (mut f, mut tool) = fixture();
		f.pointer(&mut tool, PointerKind::Move, 200.0, 150.0, Modifiers::default());
		assert_eq!(tool.cursor(Modifiers::default()), CursorShape::Move, "inside the box");
		f.pointer(&mut tool, PointerKind::Move, 0.0, 0.0, Modifiers::default());
		assert_eq!(tool.cursor(Modifiers::default()), CursorShape::Crosshair, "over a handle");
		// A tool that was never activated has no box, and no overlay.
		assert!(Crop::new("crop").overlay().is_none());
	}
}
