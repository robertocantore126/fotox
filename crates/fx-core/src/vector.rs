//! Vector shapes for shape layers (M6-T06).
//!
//! A shape is *geometry*, not pixels: a [`VectorShape`] is an outline in its
//! own local space, placed on the canvas by the layer's affine `transform`,
//! and the layer's tiles are rendered from it on demand at every level (D-050,
//! the tiny-skia renderer in `fx-render::vector`). Editing a shape therefore
//! never loses quality and never touches tiles (D-055).
//!
//! Local space: every shape is described inside the box `[0, w] × [0, h]`
//! returned by [`VectorShape::bounds`], with the origin at the box's top-left.
//! The affine `transform` is `[a, b, c, d, e, f]` and maps a local point to
//! document pixels the way a PDF/PostScript matrix does:
//!
//! ```text
//! x' = a·x + c·y + e
//! y' = b·x + d·y + f
//! ```
//!
//! (`[1, 0, 0, 1, 0, 0]` is the identity.) The types are plain data so they
//! serialise into the document, the macros and the UI protocol.

use serde::{Deserialize, Serialize};

/// The identity `[a, b, c, d, e, f]`: local coordinates are document
/// coordinates. Starting point of every shape and text layer's placement.
pub const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// One element of a path, in shape-local coordinates — the element set PSD
/// stores for a vector path.
///
/// Serde's default (externally tagged) form: `{"move_to": [x, y]}`,
/// `"close"`. A tag would not fit the newtype variants.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathEl {
	MoveTo([f64; 2]),
	LineTo([f64; 2]),
	/// Control point, end point.
	QuadTo([f64; 2], [f64; 2]),
	/// Control point 1, control point 2, end point.
	CubicTo([f64; 2], [f64; 2], [f64; 2]),
	Close,
}

/// A shape's geometry. `radii` and `star_inset` are the Photoshop tool
/// options; see each variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VectorShape {
	/// A rectangle `w × h` with up to four rounded corners. `radii` is
	/// Photoshop's order: top-left, top-right, bottom-right, bottom-left, in
	/// local pixels, each clamped to half of the shorter side.
	Rect { w: f64, h: f64, radii: [f64; 4] },
	/// An ellipse that fills its local box.
	Ellipse { w: f64, h: f64 },
	/// A regular polygon of `sides` vertices (3..=100), circumradius 1,
	/// centred in its 2 × 2 local box and pointing up, like Photoshop's tool.
	/// `star_inset` above 0 makes a star of `2 × sides` vertices: the odd ones
	/// sit on the circumradius, the even ones at `star_inset` of it (0..=1).
	Polygon { sides: u32, star_inset: f64 },
	/// A straight line of `length` local pixels and `width` pixels of
	/// thickness. Fotox keeps the Line tool's outline as a filled bar of
	/// length × width (butt caps), so a line is an ordinary filled shape;
	/// Photoshop's arrowheads and rounded caps come later.
	Line { length: f64, width: f64 },
	/// A free path (PSD import, the Pen tool later).
	Path { elements: Vec<PathEl> },
	/// An isosceles triangle pointing up in its `w × h` box, corners rounded
	/// by `radius` local pixels (the Triangle tool, M10-T07).
	Triangle { w: f64, h: f64, radius: f64 },
}

/// How a shape's paint is applied. Gradients and patterns follow.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Paint {
	/// Straight 16-bit RGBA, like every other colour in the document.
	Solid { rgba: [u16; 4] },
}

impl Paint {
	/// The paint as straight 16-bit RGBA, the document's colour unit.
	pub fn rgba(self) -> [u16; 4] {
		match self {
			Paint::Solid { rgba } => rgba,
		}
	}

	/// The paint as straight 0..=1 RGB with alpha, for the renderer.
	pub fn rgba_f32(self) -> [f32; 4] {
		self.rgba().map(|v| v as f32 / 65535.0)
	}
}

/// Where a stroke sits relative to the outline (Photoshop's Align).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeAlign {
	Inside,
	Center,
	Outside,
}

/// A shape layer's stroke: nothing here is a pixel, the renderer builds the
/// band from the geometry at the level it is drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
	/// Stroke thickness in document pixels (level 0).
	pub width: f64,
	pub align: StrokeAlign,
	pub paint: Paint,
	/// Dash pattern in document pixels, `None` = solid.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub dash: Option<Vec<f64>>,
}

/// Quarter-circle constant: the control points of a 90° cubic arc sit at
/// `kappa · radius` from the corner.
const KAPPA: f64 = 0.552_284_749_830_793_4;

impl VectorShape {
	/// The size of the shape's local box.
	pub fn bounds(&self) -> (f64, f64) {
		match self {
			VectorShape::Rect { w, h, .. } | VectorShape::Ellipse { w, h } => (w.abs(), h.abs()),
			VectorShape::Polygon { .. } => (2.0, 2.0),
			VectorShape::Line { length, width } => (length.abs(), width.abs()),
			VectorShape::Triangle { w, h, .. } => (w.abs(), h.abs()),
			VectorShape::Path { elements } => path_bounds(elements),
		}
	}

	/// The outline in local space, origin at the top-left of [`bounds`].
	///
	/// [`bounds`]: Self::bounds
	pub fn outline(&self) -> Vec<PathEl> {
		match self {
			VectorShape::Rect { w, h, radii } => rounded_rect(w.abs(), h.abs(), *radii),
			VectorShape::Ellipse { w, h } => ellipse(w.abs(), h.abs()),
			VectorShape::Polygon { sides, star_inset } => polygon(*sides, *star_inset),
			VectorShape::Line { length, width } => rounded_rect(length.abs(), width.abs(), [0.0; 4]),
			VectorShape::Triangle { w, h, radius } => triangle(w.abs(), h.abs(), *radius),
			VectorShape::Path { elements } => elements.clone(),
		}
	}

	/// Whether the local point `(x, y)` is inside the shape.
	///
	/// A hit test, not a renderer: curves are flattened to their control
	/// polygon and the test is the even-odd crossing rule, which is accurate
	/// enough to pick a shape by clicking it (M6-T06, the Path Selection tool).
	pub fn contains(&self, x: f64, y: f64) -> bool {
		let (bx0, by0, bx1, by1) = self.local_box();
		if x < bx0 || y < by0 || x > bx1 || y > by1 {
			return false;
		}
		let polygon = flatten(&self.outline());
		let mut inside = false;
		for i in 0..polygon.len() {
			let (ax, ay) = polygon[i];
			let (bx, by) = polygon[(i + 1) % polygon.len()];
			if (ay > y) != (by > y) {
				let t = (y - ay) / (by - ay);
				if x < ax + t * (bx - ax) {
					inside = !inside;
				}
			}
		}
		inside
	}

	/// The local box as a rectangle (see [`bounds`](Self::bounds)).
	fn local_box(&self) -> (f64, f64, f64, f64) {
		let (w, h) = self.bounds();
		match self {
			VectorShape::Path { elements } => {
				let mut x0 = f64::INFINITY;
				let mut y0 = f64::INFINITY;
				for element in elements {
					for p in points_of(*element) {
						x0 = x0.min(p[0]);
						y0 = y0.min(p[1]);
					}
				}
				if x0.is_finite() && y0.is_finite() {
					(x0, y0, x0 + w, y0 + h)
				} else {
					(0.0, 0.0, 0.0, 0.0)
				}
			}
			_ => (0.0, 0.0, w, h),
		}
	}

	/// The default layer name Photoshop gives a new shape of this kind.
	pub fn stem(&self) -> &'static str {
		match self {
			VectorShape::Rect { radii, .. } => {
				if radii.iter().any(|r| *r > 0.0) {
					"Rounded Rectangle"
				} else {
					"Rectangle"
				}
			}
			VectorShape::Ellipse { .. } => "Ellipse",
			VectorShape::Polygon { star_inset, .. } => {
				if *star_inset > 0.0 {
					"Star"
				} else {
					"Polygon"
				}
			}
			VectorShape::Line { .. } => "Line",
			VectorShape::Path { .. } => "Shape",
			VectorShape::Triangle { .. } => "Triangle",
		}
	}
}

/// The document-space box `[x0, y0, x1, y1]` a shape occupies through
/// `transform`: the corners of its local box, mapped and made into a box. A
/// rotated or skewed transform makes it a bound rather than the outline, which
/// is what the tile cache needs to know what to redraw (M6-T06).
///
/// A stroke reaches outside the outline: inflate the box by the stroke width
/// with [`grow_box`] when the shape has one.
pub fn document_box(shape: &VectorShape, transform: [f64; 6]) -> [f64; 4] {
	let (w, h) = shape.bounds();
	let (a, b, c, d, e, f) = (transform[0], transform[1], transform[2], transform[3], transform[4], transform[5]);
	let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
	let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
	for (x, y) in [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)] {
		let (px, py) = (a * x + c * y + e, b * x + d * y + f);
		x0 = x0.min(px);
		y0 = y0.min(py);
		x1 = x1.max(px);
		y1 = y1.max(py);
	}
	if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
		return [0.0, 0.0, 0.0, 0.0];
	}
	[x0, y0, x1, y1]
}

/// A box grown by `amount` on every side.
pub fn grow_box(box_: [f64; 4], amount: f64) -> [f64; 4] {
	[box_[0] - amount, box_[1] - amount, box_[2] + amount, box_[3] + amount]
}

/// The end and control points of one element.
fn points_of(element: PathEl) -> Vec<[f64; 2]> {
	match element {
		PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
		PathEl::QuadTo(c, p) => vec![c, p],
		PathEl::CubicTo(c1, c2, p) => vec![c1, c2, p],
		PathEl::Close => Vec::new(),
	}
}

/// The outline as a polygon: the anchor points, with every curve's control
/// points walked as straight segments. Good enough for a hit test.
fn flatten(elements: &[PathEl]) -> Vec<(f64, f64)> {
	let mut out: Vec<(f64, f64)> = Vec::new();
	let mut push = |p: [f64; 2]| {
		let point = (p[0], p[1]);
		if out.last() != Some(&point) {
			out.push(point);
		}
	};
	for element in elements {
		for p in points_of(*element) {
			push(p);
		}
	}
	out
}

/// The local box a path fills.
fn path_bounds(elements: &[PathEl]) -> (f64, f64) {
	let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
	let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
	let mut include = |p: [f64; 2]| {
		x0 = x0.min(p[0]);
		y0 = y0.min(p[1]);
		x1 = x1.max(p[0]);
		y1 = y1.max(p[1]);
	};
	for element in elements {
		match *element {
			PathEl::MoveTo(p) | PathEl::LineTo(p) => include(p),
			PathEl::QuadTo(_, p) | PathEl::CubicTo(_, _, p) => include(p),
			PathEl::Close => {}
		}
	}
	if !x0.is_finite() || !y0.is_finite() {
		return (0.0, 0.0);
	}
	((x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// A rectangle with per-corner radii (Photoshop's order), clamped so opposite
/// corners never overlap.
fn rounded_rect(w: f64, h: f64, radii: [f64; 4]) -> Vec<PathEl> {
	let limit = (w / 2.0).min(h / 2.0);
	let r: [f64; 4] = radii.map(|v| v.clamp(0.0, limit));
	let [tl, tr, br, bl] = r;
	if r.iter().all(|v| *v <= 0.0) {
		return vec![
			PathEl::MoveTo([0.0, 0.0]),
			PathEl::LineTo([w, 0.0]),
			PathEl::LineTo([w, h]),
			PathEl::LineTo([0.0, h]),
			PathEl::Close,
		];
	}
	let mut out = Vec::with_capacity(10);
	out.push(PathEl::MoveTo([tl, 0.0]));
	// Top edge, right.
	out.push(PathEl::LineTo([w - tr, 0.0]));
	if tr > 0.0 {
		out.push(PathEl::CubicTo([w - tr + tr * KAPPA, 0.0], [w, tr - tr * KAPPA], [w, tr]));
	}
	out.push(PathEl::LineTo([w, h - br]));
	if br > 0.0 {
		out.push(PathEl::CubicTo([w, h - br + br * KAPPA], [w - br + br * KAPPA, h], [w - br, h]));
	}
	out.push(PathEl::LineTo([bl, h]));
	if bl > 0.0 {
		out.push(PathEl::CubicTo([bl - bl * KAPPA, h], [0.0, h - bl + bl * KAPPA], [0.0, h - bl]));
	}
	out.push(PathEl::LineTo([0.0, tl]));
	if tl > 0.0 {
		out.push(PathEl::CubicTo([0.0, tl - tl * KAPPA], [tl - tl * KAPPA, 0.0], [tl, 0.0]));
	}
	out.push(PathEl::Close);
	out
}

/// Four cubic arcs around the ellipse that fills `w × h`.
/// A triangle pointing up with corners rounded by `radius` (M10-T07): each
/// corner is cut back along both edges and joined with a quadratic through
/// the corner.
fn triangle(w: f64, h: f64, radius: f64) -> Vec<PathEl> {
	let pts = [(w / 2.0, 0.0), (w, h), (0.0, h)];
	let r = radius.max(0.0);
	if r <= 0.0 {
		return vec![
			PathEl::MoveTo([pts[0].0, pts[0].1]),
			PathEl::LineTo([pts[1].0, pts[1].1]),
			PathEl::LineTo([pts[2].0, pts[2].1]),
			PathEl::Close,
		];
	}
	let toward = |a: (f64, f64), b: (f64, f64)| {
		let (dx, dy) = (b.0 - a.0, b.1 - a.1);
		let len = dx.hypot(dy).max(1e-9);
		let t = (r / len).min(0.5);
		[a.0 + dx * t, a.1 + dy * t]
	};
	let mut out = Vec::new();
	for i in 0..3 {
		let (prev, cur, next) = (pts[(i + 2) % 3], pts[i], pts[(i + 1) % 3]);
		let (a, b) = (toward(cur, prev), toward(cur, next));
		out.push(if i == 0 { PathEl::MoveTo(a) } else { PathEl::LineTo(a) });
		out.push(PathEl::QuadTo([cur.0, cur.1], b));
	}
	out.push(PathEl::Close);
	out
}

fn ellipse(w: f64, h: f64) -> Vec<PathEl> {
	let (rx, ry) = (w / 2.0, h / 2.0);
	let (cx, cy) = (rx, ry);
	let (kx, ky) = (rx * KAPPA, ry * KAPPA);
	vec![
		PathEl::MoveTo([cx, cy - ry]),
		PathEl::CubicTo([cx + kx, cy - ry], [cx + rx, cy - ky], [cx + rx, cy]),
		PathEl::CubicTo([cx + rx, cy + ky], [cx + kx, cy + ry], [cx, cy + ry]),
		PathEl::CubicTo([cx - kx, cy + ry], [cx - rx, cy + ky], [cx - rx, cy]),
		PathEl::CubicTo([cx - rx, cy - ky], [cx - kx, cy - ry], [cx, cy - ry]),
		PathEl::Close,
	]
}

/// A regular polygon (or star) of unit circumradius in its 2 × 2 local box,
/// first vertex at the top.
fn polygon(sides: u32, star_inset: f64) -> Vec<PathEl> {
	let sides = sides.clamp(3, 100);
	let inset = star_inset.clamp(0.0, 1.0);
	let vertices = if inset > 0.0 { sides * 2 } else { sides };
	let mut out = Vec::with_capacity(vertices as usize + 1);
	for i in 0..vertices {
		// -90° is the top, and the step is half of one arm on a star.
		let angle = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * (i as f64) / (vertices as f64);
		let radius = if inset > 0.0 && i % 2 == 1 { inset } else { 1.0 };
		let point = [1.0 + radius * angle.cos(), 1.0 + radius * angle.sin()];
		if i == 0 {
			out.push(PathEl::MoveTo(point));
		} else {
			out.push(PathEl::LineTo(point));
		}
	}
	out.push(PathEl::Close);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The shapes the tools draw, with the local box each one should report.
	fn tool_shapes() -> Vec<(VectorShape, (f64, f64))> {
		vec![
			(
				VectorShape::Rect {
					w: 100.0,
					h: 40.0,
					radii: [0.0; 4],
				},
				(100.0, 40.0),
			),
			(
				VectorShape::Rect {
					w: 100.0,
					h: 40.0,
					radii: [10.0; 4],
				},
				(100.0, 40.0),
			),
			(VectorShape::Ellipse { w: 60.0, h: 60.0 }, (60.0, 60.0)),
			(VectorShape::Polygon { sides: 5, star_inset: 0.0 }, (2.0, 2.0)),
			(VectorShape::Polygon { sides: 5, star_inset: 0.5 }, (2.0, 2.0)),
			(VectorShape::Line { length: 100.0, width: 6.0 }, (100.0, 6.0)),
		]
	}

	#[test]
	fn every_outline_stays_inside_the_box_it_reports() {
		for (shape, (w, h)) in tool_shapes() {
			assert_eq!(shape.bounds(), (w, h), "{shape:?} reports its own box");
			for element in shape.outline() {
				for point in points_of(element) {
					assert!(
						point[0] >= -1e-9 && point[0] <= w + 1e-9 && point[1] >= -1e-9 && point[1] <= h + 1e-9,
						"{shape:?} has {point:?} outside its {w} × {h} box"
					);
				}
			}
		}
	}

	#[test]
	fn a_polygon_fits_its_local_box_and_points_up() {
		let shape = VectorShape::Polygon { sides: 6, star_inset: 0.0 };
		// The top vertex is the middle of the box's top edge.
		assert!(shape.contains(1.0, 0.01), "just under the top vertex");
		assert!(!shape.contains(0.05, 0.05), "the box's corner is outside the polygon");
		assert!(shape.contains(1.0, 1.0), "the centre is inside");
		// A hexagon with a vertex at the top has one at the bottom too.
		assert!(shape.contains(1.0, 1.99), "just above the bottom vertex");
	}

	#[test]
	fn contains_agrees_with_the_box_and_the_centre() {
		for (shape, (w, h)) in tool_shapes() {
			assert!(shape.contains(w / 2.0, h / 2.0), "the centre of {shape:?} is inside it");
			assert!(!shape.contains(-1.0, h / 2.0), "and a point left of the box is not");
			assert!(!shape.contains(w / 2.0, h + 1.0), "nor one below it");
		}
		// A rectangle's corners are inside, an ellipse's are not.
		assert!(
			VectorShape::Rect {
				w: 100.0,
				h: 40.0,
				radii: [0.0; 4]
			}
			.contains(0.5, 0.5)
		);
		assert!(!VectorShape::Ellipse { w: 60.0, h: 60.0 }.contains(0.5, 0.5));
	}

	#[test]
	fn rounded_corners_are_clamped_to_half_the_shorter_side() {
		let shape = VectorShape::Rect {
			w: 100.0,
			h: 40.0,
			radii: [1000.0; 4],
		};
		// Clamped to 20 (half of 40): the corner arc's centre is (20, 20).
		assert!(!shape.contains(0.5, 0.5), "a 20 px radius cuts the corner");
		assert!(shape.contains(20.0, 20.0), "the arc's centre is inside");
		assert!(shape.contains(50.0, 20.0), "and so is the middle");
	}

	#[test]
	fn document_box_is_the_local_box_through_the_matrix() {
		let shape = VectorShape::Rect {
			w: 100.0,
			h: 40.0,
			radii: [0.0; 4],
		};
		assert_eq!(document_box(&shape, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), [0.0, 0.0, 100.0, 40.0]);
		assert_eq!(document_box(&shape, [1.0, 0.0, 0.0, 1.0, 30.0, 50.0]), [30.0, 50.0, 130.0, 90.0]);
		// The Polygon tool scales the 2 × 2 local box by half the drag's size.
		let polygon = VectorShape::Polygon { sides: 5, star_inset: 0.0 };
		assert_eq!(document_box(&polygon, [100.0, 0.0, 0.0, 50.0, 10.0, 10.0]), [10.0, 10.0, 210.0, 110.0]);
		// A quarter turn about the origin swaps the axes, so the box becomes a
		// bound rather than the outline.
		assert_eq!(document_box(&shape, [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]), [-40.0, 0.0, 0.0, 100.0]);
	}

	#[test]
	fn grow_box_inflates_every_side() {
		assert_eq!(grow_box([10.0, 20.0, 30.0, 40.0], 5.0), [5.0, 15.0, 35.0, 45.0]);
		assert_eq!(grow_box([10.0, 20.0, 30.0, 40.0], 0.0), [10.0, 20.0, 30.0, 40.0]);
	}

	#[test]
	fn a_shape_stem_names_the_layer() {
		assert_eq!(
			VectorShape::Rect {
				w: 1.0,
				h: 1.0,
				radii: [0.0; 4]
			}
			.stem(),
			"Rectangle"
		);
		assert_eq!(
			VectorShape::Rect {
				w: 1.0,
				h: 1.0,
				radii: [0.0, 2.0, 0.0, 0.0]
			}
			.stem(),
			"Rounded Rectangle"
		);
		assert_eq!(VectorShape::Ellipse { w: 1.0, h: 1.0 }.stem(), "Ellipse");
		assert_eq!(VectorShape::Polygon { sides: 5, star_inset: 0.0 }.stem(), "Polygon");
		assert_eq!(VectorShape::Polygon { sides: 5, star_inset: 0.5 }.stem(), "Star");
		assert_eq!(VectorShape::Line { length: 1.0, width: 1.0 }.stem(), "Line");
		assert_eq!(VectorShape::Path { elements: Vec::new() }.stem(), "Shape");
	}

	#[test]
	fn paints_convert_to_the_units_each_side_needs() {
		let red = Paint::Solid { rgba: [65_535, 0, 0, 32_768] };
		assert_eq!(red.rgba(), [65_535, 0, 0, 32_768]);
		// 32 768 / 65 535 is 0.5 within a 16-bit step.
		let f = red.rgba_f32();
		assert_eq!(&f[..3], &[1.0, 0.0, 0.0]);
		assert!((f[3] - 0.5).abs() < 1.0 / 65_535.0, "{f:?}");
	}

	#[test]
	fn shapes_survive_a_json_round_trip() {
		// A shape is document data: it goes into manifests and the UI protocol,
		// so its JSON form is part of the file format.
		for (shape, _) in tool_shapes() {
			let json = serde_json::to_string(&shape).expect("serialises");
			let back: VectorShape = serde_json::from_str(&json).expect("parses");
			assert_eq!(back, shape, "{json}");
		}
		let stroke = StrokeStyle {
			width: 3.0,
			align: StrokeAlign::Outside,
			paint: Paint::Solid { rgba: [1, 2, 3, 4] },
			dash: Some(vec![4.0, 2.0]),
		};
		let back: StrokeStyle = serde_json::from_str(&serde_json::to_string(&stroke).expect("serialises")).expect("parses");
		assert_eq!(back, stroke);
	}
}
