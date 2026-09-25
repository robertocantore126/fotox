//! The marquee tools (`M`): Rectangular, Elliptical, Single Row and Single
//! Column (M5-T04).
//!
//! A drag is previewed as a marching-ants rubber band (M5-T02) and becomes one
//! [`Command::Select`] on release, so the whole drag is a single History step.
//! Modifiers follow Photoshop: **Shift** at press adds to the selection,
//! **Alt** subtracts, Shift+Alt intersects (the option bar's Mode is the
//! default); while dragging Shift constrains to a square/circle and Alt draws
//! from the centre. Holding Space moves the marquee being drawn.

use fx_core::{Command, SelectMode, SelectionShape};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, OutlineDrag, Tool, ToolContext, ToolResult, mode_at_press, nudge_outline, selection_mode, selection_shape_options};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// A drag shorter than this many *screen* pixels is a click, not a marquee:
/// with Replace it clears the selection, like Photoshop (M5-T04).
const CLICK_SLOP: f64 = 3.0;

/// Points of the ellipse preview. The selection itself is rasterised by the
/// engine's anti-aliased rasteriser, not from these.
const ELLIPSE_POINTS: usize = 64;

/// Which marquee a tool id is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
	Rect,
	Ellipse,
	/// One whole row: a click, no drag.
	Row,
	/// One whole column: a click, no drag.
	Column,
}

/// The option bar's Style: `Normal`, `Fixed Ratio` (w : h from the Width and
/// Height fields) or `Fixed Size` (document pixels from the same fields).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Style {
	Normal,
	/// Width ÷ Height, e.g. 4 : 3.
	Ratio(f64),
	/// Width × Height in document pixels.
	Size(f64, f64),
}

impl Style {
	fn of(ctx: &ToolContext<'_>, id: &str) -> Self {
		let width = ctx.settings.number(id, "Width").filter(|v| *v > 0.0);
		let height = ctx.settings.number(id, "Height").filter(|v| *v > 0.0);
		match ctx.settings.string(id, "Style").as_deref() {
			Some("Fixed Ratio") => match (width, height) {
				(Some(w), Some(h)) => Style::Ratio(w / h),
				_ => Style::Normal,
			},
			Some("Fixed Size") => match (width, height) {
				(Some(w), Some(h)) => Style::Size(w, h),
				_ => Style::Normal,
			},
			_ => Style::Normal,
		}
	}
}

/// A drag in progress. It carries everything the overlay needs, because
/// [`Tool::overlay`] gets no context: the style and the modifiers the last
/// pointer event arrived with, so the preview and the final shape agree.
#[derive(Clone, Copy, Debug)]
struct Drag {
	/// Where the button went down (document pixels).
	anchor: (f64, f64),
	/// The pointer now: the opposite corner, or the centre with Alt.
	current: (f64, f64),
	/// The selection mode the press chose (modifiers or the option bar).
	mode: SelectMode,
	style: Style,
	modifiers: Modifiers,
	/// Where Space took hold of the marquee, for the move-while-drawing nudge.
	grab: Option<(f64, f64)>,
	/// Whether the pointer ever left the click slop: below it the release is a
	/// click, not a marquee.
	dragged: bool,
}

/// A marquee tool: one instance per UI tool id, with the shape it draws.
pub struct Marquee {
	id: &'static str,
	shape: Shape,
	drag: Option<Drag>,
	/// Dragging the selection outline instead of drawing a marquee.
	moving: Option<OutlineDrag>,
}

impl Marquee {
	/// The tool for UI id `id`, drawing `shape`.
	pub fn new(id: &'static str, shape: Shape) -> Self {
		Self {
			id,
			shape,
			drag: None,
			moving: None,
		}
	}

	/// The shape a drag defines: Style, then Shift (square/circle), then Alt
	/// (drawn from the centre). Row and column never get here: they select on
	/// the press, without a drag.
	fn shape_of(&self, drag: &Drag) -> SelectionShape {
		let (x, y, w, h) = self.rectangle(drag);
		match self.shape {
			Shape::Ellipse => SelectionShape::Ellipse { x, y, w, h },
			Shape::Rect | Shape::Row | Shape::Column => SelectionShape::Rect { x, y, w, h },
		}
	}

	/// The rectangle a drag defines, in document pixels.
	fn rectangle(&self, drag: &Drag) -> (f64, f64, f64, f64) {
		let (ax, ay) = drag.anchor;
		let (mut w, mut h) = (drag.current.0 - ax, drag.current.1 - ay);
		match drag.style {
			Style::Normal => {
				if drag.modifiers.shift {
					// A square (or a circle): the longer axis wins.
					let side = w.abs().max(h.abs());
					w = side * sign(w);
					h = side * sign(h);
				}
			}
			Style::Ratio(ratio) => {
				// Keep w : h, driven by the axis the user pushed further.
				if w.abs() > h.abs() * ratio {
					h = (w / ratio) * sign(h);
				} else {
					w = (h * ratio) * sign(w);
				}
			}
			Style::Size(width, height) => {
				w = width * sign(w);
				h = height * sign(h);
			}
		}
		if drag.modifiers.alt {
			// The press point is the centre.
			return (ax - w, ay - h, w * 2.0, h * 2.0);
		}
		(ax, ay, w, h)
	}

	/// The outline of the dragged shape, for the rubber band.
	fn outline(&self, drag: &Drag) -> Vec<(f64, f64)> {
		let (x, y, w, h) = self.rectangle(drag);
		match self.shape {
			Shape::Ellipse => {
				let (cx, cy) = (x + w / 2.0, y + h / 2.0);
				let (rx, ry) = (w / 2.0, h / 2.0);
				(0..ELLIPSE_POINTS)
					.map(|i| {
						let angle = std::f64::consts::TAU * (i as f64) / (ELLIPSE_POINTS as f64);
						(cx + rx * angle.cos(), cy + ry * angle.sin())
					})
					.collect()
			}
			_ => vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
		}
	}

	/// The command that selects `shape`, with the option bar's feather and
	/// anti-alias.
	fn command(&self, ctx: &ToolContext<'_>, shape: SelectionShape, mode: SelectMode) -> Command {
		let (feather, anti_alias) = selection_shape_options(ctx, self.id);
		Command::Select {
			shape,
			mode,
			feather,
			anti_alias,
		}
	}

	/// Move the marquee with Space, or follow the pointer.
	fn track(&self, drag: &mut Drag, ctx: &ToolContext<'_>, event: &DocPointer) {
		drag.modifiers = event.modifiers;
		if event.modifiers.space {
			// Space moves the marquee being drawn: nudge the whole rectangle
			// by however far the pointer travelled.
			let grab = drag.grab.get_or_insert((event.x, event.y));
			let (dx, dy) = (event.x - grab.0, event.y - grab.1);
			drag.anchor = (drag.anchor.0 + dx, drag.anchor.1 + dy);
			drag.current = (drag.current.0 + dx, drag.current.1 + dy);
			drag.grab = Some((event.x, event.y));
			return;
		}
		drag.grab = None;
		drag.current = (event.x, event.y);
		// The click slop is a screen distance, so it does not change with zoom.
		let slop = CLICK_SLOP / ctx.view.zoom.max(f64::MIN_POSITIVE);
		if (event.x - drag.anchor.0).abs() > slop || (event.y - drag.anchor.1).abs() > slop {
			drag.dragged = true;
		}
	}
}

impl Tool for Marquee {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let style = Style::of(ctx, self.id);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let mode = mode_at_press(event.modifiers, selection_mode(ctx, self.id));
				// New mode, pressed inside the selection: move the outline.
				if mode == SelectMode::Replace
					&& let Some(moving) = OutlineDrag::begin(ctx, event)
				{
					self.moving = Some(moving);
					return ToolResult {
						cursor: Some(CursorShape::Move),
						..Default::default()
					};
				}
				match self.shape {
					// A single row or column: a click, no drag (Photoshop).
					Shape::Row | Shape::Column => {
						let shape = if self.shape == Shape::Row {
							SelectionShape::RowPixel { y: event.y }
						} else {
							SelectionShape::ColumnPixel { x: event.x }
						};
						ToolResult {
							command: Some(self.command(ctx, shape, mode)),
							..Default::default()
						}
					}
					_ => {
						self.drag = Some(Drag {
							anchor: (event.x, event.y),
							current: (event.x, event.y),
							mode,
							style,
							modifiers: event.modifiers,
							grab: None,
							dragged: false,
						});
						ToolResult {
							cursor: Some(CursorShape::Crosshair),
							..Default::default()
						}
					}
				}
			}
			PointerKind::Move | PointerKind::Down if self.moving.is_some() => {
				if let Some(moving) = &mut self.moving {
					moving.track(event, ctx.view.zoom);
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if self.moving.is_some() => {
				let moving = self.moving.take().expect("checked by the guard");
				// A click inside the selection (no drag) deselects, like any
				// marquee click in New mode.
				let command = if moving.dragged() { moving.command() } else { Some(Command::Deselect) };
				ToolResult {
					command,
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move | PointerKind::Down => {
				let Some(mut drag) = self.drag else {
					return ToolResult::default();
				};
				self.track(&mut drag, ctx, event);
				self.drag = Some(drag);
				let (_, _, w, h) = self.rectangle(&drag);
				ToolResult {
					redraw: true,
					status: Some(format!("W: {} px  H: {} px", w.abs().round(), h.abs().round())),
					..Default::default()
				}
			}
			PointerKind::Up => {
				let Some(drag) = self.drag.take() else {
					return ToolResult::default();
				};
				// A click with a fixed size still makes the selection: the
				// marquee is the size the option bar asked for (Photoshop).
				if !drag.dragged && !matches!(drag.style, Style::Size(..)) {
					// With Replace a click clears the selection; with Add,
					// Subtract or Intersect Photoshop leaves it alone.
					let command = (drag.mode == SelectMode::Replace).then_some(Command::Deselect);
					return ToolResult {
						command,
						redraw: true,
						..Default::default()
					};
				}
				let shape = self.shape_of(&drag);
				ToolResult {
					command: Some(self.command(ctx, shape, drag.mode)),
					redraw: true,
					..Default::default()
				}
			}
			// A hover, or a release of a button this tool did not start.
			_ => ToolResult::default(),
		}
	}

	/// Escape drops the marquee being drawn (Escape and the pointer release
	/// are the two ways out of a drag).
	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if self.drag.is_none() && self.moving.is_none() {
			return ToolResult {
				command: nudge_outline(ctx, key),
				..Default::default()
			};
		}
		if key != "Escape" {
			return ToolResult::default();
		}
		self.drag = None;
		self.moving = None;
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let drag = self.drag?;
		Some(Overlay {
			items: vec![OverlayItem::Polyline {
				points: self.outline(&drag),
				closed: true,
				style: OverlayStyle::Ants,
			}],
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}

	fn selection_nudge(&self) -> Option<(i32, i32)> {
		self.moving.map(|m| m.delta())
	}

	fn cancel(&mut self) -> bool {
		let busy = self.drag.is_some() || self.moving.is_some();
		self.drag = None;
		self.moving = None;
		busy
	}
}

/// `-1.0` for a negative number, `1.0` otherwise (a zero-sized drag keeps the
/// direction the user drew in).
fn sign(value: f64) -> f64 {
	if value < 0.0 { -1.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tools::testing::{Fixture, polyline, selection as select};

	fn fixture(zoom: f64) -> (Fixture, Marquee) {
		(Fixture::new("marquee", (400, 300), zoom), Marquee::new("marquee", Shape::Rect))
	}

	fn shift() -> Modifiers {
		Modifiers {
			shift: true,
			..Default::default()
		}
	}

	fn alt() -> Modifiers {
		Modifiers {
			alt: true,
			..Default::default()
		}
	}

	fn space() -> Modifiers {
		Modifiers {
			space: true,
			..Default::default()
		}
	}

	#[test]
	fn escape_drops_the_marquee_being_drawn() {
		let (mut f, mut tool) = fixture(1.0);
		f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, Modifiers::default());
		f.pointer(&mut tool, PointerKind::Move, 60.0, 60.0, Modifiers::default());
		assert!(f.key(&mut tool, "Escape").redraw);
		assert!(tool.overlay().is_none(), "the rubber band is gone");
		assert!(
			f.pointer(&mut tool, PointerKind::Up, 60.0, 60.0, Modifiers::default()).command.is_none(),
			"the release selects nothing"
		);
		// Escape with nothing in progress changes nothing.
		assert!(!f.key(&mut tool, "Escape").redraw);
		assert!(!f.key(&mut tool, "Enter").redraw, "Enter is the lasso's key");
	}

	#[test]
	fn a_dragged_rectangle_previews_the_ants_and_selects_on_release() {
		let (mut f, mut tool) = fixture(1.0);
		let down = f.pointer(&mut tool, PointerKind::Down, 10.0, 20.0, Modifiers::default());
		assert!(down.command.is_none(), "nothing is selected before the release");
		assert_eq!(down.cursor, Some(CursorShape::Crosshair));
		let preview = f.pointer(&mut tool, PointerKind::Move, 60.0, 70.0, Modifiers::default());
		assert!(preview.redraw, "the rubber band needs a redraw");
		let (points, closed, style) = polyline(tool.overlay().as_ref().expect("a rubber band"));
		assert_eq!(points, [(10.0, 20.0), (60.0, 20.0), (60.0, 70.0), (10.0, 70.0)]);
		assert!(closed && style == OverlayStyle::Ants);
		let up = f.pointer(&mut tool, PointerKind::Up, 60.0, 70.0, Modifiers::default());
		assert_eq!(
			select(up),
			(
				SelectionShape::Rect {
					x: 10.0,
					y: 20.0,
					w: 50.0,
					h: 50.0
				},
				SelectMode::Replace,
				0.0,
				true
			)
		);
		assert!(tool.overlay().is_none(), "the selection's own ants take over");
	}

	#[test]
	fn shift_adds_alt_subtracts_shift_alt_intersects_and_the_bar_sets_the_mode() {
		let (mut f, mut tool) = fixture(1.0);
		let both = Modifiers {
			shift: true,
			alt: true,
			..Default::default()
		};
		let drag = [(10.0, 10.0), (60.0, 30.0)];
		assert_eq!(select(f.drag(&mut tool, &drag, shift())).1, SelectMode::Add);
		assert_eq!(select(f.drag(&mut tool, &drag, alt())).1, SelectMode::Subtract);
		assert_eq!(select(f.drag(&mut tool, &drag, both)).1, SelectMode::Intersect);
		assert_eq!(select(f.drag(&mut tool, &drag, Modifiers::default())).1, SelectMode::Replace);
		// The option bar's Mode control is the default (the button group sends
		// its index: 1 Add, 2 Subtract, 3 Intersect).
		f.options("marquee", serde_json::json!({"Mode": 2}));
		assert_eq!(select(f.drag(&mut tool, &drag, Modifiers::default())).1, SelectMode::Subtract);
	}

	#[test]
	fn the_option_bar_supplies_the_feather_and_the_anti_aliasing() {
		let (mut f, mut tool) = fixture(1.0);
		f.options("marquee", serde_json::json!({"Feather": 4.5, "Anti-alias": false}));
		let (_, _, feather, anti_alias) = select(f.drag(&mut tool, &[(0.0, 0.0), (100.0, 100.0)], Modifiers::default()));
		assert_eq!((feather, anti_alias), (4.5, false));
		// A negative feather is nonsense: it reads as no feather at all.
		f.options("marquee", serde_json::json!({"Feather": -3}));
		assert_eq!(select(f.drag(&mut tool, &[(0.0, 0.0), (100.0, 100.0)], Modifiers::default())).2, 0.0);
	}

	#[test]
	fn shift_while_dragging_keeps_the_marquee_square() {
		let (mut f, mut tool) = fixture(1.0);
		let (shape, mode, ..) = select(f.drag(&mut tool, &[(10.0, 10.0), (60.0, 30.0)], shift()));
		assert_eq!(
			shape,
			SelectionShape::Rect {
				x: 10.0,
				y: 10.0,
				w: 50.0,
				h: 50.0
			},
			"the longer axis wins"
		);
		assert_eq!(mode, SelectMode::Add, "Shift at the press still means Add");
	}

	#[test]
	fn alt_draws_the_marquee_from_the_press_point_as_its_centre() {
		let (mut f, mut tool) = fixture(1.0);
		let (shape, ..) = select(f.drag(&mut tool, &[(100.0, 100.0), (120.0, 110.0)], alt()));
		assert_eq!(
			shape,
			SelectionShape::Rect {
				x: 80.0,
				y: 90.0,
				w: 40.0,
				h: 20.0
			}
		);
	}

	#[test]
	fn space_moves_the_marquee_being_drawn() {
		let (mut f, mut tool) = fixture(1.0);
		f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, Modifiers::default());
		f.pointer(&mut tool, PointerKind::Move, 60.0, 40.0, Modifiers::default());
		// Space takes hold of the marquee where the pointer is (70, 50): the
		// first such event grabs it, and it then follows the pointer's travel.
		f.pointer(&mut tool, PointerKind::Move, 70.0, 50.0, space());
		let (points, ..) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points, [(10.0, 10.0), (60.0, 10.0), (60.0, 40.0), (10.0, 40.0)], "grabbing moves nothing");
		f.pointer(&mut tool, PointerKind::Move, 75.0, 55.0, space());
		let (points, ..) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points, [(15.0, 15.0), (65.0, 15.0), (65.0, 45.0), (15.0, 45.0)], "the marquee travelled (5, 5)");
		let (shape, ..) = select(f.pointer(&mut tool, PointerKind::Up, 75.0, 55.0, Modifiers::default()));
		assert_eq!(
			shape,
			SelectionShape::Rect {
				x: 15.0,
				y: 15.0,
				w: 50.0,
				h: 30.0
			}
		);
	}

	#[test]
	fn a_click_deselects_in_replace_mode_and_is_ignored_otherwise() {
		let (mut f, mut tool) = fixture(1.0);
		let click = |f: &mut Fixture, tool: &mut Marquee, modifiers| {
			f.pointer(tool, PointerKind::Down, 10.0, 10.0, modifiers);
			f.pointer(tool, PointerKind::Up, 10.0, 10.0, modifiers)
		};
		assert_eq!(click(&mut f, &mut tool, Modifiers::default()).command, Some(Command::Deselect));
		// Add, Subtract and Intersect leave the selection alone on a click.
		for modifiers in [shift(), alt()] {
			assert!(click(&mut f, &mut tool, modifiers).command.is_none());
		}
		// A short drag is still a click: 2 px at zoom 1 is inside the slop.
		let up = f.drag(&mut tool, &[(10.0, 10.0), (12.0, 10.0)], Modifiers::default());
		assert_eq!(up.command, Some(Command::Deselect));
	}

	#[test]
	fn the_click_slop_is_a_screen_distance_so_it_does_not_change_with_zoom() {
		// At 25 % two document pixels are half a screen pixel: still a click.
		let (mut f, mut tool) = fixture(0.25);
		assert_eq!(
			f.drag(&mut tool, &[(10.0, 10.0), (12.0, 10.0)], Modifiers::default()).command,
			Some(Command::Deselect)
		);
		// At 400 % the same drag is 8 screen pixels: a marquee.
		let (mut f, mut tool) = fixture(4.0);
		assert!(matches!(
			f.drag(&mut tool, &[(10.0, 10.0), (12.0, 10.0)], Modifiers::default()).command,
			Some(Command::Select { .. })
		));
	}

	#[test]
	fn the_elliptical_marquee_previews_an_ellipse_and_selects_it() {
		let (mut f, mut tool) = (Fixture::new("marquee", (400, 300), 1.0), Marquee::new("marquee-ellipse", Shape::Ellipse));
		f.pointer(&mut tool, PointerKind::Down, 0.0, 0.0, Modifiers::default());
		f.pointer(&mut tool, PointerKind::Move, 100.0, 50.0, Modifiers::default());
		let (points, closed, style) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points.len(), ELLIPSE_POINTS);
		assert!(closed && style == OverlayStyle::Ants);
		// The first point is the right end of the major axis.
		assert!((points[0].0 - 100.0).abs() < 1e-6 && (points[0].1 - 25.0).abs() < 1e-6, "{:?}", points[0]);
		let (shape, ..) = select(f.pointer(&mut tool, PointerKind::Up, 100.0, 50.0, Modifiers::default()));
		assert_eq!(
			shape,
			SelectionShape::Ellipse {
				x: 0.0,
				y: 0.0,
				w: 100.0,
				h: 50.0
			}
		);
	}

	#[test]
	fn the_single_row_and_column_tools_select_on_the_press() {
		let (mut f, mut row) = (Fixture::new("marquee", (400, 300), 1.0), Marquee::new("marquee-row", Shape::Row));
		let (shape, mode, ..) = select(f.pointer(&mut row, PointerKind::Down, 100.0, 7.6, Modifiers::default()));
		assert_eq!((shape, mode), (SelectionShape::RowPixel { y: 7.6 }, SelectMode::Replace));
		assert!(row.overlay().is_none(), "a row has no rubber band");
		assert!(
			f.pointer(&mut row, PointerKind::Up, 100.0, 7.6, Modifiers::default()).command.is_none(),
			"already selected"
		);
		// Shift adds a second row, like Photoshop.
		assert_eq!(select(f.pointer(&mut row, PointerKind::Down, 100.0, 20.0, shift())).1, SelectMode::Add);
		let (mut f, mut column) = (Fixture::new("marquee", (400, 300), 1.0), Marquee::new("marquee-col", Shape::Column));
		assert_eq!(
			select(f.pointer(&mut column, PointerKind::Down, 3.2, 100.0, Modifiers::default())).0,
			SelectionShape::ColumnPixel { x: 3.2 }
		);
	}

	#[test]
	fn fixed_size_selects_without_a_drag_and_fixed_ratio_keeps_the_ratio() {
		let (mut f, mut tool) = fixture(1.0);
		f.options("marquee", serde_json::json!({"Style": "Fixed Size", "Width": 80, "Height": 60}));
		let (shape, ..) = select(f.drag(&mut tool, &[(100.0, 100.0)], Modifiers::default()));
		assert_eq!(
			shape,
			SelectionShape::Rect {
				x: 100.0,
				y: 100.0,
				w: 80.0,
				h: 60.0
			},
			"a click with a fixed size still makes the selection"
		);
		let (mut f, mut tool) = fixture(1.0);
		f.options("marquee", serde_json::json!({"Style": "Fixed Ratio", "Width": 4, "Height": 3}));
		let (shape, ..) = select(f.drag(&mut tool, &[(0.0, 0.0), (80.0, 10.0)], Modifiers::default()));
		assert_eq!(
			shape,
			SelectionShape::Rect {
				x: 0.0,
				y: 0.0,
				w: 80.0,
				h: 60.0
			},
			"4:3 driven by the wider axis"
		);
	}
}
