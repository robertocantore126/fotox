//! The Artboard tool (M12-T07), under the Move tool: drag on empty canvas to
//! draw a new artboard; drag inside an artboard to move it (its layers go
//! along); Alt+drag an artboard's bottom-right corner to resize it. Every
//! artboard's outline and name are drawn.
//!
//! FAST: no side handles, no "+" buttons for adjacent artboards, no size
//! presets in the option bar (the Size field only), no snapping.

use fx_core::{Command, LayerRef};
use fx_render::{OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, PointerKind};

type Rect = (i32, i32, u32, u32);

#[derive(Clone, Copy, Debug)]
enum Drag {
	New { start: (f64, f64) },
	Move { id: fx_core::LayerId, rect: Rect, start: (f64, f64) },
	Resize { id: fx_core::LayerId, rect: Rect },
}

#[derive(Default)]
pub struct ArtboardTool {
	drag: Option<Drag>,
	at: (f64, f64),
	boards: Vec<(String, Rect)>,
}

fn boards(ctx: &ToolContext<'_>) -> Vec<(fx_core::LayerId, String, Rect, Option<[u16; 4]>)> {
	ctx.doc
		.layers
		.iter()
		.filter_map(|l| l.artboard.as_ref().map(|a| (l.id, l.name.clone(), a.rect, a.background)))
		.collect()
}

fn inside(r: Rect, p: (f64, f64)) -> bool {
	p.0 >= f64::from(r.0) && p.1 >= f64::from(r.1) && p.0 < f64::from(r.0) + f64::from(r.2) && p.1 < f64::from(r.1) + f64::from(r.3)
}

impl ArtboardTool {
	fn current(&self) -> Option<Rect> {
		match self.drag? {
			Drag::New { start } => {
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
			Drag::Resize { rect, .. } => {
				let w = (self.at.0 - f64::from(rect.0)).round().max(1.0) as u32;
				let h = (self.at.1 - f64::from(rect.1)).round().max(1.0) as u32;
				Some((rect.0, rect.1, w, h))
			}
		}
	}
}

impl Tool for ArtboardTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.at = (event.x, event.y);
		let all = boards(ctx);
		self.boards = all.iter().map(|(_, n, r, _)| (n.clone(), *r)).collect();
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let hit = all.iter().rev().find(|(_, _, r, _)| inside(*r, self.at));
				self.drag = Some(match hit {
					Some(&(id, _, rect, _)) if event.modifiers.alt => Drag::Resize { id, rect },
					Some(&(id, _, rect, _)) => Drag::Move { id, rect, start: self.at },
					None => Drag::New { start: self.at },
				});
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move if self.drag.is_some() => ToolResult {
				redraw: true,
				..Default::default()
			},
			PointerKind::Up if self.drag.is_some() => {
				let rect = self.current();
				let drag = self.drag.take().expect("checked");
				let command = match (drag, rect) {
					(Drag::New { .. }, Some(r)) if r.2 >= 4 && r.3 >= 4 => Some(Command::NewArtboard {
						rect: r,
						name: None,
						layers: Vec::new(),
						background: Some([65_535; 4]),
					}),
					(Drag::Move { id, rect: old, .. } | Drag::Resize { id, rect: old }, Some(r)) if r != old => {
						let background = all.iter().find(|b| b.0 == id).and_then(|b| b.3);
						Some(Command::SetArtboard {
							layer: LayerRef::Id(id),
							rect: r,
							background,
						})
					}
					_ => None,
				};
				ToolResult {
					command,
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
		let mut items: Vec<OverlayItem> = self
			.boards
			.iter()
			.map(|(_, r)| outline(*r, OverlayStyle::Solid([0.3, 0.6, 1.0, 1.0])))
			.collect();
		if let Some(r) = self.current() {
			items.push(outline(r, OverlayStyle::Xor));
		}
		Some(fx_render::Overlay { items })
	}

	fn cursor(&self, _modifiers: crate::Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}
