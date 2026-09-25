//! Viewport overlays (M5-T02): everything a tool draws over the image —
//! marching ants, a lasso in progress, the brush outline, handles, a clone
//! source crosshair.
//!
//! [`Overlay`] is described in **document coordinates**; [`tessellate`] is the
//! pure, tested step that turns it into screen-space triangles for the
//! viewport pass (`gpu/viewport.rs`, `gpu/overlay.wgsl`). The render thread
//! runs it every frame, so a hover (which only changes the overlay) never
//! touches the compositor.

use crate::viewport::{ViewTransform, ViewportSize};

/// Width of a normal overlay line, in physical pixels.
pub const LINE_WIDTH: f32 = 1.0;
/// Width of the halo behind an [`OverlayStyle::Xor`] line, in physical pixels.
pub const HALO_WIDTH: f32 = 3.0;
/// Screen pixels per dash cycle of the marching ants (4 px dash + 4 px gap).
pub const ANTS_PERIOD: f32 = 8.0;

/// How an overlay line is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayStyle {
	/// Black/white marching ants, animated by the shader's time uniform.
	Ants,
	/// A plain colour (straight RGBA, 0..1).
	Solid([f32; 4]),
	/// A white 1 px line over a black 3 px halo: readable on any image.
	Xor,
}

/// One thing to draw, in document coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayItem {
	/// A path; `closed` joins the last point to the first.
	Polyline {
		points: Vec<(f64, f64)>,
		closed: bool,
		style: OverlayStyle,
	},
	/// A circle (the brush outline).
	Circle { centre: (f64, f64), radius: f64, style: OverlayStyle },
	/// A screen-sized square handle (transform/crop, M6).
	Handle { at: (f64, f64), size_px: f32 },
	/// A small crosshair (a clone source).
	Crosshair { at: (f64, f64) },
	/// Everything *outside* a convex document quadrilateral, filled (the crop
	/// tool dims what the crop will throw away, M6-T03). The corners go round
	/// the quad in either direction; a straightened crop box (and any box under
	/// a rotated view, M6-T05) is not axis-aligned on screen.
	Shade { quad: [(f64, f64); 4], color: [f32; 4] },
}

/// Everything the tools draw over the document this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
	pub items: Vec<OverlayItem>,
}

impl OverlayItem {
	/// The same item moved by `(dx, dy)` document pixels (the ants of a
	/// selection outline being dragged, M5-T04).
	pub fn translated(&self, dx: f64, dy: f64) -> Self {
		let shift = |(x, y): (f64, f64)| (x + dx, y + dy);
		match self {
			OverlayItem::Polyline { points, closed, style } => OverlayItem::Polyline {
				points: points.iter().copied().map(shift).collect(),
				closed: *closed,
				style: *style,
			},
			OverlayItem::Circle { centre, radius, style } => OverlayItem::Circle {
				centre: shift(*centre),
				radius: *radius,
				style: *style,
			},
			OverlayItem::Handle { at, size_px } => OverlayItem::Handle {
				at: shift(*at),
				size_px: *size_px,
			},
			OverlayItem::Crosshair { at } => OverlayItem::Crosshair { at: shift(*at) },
			OverlayItem::Shade { quad, color } => OverlayItem::Shade {
				quad: quad.map(shift),
				color: *color,
			},
		}
	}
}

impl Overlay {
	/// Whether the overlay needs the 8 Hz redraw that animates the ants.
	pub fn has_ants(&self) -> bool {
		self.items.iter().any(|item| match item {
			OverlayItem::Polyline { style, .. } | OverlayItem::Circle { style, .. } => *style == OverlayStyle::Ants,
			OverlayItem::Handle { .. } | OverlayItem::Crosshair { .. } | OverlayItem::Shade { .. } => false,
		})
	}
}

/// One triangle vertex of the overlay, in screen pixels. `arc` is the distance
/// along the item on screen (the shader dashes it); `ants` = 1 for a marching
/// ants line.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayVertex {
	pub pos: [f32; 2],
	pub color: [f32; 4],
	pub arc: f32,
	pub ants: f32,
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Chord approximation of a circle: fine enough that the error is under the
/// line width for any radius on screen.
const CIRCLE_SEGMENTS: usize = 64;
/// Half-length of a crosshair arm, in screen pixels.
const CROSSHAIR_ARM: f32 = 8.0;

/// Turn an overlay into screen-space triangles (a triangle list), drawing in
/// item order. Pure: no GPU, no state.
pub fn tessellate(overlay: &Overlay, view: &ViewTransform, viewport: ViewportSize) -> Vec<OverlayVertex> {
	let mut out = Vec::new();
	let to_screen = |x: f64, y: f64| -> [f32; 2] {
		let (sx, sy) = view.doc_to_screen(viewport, x, y);
		[sx as f32, sy as f32]
	};
	for item in &overlay.items {
		match item {
			OverlayItem::Polyline { points, closed, style } => {
				if points.len() < 2 {
					continue;
				}
				let screen: Vec<[f32; 2]> = points.iter().map(|&(x, y)| to_screen(x, y)).collect();
				stroke(&screen, *closed, *style, &mut out);
			}
			OverlayItem::Circle { centre, radius, style } => {
				let (cx, cy) = *centre;
				// A circle in document space stays a circle on screen at any
				// zoom, and at any view rotation: the mapping is a rigid
				// transform (M6-T05).
				let points: Vec<[f32; 2]> = (0..CIRCLE_SEGMENTS)
					.map(|i| {
						let angle = std::f64::consts::TAU * (i as f64) / (CIRCLE_SEGMENTS as f64);
						to_screen(cx + radius * angle.cos(), cy + radius * angle.sin())
					})
					.collect();
				stroke(&points, true, *style, &mut out);
			}
			OverlayItem::Handle { at, size_px } => {
				let [x, y] = to_screen(at.0, at.1);
				let half = size_px * 0.5;
				let corners = [[x - half, y - half], [x + half, y - half], [x + half, y + half], [x - half, y + half]];
				stroke(&corners, true, OverlayStyle::Xor, &mut out);
			}
			OverlayItem::Crosshair { at } => {
				let [x, y] = to_screen(at.0, at.1);
				let arms = [
					vec![[x - CROSSHAIR_ARM, y], [x + CROSSHAIR_ARM, y]],
					vec![[x, y - CROSSHAIR_ARM], [x, y + CROSSHAIR_ARM]],
				];
				for arm in &arms {
					stroke(arm, false, OverlayStyle::Xor, &mut out);
				}
			}
			OverlayItem::Shade { quad, color } => {
				let screen = quad.map(|(x, y)| to_screen(x, y));
				shade(&screen, [viewport.width as f32, viewport.height as f32], *color, &mut out);
			}
		}
	}
	out
}

/// One filled convex quad (two triangles), for [`OverlayItem::Shade`].
fn fill(quad: [[f32; 2]; 4], color: [f32; 4], out: &mut Vec<OverlayVertex>) {
	for index in [0, 1, 2, 0, 2, 3] {
		out.push(OverlayVertex {
			pos: quad[index],
			color,
			arc: 0.0,
			ants: 0.0,
		});
	}
}

/// The viewport `[0, w] × [0, h]` less the convex polygon `hole` (screen
/// pixels), as triangles. The hole is first clipped to the viewport; then the
/// viewport is cut into angular sectors about the hole's centroid, one at
/// every corner of either polygon, and each sector between the hole's edge
/// and the viewport's edge is one quad. Both polygons are convex and hold the
/// centroid, so a ray from it leaves each of them exactly once.
fn shade(hole: &[[f32; 2]], size: [f32; 2], color: [f32; 4], out: &mut Vec<OverlayVertex>) {
	let [w, h] = size;
	let frame = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
	let inner = clip_to_rect(hole, w, h);
	if inner.len() < 3 || polygon_area(&inner).abs() < 1e-3 {
		// Nothing of the box is on screen: everything on screen is dimmed.
		fill(frame, color, out);
		return;
	}
	let n = inner.len() as f32;
	let centre = inner.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n]);
	let mut angles: Vec<f32> = frame.iter().chain(&inner).map(|p| (p[1] - centre[1]).atan2(p[0] - centre[0])).collect();
	angles.sort_by(f32::total_cmp);
	angles.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
	for (i, &from) in angles.iter().enumerate() {
		let to = if i + 1 < angles.len() {
			angles[i + 1]
		} else {
			angles[0] + std::f32::consts::TAU
		};
		let (Some(near0), Some(far0), Some(near1), Some(far1)) = (
			ray_exit(centre, from, &inner),
			ray_exit(centre, from, &frame),
			ray_exit(centre, to, &inner),
			ray_exit(centre, to, &frame),
		) else {
			continue;
		};
		fill([near0, far0, far1, near1], color, out);
	}
}

/// Where a ray from `origin` at `angle` leaves the convex polygon `polygon`,
/// which holds `origin`.
fn ray_exit(origin: [f32; 2], angle: f32, polygon: &[[f32; 2]]) -> Option<[f32; 2]> {
	let d = [angle.cos(), angle.sin()];
	let mut best: Option<f32> = None;
	for (i, a) in polygon.iter().enumerate() {
		let b = polygon[(i + 1) % polygon.len()];
		let e = [b[0] - a[0], b[1] - a[1]];
		let denom = d[0] * e[1] - d[1] * e[0];
		if denom.abs() < 1e-9 {
			continue;
		}
		let ao = [a[0] - origin[0], a[1] - origin[1]];
		let t = (ao[0] * e[1] - ao[1] * e[0]) / denom;
		let s = (ao[0] * d[1] - ao[1] * d[0]) / denom;
		if t >= 0.0 && (-1e-4..=1.0 + 1e-4).contains(&s) {
			best = Some(best.map_or(t, |b| b.min(t)));
		}
	}
	best.map(|t| [origin[0] + d[0] * t, origin[1] + d[1] * t])
}

/// The signed area of a polygon (shoelace).
fn polygon_area(points: &[[f32; 2]]) -> f32 {
	let mut sum = 0.0;
	for (i, a) in points.iter().enumerate() {
		let b = points[(i + 1) % points.len()];
		sum += a[0] * b[1] - b[0] * a[1];
	}
	sum / 2.0
}

/// A convex polygon clipped to `[0, w] × [0, h]` (Sutherland–Hodgman).
fn clip_to_rect(points: &[[f32; 2]], w: f32, h: f32) -> Vec<[f32; 2]> {
	// (axis, value, keep the side above it)
	let planes: [(usize, f32, bool); 4] = [(0, 0.0, true), (0, w, false), (1, 0.0, true), (1, h, false)];
	let mut poly = points.to_vec();
	for (axis, value, keep_above) in planes {
		let inside = |p: &[f32; 2]| if keep_above { p[axis] >= value } else { p[axis] <= value };
		let mut next = Vec::with_capacity(poly.len() + 2);
		for (i, a) in poly.iter().enumerate() {
			let b = poly[(i + 1) % poly.len()];
			let (ia, ib) = (inside(a), inside(&b));
			if ia {
				next.push(*a);
			}
			if ia != ib {
				let t = (value - a[axis]) / (b[axis] - a[axis]);
				next.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
			}
		}
		poly = next;
		if poly.is_empty() {
			break;
		}
	}
	poly
}

/// One stroked path: an [`OverlayStyle::Xor`] line is a black halo then the
/// white core, so the order of the vertices matters (they are blended in
/// order).
fn stroke(points: &[[f32; 2]], closed: bool, style: OverlayStyle, out: &mut Vec<OverlayVertex>) {
	match style {
		OverlayStyle::Xor => {
			ribbon(points, closed, HALO_WIDTH, BLACK, 0.0, out);
			ribbon(points, closed, LINE_WIDTH, WHITE, 0.0, out);
		}
		OverlayStyle::Solid(color) => ribbon(points, closed, LINE_WIDTH, color, 0.0, out),
		OverlayStyle::Ants => ribbon(points, closed, LINE_WIDTH, WHITE, 1.0, out),
	}
}

/// Emit one screen-space quad (two triangles) per segment of `points`, each
/// vertex carrying the arc length so far (so the ants dash continuously across
/// segment and tile joins).
fn ribbon(points: &[[f32; 2]], closed: bool, width: f32, color: [f32; 4], ants: f32, out: &mut Vec<OverlayVertex>) {
	let half = width * 0.5;
	let count = points.len();
	let segments = if closed { count } else { count - 1 };
	let mut arc = 0.0f32;
	for i in 0..segments {
		let a = points[i];
		let b = points[(i + 1) % count];
		let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
		let len = (dx * dx + dy * dy).sqrt();
		if len <= f32::EPSILON {
			continue;
		}
		let (nx, ny) = (-dy / len * half, dx / len * half);
		let corners = [[a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny], [b[0] - nx, b[1] - ny], [a[0] - nx, a[1] - ny]];
		// Two triangles: a+ b+ b-,  a+ b- a-.
		for (index, distance) in [(0, 0.0), (1, len), (2, len), (0, 0.0), (2, len), (3, 0.0)] {
			out.push(OverlayVertex {
				pos: corners[index],
				color,
				arc: arc + distance,
				ants,
			});
		}
		arc += len;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn view(zoom: f64) -> ViewTransform {
		ViewTransform {
			zoom,
			center_x: 5.0,
			center_y: 5.0,
			rotation: 0.0,
		}
	}

	const VP: ViewportSize = ViewportSize { width: 100, height: 100 };

	fn square() -> Overlay {
		Overlay {
			items: vec![OverlayItem::Polyline {
				points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
				closed: true,
				style: OverlayStyle::Solid([1.0, 0.0, 0.0, 1.0]),
			}],
		}
	}

	/// The area of one triangle of the vertex list.
	fn triangle_area(triangle: &[OverlayVertex]) -> f32 {
		let (a, b, c) = (triangle[0].pos, triangle[1].pos, triangle[2].pos);
		((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
	}

	fn bounds(vertices: &[OverlayVertex]) -> [f32; 4] {
		vertices.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |mut b, v| {
			b[0] = b[0].min(v.pos[0]);
			b[1] = b[1].min(v.pos[1]);
			b[2] = b[2].max(v.pos[0]);
			b[3] = b[3].max(v.pos[1]);
			b
		})
	}

	#[test]
	fn a_square_polyline_gives_one_quad_per_segment_at_zoom_1() {
		let vertices = tessellate(&square(), &view(1.0), VP);
		assert_eq!(vertices.len(), 4 * 6, "four segments, two triangles each");
		// Doc (0,0)..(10,10) around the centre (5,5) at zoom 1 → screen 45..55,
		// plus half the 1 px line width on each side.
		let b = bounds(&vertices);
		for (got, expected) in b[..2].iter().zip([44.5, 44.5]) {
			assert!((got - expected).abs() < 1e-4, "{b:?}");
		}
		for (got, expected) in b[2..].iter().zip([55.5, 55.5]) {
			assert!((got - expected).abs() < 1e-4, "{b:?}");
		}
	}

	#[test]
	fn zoom_scales_the_geometry_but_not_the_line_width() {
		let vertices = tessellate(&square(), &view(0.25), VP);
		let b = bounds(&vertices);
		// Doc (0,0)..(10,10) around (5,5) at zoom 0.25 → screen 48.75..51.25,
		// plus the same 0.5 px half width (it is a screen-space line).
		assert!((b[0] - 48.25).abs() < 1e-4 && (b[1] - 48.25).abs() < 1e-4, "{b:?}");
		assert!((b[2] - 51.75).abs() < 1e-4 && (b[3] - 51.75).abs() < 1e-4, "{b:?}");
	}

	#[test]
	fn xor_draws_the_halo_before_the_core() {
		let overlay = Overlay {
			items: vec![OverlayItem::Crosshair { at: (5.0, 5.0) }],
		};
		let vertices = tessellate(&overlay, &view(1.0), VP);
		// Two arms; each is a 3 px black ribbon then a 1 px white one.
		let black = vertices.iter().filter(|v| v.color == BLACK).count();
		let white = vertices.iter().filter(|v| v.color == WHITE).count();
		assert_eq!((black, white), (2 * 6, 2 * 6));
		assert_eq!(vertices[0].color, BLACK, "the halo is drawn first");
	}

	#[test]
	fn ants_carry_the_arc_length_across_segments() {
		let overlay = Overlay {
			items: vec![OverlayItem::Polyline {
				points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
				closed: false,
				style: OverlayStyle::Ants,
			}],
		};
		let vertices = tessellate(&overlay, &view(1.0), VP);
		assert!(vertices.iter().all(|v| v.ants == 1.0));
		// The second segment starts where the first ended (10 px).
		assert!(vertices.iter().any(|v| (v.arc - 10.0).abs() < 1e-4));
		assert!(vertices.iter().any(|v| (v.arc - 20.0).abs() < 1e-4), "the corner to the end");
	}

	#[test]
	fn a_shade_covers_the_viewport_but_for_the_rectangle() {
		let square = |x0: f64, y0: f64, x1: f64, y1: f64| [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
		let overlay = Overlay {
			items: vec![OverlayItem::Shade {
				quad: square(0.0, 0.0, 10.0, 10.0),
				color: [0.0, 0.0, 0.0, 0.5],
			}],
		};
		let vertices = tessellate(&overlay, &view(1.0), VP);
		assert!(vertices.iter().all(|v| v.color == [0.0, 0.0, 0.0, 0.5] && v.ants == 0.0));
		// The document rectangle is screen 45..55: 10 000 pixels of viewport
		// less the 100 it covers.
		let area: f32 = vertices.chunks_exact(3).map(triangle_area).sum();
		assert!((area - 9900.0).abs() < 1e-3, "{area}");
		// A rectangle that covers the whole viewport dims nothing at all.
		let overlay = Overlay {
			items: vec![OverlayItem::Shade {
				quad: square(-50.0, -50.0, 60.0, 60.0),
				color: [0.0, 0.0, 0.0, 0.5],
			}],
		};
		let area: f32 = tessellate(&overlay, &view(1.0), VP).chunks_exact(3).map(triangle_area).sum();
		assert!(area < 1e-3, "{area}");
		// A box wholly off screen dims all of it.
		let overlay = Overlay {
			items: vec![OverlayItem::Shade {
				quad: square(500.0, 500.0, 510.0, 510.0),
				color: [0.0, 0.0, 0.0, 0.5],
			}],
		};
		let area: f32 = tessellate(&overlay, &view(1.0), VP).chunks_exact(3).map(triangle_area).sum();
		assert!((area - 10_000.0).abs() < 1e-2, "{area}");
	}

	#[test]
	fn a_turned_shade_leaves_exactly_the_turned_box() {
		// A 20 × 20 square turned 45° about the document point (0, 0), which is
		// the viewport's centre: a diamond of area 400.
		let r = 200f64.sqrt();
		let overlay = Overlay {
			items: vec![OverlayItem::Shade {
				quad: [(0.0, -r), (r, 0.0), (0.0, r), (-r, 0.0)],
				color: [0.0, 0.0, 0.0, 0.5],
			}],
		};
		let vertices = tessellate(&overlay, &view(1.0), VP);
		let area: f32 = vertices.chunks_exact(3).map(triangle_area).sum();
		assert!((area - 9600.0).abs() < 0.05, "{area}");
		// Nothing is drawn inside the diamond: every triangle's centroid lies
		// outside it.
		let [cx, cy] = {
			let (x, y) = view(1.0).doc_to_screen(VP, 0.0, 0.0);
			[x as f32, y as f32]
		};
		for triangle in vertices.chunks_exact(3).filter(|t| triangle_area(t) > 1e-3) {
			let c = [
				(triangle[0].pos[0] + triangle[1].pos[0] + triangle[2].pos[0]) / 3.0 - cx,
				(triangle[0].pos[1] + triangle[1].pos[1] + triangle[2].pos[1]) / 3.0 - cy,
			];
			assert!(c[0].abs() + c[1].abs() >= r as f32 - 1e-3, "{c:?}");
		}
	}

	#[test]
	fn ants_are_detected_for_the_animation_timer() {
		assert!(!square().has_ants());
		let ants = Overlay {
			items: vec![OverlayItem::Circle {
				centre: (0.0, 0.0),
				radius: 5.0,
				style: OverlayStyle::Ants,
			}],
		};
		assert!(ants.has_ants());
	}
}
