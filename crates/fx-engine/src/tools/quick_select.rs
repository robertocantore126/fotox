//! The Quick Selection tool (W, M9-T06): a brush whose stroke grows a
//! selection from the pixels under it (`fx_ops::select::quick`).
//!
//! The dabs are collected while dragging (every half radius) and the stroke
//! is one `Command::SelectBy` at release, "Quick Selection": Add when there
//! is a selection, Alt subtracts, the first stroke replaces.
//!
//! FAST: the outline updates at release, not live (the brush circles and the
//! path are drawn meanwhile).

use fx_core::select_ops::SelectOp;
use fx_core::{Command, SelectMode};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult, selection_mode};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

const ID: &str = "quick-select";

#[derive(Default)]
pub struct QuickSelect {
	dabs: Vec<(f64, f64, f64)>,
	hover: Option<(f64, f64)>,
	radius: f64,
	subtract: bool,
}

impl QuickSelect {
	fn radius(ctx: &ToolContext<'_>) -> f64 {
		(ctx.settings.number(ID, "Size").unwrap_or(30.0) / 2.0).clamp(0.5, 2500.0)
	}
}

impl Tool for QuickSelect {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.hover = Some((event.x, event.y));
		self.radius = Self::radius(ctx);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				self.subtract = event.modifiers.alt || selection_mode(ctx, ID) == SelectMode::Subtract;
				self.dabs = vec![(event.x, event.y, self.radius)];
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move if !self.dabs.is_empty() => {
				let (lx, ly, _) = *self.dabs.last().expect("not empty");
				if (event.x - lx).hypot(event.y - ly) >= self.radius / 2.0 {
					self.dabs.push((event.x, event.y, self.radius));
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if !self.dabs.is_empty() => {
				let dabs = std::mem::take(&mut self.dabs);
				let mode = if self.subtract {
					SelectMode::Subtract
				} else if ctx.doc.selection.is_some() || selection_mode(ctx, ID) == SelectMode::Add || event.modifiers.shift {
					SelectMode::Add
				} else {
					SelectMode::Replace
				};
				ToolResult {
					command: Some(Command::SelectBy {
						select: SelectOp::QuickSelect {
							dabs,
							sample_all: ctx.settings.bool(ID, "Sample All Layers").unwrap_or(false),
							enhance_edge: ctx.settings.bool(ID, "Auto-Enhance").unwrap_or(true),
						},
						mode,
					}),
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult {
				redraw: true,
				..Default::default()
			},
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let mut items = Vec::new();
		if let Some(at) = self.hover {
			items.push(OverlayItem::Circle {
				centre: at,
				radius: self.radius.max(0.5),
				style: OverlayStyle::Xor,
			});
		}
		if self.dabs.len() > 1 {
			items.push(OverlayItem::Polyline {
				points: self.dabs.iter().map(|d| (d.0, d.1)).collect(),
				closed: false,
				style: OverlayStyle::Xor,
			});
		}
		Some(Overlay { items })
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::None
	}
}
