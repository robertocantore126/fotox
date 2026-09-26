//! The pen tools (M10-T02..T05): Pen, Freeform Pen, Curvature Pen, Add /
//! Delete / Convert Anchor Point and Direct Selection.
//!
//! They edit **the target path**: the path selected in the Paths panel, else
//! the Work Path; Direct Selection and the anchor tools also edit the active
//! shape layer's outline when no path is selected (a live shape becomes a
//! path shape, as in Photoshop). A new drawing stays in the tool until it is
//! finished (closed, Enter, Escape or another tool), then it is one step:
//! `SetPath` (Path mode), a new shape layer (Shape mode) or a Work Path filled
//! on the layer (Pixels mode). Edits of an existing path are one step per
//! gesture.
//!
//! FAST: no marquee or Shift multi-selection of anchors; Delete Anchor keeps
//! the neighbours' handles (no refit); the Freeform Pen's Magnetic option is
//! not there; Ctrl does not switch to Direct Selection while drawing.

use fx_core::command::NewLayer;
use fx_core::path::{Anchor, Path, PathOp, PathTarget, Subpath, simplify, smooth_through};
use fx_core::vector::{IDENTITY, Paint, VectorShape};
use fx_core::{Command, LayerKind, LayerRef};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

const PICK_PX: f64 = 7.0;
const DOUBLE_CLICK_US: u64 = 400_000;

/// Which pen tool an instance is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	Pen,
	Freeform,
	Curvature,
	AnchorAdd,
	AnchorDelete,
	AnchorConvert,
	Direct,
}

/// What the tool edits.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
	Doc(PathTarget),
	/// A shape layer's outline, in document coordinates.
	Shape(fx_core::LayerId),
}

/// A drag in progress on an existing path.
#[derive(Clone, Copy, Debug)]
enum Grab {
	Anchor(usize, usize),
	/// (subpath, anchor, true = out handle)
	Handle(usize, usize, bool),
	/// Convert Point: a new smooth anchor pulled from (subpath, anchor).
	Pull(usize, usize),
}

pub struct Pen {
	id: &'static str,
	kind: Kind,
	/// The path being drawn (not yet in the document).
	drawing: Option<Subpath>,
	/// Curvature Pen: the points and which are corners.
	curve: Vec<((f64, f64), bool)>,
	/// Freeform: the pointer samples of the drag.
	freehand: Vec<(f64, f64)>,
	/// An existing path being edited, with its target.
	editing: Option<(Target, Path)>,
	grab: Option<Grab>,
	/// Direct Selection's selected anchor.
	selected: Option<(usize, usize)>,
	hover: Option<(f64, f64)>,
	button: bool,
	last_press: Option<(u64, (f64, f64))>,
	zoom: f64,
}

impl Pen {
	pub fn new(id: &'static str, kind: Kind) -> Self {
		Self {
			id,
			kind,
			drawing: None,
			curve: Vec::new(),
			freehand: Vec::new(),
			editing: None,
			grab: None,
			selected: None,
			hover: None,
			button: false,
			last_press: None,
			zoom: 1.0,
		}
	}

	fn reach(&self) -> f64 {
		PICK_PX / self.zoom.max(1e-6)
	}

	fn mode(ctx: &ToolContext<'_>, id: &str) -> String {
		// The bar's first button group: 0 Path, 1 Shape; a "Mode" select too.
		match ctx.settings.number(id, "Tool Mode") {
			Some(1.0) => "Shape".into(),
			Some(2.0) => "Pixels".into(),
			_ => ctx.settings.string(id, "Mode").unwrap_or_else(|| "Path".into()),
		}
	}

	fn op(ctx: &ToolContext<'_>, id: &str) -> PathOp {
		match ctx.settings.number(id, "Path Operations") {
			Some(1.0) => PathOp::Subtract,
			Some(2.0) => PathOp::Intersect,
			Some(3.0) => PathOp::Exclude,
			_ => PathOp::Combine,
		}
	}

	/// The path this tool edits and where it lives.
	fn target(ctx: &ToolContext<'_>, shapes: bool) -> Option<(Target, Path)> {
		if let Some(t) = ctx.doc.active_path
			&& let Some(p) = ctx.doc.path(t)
		{
			return Some((Target::Doc(t), p.clone()));
		}
		if shapes
			&& let Some(id) = ctx.doc.active_layer()
			&& let Some(layer) = ctx.doc.layer(id)
			&& let LayerKind::Shape { shape, transform, .. } = &layer.kind
		{
			let m = *transform;
			let local = Path::from_elements(&shape.outline());
			let doc = local.map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
			return Some((Target::Shape(id), doc));
		}
		ctx.doc.work_path.clone().map(|p| (Target::Doc(PathTarget::Work), p))
	}

	/// The command that stores an edited path.
	fn store(target: Target, path: Path, label: &str) -> Command {
		match target {
			Target::Doc(t) => Command::SetPath {
				target: t,
				path,
				name: None,
				label: label.into(),
			},
			// FAST: the outline is stored in document coordinates with an
			// identity placement (the layer's old matrix is folded in).
			Target::Shape(id) => Command::SetShape {
				layer: LayerRef::Id(id),
				shape: Some(VectorShape::Path { elements: path.to_elements() }),
				fill: None,
				stroke: None,
				transform: Some(IDENTITY),
			},
		}
	}

	/// Finish the drawing in progress: one step by the bar's Mode.
	fn finish(&mut self, ctx: &ToolContext<'_>) -> ToolResult {
		let sub = match self.kind {
			Kind::Curvature => {
				let pts: Vec<(f64, f64)> = self.curve.iter().map(|c| c.0).collect();
				let corners: Vec<bool> = self.curve.iter().map(|c| c.1).collect();
				self.curve.clear();
				(pts.len() >= 2).then(|| smooth_through(&pts, self.drawing.as_ref().is_some_and(|d| d.closed), &corners))
			}
			_ => self.drawing.take().filter(|s| s.anchors.len() >= 2),
		};
		self.drawing = None;
		let Some(mut sub) = sub else {
			return ToolResult {
				redraw: true,
				..Default::default()
			};
		};
		sub.op = Self::op(ctx, self.id);
		let mode = Self::mode(ctx, self.id);
		let mut result = ToolResult {
			redraw: true,
			..Default::default()
		};
		if mode == "Shape" {
			let path = Path { subpaths: vec![sub] };
			result.command = Some(Command::AddLayer {
				layer: NewLayer::Shape {
					shape: VectorShape::Path { elements: path.to_elements() },
					fill: Some(Paint::Solid { rgba: ctx.settings.fg }),
					stroke: None,
					transform: IDENTITY,
				},
				name: None,
			});
			return result;
		}
		// Path (and Pixels): into the selected path, or a new Work Path.
		let (target, mut path) = match ctx.doc.active_path.and_then(|t| ctx.doc.path(t).map(|p| (t, p.clone()))) {
			Some(found) => found,
			None => (PathTarget::Work, Path::default()),
		};
		path.subpaths.push(sub);
		result.command = Some(Command::SetPath {
			target,
			path,
			name: None,
			label: "Work Path".into(),
		});
		if mode == "Pixels" {
			result.then.push(Command::FillPath {
				target,
				source: fx_core::fill::FillSource::Color { rgba: ctx.settings.fg },
				mode: fx_core::BlendMode::Normal,
				opacity: 1.0,
			});
		}
		result
	}

	fn pen_down(&mut self, ctx: &ToolContext<'_>, p: (f64, f64), modifiers: Modifiers, double: bool) -> ToolResult {
		// Auto Add/Delete on the target path when not drawing.
		if self.drawing.is_none()
			&& ctx.settings.bool(self.id, "Auto Add/Delete").unwrap_or(true)
			&& self.kind == Kind::Pen
			&& let Some((target, mut path)) = Self::target(ctx, false)
		{
			if let Some((si, ai)) = path.anchor_at(p, self.reach()) {
				path.subpaths[si].anchors.remove(ai);
				return ToolResult {
					command: Some(Self::store(target, path, "Delete Anchor Point")),
					redraw: true,
					..Default::default()
				};
			}
			if let Some((si, seg, t)) = path.segment_at(p, self.reach()) {
				path.split(si, seg, t);
				return ToolResult {
					command: Some(Self::store(target, path, "Add Anchor Point")),
					redraw: true,
					..Default::default()
				};
			}
		}
		let reach = self.reach();
		if let Some(d) = &mut self.drawing {
			// A click on the first anchor closes the subpath.
			if d.anchors.len() >= 2 && (d.anchors[0].pos.0 - p.0).hypot(d.anchors[0].pos.1 - p.1) <= reach {
				d.closed = true;
				return self.finish(ctx);
			}
			// Alt+click on the last anchor removes its out handle (a corner).
			if modifiers.alt
				&& let Some(last) = d.anchors.last_mut()
				&& (last.pos.0 - p.0).hypot(last.pos.1 - p.1) <= reach
			{
				last.out = last.pos;
				last.smooth = false;
				return ToolResult {
					redraw: true,
					..Default::default()
				};
			}
			if double {
				return self.finish(ctx);
			}
			d.anchors.push(Anchor::corner(p));
		} else {
			self.drawing = Some(Subpath {
				anchors: vec![Anchor::corner(p)],
				closed: false,
				op: PathOp::Combine,
			});
		}
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}

	fn pen_drag(&mut self, p: (f64, f64), modifiers: Modifiers) {
		let Some(d) = &mut self.drawing else { return };
		let Some(last) = d.anchors.last_mut() else { return };
		if modifiers.alt {
			// Break the symmetry: only the outgoing handle follows.
			last.out = p;
			last.smooth = false;
		} else {
			*last = Anchor::smooth(last.pos, p);
		}
	}

	fn curvature_down(&mut self, ctx: &ToolContext<'_>, p: (f64, f64), double: bool) -> ToolResult {
		let reach = self.reach();
		if double && let Some(last) = self.curve.last_mut() {
			last.1 = !last.1;
			return ToolResult {
				redraw: true,
				..Default::default()
			};
		}
		if self.curve.len() >= 3 && (self.curve[0].0.0 - p.0).hypot(self.curve[0].0.1 - p.1) <= reach {
			self.drawing = Some(Subpath {
				closed: true,
				..Default::default()
			});
			return self.finish(ctx);
		}
		// Press on an existing point: drag it.
		if let Some(i) = self.curve.iter().position(|c| (c.0.0 - p.0).hypot(c.0.1 - p.1) <= reach) {
			self.grab = Some(Grab::Anchor(0, i));
			return ToolResult::default();
		}
		self.curve.push((p, false));
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}

	/// Direct Selection and the anchor tools: a press on the target path.
	fn edit_down(&mut self, ctx: &ToolContext<'_>, p: (f64, f64)) -> ToolResult {
		let shapes = matches!(self.kind, Kind::Direct | Kind::AnchorAdd | Kind::AnchorDelete | Kind::AnchorConvert);
		let Some((target, mut path)) = Self::target(ctx, shapes) else {
			return ToolResult::default();
		};
		let reach = self.reach();
		match self.kind {
			Kind::AnchorAdd => {
				if let Some((si, seg, t)) = path.segment_at(p, reach) {
					path.split(si, seg, t);
					return ToolResult {
						command: Some(Self::store(target, path, "Add Anchor Point")),
						redraw: true,
						..Default::default()
					};
				}
				ToolResult::default()
			}
			Kind::AnchorDelete => {
				if let Some((si, ai)) = path.anchor_at(p, reach) {
					path.subpaths[si].anchors.remove(ai);
					if path.subpaths[si].anchors.is_empty() {
						path.subpaths.remove(si);
					}
					return ToolResult {
						command: Some(Self::store(target, path, "Delete Anchor Point")),
						redraw: true,
						..Default::default()
					};
				}
				ToolResult::default()
			}
			Kind::AnchorConvert => {
				if let Some((si, ai)) = path.anchor_at(p, reach) {
					// A click makes a corner; a drag (see `edit_drag`) pulls
					// smooth handles.
					let a = &mut path.subpaths[si].anchors[ai];
					a.inh = a.pos;
					a.out = a.pos;
					a.smooth = false;
					self.editing = Some((target, path));
					self.grab = Some(Grab::Pull(si, ai));
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			_ => {
				// Direct Selection: the selected anchor's handles first, then
				// any anchor.
				if let Some((si, ai)) = self.selected
					&& let Some(a) = path.subpaths.get(si).and_then(|s| s.anchors.get(ai))
				{
					for (out, h) in [(true, a.out), (false, a.inh)] {
						if h != a.pos && (h.0 - p.0).hypot(h.1 - p.1) <= reach {
							self.grab = Some(Grab::Handle(si, ai, out));
							self.editing = Some((target, path));
							return ToolResult::default();
						}
					}
				}
				match path.anchor_at(p, reach) {
					Some((si, ai)) => {
						self.selected = Some((si, ai));
						self.grab = Some(Grab::Anchor(si, ai));
						if matches!(target, Target::Shape(_))
							&& !ctx
								.doc
								.layer(match target {
									Target::Shape(id) => id,
									_ => unreachable!(),
								})
								.is_some_and(|l| {
									matches!(
										&l.kind,
										LayerKind::Shape {
											shape: VectorShape::Path { .. },
											..
										}
									)
								}) {
							self.editing = Some((target, path));
							return ToolResult {
								info: Some("The live shape becomes a regular path".into()),
								redraw: true,
								..Default::default()
							};
						}
						self.editing = Some((target, path));
					}
					None => {
						self.selected = None;
						self.editing = Some((target, path));
					}
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
		}
	}

	fn edit_drag(&mut self, p: (f64, f64), modifiers: Modifiers) {
		let (Some(grab), Some((_, path))) = (self.grab, self.editing.as_mut()) else {
			return;
		};
		match grab {
			Grab::Anchor(si, ai) => {
				if let Some(a) = path.subpaths.get_mut(si).and_then(|s| s.anchors.get_mut(ai)) {
					let (dx, dy) = (p.0 - a.pos.0, p.1 - a.pos.1);
					a.pos = p;
					a.inh = (a.inh.0 + dx, a.inh.1 + dy);
					a.out = (a.out.0 + dx, a.out.1 + dy);
				}
			}
			Grab::Handle(si, ai, out) => {
				if let Some(a) = path.subpaths.get_mut(si).and_then(|s| s.anchors.get_mut(ai)) {
					let mirror = (2.0 * a.pos.0 - p.0, 2.0 * a.pos.1 - p.1);
					if out {
						a.out = p;
						if a.smooth && !modifiers.alt {
							a.inh = mirror;
						}
					} else {
						a.inh = p;
						if a.smooth && !modifiers.alt {
							a.out = mirror;
						}
					}
					if modifiers.alt {
						a.smooth = false;
					}
				}
			}
			Grab::Pull(si, ai) => {
				if let Some(a) = path.subpaths.get_mut(si).and_then(|s| s.anchors.get_mut(ai)) {
					*a = Anchor::smooth(a.pos, p);
				}
			}
		}
	}

	fn draw_path(path: &Path, items: &mut Vec<OverlayItem>) {
		for (points, closed, _) in path.flatten(0.5) {
			if points.len() >= 2 {
				items.push(OverlayItem::Polyline {
					points,
					closed,
					style: OverlayStyle::Xor,
				});
			}
		}
	}
}

impl Tool for Pen {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let p = (event.x, event.y);
		self.zoom = ctx.view.zoom;
		self.hover = Some(p);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let double = self
					.last_press
					.is_some_and(|(t, q)| event.time_us.saturating_sub(t) <= DOUBLE_CLICK_US && (q.0 - p.0).hypot(q.1 - p.1) <= self.reach());
				self.last_press = Some((event.time_us, p));
				self.button = true;
				match self.kind {
					Kind::Pen => self.pen_down(ctx, p, event.modifiers, double),
					Kind::Curvature => self.curvature_down(ctx, p, double),
					Kind::Freeform => {
						self.freehand = vec![p];
						ToolResult::default()
					}
					_ => self.edit_down(ctx, p),
				}
			}
			PointerKind::Move if self.button => {
				match self.kind {
					Kind::Pen => self.pen_drag(p, event.modifiers),
					Kind::Freeform => {
						if self.freehand.last().is_none_or(|q| (q.0 - p.0).hypot(q.1 - p.1) >= 0.5 / self.zoom.max(1e-6)) {
							self.freehand.push(p);
						}
					}
					Kind::Curvature => {
						if let Some(Grab::Anchor(_, i)) = self.grab
							&& let Some(c) = self.curve.get_mut(i)
						{
							c.0 = p;
						}
					}
					_ => self.edit_drag(p, event.modifiers),
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if self.button => {
				self.button = false;
				match self.kind {
					Kind::Freeform => {
						let pts = std::mem::take(&mut self.freehand);
						if pts.len() < 2 {
							return ToolResult::default();
						}
						let fit = ctx.settings.number(self.id, "Curve Fit").unwrap_or(2.0).clamp(0.5, 10.0);
						let simple = simplify(&pts, fit);
						let n = simple.len();
						let corners: Vec<bool> = (0..n)
							.map(|i| {
								if i == 0 || i + 1 == n {
									return true;
								}
								let (a, b, c) = (simple[i - 1], simple[i], simple[i + 1]);
								let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1));
								(u.0 * v.0 + u.1 * v.1) / (u.0.hypot(u.1) * v.0.hypot(v.1)).max(1e-9) < 0.3
							})
							.collect();
						self.drawing = Some(smooth_through(&simple, false, &corners));
						self.finish(ctx)
					}
					Kind::Curvature => {
						self.grab = None;
						ToolResult {
							redraw: true,
							..Default::default()
						}
					}
					Kind::Pen => ToolResult {
						redraw: true,
						..Default::default()
					},
					_ => {
						self.grab = None;
						match self.editing.take() {
							Some((target, path)) => ToolResult {
								command: Some(Self::store(
									target,
									path,
									match self.kind {
										Kind::AnchorConvert => "Convert Point",
										_ => "Drag Anchor",
									},
								)),
								redraw: true,
								..Default::default()
							},
							None => ToolResult::default(),
						}
					}
				}
			}
			_ => ToolResult {
				redraw: true,
				..Default::default()
			},
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		match key {
			"Enter" | "Escape" if self.drawing.is_some() || !self.curve.is_empty() => self.finish(ctx),
			"Delete" | "Backspace" if self.kind == Kind::Curvature && !self.curve.is_empty() => {
				self.curve.pop();
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			"Delete" | "Backspace" if self.kind == Kind::Direct => {
				let Some((si, ai)) = self.selected.take() else { return ToolResult::default() };
				let Some((target, mut path)) = Self::target(ctx, true) else {
					return ToolResult::default();
				};
				if let Some(s) = path.subpaths.get_mut(si)
					&& ai < s.anchors.len()
				{
					s.anchors.remove(ai);
				}
				ToolResult {
					command: Some(Self::store(target, path, "Delete Anchor Point")),
					redraw: true,
					..Default::default()
				}
			}
			"ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Shift+ArrowLeft" | "Shift+ArrowRight" | "Shift+ArrowUp" | "Shift+ArrowDown"
				if self.kind == Kind::Direct && self.selected.is_some() =>
			{
				let (si, ai) = self.selected.expect("checked");
				let step = if key.starts_with("Shift+") { 10.0 } else { 1.0 };
				let (dx, dy) = match key.trim_start_matches("Shift+") {
					"ArrowLeft" => (-step, 0.0),
					"ArrowRight" => (step, 0.0),
					"ArrowUp" => (0.0, -step),
					_ => (0.0, step),
				};
				let Some((target, mut path)) = Self::target(ctx, true) else {
					return ToolResult::default();
				};
				if let Some(a) = path.subpaths.get_mut(si).and_then(|s| s.anchors.get_mut(ai)) {
					for q in [&mut a.pos, &mut a.inh, &mut a.out] {
						q.0 += dx;
						q.1 += dy;
					}
				}
				ToolResult {
					command: Some(Self::store(target, path, "Nudge Anchor")),
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let mut items = Vec::new();
		// The path being edited, else nothing of the document (FAST: the
		// target path is drawn by the tool only while it edits it).
		if let Some((_, path)) = &self.editing {
			Self::draw_path(path, &mut items);
			for s in &path.subpaths {
				for a in &s.anchors {
					items.push(OverlayItem::Handle { at: a.pos, size_px: 6.0 });
				}
			}
		}
		if let Some((si, ai)) = self.selected
			&& let Some((_, path)) = &self.editing
			&& let Some(a) = path.subpaths.get(si).and_then(|s| s.anchors.get(ai))
		{
			for h in [a.inh, a.out] {
				if h != a.pos {
					items.push(OverlayItem::Polyline {
						points: vec![a.pos, h],
						closed: false,
						style: OverlayStyle::Xor,
					});
					items.push(OverlayItem::Crosshair { at: h });
				}
			}
		}
		if let Some(d) = &self.drawing {
			Self::draw_path(&Path { subpaths: vec![d.clone()] }, &mut items);
			for a in &d.anchors {
				items.push(OverlayItem::Handle { at: a.pos, size_px: 6.0 });
			}
			if let Some(a) = d.anchors.last() {
				if a.out != a.pos {
					items.push(OverlayItem::Polyline {
						points: vec![a.inh, a.pos, a.out],
						closed: false,
						style: OverlayStyle::Xor,
					});
				}
				// Rubber band: the next segment.
				if let Some(h) = self.hover
					&& !self.button
				{
					items.push(OverlayItem::Polyline {
						points: vec![a.pos, h],
						closed: false,
						style: OverlayStyle::Ants,
					});
				}
			}
		}
		if !self.curve.is_empty() {
			let pts: Vec<(f64, f64)> = self.curve.iter().map(|c| c.0).collect();
			let corners: Vec<bool> = self.curve.iter().map(|c| c.1).collect();
			let mut preview = pts.clone();
			if let Some(h) = self.hover
				&& !self.button
			{
				preview.push(h);
			}
			let mut corners2 = corners.clone();
			corners2.push(false);
			Self::draw_path(
				&Path {
					subpaths: vec![smooth_through(&preview, false, &corners2)],
				},
				&mut items,
			);
			for (p, corner) in &self.curve {
				items.push(if *corner {
					OverlayItem::Handle { at: *p, size_px: 6.0 }
				} else {
					OverlayItem::Crosshair { at: *p }
				});
			}
		}
		if self.freehand.len() >= 2 {
			items.push(OverlayItem::Polyline {
				points: self.freehand.clone(),
				closed: false,
				style: OverlayStyle::Xor,
			});
		}
		Some(Overlay { items })
	}

	fn activate(&mut self, doc: &fx_core::Document) {
		// Show the target path from the start (Direct Selection, anchor tools).
		if matches!(self.kind, Kind::Direct | Kind::AnchorAdd | Kind::AnchorDelete | Kind::AnchorConvert)
			&& let Some(t) = doc.active_path.or(doc.work_path.as_ref().map(|_| PathTarget::Work))
			&& let Some(p) = doc.path(t)
		{
			self.editing = Some((Target::Doc(t), p.clone()));
		}
	}

	fn deactivate(&mut self, ctx: &mut ToolContext<'_>) -> ToolResult {
		self.editing = None;
		self.selected = None;
		if self.drawing.is_some() || !self.curve.is_empty() {
			return self.finish(ctx);
		}
		ToolResult::default()
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		match self.kind {
			Kind::Direct => CursorShape::Default,
			_ => CursorShape::Crosshair,
		}
	}
}
