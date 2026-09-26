//! Tool kinds (M7-T08): the extension points of M8's tools.
//!
//! * [`ClickTool`]: a click → a command built from the document and the point
//!   (Paint Bucket, Magic Eraser, Red Eye).
//! * [`DragTool`]: a drag with a line overlay → a command at release
//!   (Gradient). FAST: no live preview yet; M8 adds it on M6-T04's path.
//! * Painting tools are [`fx_ops::brush::DabOp`]s behind the stroke engine.
//!
//! A new tool is a type implementing one of the traits plus one line in
//! [`registered`]; see `docs/tasks/HOWTO.md` R11.

use fx_core::Command;
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use super::{DocPointer, Tool, ToolContext, ToolResult};
use crate::{CursorShape, Modifiers, PointerKind};

/// A tool that acts on a click.
pub trait ClickTool: Send {
	/// The option-bar key this tool reads (`ctx.settings.*(key, …)`).
	fn id(&self) -> &'static str;
	/// The command for a click at `p` (document pixels), or a message why not.
	fn click(&mut self, ctx: &mut ToolContext<'_>, p: (f64, f64), modifiers: Modifiers) -> Result<Option<Command>, String>;
}

/// A tool that acts on a drag from `a` to `b`.
pub trait DragTool: Send {
	fn id(&self) -> &'static str;
	fn release(&mut self, ctx: &mut ToolContext<'_>, a: (f64, f64), b: (f64, f64), modifiers: Modifiers) -> Result<Option<Command>, String>;
}

/// Adapter: a [`ClickTool`] as a viewport [`Tool`].
pub struct Click<T: ClickTool>(pub T);

impl<T: ClickTool> Tool for Click<T> {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		if event.kind != PointerKind::Down {
			return ToolResult::default();
		}
		match self.0.click(ctx, (event.x, event.y), event.modifiers) {
			Ok(command) => ToolResult { command, ..Default::default() },
			Err(info) => ToolResult {
				info: Some(info),
				..Default::default()
			},
		}
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

/// Adapter: a [`DragTool`] as a viewport [`Tool`], with its line overlay.
pub struct Drag<T: DragTool> {
	pub tool: T,
	start: Option<(f64, f64)>,
	end: (f64, f64),
}

impl<T: DragTool> Drag<T> {
	pub fn new(tool: T) -> Self {
		Self {
			tool,
			start: None,
			end: (0.0, 0.0),
		}
	}
}

impl<T: DragTool> Tool for Drag<T> {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let p = (event.x, event.y);
		match event.kind {
			PointerKind::Down => {
				self.start = Some(p);
				self.end = p;
				ToolResult::default()
			}
			PointerKind::Move if self.start.is_some() => {
				let mut end = p;
				if event.modifiers.shift
					&& let Some(a) = self.start
				{
					// 45° steps, as Photoshop's Gradient tool.
					let (dx, dy) = (p.0 - a.0, p.1 - a.1);
					let angle = (dy.atan2(dx) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
					let len = dx.hypot(dy);
					end = (a.0 + len * angle.cos(), a.1 + len * angle.sin());
				}
				self.end = end;
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up => {
				let Some(a) = self.start.take() else {
					return ToolResult::default();
				};
				let result = self.tool.release(ctx, a, self.end, event.modifiers);
				let mut out = ToolResult {
					redraw: true,
					..Default::default()
				};
				match result {
					Ok(command) => out.command = command,
					Err(info) => out.info = Some(info),
				}
				out
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let a = self.start?;
		Some(Overlay {
			items: vec![OverlayItem::Polyline {
				points: vec![a, self.end],
				closed: false,
				style: OverlayStyle::Xor,
			}],
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

/// A Mode drop-down's blend mode ("Linear Dodge (Add)" → `linear_dodge`);
/// Normal for anything unknown.
pub fn blend_mode(mode: Option<String>) -> fx_core::BlendMode {
	mode.and_then(|m| {
		let m = m.split(" (").next().unwrap_or(&m).to_lowercase().replace([' ', '-'], "_");
		serde_json::from_value::<fx_core::BlendMode>(serde_json::Value::String(m)).ok()
	})
	.unwrap_or_default()
}

/// The tools built from a kind, by UI tool id. `None` for any other id.
pub fn registered(id: &str) -> Option<Box<dyn Tool>> {
	match id {
		"paint-bucket" => Some(Box::new(Click(super::bucket::Bucket))),
		"eraser-magic" => Some(Box::new(Click(super::bucket::MagicEraser))),
		"gradient" => Some(Box::new(Drag::new(super::gradient::GradientTool))),
		"red-eye" => Some(Box::new(Click(super::bucket::RedEye))),
		_ => None,
	}
}
