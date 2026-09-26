//! The Slice and Slice Select tools (M12-T08). Slice: drag to add a user
//! slice. Slice Select: click picks a slice, drag moves it, Alt+drag resizes
//! it from its bottom-right corner, Delete / Backspace removes it. Both draw
//! the user slices (blue) and the auto slices that fill the rest (grey).
//!
//! FAST: no Divide Slice, no slice options dialog (name only via its
//! number), no layer-based slices, no stacking order controls.

use fx_core::Command;
use fx_core::comps::{Slice, auto_slices};
use fx_render::{OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, PointerKind};

type Rect = (i32, i32, u32, u32);

#[derive(Clone, Copy, Debug)]
enum Drag {
	New((f64, f64)),
	Move { index: usize, rect: Rect, start: (f64, f64) },
	Resize { index: usize, rect: Rect },
}

pub struct SliceTool {
	select: bool,
	drag: Option<Drag>,
	at: (f64, f64),
	selected: Option<usize>,
	user: Vec<Rect>,
	auto: Vec<Rect>,
}

impl SliceTool {
	pub fn new(select: bool) -> Self {
		Self {
			select,
			drag: None,
			at: (0.0, 0.0),
			selected: None,
			user: Vec::new(),
			auto: Vec::new(),
		}
	}

	fn refresh(&mut self, ctx: &ToolContext<'_>) {
		self.user = ctx.doc.slices.iter().map(|s| s.rect).collect();
		self.auto = auto_slices(&ctx.doc.slices, (ctx.doc.width, ctx.doc.height));
	}

	fn current(&self) -> Option<Rect> {
		match self.drag? {
			Drag::New(start) => {
				let (x0, y0) = (start.0.min(self.at.0).max(0.0), start.1.min(self.at.1).max(0.0));
				let (x1, y1) = (start.0.max(self.at.0), start.1.max(self.at.1));
				Some((
					x0.round() as i32,
					y0.round() as i32,
					(x1 - x0).round().max(0.0) as u32,
					(y1 - y0).round().max(0.0) as u32,
				))
			}
			Drag::Move { rect, start, .. } => {
				let (dx, dy) = ((self.at.0 - start.0).round() as i32, (self.at.1 - start.1).round() as i32);
				Some(((rect.0 + dx).max(0), (rect.1 + dy).max(0), rect.2, rect.3))
			}
			Drag::Resize { rect, .. } => Some((
				rect.0,
				rect.1,
				(self.at.0 - f64::from(rect.0)).round().max(1.0) as u32,
				(self.at.1 - f64::from(rect.1)).round().max(1.0) as u32,
			)),
		}
	}

	fn set(ctx: &ToolContext<'_>, slices: Vec<Slice>, label: &str) -> Option<Command> {
		(slices != ctx.doc.slices).then(|| Command::SetSlices { slices, label: label.into() })
	}
}

fn inside(r: Rect, p: (f64, f64)) -> bool {
	p.0 >= f64::from(r.0) && p.1 >= f64::from(r.1) && p.0 < f64::from(r.0) + f64::from(r.2) && p.1 < f64::from(r.1) + f64::from(r.3)
}

impl Tool for SliceTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.at = (event.x, event.y);
		self.refresh(ctx);
		let redraw = ToolResult {
			redraw: true,
			..Default::default()
		};
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let hit = self.user.iter().rposition(|r| inside(*r, self.at));
				self.drag = match (self.select, hit) {
					(true, Some(index)) => {
						self.selected = Some(index);
						let rect = self.user[index];
						Some(if event.modifiers.alt {
							Drag::Resize { index, rect }
						} else {
							Drag::Move { index, rect, start: self.at }
						})
					}
					(true, None) => {
						self.selected = None;
						None
					}
					(false, _) => Some(Drag::New(self.at)),
				};
				redraw
			}
			PointerKind::Up if self.drag.is_some() => {
				let rect = self.current();
				let drag = self.drag.take().expect("checked");
				let mut slices = ctx.doc.slices.clone();
				let command = match (drag, rect) {
					(Drag::New(_), Some(r)) if r.2 >= 2 && r.3 >= 2 => {
						slices.push(Slice {
							name: format!("{:02}", slices.len() + 1),
							rect: r,
						});
						Self::set(ctx, slices, "Slice Tool")
					}
					(Drag::Move { index, .. } | Drag::Resize { index, .. }, Some(r)) if index < slices.len() => {
						slices[index].rect = r;
						Self::set(ctx, slices, "Slice Select")
					}
					_ => None,
				};
				ToolResult { command, ..redraw }
			}
			_ => redraw,
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if matches!(key, "Delete" | "Backspace")
			&& let Some(i) = self.selected.take()
			&& i < ctx.doc.slices.len()
		{
			let mut slices = ctx.doc.slices.clone();
			slices.remove(i);
			return ToolResult {
				command: Self::set(ctx, slices, "Delete Slice"),
				redraw: true,
				..Default::default()
			};
		}
		ToolResult::default()
	}

	fn overlay(&self) -> Option<fx_render::Overlay> {
		let outline = |r: Rect, style| OverlayItem::Polyline {
			points: vec![
				(f64::from(r.0), f64::from(r.1)),
				(f64::from(r.0) + f64::from(r.2), f64::from(r.1)),
				(f64::from(r.0) + f64::from(r.2), f64::from(r.1) + f64::from(r.3)),
				(f64::from(r.0), f64::from(r.1) + f64::from(r.3)),
			],
			closed: true,
			style,
		};
		let mut items: Vec<OverlayItem> = self.auto.iter().map(|r| outline(*r, OverlayStyle::Solid([0.6, 0.6, 0.6, 0.8]))).collect();
		for (i, r) in self.user.iter().enumerate() {
			let style = if Some(i) == self.selected {
				OverlayStyle::Solid([1.0, 0.7, 0.1, 1.0])
			} else {
				OverlayStyle::Solid([0.2, 0.55, 1.0, 1.0])
			};
			items.push(outline(*r, style));
		}
		if let Some(r) = self.current() {
			items.push(outline(r, OverlayStyle::Xor));
		}
		Some(fx_render::Overlay { items })
	}

	fn cursor(&self, _modifiers: crate::Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}
