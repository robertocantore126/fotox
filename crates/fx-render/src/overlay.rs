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
		}
	}
}

impl Overlay {
	/// Whether the overlay needs the 8 Hz redraw that animates the ants.
	pub fn has_ants(&self) -> bool {
		self.items.iter().any(|item| match item {
			OverlayItem::Polyline { style, .. } | OverlayItem::Circle { style, .. } => *style == OverlayStyle::Ants,
			OverlayItem::Handle { .. } | OverlayItem::Crosshair { .. } => false,
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
				// zoom (the view has no rotation yet).
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
		}
	}
	out
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
