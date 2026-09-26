//! The Object Selection tool (W, M13-T04).
//!
//! Rectangle mode: drag a box around the object (a click is a point
//! prompt). Lasso mode: draw roughly around it; its bounding box and its
//! centre go to the decoder. The release asks the engine for an EfficientSAM
//! run (`AiRequest::Object`); the engine reuses the document's embedding when
//! the content has not changed (S39), and the mask comes back as one
//! `SelectBy` step, "Object Selection". Shift adds, Alt subtracts; with a
//! selection and no modifier the option bar's Mode applies.
//!
//! FAST: no Object Finder (the hover highlight); the lasso's centre is its
//! box's centre, even when that falls outside a concave outline.

use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{AiRequest, DocPointer, Tool, ToolContext, ToolResult, mode_at_press, selection_mode};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

const ID: &str = "object-select";

/// A drag shorter than this many screen pixels is a click.
const CLICK_PX: f64 = 3.0;

#[derive(Default)]
pub struct ObjectSelect {
	/// The press and the samples (the lasso) or the two corners (the box).
	points: Vec<(f64, f64)>,
	dragging: bool,
	lasso: bool,
	mode: Option<fx_core::SelectMode>,
}

impl Tool for ObjectSelect {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let at = (event.x, event.y);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				self.lasso = ctx.settings.number(ID, "Shape") == Some(1.0);
				self.mode = Some(mode_at_press(event.modifiers, selection_mode(ctx, ID)));
				self.points = vec![at];
				self.dragging = true;
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move if self.dragging => {
				if self.lasso {
					self.points.push(at);
				} else {
					self.points.truncate(1);
					self.points.push(at);
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if self.dragging => {
				self.dragging = false;
				let points = std::mem::take(&mut self.points);
				let mode = self.mode.take().unwrap_or(fx_core::SelectMode::Replace);
				let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
				for (x, y) in points.iter().copied().chain(std::iter::once(at)) {
					(x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
				}
				let zoom = ctx.view.zoom.max(1e-6);
				let click = (x1 - x0).max(y1 - y0) * zoom < CLICK_PX;
				let request = if click {
					AiRequest::Object {
						boxed: None,
						points: vec![(at, true)],
						mode,
					}
				} else {
					AiRequest::Object {
						boxed: Some([x0, y0, x1, y1]),
						points: if self.lasso {
							vec![(((x0 + x1) / 2.0, (y0 + y1) / 2.0), true)]
						} else {
							Vec::new()
						},
						mode,
					}
				};
				ToolResult {
					ai: Some(request),
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn key(&mut self, _ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if key == "Escape" && !self.points.is_empty() {
			self.points.clear();
			self.dragging = false;
			return ToolResult {
				redraw: true,
				..Default::default()
			};
		}
		ToolResult::default()
	}

	fn overlay(&self) -> Option<Overlay> {
		if self.points.len() < 2 {
			return None;
		}
		let points = if self.lasso {
			self.points.clone()
		} else {
			let ((ax, ay), (bx, by)) = (self.points[0], self.points[1]);
			vec![(ax, ay), (bx, ay), (bx, by), (ax, by)]
		};
		Some(Overlay {
			items: vec![OverlayItem::Polyline {
				points,
				closed: true,
				style: OverlayStyle::Ants,
			}],
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}
