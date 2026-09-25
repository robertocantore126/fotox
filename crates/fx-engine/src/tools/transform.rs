//! Free Transform (Ctrl+T) and Edit ▸ Transform ▸ … (M6-T04).
//!
//! A [`Session`] is the box drawn over the layer (or over the selection's
//! bounds): its four corners, the reference point and, in Warp mode, the
//! 4 × 4 control points of a Bézier patch (D-053). It never touches the
//! document: the engine turns [`Session::mapping`] into a live preview on
//! the visible tiles and, on Enter or ✓, into one [`Command::Transform`].
//!
//! Gestures, as in Photoshop 2019+:
//!
//! * a corner scales proportionally about the opposite corner, Shift frees
//!   the proportions, Alt scales about the reference point;
//! * a side scales one axis (Shift: both);
//! * inside moves, outside rotates about the reference point (Shift: 15°
//!   steps); the reference point itself can be dragged;
//! * Ctrl + corner distorts (the corner moves freely), Ctrl + side moves that
//!   side, Ctrl+Shift + side skews along the side, Ctrl+Alt+Shift + corner
//!   is perspective (the corner and its neighbour move apart symmetrically);
//! * the Transform submenu's Scale / Rotate / Skew / Distort / Perspective
//!   make that gesture the default; Warp shows the patch's control points
//!   (drag one, or drag inside the surface to bend it where the pointer is).
//!
//! Every gesture is computed from the box as it was at the press, never from
//! the previous event, and a gesture that would fold the box is ignored.

use fx_core::transform::quad_is_convex;
use fx_core::{BezierPatch, Command, Filter, LayerId, LayerRef, Mapping};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::DocPointer;
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// A document point.
pub type Point = (f64, f64);

/// Screen pixels a press may miss a handle by and still take it.
const HANDLE_SLOP: f64 = 7.0;
/// The square handle, on screen.
const HANDLE_PX: f32 = 8.0;
/// The slops are screen distances; below this zoom they would be huge.
const MIN_ZOOM: f64 = 0.02;
/// Shift snaps a rotation to this many degrees.
const ROTATION_STEP: f64 = 15.0;

/// Which gesture a plain drag of a handle makes (the Transform submenu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
	/// Free Transform: every gesture through the modifiers.
	Free,
	Scale,
	Rotate,
	Skew,
	Distort,
	Perspective,
	Warp,
}

impl Mode {
	/// The mode an action starts (`xf:free`, `xf:scale` …).
	pub fn of_action(id: &str) -> Option<Mode> {
		Some(match id {
			"xf:free" => Mode::Free,
			"xf:scale" => Mode::Scale,
			"xf:rotate" => Mode::Rotate,
			"xf:skew" => Mode::Skew,
			"xf:distort" => Mode::Distort,
			"xf:perspective" => Mode::Perspective,
			"xf:warp" => Mode::Warp,
			_ => return None,
		})
	}
}

/// What an event did to the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
	/// Nothing to redraw.
	None,
	/// The box changed (`dragging` while a button is down: the engine
	/// previews at a coarser level until the pointer rests).
	Changed { dragging: bool },
	/// Only the overlay changed (a hover, the reference point).
	Redraw,
	/// Enter or ✓ with a real transform: apply this.
	Commit(Command),
	/// Escape, ✗, or Enter with nothing transformed.
	Cancel,
}

/// What a press took hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
	Corner(usize),
	/// Side `i` runs from corner `i` to corner `i + 1`.
	Side(usize),
	Reference,
	Inside,
	Outside,
	/// A Warp control point.
	WarpPoint(usize),
	/// Inside the warp surface at parameter `(u, v)`.
	WarpInside(f64, f64),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
	grab: Grab,
	start: Point,
	quad: [Point; 4],
	reference: Point,
	patch: Option<BezierPatch>,
	/// The modifiers at the press pick the gesture; Shift and Alt are read
	/// live, as in Photoshop.
	at_press: Modifiers,
}

/// A Free Transform in progress.
#[derive(Clone, Debug)]
pub struct Session {
	/// The layer being transformed.
	pub layer: LayerId,
	/// The source rectangle `[x0, y0, x1, y1]` in canvas pixels.
	rect: [f64; 4],
	/// The box: top-left, top-right, bottom-right, bottom-left.
	quad: [Point; 4],
	reference: Point,
	/// Warp's control points once Warp was chosen.
	patch: Option<BezierPatch>,
	mode: Mode,
	/// The option bar's interpolation.
	pub filter: Filter,
	drag: Option<Drag>,
	hover: Option<Point>,
	zoom: f64,
}

impl Session {
	/// A box over `rect` (`[x0, y0, x1, y1]`, canvas pixels) of `layer`.
	pub fn new(layer: LayerId, rect: [f64; 4], mode: Mode, filter: Filter) -> Self {
		let quad = corners_of(rect);
		let mut session = Self {
			layer,
			rect,
			quad,
			reference: ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0),
			patch: None,
			mode: Mode::Free,
			filter,
			drag: None,
			hover: None,
			zoom: 1.0,
		};
		session.set_mode(mode);
		session
	}

	/// Switch the default gesture (the Transform submenu while a box is up).
	/// Warp keeps the box's current shape as its starting surface.
	pub fn set_mode(&mut self, mode: Mode) {
		if mode == Mode::Warp && self.patch.is_none() {
			let mut patch = BezierPatch::rect(self.rect, self.rect);
			for i in 0..4 {
				for j in 0..4 {
					let (x, y) = bilinear(&self.quad, j as f64 / 3.0, i as f64 / 3.0);
					patch.set_point(i, j, x, y);
				}
			}
			self.patch = Some(patch);
		}
		self.mode = mode;
	}

	/// The source rectangle.
	pub fn rect(&self) -> [f64; 4] {
		self.rect
	}

	/// Source pixels → canvas pixels, or `None` while the box cannot be
	/// resampled (it never is: folding gestures are refused).
	pub fn mapping(&self) -> Option<Mapping> {
		match self.patch {
			Some(patch) => Some(Mapping::Warp(patch)),
			None => Mapping::from_quad(self.rect, self.quad),
		}
	}

	/// Whether the box is still the source rectangle (Enter then cancels).
	pub fn is_identity(&self) -> bool {
		match self.patch {
			Some(patch) => {
				let mut identity = BezierPatch::rect(self.rect, self.rect);
				identity.src_rect = patch.src_rect;
				patch
					.points
					.iter()
					.zip(identity.points.iter())
					.all(|(a, b)| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9)
			}
			None => self
				.quad
				.iter()
				.zip(corners_of(self.rect))
				.all(|(a, b)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9),
		}
	}

	/// Whether a button is down on the box.
	pub fn dragging(&self) -> bool {
		self.drag.is_some()
	}

	/// The status line: size in percent of the source and the angle of the
	/// top side (Photoshop's option bar numbers).
	pub fn status(&self) -> String {
		let [tl, tr, _, bl] = self.quad;
		let (w, h) = (self.rect[2] - self.rect[0], self.rect[3] - self.rect[1]);
		let width = 100.0 * distance(tl, tr) / w;
		let height = 100.0 * distance(tl, bl) / h;
		let angle = (tr.1 - tl.1).atan2(tr.0 - tl.0).to_degrees();
		format!("W: {width:.1}%  H: {height:.1}%  Angle: {angle:.1}°")
	}

	/// Turn or mirror the box about its reference point (Edit ▸ Transform ▸
	/// Rotate 90°, Flip… while the box is up). `linear` is a 2 × 2 matrix
	/// `[a, b, c, d]`: `x' = a·x + b·y`, `y' = c·x + d·y`.
	pub fn turn(&mut self, linear: [f64; 4]) {
		let r = self.reference;
		let apply = |p: Point| -> Point {
			let (dx, dy) = sub(p, r);
			(r.0 + linear[0] * dx + linear[1] * dy, r.1 + linear[2] * dx + linear[3] * dy)
		};
		self.quad = self.quad.map(apply);
		if let Some(patch) = &mut self.patch {
			for p in &mut patch.points {
				let (x, y) = apply((p[0], p[1]));
				*p = [x, y];
			}
		}
	}

	/// Move the whole box by whole pixels (the arrow keys).
	pub fn nudge(&mut self, dx: f64, dy: f64) {
		let shift = |p: Point| (p.0 + dx, p.1 + dy);
		self.quad = self.quad.map(shift);
		self.reference = shift(self.reference);
		if let Some(patch) = &mut self.patch {
			for p in &mut patch.points {
				p[0] += dx;
				p[1] += dy;
			}
		}
	}

	/// A pointer event in document coordinates.
	pub fn pointer(&mut self, event: &DocPointer, zoom: f64) -> Update {
		self.zoom = zoom.max(MIN_ZOOM);
		let pointer = (event.x, event.y);
		self.hover = Some(pointer);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let grab = self.grab_at(pointer);
				self.drag = Some(Drag {
					grab,
					start: pointer,
					quad: self.quad,
					reference: self.reference,
					patch: self.patch,
					at_press: event.modifiers,
				});
				Update::Redraw
			}
			PointerKind::Move | PointerKind::Down => {
				let Some(drag) = self.drag else {
					return Update::Redraw;
				};
				if event.buttons & BUTTON_LEFT == 0 {
					// The release was lost (the window lost the pointer).
					self.drag = None;
					return Update::Changed { dragging: false };
				}
				self.track(&drag, pointer, event.modifiers);
				Update::Changed { dragging: true }
			}
			PointerKind::Up => match self.drag.take() {
				Some(drag) => {
					self.track(&drag, pointer, event.modifiers);
					Update::Changed { dragging: false }
				}
				None => Update::None,
			},
			_ => Update::None,
		}
	}

	/// Enter commits, Escape cancels, the arrows nudge (Shift: 10 px).
	pub fn key(&mut self, key: &str) -> Update {
		let step = |n: f64| if key.starts_with("Shift+") { n * 10.0 } else { n };
		match key.trim_start_matches("Shift+") {
			"Enter" => self.commit(),
			"Escape" => Update::Cancel,
			"ArrowLeft" => {
				self.nudge(-step(1.0), 0.0);
				Update::Changed { dragging: false }
			}
			"ArrowRight" => {
				self.nudge(step(1.0), 0.0);
				Update::Changed { dragging: false }
			}
			"ArrowUp" => {
				self.nudge(0.0, -step(1.0));
				Update::Changed { dragging: false }
			}
			"ArrowDown" => {
				self.nudge(0.0, step(1.0));
				Update::Changed { dragging: false }
			}
			_ => Update::None,
		}
	}

	/// The command Enter applies, or a cancel when nothing changed.
	pub fn commit(&self) -> Update {
		match self.mapping() {
			Some(mapping) if !self.is_identity() => Update::Commit(Command::Transform {
				layer: LayerRef::Id(self.layer),
				mapping: Box::new(mapping),
				filter: self.filter,
			}),
			_ => Update::Cancel,
		}
	}

	/// The cursor for the pointer's position.
	pub fn cursor(&self) -> CursorShape {
		match self.hover.map(|p| self.grab_at(p)) {
			Some(Grab::Inside | Grab::WarpInside(..)) => CursorShape::Move,
			_ => CursorShape::Crosshair,
		}
	}

	/// The box, its handles and the reference point (or the warp's control
	/// points and surface).
	pub fn overlay(&self) -> Overlay {
		let mut items = Vec::new();
		match &self.patch {
			Some(patch) => {
				// The surface's outline and its thirds, sampled.
				for t in [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0] {
					let along_u: Vec<Point> = (0..=24).map(|k| fx_ops::resample::warp::evaluate(patch, k as f64 / 24.0, t)).collect();
					let along_v: Vec<Point> = (0..=24).map(|k| fx_ops::resample::warp::evaluate(patch, t, k as f64 / 24.0)).collect();
					for points in [along_u, along_v] {
						items.push(OverlayItem::Polyline {
							points,
							closed: false,
							style: OverlayStyle::Xor,
						});
					}
				}
				for p in &patch.points {
					items.push(OverlayItem::Handle {
						at: (p[0], p[1]),
						size_px: HANDLE_PX,
					});
				}
			}
			None => {
				items.push(OverlayItem::Polyline {
					points: self.quad.to_vec(),
					closed: true,
					style: OverlayStyle::Xor,
				});
				for i in 0..4 {
					items.push(OverlayItem::Handle {
						at: self.quad[i],
						size_px: HANDLE_PX,
					});
					items.push(OverlayItem::Handle {
						at: midpoint(self.quad[i], self.quad[(i + 1) % 4]),
						size_px: HANDLE_PX,
					});
				}
				items.push(OverlayItem::Crosshair { at: self.reference });
			}
		}
		Overlay { items }
	}

	fn grab_at(&self, p: Point) -> Grab {
		let slop = HANDLE_SLOP / self.zoom;
		let near = |q: Point| (p.0 - q.0).abs() <= slop && (p.1 - q.1).abs() <= slop;
		if let Some(patch) = &self.patch {
			if let Some(i) = (0..16).find(|&i| near((patch.points[i][0], patch.points[i][1]))) {
				return Grab::WarpPoint(i);
			}
			return match surface_parameter(patch, p, slop) {
				Some((u, v)) => Grab::WarpInside(u, v),
				None => Grab::Outside,
			};
		}
		if let Some(i) = (0..4).find(|&i| near(self.quad[i])) {
			return Grab::Corner(i);
		}
		if let Some(i) = (0..4).find(|&i| near(midpoint(self.quad[i], self.quad[(i + 1) % 4]))) {
			return Grab::Side(i);
		}
		if near(self.reference) {
			return Grab::Reference;
		}
		if inside(&self.quad, p) { Grab::Inside } else { Grab::Outside }
	}

	/// Apply one move of a drag.
	fn track(&mut self, drag: &Drag, p: Point, live: Modifiers) {
		let delta = (p.0 - drag.start.0, p.1 - drag.start.1);
		let press = drag.at_press;
		let mut quad = drag.quad;
		let mut reference = drag.reference;
		match drag.grab {
			Grab::WarpPoint(i) => {
				if let Some(mut patch) = drag.patch {
					patch.points[i][0] += delta.0;
					patch.points[i][1] += delta.1;
					self.patch = Some(patch);
				}
				return;
			}
			Grab::WarpInside(u, v) => {
				if let Some(mut patch) = drag.patch {
					// The points pull in proportion to their weight at the
					// pressed parameter, the strongest one fully.
					let (bu, bv) = (bernstein(u), bernstein(v));
					let max = (0..16).map(|k| bv[k / 4] * bu[k % 4]).fold(0.0, f64::max).max(1e-9);
					for (k, point) in patch.points.iter_mut().enumerate() {
						let w = bv[k / 4] * bu[k % 4] / max;
						point[0] += delta.0 * w;
						point[1] += delta.1 * w;
					}
					self.patch = Some(patch);
				}
				return;
			}
			Grab::Reference => {
				self.reference = (drag.reference.0 + delta.0, drag.reference.1 + delta.1);
				return;
			}
			// Outside a warp's surface there is nothing to take hold of.
			Grab::Outside if self.patch.is_some() => return,
			Grab::Inside if self.mode != Mode::Rotate => {
				quad = quad.map(|q| (q.0 + delta.0, q.1 + delta.1));
				reference = (reference.0 + delta.0, reference.1 + delta.1);
			}
			Grab::Inside | Grab::Outside => {
				let (a0, a1) = (angle(drag.reference, drag.start), angle(drag.reference, p));
				let mut turn = (a1 - a0).to_degrees();
				if live.shift {
					turn = (turn / ROTATION_STEP).round() * ROTATION_STEP;
				}
				quad = quad.map(|q| rotate(q, drag.reference, turn.to_radians()));
			}
			Grab::Corner(k) => {
				let perspective = self.mode == Mode::Perspective || (press.ctrl && press.alt && press.shift);
				let distort = self.mode == Mode::Distort || (self.mode != Mode::Scale && press.ctrl);
				if self.mode == Mode::Rotate {
					let turn = (angle(drag.reference, p) - angle(drag.reference, drag.start)).to_degrees();
					let turn = if live.shift { (turn / ROTATION_STEP).round() * ROTATION_STEP } else { turn };
					quad = quad.map(|q| rotate(q, drag.reference, turn.to_radians()));
				} else if perspective {
					let (h, v) = (k ^ 1, 3 - k);
					let dir_h = unit(sub(quad[h], quad[k]));
					let dir_v = unit(sub(quad[v], quad[k]));
					let (dh, dv) = (dot(delta, dir_h), dot(delta, dir_v));
					let (partner, dir, d) = if dh.abs() >= dv.abs() { (h, dir_h, dh) } else { (v, dir_v, dv) };
					quad[k] = add(quad[k], scale(dir, d));
					quad[partner] = add(quad[partner], scale(dir, -d));
				} else if distort {
					quad[k] = add(quad[k], delta);
				} else {
					// Scale in the box's own axes, about the opposite corner or
					// (Alt) the reference point.
					let opposite = quad[(k + 2) % 4];
					let anchor = if live.alt { drag.reference } else { opposite };
					let e1 = sub(quad[(k + 1) % 4], opposite);
					let e2 = sub(quad[(k + 3) % 4], opposite);
					let corner = quad[k];
					let (s, t) = if !live.shift {
						// Proportional (the default since Photoshop 2019): along
						// the diagonal.
						let d = sub(corner, anchor);
						let k = dot(sub(p, anchor), d) / dot(d, d).max(1e-12);
						(k, k)
					} else {
						axis_scales(anchor, e1, e2, corner, p)
					};
					match scaled(&quad, anchor, e1, e2, s, t) {
						Some(q) => {
							reference = scaled_point(reference, anchor, e1, e2, s, t);
							quad = q;
						}
						None => return,
					}
				}
			}
			Grab::Side(i) => {
				let (a, b) = (quad[i], quad[(i + 1) % 4]);
				let skew = self.mode == Mode::Skew || (press.ctrl && press.shift);
				let distort = self.mode == Mode::Distort || (press.ctrl && !press.shift);
				if skew {
					let dir = unit(sub(b, a));
					let d = scale(dir, dot(delta, dir));
					quad[i] = add(a, d);
					quad[(i + 1) % 4] = add(b, d);
					if live.alt {
						quad[(i + 2) % 4] = sub(quad[(i + 2) % 4], d);
						quad[(i + 3) % 4] = sub(quad[(i + 3) % 4], d);
					}
				} else if distort {
					quad[i] = add(a, delta);
					quad[(i + 1) % 4] = add(b, delta);
				} else {
					let opposite = midpoint(quad[(i + 2) % 4], quad[(i + 3) % 4]);
					let anchor = if live.alt { drag.reference } else { opposite };
					let normal = sub(midpoint(a, b), opposite);
					let along = sub(b, a);
					let (s, _) = axis_scales(anchor, normal, along, midpoint(a, b), p);
					let t = if live.shift { s } else { 1.0 };
					match scaled(&quad, anchor, normal, along, s, t) {
						Some(q) => {
							reference = scaled_point(reference, anchor, normal, along, s, t);
							quad = q;
						}
						None => return,
					}
				}
			}
		}
		if quad_is_convex(quad) {
			self.quad = quad;
			self.reference = reference;
		}
	}
}

/// The linear part of Edit ▸ Transform's instant items (`xf:rot180`,
/// `xf:rot90cw`, `xf:rot90ccw`, `xf:flip-h`, `xf:flip-v`) as `[a, b, c, d]`
/// (`x' = a·x + b·y`, `y' = c·x + d·y`, y down).
pub fn instant_turn(id: &str) -> Option<[f64; 4]> {
	Some(match id {
		"xf:rot180" => [-1.0, 0.0, 0.0, -1.0],
		"xf:rot90cw" => [0.0, -1.0, 1.0, 0.0],
		"xf:rot90ccw" => [0.0, 1.0, -1.0, 0.0],
		"xf:flip-h" => [-1.0, 0.0, 0.0, 1.0],
		"xf:flip-v" => [1.0, 0.0, 0.0, -1.0],
		_ => return None,
	})
}

/// The mapping of an instant turn about the centre of `rect`. The centre is
/// put on the half-pixel grid with equal fractions, so every pixel centre
/// lands on a pixel centre and the turn copies pixels exactly (the kernels
/// weigh a sample that falls on a centre 1, its neighbours 0).
pub fn turn_mapping(linear: [f64; 4], rect: [f64; 4]) -> Mapping {
	let half = |v: f64| (v * 2.0).round() / 2.0;
	let cx = half((rect[0] + rect[2]) / 2.0);
	let mut cy = half((rect[1] + rect[3]) / 2.0);
	if (cx.fract() - cy.fract()).abs() > 0.25 {
		cy += 0.5;
	}
	let [a, b, c, d] = linear;
	Mapping::Affine([a, c, b, d, cx - a * cx - b * cy, cy - c * cx - d * cy])
}

/// The corners of a rectangle `[x0, y0, x1, y1]`: TL, TR, BR, BL.
fn corners_of(rect: [f64; 4]) -> [Point; 4] {
	[(rect[0], rect[1]), (rect[2], rect[1]), (rect[2], rect[3]), (rect[0], rect[3])]
}

fn add(a: Point, b: Point) -> Point {
	(a.0 + b.0, a.1 + b.1)
}

fn sub(a: Point, b: Point) -> Point {
	(a.0 - b.0, a.1 - b.1)
}

fn scale(a: Point, k: f64) -> Point {
	(a.0 * k, a.1 * k)
}

fn dot(a: Point, b: Point) -> f64 {
	a.0 * b.0 + a.1 * b.1
}

fn distance(a: Point, b: Point) -> f64 {
	dot(sub(a, b), sub(a, b)).sqrt()
}

fn unit(a: Point) -> Point {
	let length = dot(a, a).sqrt();
	if length > 0.0 { scale(a, 1.0 / length) } else { (0.0, 0.0) }
}

fn midpoint(a: Point, b: Point) -> Point {
	((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

/// The direction from `centre` to `p`, radians.
fn angle(centre: Point, p: Point) -> f64 {
	(p.1 - centre.1).atan2(p.0 - centre.0)
}

/// `p` turned by `radians` (clockwise on screen) about `centre`.
fn rotate(p: Point, centre: Point, radians: f64) -> Point {
	let (sin, cos) = radians.sin_cos();
	let (dx, dy) = sub(p, centre);
	(centre.0 + dx * cos - dy * sin, centre.1 + dx * sin + dy * cos)
}

/// `p` in the basis `(e1, e2)` about `origin`.
fn coords(p: Point, origin: Point, e1: Point, e2: Point) -> Option<(f64, f64)> {
	let det = e1.0 * e2.1 - e2.0 * e1.1;
	if det.abs() < 1e-12 {
		return None;
	}
	let d = sub(p, origin);
	Some(((d.0 * e2.1 - d.1 * e2.0) / det, (e1.0 * d.1 - e1.1 * d.0) / det))
}

/// The scale along each axis that sends `from` to `to` about `anchor`.
fn axis_scales(anchor: Point, e1: Point, e2: Point, from: Point, to: Point) -> (f64, f64) {
	let (Some(a), Some(b)) = (coords(from, anchor, e1, e2), coords(to, anchor, e1, e2)) else {
		return (1.0, 1.0);
	};
	let ratio = |new: f64, old: f64| if old.abs() > 1e-9 { new / old } else { 1.0 };
	(ratio(b.0, a.0), ratio(b.1, a.1))
}

/// `p` scaled by `(s, t)` along `(e1, e2)` about `anchor`.
fn scaled_point(p: Point, anchor: Point, e1: Point, e2: Point, s: f64, t: f64) -> Point {
	match coords(p, anchor, e1, e2) {
		Some((a, b)) => add(anchor, add(scale(e1, a * s), scale(e2, b * t))),
		None => p,
	}
}

/// Every corner scaled; `None` when the box would collapse.
fn scaled(quad: &[Point; 4], anchor: Point, e1: Point, e2: Point, s: f64, t: f64) -> Option<[Point; 4]> {
	if !(s.is_finite() && t.is_finite()) || s.abs() < 1e-6 || t.abs() < 1e-6 {
		return None;
	}
	Some(quad.map(|q| scaled_point(q, anchor, e1, e2, s, t)))
}

/// A point of the box by bilinear interpolation of its corners.
fn bilinear(quad: &[Point; 4], u: f64, v: f64) -> Point {
	let top = add(scale(quad[0], 1.0 - u), scale(quad[1], u));
	let bottom = add(scale(quad[3], 1.0 - u), scale(quad[2], u));
	add(scale(top, 1.0 - v), scale(bottom, v))
}

/// Whether `p` is inside the convex quad.
fn inside(quad: &[Point; 4], p: Point) -> bool {
	let mut sign = 0.0;
	for i in 0..4 {
		let (a, b) = (quad[i], quad[(i + 1) % 4]);
		let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
		if cross == 0.0 {
			continue;
		}
		if sign == 0.0 {
			sign = cross.signum();
		} else if cross.signum() != sign {
			return false;
		}
	}
	true
}

/// The cubic Bernstein weights at `t`.
fn bernstein(t: f64) -> [f64; 4] {
	let s = 1.0 - t;
	[s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

/// The parameter of the surface point nearest to `p`, if one lies within
/// `slop` (a coarse search, then a finer one around the best cell).
fn surface_parameter(patch: &BezierPatch, p: Point, slop: f64) -> Option<(f64, f64)> {
	let search = |best: (f64, f64, f64), u0: f64, v0: f64, span: f64, steps: usize| {
		let mut found = best;
		for i in 0..=steps {
			for j in 0..=steps {
				let u = (u0 + span * (j as f64 / steps as f64 - 0.5)).clamp(0.0, 1.0);
				let v = (v0 + span * (i as f64 / steps as f64 - 0.5)).clamp(0.0, 1.0);
				let d = distance(fx_ops::resample::warp::evaluate(patch, u, v), p);
				if d < found.0 {
					found = (d, u, v);
				}
			}
		}
		found
	};
	let coarse = search((f64::INFINITY, 0.5, 0.5), 0.5, 0.5, 1.0, 32);
	let (d, u, v) = search(coarse, coarse.1, coarse.2, 2.0 / 32.0, 16);
	// Inside the surface the nearest point is under the pointer.
	(d <= slop.max(1.0)).then_some((u, v))
}

#[cfg(test)]
mod tests {
	use super::*;

	const RECT: [f64; 4] = [100.0, 100.0, 300.0, 200.0];

	fn session() -> Session {
		Session::new(LayerId(1), RECT, Mode::Free, Filter::Bicubic)
	}

	fn event(kind: PointerKind, p: Point, modifiers: Modifiers) -> DocPointer {
		DocPointer {
			kind,
			x: p.0,
			y: p.1,
			pressure: 1.0,
			tilt_x: 0.0,
			tilt_y: 0.0,
			buttons: if kind == PointerKind::Up { 0 } else { BUTTON_LEFT },
			modifiers,
			time_us: 0,
		}
	}

	fn drag(s: &mut Session, from: Point, to: Point, modifiers: Modifiers) {
		s.pointer(&event(PointerKind::Down, from, modifiers), 1.0);
		s.pointer(&event(PointerKind::Move, to, modifiers), 1.0);
		s.pointer(&event(PointerKind::Up, to, modifiers), 1.0);
	}

	fn close(a: Point, b: Point) -> bool {
		(a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
	}

	fn maps(s: &Session, from: Point, to: Point) {
		let got = s.mapping().expect("a mapping").forward_point(from.0, from.1).expect("finite");
		assert!(close(got, to), "{from:?} → {got:?}, want {to:?}");
	}

	#[test]
	fn a_corner_scales_proportionally_about_the_opposite_corner() {
		let mut s = session();
		// Dragging the bottom-right corner along the diagonal to twice the size.
		drag(&mut s, (300.0, 200.0), (500.0, 300.0), Modifiers::default());
		maps(&s, (100.0, 100.0), (100.0, 100.0));
		maps(&s, (300.0, 200.0), (500.0, 300.0));
		// Off the diagonal the proportions still hold.
		let mut s = session();
		drag(&mut s, (300.0, 200.0), (500.0, 200.0), Modifiers::default());
		let [tl, tr, _, bl] = s.quad;
		assert!(((tr.0 - tl.0) / (bl.1 - tl.1) - 2.0).abs() < 1e-9, "{:?}", s.quad);
		// Shift frees them.
		let mut s = session();
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		drag(&mut s, (300.0, 200.0), (500.0, 200.0), shift);
		maps(&s, (300.0, 200.0), (500.0, 200.0));
		maps(&s, (100.0, 200.0), (100.0, 200.0));
		assert!(s.status().starts_with("W: 200.0%  H: 100.0%"), "{}", s.status());
	}

	#[test]
	fn alt_scales_about_the_reference_point_and_a_side_scales_one_axis() {
		let mut s = session();
		let alt = Modifiers {
			alt: true,
			..Default::default()
		};
		// The right side dragged 50 px out with Alt: both sides move.
		drag(&mut s, (300.0, 150.0), (350.0, 150.0), alt);
		maps(&s, (100.0, 100.0), (50.0, 100.0));
		maps(&s, (300.0, 200.0), (350.0, 200.0));
		// Without Alt the left side stays.
		let mut s = session();
		drag(&mut s, (300.0, 150.0), (350.0, 150.0), Modifiers::default());
		maps(&s, (100.0, 100.0), (100.0, 100.0));
		maps(&s, (300.0, 200.0), (350.0, 200.0));
	}

	#[test]
	fn outside_rotates_about_the_reference_point_and_shift_snaps() {
		let mut s = session();
		// From straight right of the centre (200, 150) to straight below it.
		drag(&mut s, (400.0, 150.0), (200.0, 350.0), Modifiers::default());
		maps(&s, (200.0, 150.0), (200.0, 150.0));
		maps(&s, (300.0, 150.0), (200.0, 250.0));
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		let mut s = session();
		drag(&mut s, (400.0, 150.0), (400.0, 150.0 + 200.0 * 20f64.to_radians().tan()), shift);
		assert!(s.status().ends_with("Angle: 15.0°"), "{}", s.status());
		// Moving the reference point moves the pivot.
		let mut s = session();
		drag(&mut s, (200.0, 150.0), (100.0, 100.0), Modifiers::default());
		drag(&mut s, (400.0, 100.0), (100.0, 400.0), Modifiers::default());
		maps(&s, (100.0, 100.0), (100.0, 100.0));
	}

	#[test]
	fn inside_moves_and_the_arrows_nudge() {
		let mut s = session();
		drag(&mut s, (150.0, 150.0), (160.0, 145.0), Modifiers::default());
		maps(&s, (100.0, 100.0), (110.0, 95.0));
		assert!(matches!(s.key("Shift+ArrowRight"), Update::Changed { .. }));
		maps(&s, (100.0, 100.0), (120.0, 95.0));
		assert_eq!(s.cursor(), CursorShape::Move, "the pointer rests inside");
	}

	#[test]
	fn ctrl_distorts_skews_and_makes_perspective() {
		let ctrl = Modifiers {
			ctrl: true,
			..Default::default()
		};
		let mut s = session();
		drag(&mut s, (300.0, 200.0), (330.0, 240.0), ctrl);
		assert!(matches!(s.mapping(), Some(Mapping::Projective(_))), "a distorted box is a homography");
		maps(&s, (300.0, 200.0), (330.0, 240.0));
		maps(&s, (300.0, 100.0), (300.0, 100.0));

		// Ctrl+Shift on the top side slides it along itself: a skew.
		let skew = Modifiers {
			ctrl: true,
			shift: true,
			..Default::default()
		};
		let mut s = session();
		drag(&mut s, (200.0, 100.0), (240.0, 80.0), skew);
		maps(&s, (100.0, 100.0), (140.0, 100.0));
		maps(&s, (100.0, 200.0), (100.0, 200.0));
		assert!(matches!(s.mapping(), Some(Mapping::Affine(_))));

		// Ctrl+Alt+Shift on a corner: its neighbour moves the other way.
		let perspective = Modifiers {
			ctrl: true,
			alt: true,
			shift: true,
			space: false,
		};
		let mut s = session();
		drag(&mut s, (100.0, 100.0), (130.0, 100.0), perspective);
		maps(&s, (100.0, 100.0), (130.0, 100.0));
		maps(&s, (300.0, 100.0), (270.0, 100.0));
		maps(&s, (300.0, 200.0), (300.0, 200.0));
	}

	#[test]
	fn instant_turns_land_pixel_centres_on_pixel_centres() {
		for id in ["xf:rot180", "xf:rot90cw", "xf:rot90ccw", "xf:flip-h", "xf:flip-v"] {
			let linear = instant_turn(id).expect("an instant item");
			// Odd and even sizes, odd and even offsets.
			for rect in [[0.0, 0.0, 100.0, 60.0], [3.0, 7.0, 104.0, 58.0], [10.0, 10.0, 11.0, 12.0]] {
				let mapping = turn_mapping(linear, rect);
				for (x, y) in [(3.5, 7.5), (50.5, 20.5), (0.5, 0.5)] {
					let (u, v) = mapping.forward_point(x, y).expect("finite");
					assert!(
						(u - 0.5).fract().abs() < 1e-9 && (v - 0.5).fract().abs() < 1e-9,
						"{id} {rect:?}: ({x}, {y}) → ({u}, {v})"
					);
				}
			}
		}
		// A quarter turn clockwise about (50, 30): the right-middle goes below.
		let mapping = turn_mapping(instant_turn("xf:rot90cw").expect("known"), [0.0, 0.0, 100.0, 60.0]);
		let (u, v) = mapping.forward_point(100.0, 30.0).expect("finite");
		assert!((u - 50.0).abs() < 1e-9 && (v - 80.0).abs() < 1e-9, "({u}, {v})");
		// The same turn on a box that is up turns the box.
		let mut s = session();
		s.turn(instant_turn("xf:rot180").expect("known"));
		maps(&s, (100.0, 100.0), (300.0, 200.0));
	}

	#[test]
	fn a_folding_gesture_is_ignored() {
		let ctrl = Modifiers {
			ctrl: true,
			..Default::default()
		};
		let mut s = session();
		// The top-left corner dragged past the bottom-right one.
		drag(&mut s, (100.0, 100.0), (400.0, 300.0), ctrl);
		assert!(s.is_identity(), "the box stays as it was");
		assert_eq!(s.commit(), Update::Cancel, "nothing to apply");
	}

	#[test]
	fn enter_commits_one_transform_and_escape_cancels() {
		let mut s = session();
		drag(&mut s, (150.0, 150.0), (170.0, 150.0), Modifiers::default());
		match s.key("Enter") {
			Update::Commit(Command::Transform { layer, mapping, filter }) => {
				assert_eq!(layer, LayerRef::Id(LayerId(1)));
				assert_eq!(*mapping, Mapping::translation(20.0, 0.0));
				assert_eq!(filter, Filter::Bicubic);
			}
			other => panic!("expected a commit, got {other:?}"),
		}
		assert_eq!(s.key("Escape"), Update::Cancel);
	}

	#[test]
	fn warp_starts_from_the_box_and_bends_where_it_is_dragged() {
		let mut s = session();
		drag(&mut s, (150.0, 150.0), (160.0, 150.0), Modifiers::default());
		s.set_mode(Mode::Warp);
		let Some(Mapping::Warp(patch)) = s.mapping() else { panic!("a warp mapping") };
		assert!(close((patch.points[0][0], patch.points[0][1]), (110.0, 100.0)), "the moved box's corner");
		assert!(close((patch.points[15][0], patch.points[15][1]), (310.0, 200.0)));
		// A control point follows the pointer.
		drag(&mut s, (110.0, 100.0), (90.0, 80.0), Modifiers::default());
		let Some(Mapping::Warp(patch)) = s.mapping() else { panic!("a warp mapping") };
		assert!(close((patch.points[0][0], patch.points[0][1]), (90.0, 80.0)));
		// A drag in the middle bends the middle points most.
		let before = patch;
		let centre = fx_ops::resample::warp::evaluate(&patch, 0.5, 0.5);
		drag(&mut s, centre, (centre.0, centre.1 + 30.0), Modifiers::default());
		let Some(Mapping::Warp(after)) = s.mapping() else { panic!("a warp mapping") };
		let moved = |k: usize| after.points[k][1] - before.points[k][1];
		assert!(moved(5) > 29.0 && moved(10) > 29.0, "the inner points follow: {} {}", moved(5), moved(10));
		assert!(moved(0).abs() < 5.0, "a far corner barely moves: {}", moved(0));
		assert!(!s.is_identity());
	}
}
