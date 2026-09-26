//! The shape tools (`U`): Rectangle, Rounded Rectangle, Ellipse, Polygon and
//! Line (M6-T06).
//!
//! A drag is the shape's bounding box: it is previewed as the shape's own
//! outline (M5-T02) and becomes **one** [`Command::AddLayer`] with a
//! [`NewLayer::Shape`] on release, so the whole drag is a single History step
//! ("New Rectangle"). The shape stays vector — editing it never resamples
//! anything (D-055) and its tiles are drawn from the geometry at whatever
//! level is on screen.
//!
//! Modifiers follow Photoshop: **Shift** constrains to a square (a circle for
//! the ellipse, an equilateral polygon), **Alt** draws from the centre. The
//! option bar supplies the per-kind options — `Sides` and `Kind`
//! (Smooth / Star) for the polygon, `Radius` for the rounded rectangle,
//! `Weight` for the line — and the fill is the foreground colour, as in
//! Photoshop. A stroke is added only when the bar asks for one
//! (`Stroke Width`, with the background colour): T09 wires those fields.

use fx_core::Command;
use fx_core::command::NewLayer;
use fx_core::vector::{Paint, PathEl, StrokeAlign, StrokeStyle, VectorShape};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// A drag shorter than this many *screen* pixels is a click: Photoshop opens
/// the shape's dialog there, Fotox does nothing (no such dialog yet).
const CLICK_SLOP: f64 = 3.0;

/// Which shape a tool id draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	Rect,
	Rounded,
	Ellipse,
	Polygon,
	Line,
}

/// The drag in progress: the press, the current point, and the modifiers as
/// they were at the last event (Alt from the centre is decided at the press,
/// Shift is read live, both like Photoshop).
#[derive(Clone, Copy, Debug)]
struct Drag {
	from: (f64, f64),
	to: (f64, f64),
	from_centre: bool,
	shift: bool,
}

impl Drag {
	/// The document box the drag describes, `(x, y, w, h)`.
	fn box_of(&self) -> (f64, f64, f64, f64) {
		let (mut dx, mut dy) = (self.to.0 - self.from.0, self.to.1 - self.from.1);
		if self.shift {
			// Square: the larger extent, keeping the direction drawn in.
			let side = dx.abs().max(dy.abs());
			dx = side * if dx < 0.0 { -1.0 } else { 1.0 };
			dy = side * if dy < 0.0 { -1.0 } else { 1.0 };
		}
		if self.from_centre {
			(self.from.0 - dx.abs(), self.from.1 - dy.abs(), 2.0 * dx.abs(), 2.0 * dy.abs())
		} else {
			(self.from.0.min(self.from.0 + dx), self.from.1.min(self.from.1 + dy), dx.abs(), dy.abs())
		}
	}
}

/// A number from the option bar: a `num` field sends a number, while a
/// `select` sends the text of the choice it shows ("5", "3 px"). A leading
/// number is taken, so a value with a unit still reads (M6-T06).
fn number(ctx: &ToolContext<'_>, tool: &str, key: &str) -> Option<f64> {
	if let Some(value) = ctx.settings.number(tool, key) {
		return Some(value);
	}
	let text = ctx.settings.string(tool, key)?;
	let digits: String = text.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
	digits.parse().ok()
}

/// The Shape tool.
pub struct Shape {
	tool: String,
	kind: Kind,
	drag: Option<Drag>,
	/// The pending shape's outline in document coordinates, ready for the
	/// overlay pass (which gets no context of its own).
	preview: Vec<(f64, f64)>,
}

impl Shape {
	pub fn new(tool: &str, kind: Kind) -> Self {
		Self {
			tool: tool.to_owned(),
			kind,
			drag: None,
			preview: Vec::new(),
		}
	}

	/// The shape the box describes, in its own local space.
	fn geometry(&self, ctx: &ToolContext<'_>, box_: (f64, f64, f64, f64)) -> Option<VectorShape> {
		let (_, _, w, h) = box_;
		if w < 0.5 || h < 0.5 {
			return None;
		}
		Some(match self.kind {
			Kind::Rect => VectorShape::Rect { w, h, radii: [0.0; 4] },
			Kind::Rounded => {
				let radius = number(ctx, &self.tool, "Radius").unwrap_or(10.0).max(0.0);
				VectorShape::Rect { w, h, radii: [radius; 4] }
			}
			Kind::Ellipse => VectorShape::Ellipse { w, h },
			Kind::Polygon => {
				let sides = number(ctx, &self.tool, "Sides").unwrap_or(5.0).clamp(3.0, 100.0) as u32;
				let star = ctx.settings.string(&self.tool, "Kind").is_some_and(|kind| kind.eq_ignore_ascii_case("star"));
				VectorShape::Polygon {
					sides,
					star_inset: if star { 0.5 } else { 0.0 },
				}
			}
			// The Line tool is handled by [`Shape::pending`], which keeps the
			// drawn direction; this is only its fallback bounding box.
			Kind::Line => VectorShape::Line { length: w, width: h },
		})
	}

	/// The local → document matrix that places the outline in `box_`.
	fn transform(&self, box_: (f64, f64, f64, f64)) -> [f64; 6] {
		let (x, y, w, h) = box_;
		match self.kind {
			// A polygon's local box is its 2 × 2 circumscribed square.
			Kind::Polygon => [w / 2.0, 0.0, 0.0, h / 2.0, x, y],
			_ => [1.0, 0.0, 0.0, 1.0, x, y],
		}
	}

	/// The Line tool's geometry and placement: the segment from the press
	/// point at the angle drawn, so a line goes the way the pointer went
	/// (Photoshop's rule; Shift snaps the angle to 45° steps). The other kinds
	/// are their bounding box.
	fn pending(&self, ctx: &ToolContext<'_>, drag: &Drag) -> Option<(VectorShape, [f64; 6])> {
		if self.kind != Kind::Line {
			let box_ = drag.box_of();
			return Some((self.geometry(ctx, box_)?, self.transform(box_)));
		}
		let (dx, dy) = (drag.to.0 - drag.from.0, drag.to.1 - drag.from.1);
		let mut angle = dy.atan2(dx);
		if drag.shift {
			let step = std::f64::consts::FRAC_PI_4;
			angle = (angle / step).round() * step;
		}
		let length = dx.hypot(dy);
		if length < 0.5 {
			return None;
		}
		let weight = number(ctx, &self.tool, "Weight").unwrap_or(3.0).max(0.0);
		let (sin, cos) = angle.sin_cos();
		// Rotate about the press point, so the segment starts where it was drawn.
		let at = [cos, sin, -sin, cos, drag.from.0, drag.from.1];
		Some((VectorShape::Line { length, width: weight }, at))
	}

	/// The stroke the option bar asks for, if any.
	fn stroke(&self, ctx: &ToolContext<'_>) -> Option<StrokeStyle> {
		let width = number(ctx, &self.tool, "Stroke Width").unwrap_or(0.0);
		if width <= 0.0 {
			return None;
		}
		Some(StrokeStyle {
			width,
			align: StrokeAlign::Center,
			paint: Paint::Solid { rgba: ctx.settings.bg },
			dash: None,
		})
	}

	/// Recompute the overlay outline from the drag in progress.
	fn refresh_preview(&mut self, ctx: &ToolContext<'_>) {
		self.preview.clear();
		let Some(drag) = self.drag else { return };
		let Some((shape, transform)) = self.pending(ctx, &drag) else { return };
		self.preview = place(&shape.outline(), transform);
	}
}

/// The local outline through a `[a, b, c, d, e, f]` matrix.
pub(crate) fn place(elements: &[PathEl], transform: [f64; 6]) -> Vec<(f64, f64)> {
	let (a, b, c, d, e, f) = (transform[0], transform[1], transform[2], transform[3], transform[4], transform[5]);
	let at = |p: [f64; 2]| (a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + f);
	let mut points = Vec::new();
	for element in elements {
		match *element {
			PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(at(p)),
			// Curves are previewed by their control polygon: the overlay is a
			// hairline guide, not the rendered shape.
			PathEl::QuadTo(control, p) => {
				points.push(at(control));
				points.push(at(p));
			}
			PathEl::CubicTo(c1, c2, p) => {
				points.push(at(c1));
				points.push(at(c2));
				points.push(at(p));
			}
			PathEl::Close => {}
		}
	}
	points
}

impl Tool for Shape {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				self.drag = Some(Drag {
					from: (event.x, event.y),
					to: (event.x, event.y),
					from_centre: event.modifiers.alt,
					shift: event.modifiers.shift,
				});
				self.refresh_preview(ctx);
				ToolResult {
					cursor: Some(CursorShape::Crosshair),
					..Default::default()
				}
			}
			PointerKind::Move => {
				let Some(drag) = self.drag else {
					return ToolResult {
						cursor: Some(CursorShape::Crosshair),
						..Default::default()
					};
				};
				let drag = Drag {
					to: (event.x, event.y),
					shift: event.modifiers.shift,
					..drag
				};
				self.drag = Some(drag);
				self.refresh_preview(ctx);
				if self.kind == Kind::Line {
					let length = (drag.to.0 - drag.from.0).hypot(drag.to.1 - drag.from.1);
					return ToolResult {
						redraw: true,
						status: Some(format!("Length: {} px", length.round() as i64)),
						..Default::default()
					};
				}
				let (_, _, w, h) = drag.box_of();
				ToolResult {
					redraw: true,
					status: Some(format!("W: {} px   H: {} px", w.round() as i64, h.round() as i64)),
					..Default::default()
				}
			}
			PointerKind::Up if self.drag.is_some() && event.buttons == 0 => {
				let Some(drag) = self.drag.take() else {
					return ToolResult::default();
				};
				self.preview.clear();
				let slop = CLICK_SLOP / ctx.view.zoom.max(f64::MIN_POSITIVE);
				// A click, not a drag: Photoshop opens the shape's dialog here.
				let box_ = drag.box_of();
				let span = match self.kind {
					Kind::Line => (drag.to.0 - drag.from.0).hypot(drag.to.1 - drag.from.1),
					_ => box_.2.min(box_.3),
				};
				if span < slop {
					return ToolResult {
						redraw: true,
						..Default::default()
					};
				}
				let Some((shape, transform)) = self.pending(ctx, &drag) else {
					return ToolResult {
						redraw: true,
						..Default::default()
					};
				};
				let layer = NewLayer::Shape {
					shape,
					fill: Some(Paint::Solid { rgba: ctx.settings.fg }),
					stroke: self.stroke(ctx),
					transform,
				};
				ToolResult {
					command: Some(Command::AddLayer { layer, name: None }),
					cursor: Some(CursorShape::Crosshair),
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		if self.preview.len() < 2 {
			return None;
		}
		Some(Overlay {
			items: vec![OverlayItem::Polyline {
				points: self.preview.clone(),
				closed: true,
				style: OverlayStyle::Ants,
			}],
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}
