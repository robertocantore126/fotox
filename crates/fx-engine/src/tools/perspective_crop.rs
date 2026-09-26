//! The Perspective Crop tool (C's flyout, M9-T09).
//!
//! A drag draws the box; then each corner handle moves on its own (a
//! drag inside moves the whole quad). A 3 × 3 grid follows the quad. Enter
//! commits one `Command::PerspectiveCrop`: the quad becomes a rectangle of
//! the option bar's W × H, or of the quad's average side lengths; Escape
//! cancels.

use fx_core::Command;
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

const ID: &str = "crop-persp";
const HANDLE_SLOP: f64 = 8.0;

type P = (f64, f64);

#[derive(Default)]
pub struct PerspectiveCrop {
	/// Top-left, top-right, bottom-right, bottom-left.
	quad: Option<[P; 4]>,
	/// What the press grabbed: a corner, the whole quad, or a new box.
	grab: Option<Grab>,
	start: P,
	before: Option<[P; 4]>,
}

#[derive(Clone, Copy)]
enum Grab {
	New,
	Corner(usize),
	Move,
}

fn lerp(a: P, b: P, t: f64) -> P {
	(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

fn inside(q: &[P; 4], p: P) -> bool {
	// Convex quad: the point is on the same side of every edge.
	let mut sign = 0.0;
	for i in 0..4 {
		let (a, b) = (q[i], q[(i + 1) % 4]);
		let c = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
		if c != 0.0 {
			if sign != 0.0 && c.signum() != sign {
				return false;
			}
			sign = c.signum();
		}
	}
	true
}

impl PerspectiveCrop {
	fn commit(&mut self, ctx: &ToolContext<'_>) -> ToolResult {
		let Some(q) = self.quad.take() else { return ToolResult::default() };
		let d = |a: P, b: P| (a.0 - b.0).hypot(a.1 - b.1);
		let s = ctx.settings;
		let width = s.number(ID, "W").filter(|v| *v >= 1.0).unwrap_or((d(q[0], q[1]) + d(q[3], q[2])) / 2.0);
		let height = s.number(ID, "H").filter(|v| *v >= 1.0).unwrap_or((d(q[0], q[3]) + d(q[1], q[2])) / 2.0);
		ToolResult {
			command: Some(Command::PerspectiveCrop {
				quad: q,
				width: width.round().max(1.0) as u32,
				height: height.round().max(1.0) as u32,
			}),
			redraw: true,
			..Default::default()
		}
	}
}

impl Tool for PerspectiveCrop {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let p = (event.x, event.y);
		let slop = HANDLE_SLOP / ctx.view.zoom.max(1e-6);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				self.start = p;
				self.before = self.quad;
				self.grab = Some(match &self.quad {
					Some(q) => match (0..4).find(|&i| (q[i].0 - p.0).hypot(q[i].1 - p.1) <= slop) {
						Some(i) => Grab::Corner(i),
						None if inside(q, p) => Grab::Move,
						None => Grab::New,
					},
					None => Grab::New,
				});
			}
			PointerKind::Move if self.grab.is_some() => match self.grab.expect("checked") {
				Grab::New => {
					let (a, b) = (self.start, p);
					self.quad = Some([(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)]);
				}
				Grab::Corner(i) => {
					if let Some(mut q) = self.before {
						q[i] = p;
						if fx_core::transform::quad_is_convex(q) {
							self.quad = Some(q);
						}
					}
				}
				Grab::Move => {
					if let Some(q) = self.before {
						let (dx, dy) = (p.0 - self.start.0, p.1 - self.start.1);
						self.quad = Some(q.map(|c| (c.0 + dx, c.1 + dy)));
					}
				}
			},
			PointerKind::Up => {
				self.grab = None;
				// A click without a drag leaves no box.
				if let Some(q) = self.quad
					&& (q[0].0 - q[2].0).abs() < 1.0
					&& (q[0].1 - q[2].1).abs() < 1.0
				{
					self.quad = None;
				}
			}
			_ => return ToolResult::default(),
		}
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		match key {
			"Enter" => self.commit(ctx),
			"Escape" => {
				self.quad = None;
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let q = self.quad?;
		let mut items = vec![OverlayItem::Polyline {
			points: q.to_vec(),
			closed: true,
			style: OverlayStyle::Xor,
		}];
		for t in [1.0 / 3.0, 2.0 / 3.0] {
			items.push(OverlayItem::Polyline {
				points: vec![lerp(q[0], q[1], t), lerp(q[3], q[2], t)],
				closed: false,
				style: OverlayStyle::Solid([1.0, 1.0, 1.0, 0.5]),
			});
			items.push(OverlayItem::Polyline {
				points: vec![lerp(q[0], q[3], t), lerp(q[1], q[2], t)],
				closed: false,
				style: OverlayStyle::Solid([1.0, 1.0, 1.0, 0.5]),
			});
		}
		for c in q {
			items.push(OverlayItem::Handle { at: c, size_px: 8.0 });
		}
		Some(Overlay { items })
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}

	fn deactivate(&mut self, _ctx: &mut ToolContext<'_>) -> ToolResult {
		self.quad = None;
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}
}
