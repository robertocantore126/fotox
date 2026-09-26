//! The measuring tools (M9-T08): Color Sampler, Ruler, Note and Count.
//!
//! Samplers, notes and counts are document data (`Document::annotations`,
//! D-072): a click edits them through one `Command::SetAnnotations` step. The
//! Ruler is a tool-only line whose length and angle go to the status bar /
//! Info panel; its "Straighten Layer" rotates the active layer so the line is
//! level.
//!
//! FAST: the annotations are drawn only while their tool is active; counts
//! and samplers show crosshairs, not their numbers.

use fx_core::annotations::{Annotations, CountGroup, Note, Sampler};
use fx_core::{Command, LayerRef, Mapping};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// The most samplers a document holds (Photoshop CC: 10).
const MAX_SAMPLERS: usize = 10;
/// Screen pixels a click may miss a point by and still take it.
const PICK_PX: f64 = 8.0;

fn edit(ctx: &ToolContext<'_>, label: &str, f: impl FnOnce(&mut Annotations)) -> ToolResult {
	let mut annotations = ctx.doc.annotations.clone();
	f(&mut annotations);
	ToolResult {
		command: Some(Command::SetAnnotations {
			annotations,
			label: label.into(),
		}),
		redraw: true,
		..Default::default()
	}
}

fn nearest(points: impl Iterator<Item = (f64, f64)>, p: (f64, f64), reach: f64) -> Option<usize> {
	let mut best = None;
	let mut d = reach;
	for (i, q) in points.enumerate() {
		let e = (q.0 - p.0).hypot(q.1 - p.1);
		if e <= d {
			d = e;
			best = Some(i);
		}
	}
	best
}

fn inside(ctx: &ToolContext<'_>, p: (f64, f64)) -> bool {
	p.0 >= 0.0 && p.1 >= 0.0 && p.0 < f64::from(ctx.doc.width) && p.1 < f64::from(ctx.doc.height)
}

/// The Color Sampler (I's flyout).
#[derive(Default)]
pub struct ColorSampler {
	points: Vec<(f64, f64)>,
}

impl Tool for ColorSampler {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.points = ctx.doc.annotations.samplers.iter().map(|s| (s.x, s.y)).collect();
		if event.kind != PointerKind::Down || event.buttons & BUTTON_LEFT == 0 {
			return ToolResult::default();
		}
		let p = (event.x.floor() + 0.5, event.y.floor() + 0.5);
		let reach = PICK_PX / ctx.view.zoom.max(1e-6);
		if event.modifiers.alt {
			let Some(i) = nearest(self.points.iter().copied(), p, reach) else {
				return ToolResult::default();
			};
			return edit(ctx, "Delete Color Sampler", |a| {
				a.samplers.remove(i);
			});
		}
		if !inside(ctx, p) {
			return ToolResult::default();
		}
		if self.points.len() >= MAX_SAMPLERS {
			return ToolResult {
				info: Some(format!("A document holds at most {MAX_SAMPLERS} color samplers")),
				..Default::default()
			};
		}
		edit(ctx, "Color Sampler", |a| a.samplers.push(Sampler { x: p.0, y: p.1 }))
	}

	fn overlay(&self) -> Option<Overlay> {
		Some(Overlay {
			items: self.points.iter().map(|&at| OverlayItem::Crosshair { at }).collect(),
		})
	}

	fn activate(&mut self, doc: &fx_core::Document) {
		self.points = doc.annotations.samplers.iter().map(|s| (s.x, s.y)).collect();
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}

	fn options_changed(&mut self, _ctx: &mut ToolContext<'_>) -> ToolResult {
		ToolResult::default()
	}
}

/// The Ruler.
#[derive(Default)]
pub struct Ruler {
	line: Option<((f64, f64), (f64, f64))>,
	dragging: bool,
}

impl Ruler {
	fn status(&self) -> Option<String> {
		let (a, b) = self.line?;
		let (dx, dy) = (b.0 - a.0, b.1 - a.1);
		let angle = (-dy).atan2(dx).to_degrees();
		Some(format!(
			"X: {:.0}  Y: {:.0}  W: {:.1}  H: {:.1}  A: {:.1}°  L1: {:.2}",
			a.0,
			a.1,
			dx,
			dy,
			angle,
			dx.hypot(dy)
		))
	}
}

impl Tool for Ruler {
	fn pointer(&mut self, _ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let p = (event.x, event.y);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				self.line = Some((p, p));
				self.dragging = true;
			}
			PointerKind::Move if self.dragging => {
				if let Some((a, _)) = self.line {
					let mut b = p;
					if event.modifiers.shift {
						let (dx, dy) = (p.0 - a.0, p.1 - a.1);
						let step = std::f64::consts::FRAC_PI_4;
						let angle = (dy.atan2(dx) / step).round() * step;
						let len = dx.hypot(dy);
						b = (a.0 + len * angle.cos(), a.1 + len * angle.sin());
					}
					self.line = Some((a, b));
				}
			}
			PointerKind::Up => self.dragging = false,
			_ => return ToolResult::default(),
		}
		ToolResult {
			status: self.status(),
			redraw: true,
			..Default::default()
		}
	}

	/// "Clear" and "Straighten" come from the option bar's buttons.
	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		match key {
			"Clear" | "Escape" => {
				self.line = None;
				ToolResult {
					status: Some(String::new()),
					redraw: true,
					..Default::default()
				}
			}
			"Straighten" => {
				let Some((a, b)) = self.line else {
					return ToolResult {
						info: Some("Drag the ruler along a line that should be level".into()),
						..Default::default()
					};
				};
				let mut angle = (b.1 - a.1).atan2(b.0 - a.0);
				// Level the nearest axis: a near-vertical line is made vertical.
				let quarter = std::f64::consts::FRAC_PI_2;
				angle -= (angle / quarter).round() * quarter;
				self.line = None;
				let (cx, cy) = (f64::from(ctx.doc.width) / 2.0, f64::from(ctx.doc.height) / 2.0);
				// FAST: rotates the active layer only; Photoshop also crops.
				ToolResult {
					command: Some(Command::Transform {
						layer: LayerRef::Active,
						mapping: Box::new(Mapping::rotation_about(-angle, cx, cy)),
						filter: fx_core::Filter::Bicubic,
					}),
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let (a, b) = self.line?;
		Some(Overlay {
			items: vec![
				OverlayItem::Polyline {
					points: vec![a, b],
					closed: false,
					style: OverlayStyle::Xor,
				},
				OverlayItem::Crosshair { at: a },
				OverlayItem::Crosshair { at: b },
			],
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

/// The Note tool.
#[derive(Default)]
pub struct NoteTool {
	points: Vec<(f64, f64)>,
}

impl Tool for NoteTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.points = ctx.doc.annotations.notes.iter().map(|n| (n.x, n.y)).collect();
		if event.kind != PointerKind::Down || event.buttons & BUTTON_LEFT == 0 {
			return ToolResult::default();
		}
		let p = (event.x, event.y);
		let reach = PICK_PX / ctx.view.zoom.max(1e-6);
		if let Some(i) = nearest(self.points.iter().copied(), p, reach) {
			if event.modifiers.alt {
				return edit(ctx, "Delete Note", |a| {
					a.notes.remove(i);
				});
			}
			return ToolResult {
				info: Some("Edit the note in the Notes panel".into()),
				..Default::default()
			};
		}
		if !inside(ctx, p) {
			return ToolResult::default();
		}
		let author = ctx.settings.string("note-tool", "Author").unwrap_or_default();
		let mut result = edit(ctx, "New Note", |a| {
			a.notes.push(Note {
				x: p.0,
				y: p.1,
				author,
				text: String::new(),
				color: [65535, 52000, 0, 65535],
			})
		});
		result.info = Some("Note added: type it in the Notes panel".into());
		result
	}

	fn activate(&mut self, doc: &fx_core::Document) {
		self.points = doc.annotations.notes.iter().map(|n| (n.x, n.y)).collect();
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if key == "Clear" {
			return edit(ctx, "Delete All Notes", |a| a.notes.clear());
		}
		ToolResult::default()
	}

	fn overlay(&self) -> Option<Overlay> {
		Some(Overlay {
			items: self.points.iter().map(|&at| OverlayItem::Handle { at, size_px: 10.0 }).collect(),
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

/// The Count tool.
#[derive(Default)]
pub struct CountTool {
	points: Vec<(f64, f64)>,
}

impl Tool for CountTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.points = ctx.doc.annotations.counts.iter().flat_map(|g| g.points.iter().copied()).collect();
		if event.kind != PointerKind::Down || event.buttons & BUTTON_LEFT == 0 {
			return ToolResult::default();
		}
		let p = (event.x, event.y);
		let reach = PICK_PX / ctx.view.zoom.max(1e-6);
		if event.modifiers.alt {
			// Remove the nearest point of the current (last) group.
			let Some(group) = ctx.doc.annotations.counts.last() else {
				return ToolResult::default();
			};
			let Some(i) = nearest(group.points.iter().copied(), p, reach) else {
				return ToolResult::default();
			};
			return edit(ctx, "Count", |a| {
				if let Some(g) = a.counts.last_mut() {
					g.points.remove(i);
				}
			});
		}
		if !inside(ctx, p) {
			return ToolResult::default();
		}
		let n = ctx.doc.annotations.counts.last().map_or(0, |g| g.points.len()) + 1;
		let mut result = edit(ctx, "Count", |a| {
			if a.counts.is_empty() {
				a.counts.push(CountGroup {
					name: "Count Group 1".into(),
					color: [65535, 20000, 20000, 65535],
					marker: 8.0,
					label: 12.0,
					visible: true,
					points: Vec::new(),
				});
			}
			a.counts.last_mut().expect("made above").points.push(p);
		});
		result.status = Some(format!("Count: {n}"));
		result
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		match key {
			"Clear" => edit(ctx, "Clear Count", |a| {
				if let Some(g) = a.counts.last_mut() {
					g.points.clear();
				}
			}),
			"NewGroup" => edit(ctx, "New Count Group", |a| {
				let n = a.counts.len() + 1;
				a.counts.push(CountGroup {
					name: format!("Count Group {n}"),
					color: [20000, 40000, 65535, 65535],
					marker: 8.0,
					label: 12.0,
					visible: true,
					points: Vec::new(),
				});
			}),
			_ => ToolResult::default(),
		}
	}

	fn activate(&mut self, doc: &fx_core::Document) {
		self.points = doc.annotations.counts.iter().flat_map(|g| g.points.iter().copied()).collect();
	}

	fn overlay(&self) -> Option<Overlay> {
		Some(Overlay {
			items: self.points.iter().map(|&at| OverlayItem::Crosshair { at }).collect(),
		})
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}
