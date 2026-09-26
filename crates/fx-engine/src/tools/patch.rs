//! The Patch tool (M11-T03) and the Content-Aware Move tool (M11-T04).
//!
//! Both draw their selection with a freehand lasso (or use the current one);
//! a drag that starts inside the selection is the patch / move, committed on
//! release as one `Command::Patch` / `Command::ContentAwareMove`. While
//! dragging the marching ants follow the pointer.
//!
//! FAST: no live preview of the patched content while dragging; Transparent,
//! Structure, Color, Sample All Layers and Transform on Drop are ignored;
//! Content-Aware Move's Duplicate mode acts as Extend.

use fx_core::{Command, LayerRef};
use fx_render::Overlay;

use crate::tools::lasso::{Kind as LassoKind, Lasso};
use crate::tools::{DocPointer, OutlineDrag, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	Patch,
	ContentMove,
}

pub struct Patch {
	id: &'static str,
	kind: Kind,
	lasso: Lasso,
	drag: Option<OutlineDrag>,
}

impl Patch {
	pub fn new(id: &'static str, kind: Kind) -> Self {
		Self {
			id,
			kind,
			lasso: Lasso::new(id, LassoKind::Freehand),
			drag: None,
		}
	}

	fn commit(&self, ctx: &ToolContext<'_>, (dx, dy): (i32, i32)) -> Option<Command> {
		if dx == 0 && dy == 0 {
			return None;
		}
		let (dx, dy) = (i64::from(dx), i64::from(dy));
		let s = ctx.settings;
		Some(match self.kind {
			Kind::Patch => Command::Patch {
				layer: LayerRef::Active,
				dx,
				dy,
				// The bar's first group: 0 Source, 1 Destination; "Patch:" select.
				destination: s.number(self.id, "Source/Destination") == Some(1.0),
				content_aware: s.string(self.id, "Patch").as_deref() == Some("Content-Aware"),
			},
			Kind::ContentMove => Command::ContentAwareMove {
				layer: LayerRef::Active,
				dx,
				dy,
				extend: s.string(self.id, "Mode").is_some_and(|m| m != "Move"),
			},
		})
	}
}

impl Tool for Patch {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 && self.drag.is_none() => {
				if let Some(drag) = OutlineDrag::begin(ctx, event) {
					self.drag = Some(drag);
					return ToolResult {
						cursor: Some(CursorShape::Move),
						..Default::default()
					};
				}
				self.lasso.pointer(ctx, event)
			}
			PointerKind::Move if self.drag.is_some() => {
				if let Some(drag) = self.drag.as_mut() {
					drag.track(event, ctx.view.zoom);
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if self.drag.is_some() => {
				let drag = self.drag.take().expect("checked");
				let command = self.commit(ctx, drag.delta());
				let working = command.is_some();
				ToolResult {
					command,
					status: working.then(|| if self.kind == Kind::Patch { "Patching…".into() } else { "Moving…".into() }),
					redraw: true,
					..Default::default()
				}
			}
			_ => self.lasso.pointer(ctx, event),
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if key == "Escape" && self.drag.take().is_some() {
			return ToolResult {
				redraw: true,
				..Default::default()
			};
		}
		self.lasso.key(ctx, key)
	}

	fn overlay(&self) -> Option<Overlay> {
		self.lasso.overlay()
	}

	fn cursor(&self, modifiers: Modifiers) -> CursorShape {
		self.lasso.cursor(modifiers)
	}

	fn selection_nudge(&self) -> Option<(i32, i32)> {
		self.drag.as_ref().map(OutlineDrag::delta).or_else(|| self.lasso.selection_nudge())
	}

	fn deactivate(&mut self, ctx: &mut ToolContext<'_>) -> ToolResult {
		self.drag = None;
		self.lasso.deactivate(ctx)
	}
}
