//! The Lasso (`L`) and Polygonal Lasso tools (M5-T04).
//!
//! The lasso samples the pointer as it is dragged (`docs/tasks/M5.md`, M5-T04:
//! a point every 0.5 *screen* pixels, so the path is as detailed at fit as at
//! 100 %) and closes on release. The polygonal lasso adds a point per click;
//! Enter, a double-click or a click on the first point closes the path,
//! Backspace drops the last point and Escape cancels it.
//!
//! While a path is open it is drawn as a marching-ants rubber band (M5-T02),
//! including the segment from the last point to the pointer.

use fx_core::{Command, SelectMode, SelectionShape};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult, mode_at_press, selection_mode, selection_shape_options};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// Freehand samples closer together than this many *screen* pixels are
/// dropped: without it a slow drag would store thousands of points.
const SAMPLE_STEP: f64 = 0.5;

/// A click within this many *screen* pixels of the first point closes the
/// polygon: the target is a mouse, not a pixel.
const CLOSE_RADIUS: f64 = 5.0;

/// Two presses within this long and this close are a double-click, which
/// closes the polygon (`DocPointer` carries no click count).
const DOUBLE_CLICK_US: u64 = 400_000;

/// Which lasso a tool id is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	/// Points follow the pointer while the button is down.
	Freehand,
	/// One point per click, closed on Enter, a double-click or the first point.
	Polygonal,
}

/// A lasso tool: one instance per UI tool id.
pub struct Lasso {
	id: &'static str,
	kind: Kind,
	/// The path so far, in document pixels. Empty = nothing in progress.
	points: Vec<(f64, f64)>,
	/// Where the pointer is, for the rubber band's live segment.
	hover: (f64, f64),
	/// Whether the button is down (freehand only).
	dragging: bool,
	/// The selection mode the first press chose.
	mode: SelectMode,
	/// The last press: `(time_us, position)`, for the double-click test.
	last_press: Option<(u64, (f64, f64))>,
}

impl Lasso {
	/// The tool for UI id `id`.
	pub fn new(id: &'static str, kind: Kind) -> Self {
		Self {
			id,
			kind,
			points: Vec::new(),
			hover: (0.0, 0.0),
			dragging: false,
			mode: SelectMode::Replace,
			last_press: None,
		}
	}

	/// A closed path of at least three points becomes one `Select` command.
	fn close(&mut self, ctx: &ToolContext<'_>) -> ToolResult {
		let points = std::mem::take(&mut self.points);
		self.dragging = false;
		self.last_press = None;
		if points.len() < 3 {
			return ToolResult {
				redraw: true,
				..Default::default()
			};
		}
		let (feather, anti_alias) = selection_shape_options(ctx, self.id);
		ToolResult {
			command: Some(Command::Select {
				shape: SelectionShape::Polygon { points },
				mode: self.mode,
				feather,
				anti_alias,
			}),
			redraw: true,
			..Default::default()
		}
	}

	/// The freehand lasso's release with fewer than three points: a click. With
	/// Replace it clears the selection, like a marquee click.
	fn click(&mut self) -> ToolResult {
		let replace = self.mode == SelectMode::Replace;
		self.points.clear();
		self.dragging = false;
		self.last_press = None;
		ToolResult {
			command: replace.then_some(Command::Deselect),
			redraw: true,
			..Default::default()
		}
	}

	/// Drop the path without selecting anything (Escape).
	fn cancel(&mut self) -> ToolResult {
		self.points.clear();
		self.dragging = false;
		self.last_press = None;
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}

	/// Whether `(x, y)` is within `CLOSE_RADIUS` screen pixels of `point`.
	fn near(&self, ctx: &ToolContext<'_>, at: (f64, f64), point: (f64, f64)) -> bool {
		let radius = CLOSE_RADIUS / ctx.view.zoom.max(f64::MIN_POSITIVE);
		let (dx, dy) = (at.0 - point.0, at.1 - point.1);
		dx * dx + dy * dy <= radius * radius
	}

	/// Freehand: record the pointer if it moved far enough on screen.
	fn sample(&mut self, ctx: &ToolContext<'_>, at: (f64, f64)) {
		let step = SAMPLE_STEP / ctx.view.zoom.max(f64::MIN_POSITIVE);
		let Some(&last) = self.points.last() else {
			self.points.push(at);
			return;
		};
		let (dx, dy) = (at.0 - last.0, at.1 - last.1);
		if dx * dx + dy * dy >= step * step {
			self.points.push(at);
		}
	}
}

impl Tool for Lasso {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.hover = (event.x, event.y);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				// A double-click closes the polygon (Photoshop); `DocPointer`
				// has no click count, so the two presses are compared here.
				let double = self
					.last_press
					.is_some_and(|(at, p)| event.time_us.saturating_sub(at) <= DOUBLE_CLICK_US && self.near(ctx, p, (event.x, event.y)));
				self.last_press = Some((event.time_us, (event.x, event.y)));
				let at = (event.x, event.y);
				match self.kind {
					Kind::Freehand => {
						if self.points.is_empty() {
							self.mode = mode_at_press(event.modifiers, selection_mode(ctx, self.id));
							self.points.push(at);
							self.dragging = true;
						}
						ToolResult {
							redraw: true,
							..Default::default()
						}
					}
					Kind::Polygonal => {
						if self.points.is_empty() {
							self.mode = mode_at_press(event.modifiers, selection_mode(ctx, self.id));
							self.points.push(at);
							return ToolResult {
								cursor: Some(CursorShape::Crosshair),
								redraw: true,
								..Default::default()
							};
						}
						// A click on the first point (or a double-click) closes the path.
						let first = self.points[0];
						if double || self.near(ctx, at, first) {
							return if self.points.len() >= 3 { self.close(ctx) } else { self.cancel() };
						}
						self.points.push(at);
						ToolResult {
							redraw: true,
							..Default::default()
						}
					}
				}
			}
			PointerKind::Move => {
				let at = (event.x, event.y);
				match self.kind {
					Kind::Freehand if self.dragging => {
						self.sample(ctx, at);
						ToolResult {
							redraw: true,
							..Default::default()
						}
					}
					// The polygonal lasso's rubber band follows the pointer,
					// so a hover redraws it too.
					Kind::Polygonal if !self.points.is_empty() => ToolResult {
						redraw: true,
						..Default::default()
					},
					_ => ToolResult::default(),
				}
			}
			PointerKind::Up if self.kind == Kind::Freehand && self.dragging => {
				if self.points.len() >= 3 {
					self.close(ctx)
				} else {
					self.click()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		let idle = ToolResult::default();
		match key {
			"Escape" if !self.points.is_empty() => self.cancel(),
			"Enter" if self.kind == Kind::Polygonal && !self.points.is_empty() => {
				if self.points.len() >= 3 {
					self.close(ctx)
				} else {
					self.cancel()
				}
			}
			// Backspace drops the last point; dropping the first cancels.
			"Backspace" | "Delete" if self.kind == Kind::Polygonal && !self.points.is_empty() => {
				self.points.pop();
				if self.points.is_empty() {
					self.cancel()
				} else {
					ToolResult { redraw: true, ..idle }
				}
			}
			_ => idle,
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		if self.points.is_empty() {
			return None;
		}
		let mut points = self.points.clone();
		// The live segment from the last click to the pointer: the polygonal
		// lasso's rubber band.
		if self.kind == Kind::Polygonal && points.last() != Some(&self.hover) {
			points.push(self.hover);
		}
		Some(Overlay {
			items: vec![OverlayItem::Polyline {
				points,
				// A freehand path being dragged shows its closing edge, the way
				// Photoshop's lasso reads while the button is down. The
				// polygonal path stays open: it is still being clicked out.
				closed: self.dragging,
				style: OverlayStyle::Ants,
			}],
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tools::testing::{Fixture, polyline, selection};

	fn freehand(zoom: f64) -> (Fixture, Lasso) {
		(Fixture::new("lasso", (400, 300), zoom), Lasso::new("lasso", Kind::Freehand))
	}

	fn polygonal(zoom: f64) -> (Fixture, Lasso) {
		(Fixture::new("lasso", (400, 300), zoom), Lasso::new("lasso-poly", Kind::Polygonal))
	}

	/// One click of the polygonal lasso. The answer is the *press*': that is
	/// where a click adds a point or closes the path (the release is quiet).
	fn click(f: &mut Fixture, tool: &mut Lasso, x: f64, y: f64) -> ToolResult {
		let down = f.pointer(tool, PointerKind::Down, x, y, Modifiers::default());
		f.pointer(tool, PointerKind::Up, x, y, Modifiers::default());
		down
	}

	#[test]
	fn a_freehand_drag_samples_the_pointer_and_closes_on_release() {
		let (mut f, mut tool) = freehand(1.0);
		f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, Modifiers::default());
		for (x, y) in [(60.0, 10.0), (60.0, 60.0), (10.0, 60.0)] {
			f.pointer(&mut tool, PointerKind::Move, x, y, Modifiers::default());
		}
		let (points, closed, style) = polyline(tool.overlay().as_ref().expect("a rubber band"));
		assert_eq!(points.len(), 4);
		assert!(closed && style == OverlayStyle::Ants, "the lasso shows its closing edge");
		let (shape, mode, feather, anti_alias) = selection(f.pointer(&mut tool, PointerKind::Up, 10.0, 60.0, Modifiers::default()));
		assert_eq!(
			shape,
			SelectionShape::Polygon {
				points: vec![(10.0, 10.0), (60.0, 10.0), (60.0, 60.0), (10.0, 60.0)]
			}
		);
		assert_eq!((mode, feather, anti_alias), (SelectMode::Replace, 0.0, true));
		assert!(tool.overlay().is_none(), "the selection's ants take over");
	}

	#[test]
	fn samples_closer_than_half_a_screen_pixel_are_dropped() {
		let (mut f, mut tool) = freehand(1.0);
		f.pointer(&mut tool, PointerKind::Down, 0.0, 0.0, Modifiers::default());
		for step in 1..=10 {
			// 0.2 px apart: only every third one lands (0.6, then 1.2…).
			f.pointer(&mut tool, PointerKind::Move, f64::from(step) * 0.2, 0.0, Modifiers::default());
		}
		let (points, ..) = polyline(tool.overlay().as_ref().unwrap());
		assert!(points.len() < 6, "{} points for 2 document pixels of travel", points.len());
		// At 400 % the same travel is 8 screen pixels: every sample lands.
		let (mut f, mut tool) = freehand(4.0);
		f.pointer(&mut tool, PointerKind::Down, 0.0, 0.0, Modifiers::default());
		for step in 1..=10 {
			f.pointer(&mut tool, PointerKind::Move, f64::from(step) * 0.2, 0.0, Modifiers::default());
		}
		let (points, ..) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points.len(), 11, "every sample is a screen pixel apart");
	}

	#[test]
	fn a_click_with_the_lasso_clears_the_selection() {
		let (mut f, mut tool) = freehand(1.0);
		let up = f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, Modifiers::default());
		assert!(up.command.is_none(), "nothing happens before the release");
		assert_eq!(
			f.pointer(&mut tool, PointerKind::Up, 10.0, 10.0, Modifiers::default()).command,
			Some(Command::Deselect)
		);
		// In Add mode a click leaves the selection alone.
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, shift);
		assert!(f.pointer(&mut tool, PointerKind::Up, 10.0, 10.0, shift).command.is_none());
	}

	#[test]
	fn the_polygonal_lasso_takes_a_point_per_click_and_closes_on_enter() {
		let (mut f, mut tool) = polygonal(1.0);
		assert!(click(&mut f, &mut tool, 10.0, 10.0).command.is_none());
		assert!(click(&mut f, &mut tool, 200.0, 10.0).command.is_none());
		// The rubber band runs from the last point to the pointer.
		f.pointer(&mut tool, PointerKind::Move, 200.0, 200.0, Modifiers::default());
		let (points, closed, style) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points, [(10.0, 10.0), (200.0, 10.0), (200.0, 200.0)]);
		assert!(!closed && style == OverlayStyle::Ants, "an open path is still being drawn");
		// A third click, then Enter closes it.
		click(&mut f, &mut tool, 10.0, 200.0);
		let (shape, mode, ..) = selection(f.key(&mut tool, "Enter"));
		assert_eq!(
			shape,
			SelectionShape::Polygon {
				points: vec![(10.0, 10.0), (200.0, 10.0), (10.0, 200.0)]
			}
		);
		assert_eq!(mode, SelectMode::Replace);
		assert!(tool.overlay().is_none());
	}

	#[test]
	fn a_click_on_the_first_point_or_a_double_click_closes_the_polygon() {
		let (mut f, mut tool) = polygonal(1.0);
		for (x, y) in [(10.0, 10.0), (200.0, 10.0), (200.0, 200.0)] {
			click(&mut f, &mut tool, x, y);
		}
		// Back on the first point: the target is 5 screen pixels wide.
		let (shape, ..) = selection(click(&mut f, &mut tool, 12.0, 12.0));
		assert!(matches!(shape, SelectionShape::Polygon { points } if points.len() == 3));
		// Two presses in the same place, within 400 ms, are a double-click.
		let (mut f, mut tool) = polygonal(1.0);
		click(&mut f, &mut tool, 10.0, 10.0);
		click(&mut f, &mut tool, 200.0, 10.0);
		f.pointer_now(&mut tool, PointerKind::Down, 200.0, 200.0, Modifiers::default());
		f.pointer_now(&mut tool, PointerKind::Up, 200.0, 200.0, Modifiers::default());
		f.pointer_now(&mut tool, PointerKind::Down, 30.0, 30.0, Modifiers::default());
		let (shape, ..) = selection(f.pointer_now(&mut tool, PointerKind::Down, 30.0, 30.0, Modifiers::default()));
		assert!(
			matches!(shape, SelectionShape::Polygon { points } if points.len() == 4),
			"the whole path, closed by the second press in the same place"
		);
	}

	#[test]
	fn backspace_drops_the_last_point_and_escape_cancels() {
		let (mut f, mut tool) = polygonal(1.0);
		for (x, y) in [(10.0, 10.0), (200.0, 10.0), (200.0, 200.0)] {
			click(&mut f, &mut tool, x, y);
		}
		f.key(&mut tool, "Backspace");
		f.pointer(&mut tool, PointerKind::Move, 100.0, 100.0, Modifiers::default());
		let (points, ..) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points, [(10.0, 10.0), (200.0, 10.0), (100.0, 100.0)], "the third point is gone");
		// Enter with two points left cancels instead of making a sliver.
		let result = f.key(&mut tool, "Enter");
		assert!(result.command.is_none() && tool.overlay().is_none());
		// Escape cancels a path in progress, whatever its length.
		for (x, y) in [(10.0, 10.0), (200.0, 10.0), (200.0, 200.0)] {
			click(&mut f, &mut tool, x, y);
		}
		assert!(f.key(&mut tool, "Escape").redraw);
		assert!(tool.overlay().is_none());
		// A key the tool does not use changes nothing.
		assert!(!f.key(&mut tool, "ArrowLeft").redraw);
	}

	#[test]
	fn the_freehand_lasso_ignores_the_polygon_keys() {
		let (mut f, mut tool) = freehand(1.0);
		f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, Modifiers::default());
		f.pointer(&mut tool, PointerKind::Move, 60.0, 60.0, Modifiers::default());
		assert!(f.key(&mut tool, "Backspace").command.is_none());
		let (points, ..) = polyline(tool.overlay().as_ref().unwrap());
		assert_eq!(points.len(), 2, "the freehand path is untouched");
		// Escape still cancels it.
		f.key(&mut tool, "Escape");
		assert!(tool.overlay().is_none());
		assert!(f.pointer(&mut tool, PointerKind::Up, 60.0, 60.0, Modifiers::default()).command.is_none());
	}

	#[test]
	fn the_option_bar_supplies_the_mode_the_feather_and_the_anti_aliasing() {
		let (mut f, mut tool) = freehand(1.0);
		f.options("lasso", serde_json::json!({"Mode": 1, "Feather": 2.5, "Anti-alias": false}));
		f.pointer(&mut tool, PointerKind::Down, 10.0, 10.0, Modifiers::default());
		for (x, y) in [(60.0, 10.0), (60.0, 60.0)] {
			f.pointer(&mut tool, PointerKind::Move, x, y, Modifiers::default());
		}
		let (_, mode, feather, anti_alias) = selection(f.pointer(&mut tool, PointerKind::Up, 60.0, 60.0, Modifiers::default()));
		assert_eq!((mode, feather, anti_alias), (SelectMode::Add, 2.5, false));
	}
}
