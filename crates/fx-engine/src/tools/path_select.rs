//! The Path Selection tool (`A`) for shape layers (M6-T06).
//!
//! Clicking a shape picks its layer; dragging moves the shape by changing its
//! **transform** — no pixel is ever resampled (D-055), which is why the whole
//! drag is one cheap `SetShape` on release. Direct Selection (moving single
//! anchors) and the Pen tools are out of scope for M6, as the card says.
//!
//! The hit test works in the shape's own local space: the document point is
//! put through the inverse of the layer's matrix and tested against the
//! outline ([`VectorShape::contains`]).

use fx_core::{Command, Document, Layer, LayerKind, LayerRef};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::shape::place;
use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// Screen pixels a press must travel before it moves the shape instead of
/// just selecting it.
const MOVE_SLOP: f64 = 3.0;

/// A shape being moved.
#[derive(Clone, Debug)]
struct Moving {
	layer: fx_core::LayerId,
	/// The pointer at the press, in document pixels.
	from: (f64, f64),
	to: (f64, f64),
	/// The layer's matrix at the press.
	transform: [f64; 6],
	dragged: bool,
	/// The outline as it was at the press, in document coordinates.
	outline: Vec<(f64, f64)>,
}

/// The Path Selection tool.
#[derive(Debug, Default)]
pub struct PathSelect {
	moving: Option<Moving>,
	/// The outline drawn while moving, shifted by the drag.
	preview: Vec<(f64, f64)>,
}

impl PathSelect {
	/// The whole-pixel move so far, in document pixels.
	fn delta(moving: &Moving) -> (f64, f64) {
		if !moving.dragged {
			return (0.0, 0.0);
		}
		(moving.to.0 - moving.from.0, moving.to.1 - moving.from.1)
	}

	/// Refresh the preview outline from the drag in progress.
	fn refresh_preview(&mut self) {
		self.preview.clear();
		let Some(moving) = &self.moving else { return };
		let (dx, dy) = Self::delta(moving);
		self.preview = moving.outline.iter().map(|(x, y)| (x + dx, y + dy)).collect();
	}
}

/// The topmost shape layer under the document point, top → bottom.
fn hit(doc: &Document, x: f64, y: f64) -> Option<fx_core::LayerId> {
	doc.panel_order()
		.into_iter()
		.find(|&id| doc.layer(id).is_some_and(|layer| layer.visible && contains_shape(layer, x, y)))
}

/// Whether a document point is inside a layer's shape, in the shape's own
/// local space.
fn contains_shape(layer: &Layer, x: f64, y: f64) -> bool {
	let LayerKind::Shape { shape, transform, .. } = &layer.kind else {
		return false;
	};
	let Some((lx, ly)) = to_local(*transform, x, y) else {
		return false;
	};
	shape.contains(lx, ly)
}

/// A document point through the inverse of a `[a, b, c, d, e, f]` matrix;
/// `None` for a degenerate (non-invertible) matrix.
fn to_local(transform: [f64; 6], x: f64, y: f64) -> Option<(f64, f64)> {
	let (a, b, c, d, e, f) = (transform[0], transform[1], transform[2], transform[3], transform[4], transform[5]);
	let det = a * d - b * c;
	if det.abs() < f64::EPSILON {
		return None;
	}
	let (px, py) = (x - e, y - f);
	Some(((d * px - c * py) / det, (-b * px + a * py) / det))
}

/// An outline in document coordinates, with the matrix that placed it there.
type Placed = (Vec<(f64, f64)>, [f64; 6]);

/// The outline of a shape layer in document coordinates, when it has one.
fn outline_of(kind: &LayerKind) -> Option<Placed> {
	let LayerKind::Shape { shape, transform, .. } = kind else {
		return None;
	};
	Some((place(&shape.outline(), *transform), *transform))
}

impl Tool for PathSelect {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let Some(id) = hit(ctx.doc, event.x, event.y) else {
					return ToolResult {
						cursor: Some(CursorShape::Default),
						..Default::default()
					};
				};
				let Some(layer) = ctx.doc.layer(id) else {
					return ToolResult::default();
				};
				let Some((outline, transform)) = outline_of(&layer.kind) else {
					return ToolResult::default();
				};
				let already = ctx.doc.selected.contains(&id);
				self.moving = Some(Moving {
					layer: id,
					from: (event.x, event.y),
					to: (event.x, event.y),
					transform,
					dragged: false,
					outline,
				});
				self.refresh_preview();
				ToolResult {
					// Photoshop selects the path's layer on the press. A layer
					// that is already selected keeps the click for the move.
					command: (!already).then_some(Command::SelectLayers {
						layers: vec![LayerRef::Id(id)],
					}),
					cursor: Some(CursorShape::Move),
					..Default::default()
				}
			}
			PointerKind::Move => {
				let Some(moving) = &mut self.moving else {
					// Hovering a shape shows the move cursor, like Photoshop.
					let over = hit(ctx.doc, event.x, event.y).is_some();
					return ToolResult {
						cursor: Some(if over { CursorShape::Move } else { CursorShape::Default }),
						..Default::default()
					};
				};
				moving.to = (event.x, event.y);
				let slop = MOVE_SLOP / ctx.view.zoom.max(f64::MIN_POSITIVE);
				if !moving.dragged && ((moving.to.0 - moving.from.0).abs() > slop || (moving.to.1 - moving.from.1).abs() > slop) {
					moving.dragged = true;
				}
				let (dx, dy) = Self::delta(moving);
				let dragged = moving.dragged;
				self.refresh_preview();
				ToolResult {
					redraw: true,
					status: dragged.then(|| format!("X: {} px   Y: {} px", dx.round() as i64, dy.round() as i64)),
					..Default::default()
				}
			}
			PointerKind::Up if self.moving.is_some() && event.buttons == 0 => {
				let Some(moving) = self.moving.take() else {
					return ToolResult::default();
				};
				self.preview.clear();
				let (dx, dy) = Self::delta(&moving);
				if !moving.dragged || (dx == 0.0 && dy == 0.0) {
					return ToolResult {
						redraw: true,
						..Default::default()
					};
				}
				let mut transform = moving.transform;
				transform[4] += dx;
				transform[5] += dy;
				ToolResult {
					command: Some(Command::SetShape {
						layer: LayerRef::Id(moving.layer),
						shape: None,
						fill: None,
						stroke: None,
						transform: Some(transform),
					}),
					cursor: Some(CursorShape::Move),
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
		CursorShape::Default
	}
}
