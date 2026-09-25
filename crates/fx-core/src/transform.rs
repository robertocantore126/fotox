//! Transform parameters (M6): the resampling filters and the source →
//! destination geometry used by Free Transform, Warp, Image Size and the
//! arbitrary rotations.
//!
//! These are **data** — they end up inside a `Command` (M6-T04) and therefore
//! must serialise. The algorithms that consume them live in `fx-ops`
//! (`fx_ops::resample`), which depends on this crate: the same split as
//! [`crate::ops::FilterParams`] (recipe R1a in `docs/tasks/HOWTO.md`).

use serde::{Deserialize, Serialize};

/// A resampling filter (M6-T01, decision D-052).
///
/// Thresholds and kernel constants are Photoshop's; `Lanczos3` is a Fotox
/// extra. `BicubicAutomatic` is the dialog's default and resolves to a
/// concrete filter once the local scale is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
	/// Nearest neighbour: the one source pixel whose centre is closest.
	Nearest,
	/// Bilinear (a triangle kernel).
	Bilinear,
	/// Keys' cubic with `a = −0.5` — Photoshop's "Bicubic".
	Bicubic,
	/// Mitchell–Netravali with `B = C = 1/3` — "Bicubic Smoother".
	BicubicSmoother,
	/// Keys' cubic with `a = −0.75` — "Bicubic Sharper".
	BicubicSharper,
	/// Lanczos with 3 lobes.
	Lanczos3,
	/// `BicubicSharper` when reducing, `BicubicSmoother` when enlarging.
	BicubicAutomatic,
}

impl Filter {
	/// The menu label, as Photoshop spells it.
	pub fn label(self) -> &'static str {
		match self {
			Filter::Nearest => "Nearest Neighbor",
			Filter::Bilinear => "Bilinear",
			Filter::Bicubic => "Bicubic",
			Filter::BicubicSmoother => "Bicubic Smoother",
			Filter::BicubicSharper => "Bicubic Sharper",
			Filter::Lanczos3 => "Lanczos 3",
			Filter::BicubicAutomatic => "Bicubic Automatic",
		}
	}

	/// Resolve `BicubicAutomatic` for a local scale: `scale` is destination
	/// pixels per source pixel, so `< 1` means the image is being **reduced**.
	/// Every other filter is returned unchanged.
	pub fn resolve(self, scale: f64) -> Filter {
		match self {
			Filter::BicubicAutomatic if scale < 1.0 => Filter::BicubicSharper,
			Filter::BicubicAutomatic => Filter::BicubicSmoother,
			other => other,
		}
	}

	/// The kernel's half-width in source pixels. A tap at distance `≤ support`
	/// contributes; the tap count over one axis is `2 · support` (rounded up).
	pub fn support(self) -> f64 {
		match self {
			Filter::Nearest => 0.5,
			Filter::Bilinear => 1.0,
			Filter::Bicubic | Filter::BicubicSmoother | Filter::BicubicSharper | Filter::BicubicAutomatic => 2.0,
			Filter::Lanczos3 => 3.0,
		}
	}

	/// Whether sampling averages several pixels. Nearest does not, so it never
	/// reads a mip level when reducing (Photoshop's Nearest aliases too).
	pub fn is_interpolating(self) -> bool {
		!matches!(self, Filter::Nearest)
	}
}

/// The geometry of a transform: **source image pixels** → **destination
/// document pixels**, both at mip level 0.
///
/// `fx_ops::resample` walks this backwards: for every destination pixel centre
/// it asks for the source point, so a transform never leaves holes (SNIPPETS
/// §11: always map destination → source, never splat forwards).
// A warp carries 16 control points (288 bytes); boxing it would cost an
// allocation per transform and lose `Copy`, which the sampler relies on. The
// enum is a command payload, copied once per transform, never per tile.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Mapping {
	/// Row-major `[a, b, c, d, e, f]`: `x' = a·x + c·y + e`,
	/// `y' = b·x + d·y + f`.
	Affine([f64; 6]),
	/// Row-major 3 × 3 homography applied to `(x, y, 1)`; the result is
	/// divided by `w`.
	Projective([f64; 9]),
	/// A bicubic Bézier patch (Free Transform's Warp mode).
	Warp(BezierPatch),
}

impl Mapping {
	/// The identity: destination = source.
	pub fn identity() -> Self {
		Mapping::Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
	}

	/// An affine mapping `[a, b, c, d, e, f]`.
	pub fn affine(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
		Mapping::Affine([a, b, c, d, e, f])
	}

	/// A translation by `(dx, dy)` destination pixels.
	pub fn translation(dx: f64, dy: f64) -> Self {
		Mapping::Affine([1.0, 0.0, 0.0, 1.0, dx, dy])
	}

	/// A scale about the source origin.
	pub fn scale(sx: f64, sy: f64) -> Self {
		Mapping::Affine([sx, 0.0, 0.0, sy, 0.0, 0.0])
	}

	/// A rotation by `radians` about `(cx, cy)` (source coordinates).
	pub fn rotation_about(radians: f64, cx: f64, cy: f64) -> Self {
		let (sin, cos) = radians.sin_cos();
		// x' = cos·(x − cx) − sin·(y − cy) + cx
		Mapping::Affine([cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy])
	}

	/// True when every coefficient is finite (a command must reject a mapping
	/// with NaN or infinities, which would poison every sampled pixel).
	pub fn is_finite(&self) -> bool {
		match self {
			Mapping::Affine(m) => m.iter().all(|v| v.is_finite()),
			Mapping::Projective(m) => m.iter().all(|v| v.is_finite()),
			Mapping::Warp(patch) => patch.points.iter().flatten().all(|v| v.is_finite()) && patch.src_rect.iter().all(|v| v.is_finite()),
		}
	}

	/// The mapping that sends `(x + dx, y + dy)` where this one sends `(x, y)`
	/// — `self ∘ translate(dx, dy)` (a source-side translation). Used to place
	/// a layer's own pixels in canvas coordinates before mapping them.
	pub fn after_translation(self, dx: f64, dy: f64) -> Self {
		match self {
			Mapping::Affine([a, b, c, d, e, f]) => Mapping::Affine([a, b, c, d, e + a * dx + c * dy, f + b * dx + d * dy]),
			Mapping::Projective([a, b, c, d, e, f, g, h, i]) => {
				Mapping::Projective([a, b, c + a * dx + b * dy, d, e, f + d * dx + e * dy, g, h, i + g * dx + h * dy])
			}
			// A warp's surface is fixed; moving the source moves its rect.
			Mapping::Warp(mut patch) => {
				patch.src_rect[0] += dx;
				patch.src_rect[1] += dy;
				Mapping::Warp(patch)
			}
		}
	}

	/// The mapping that sends every destination point of this one moved by
	/// `(dx, dy)` (a destination-side translation). Used to make a mapped
	/// bounding box start at the origin of the image that will hold it.
	pub fn after_destination_translation(self, dx: f64, dy: f64) -> Self {
		match self {
			Mapping::Affine([a, b, c, d, e, f]) => Mapping::Affine([a, b, c, d, e + dx, f + dy]),
			Mapping::Projective([a, b, c, d, e, f, g, h, i]) => {
				Mapping::Projective([a + dx * g, b + dx * h, c + dx * i, d + dy * g, e + dy * h, f + dy * i, g, h, i])
			}
			Mapping::Warp(mut patch) => {
				for point in &mut patch.points {
					point[0] += dx;
					point[1] += dy;
				}
				Mapping::Warp(patch)
			}
		}
	}

	/// Where the mapping sends a source point, or `None` when it cannot be
	/// evaluated there (outside a warp's surface, a degenerate homography, a
	/// non-finite result). A warp's surface is not evaluated here — the
	/// sampler owns it (`fx_ops::resample::warp`).
	pub fn forward_point(&self, x: f64, y: f64) -> Option<(f64, f64)> {
		match self {
			Mapping::Affine([a, b, c, d, e, f]) => {
				let point = (a * x + c * y + e, b * x + d * y + f);
				(point.0.is_finite() && point.1.is_finite()).then_some(point)
			}
			Mapping::Projective(m) => {
				let w = m[6] * x + m[7] * y + m[8];
				if !w.is_finite() || w.abs() < 1e-12 {
					return None;
				}
				let point = ((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w);
				(point.0.is_finite() && point.1.is_finite()).then_some(point)
			}
			Mapping::Warp(_) => None,
		}
	}

	/// The mapping that sends the source rectangle `rect` (`[x0, y0, x1, y1]`)
	/// onto the quadrilateral `quad` — its top-left, top-right, bottom-right and
	/// bottom-left corners, in that order (M6-T04: Free Transform's box). A
	/// parallelogram gives an affine mapping (scale, rotate, skew), anything
	/// else a homography (distort, perspective; Heckbert's square-to-quad).
	/// `None` for an empty rectangle or a quad that is not strictly convex (a
	/// folded or collapsed box has no homography that keeps it on screen).
	pub fn from_quad(rect: [f64; 4], quad: [(f64, f64); 4]) -> Option<Mapping> {
		let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
		if !(w > 0.0 && h > 0.0) || !quad_is_convex(quad) {
			return None;
		}
		let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = quad;
		let size = (x2 - x0).abs().max((y2 - y0).abs()).max((x3 - x1).abs()).max((y3 - y1).abs()).max(1.0);
		let (dx3, dy3) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
		let mapping = if dx3.abs() <= 1e-9 * size && dy3.abs() <= 1e-9 * size {
			// A parallelogram: TL + (TR − TL)·u + (BL − TL)·v.
			let (a, b) = ((x1 - x0) / w, (y1 - y0) / w);
			let (c, d) = ((x3 - x0) / h, (y3 - y0) / h);
			Mapping::Affine([a, b, c, d, x0 - a * rect[0] - c * rect[1], y0 - b * rect[0] - d * rect[1]])
		} else {
			// The unit square onto the quad (u along x, v along y) …
			let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
			let den = dx1 * dy2 - dx2 * dy1;
			if den.abs() < 1e-12 {
				return None;
			}
			let g = (dx3 * dy2 - dx2 * dy3) / den;
			let h2 = (dx1 * dy3 - dx3 * dy1) / den;
			let square = [x1 - x0 + g * x1, x3 - x0 + h2 * x3, x0, y1 - y0 + g * y1, y3 - y0 + h2 * y3, y0, g, h2, 1.0];
			// … after the rectangle onto the unit square.
			let s = [1.0 / w, 0.0, -rect[0] / w, 0.0, 1.0 / h, -rect[1] / h, 0.0, 0.0, 1.0];
			let mut m = [0.0; 9];
			for r in 0..3 {
				for c in 0..3 {
					m[r * 3 + c] = (0..3).map(|k| square[r * 3 + k] * s[k * 3 + c]).sum();
				}
			}
			Mapping::Projective(m)
		};
		mapping.is_finite().then_some(mapping)
	}

	/// If this mapping is a whole-pixel translation, its offset: such a
	/// transform needs no resampling at all (recipe R1.7, exact case).
	pub fn integer_translation(&self) -> Option<(i32, i32)> {
		let Mapping::Affine([a, b, c, d, e, f]) = self else {
			return None;
		};
		if *a != 1.0 || *b != 0.0 || *c != 0.0 || *d != 1.0 {
			return None;
		}
		if e.fract() != 0.0 || f.fract() != 0.0 {
			return None;
		}
		Some((*e as i32, *f as i32))
	}
}

/// The whole-pixel rectangle that contains the image of `rect` under
/// `mapping`: the floor of the mapped box's top-left corner and its size, so
/// `offset` + `size` cover every mapped pixel (M6-T02: rotating the canvas by
/// an arbitrary angle grows it to exactly this box). `None` when the mapping
/// cannot be evaluated there (a warp, a degenerate mapping, an absurd size).
pub fn dest_rect(mapping: &Mapping, rect: [f64; 4]) -> Option<((i32, i32), (u32, u32))> {
	let corners = [(rect[0], rect[1]), (rect[2], rect[1]), (rect[2], rect[3]), (rect[0], rect[3])];
	let mut box_ = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
	let mut add = |(x, y): (f64, f64)| {
		box_[0] = box_[0].min(x);
		box_[1] = box_[1].min(y);
		box_[2] = box_[2].max(x);
		box_[3] = box_[3].max(y);
	};
	match mapping {
		// A Bézier surface lies inside the convex hull of its control points
		// (M6-T04's Warp): their box holds every mapped pixel. The source
		// outside the patch's rectangle maps nowhere.
		Mapping::Warp(patch) => patch.points.iter().for_each(|p| add((p[0], p[1]))),
		_ => {
			for (x, y) in corners {
				add(mapping.forward_point(x, y)?);
			}
		}
	}
	if !box_.iter().all(|v| v.is_finite()) {
		return None;
	}
	let left = snap(box_[0]).floor();
	let top = snap(box_[1]).floor();
	let width = snap(box_[2]).ceil() - left;
	let height = snap(box_[3]).ceil() - top;
	Some((
		(i32::try_from(left as i64).ok()?, i32::try_from(top as i64).ok()?),
		(u32::try_from(width.max(1.0) as i64).ok()?, u32::try_from(height.max(1.0) as i64).ok()?),
	))
}

/// Whether the quadrilateral (corners in order round it) is strictly convex:
/// every turn goes the same way and no side is collapsed (M6-T04: a Free
/// Transform box that folds over itself is refused, as in Photoshop).
pub fn quad_is_convex(quad: [(f64, f64); 4]) -> bool {
	let mut sign = 0.0;
	for i in 0..4 {
		let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
		let cross = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
		if !cross.is_finite() || cross.abs() < 1e-9 {
			return false;
		}
		if sign == 0.0 {
			sign = cross.signum();
		} else if cross.signum() != sign {
			return false;
		}
	}
	true
}

/// A coordinate that is a rounding error away from a whole pixel is that
/// whole pixel: a quarter-turn of a rectangle must not grow by one pixel
/// because `sin(π)` is 1.2e-16 rather than 0.
fn snap(value: f64) -> f64 {
	let rounded = value.round();
	if (value - rounded).abs() < 1e-9 { rounded } else { value }
}

/// An exact rotation or mirror of a whole image (M6-T02).
///
/// These are **permutations**: every destination pixel takes exactly one source
/// pixel, unchanged, so 16-bit data survives bit-exactly and no kernel runs.
/// `Image ▸ Rotate 90° CW/CCW/180°` and `Flip Canvas` are the light-grey ones;
/// the arbitrary angle is a resample (`fx_ops::resample`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permutation {
	/// A quarter turn clockwise.
	Rot90Cw,
	/// A quarter turn anticlockwise.
	Rot90Ccw,
	/// A half turn.
	Rot180,
	/// Mirror left ↔ right.
	FlipHorizontal,
	/// Mirror top ↔ bottom.
	FlipVertical,
}

impl Permutation {
	/// The History label, as Photoshop writes it.
	pub fn label(self) -> &'static str {
		match self {
			Permutation::Rot90Cw => "Rotate 90° Clockwise",
			Permutation::Rot90Ccw => "Rotate 90° Counter Clockwise",
			Permutation::Rot180 => "Rotate 180°",
			Permutation::FlipHorizontal => "Flip Canvas Horizontal",
			Permutation::FlipVertical => "Flip Canvas Vertical",
		}
	}

	/// The permutation of whole quarter turns, `1` = clockwise, `2` = 180°,
	/// `3` = anticlockwise; `None` for a whole number of full turns (the
	/// command refuses those instead of recording a no-op).
	pub fn from_quarter_turns(turns: i8) -> Option<Self> {
		match turns.rem_euclid(4) {
			1 => Some(Permutation::Rot90Cw),
			2 => Some(Permutation::Rot180),
			3 => Some(Permutation::Rot90Ccw),
			_ => None,
		}
	}

	/// The size of the permuted image (a quarter turn swaps the axes).
	pub fn size(self, (width, height): (u32, u32)) -> (u32, u32) {
		match self {
			Permutation::Rot90Cw | Permutation::Rot90Ccw => (height, width),
			_ => (width, height),
		}
	}

	/// The source pixel a destination pixel takes, for a `size` source image.
	/// `None` when `dst` lies outside the permuted image (destination tiles are
	/// padded to whole tiles, so the last row and column have pixels past it).
	pub fn source_pixel(self, dst: (u32, u32), size: (u32, u32)) -> Option<(u32, u32)> {
		let (width, height) = size;
		if dst.0 >= self.size(size).0 || dst.1 >= self.size(size).1 {
			return None;
		}
		let (x, y) = (i64::from(dst.0), i64::from(dst.1));
		let (w, h) = (i64::from(width), i64::from(height));
		let (sx, sy) = match self {
			Permutation::Rot90Cw => (y, h - 1 - x),
			Permutation::Rot90Ccw => (w - 1 - y, x),
			Permutation::Rot180 => (w - 1 - x, h - 1 - y),
			Permutation::FlipHorizontal => (w - 1 - x, y),
			Permutation::FlipVertical => (x, h - 1 - y),
		};
		if sx < 0 || sy < 0 || sx >= w || sy >= h {
			return None;
		}
		Some((sx as u32, sy as u32))
	}

	/// Where an image placed at `offset` in a `canvas` ends up: the image's own
	/// pixels are permuted and its offset recomputed so the content stays where
	/// it was relative to the canvas. `image` is the image's size before the
	/// permutation. `None` when the new offset does not fit an `i32`.
	pub fn offset(self, offset: (i32, i32), image: (u32, u32), canvas: (u32, u32)) -> Option<(i32, i32)> {
		let (ix, iy) = (i64::from(image.0), i64::from(image.1));
		let (cx, cy) = (i64::from(canvas.0), i64::from(canvas.1));
		let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
		// The canvas point `(x, y)` moves to `(cx − 1 − y, x)` (a quarter turn
		// clockwise), so the image's corner travels with its pixels.
		let (x, y) = match self {
			Permutation::Rot90Cw => (cy - oy - iy, ox),
			Permutation::Rot90Ccw => (oy, cx - ox - ix),
			Permutation::Rot180 => (cx - ox - ix, cy - oy - iy),
			Permutation::FlipHorizontal => (cx - ox - ix, oy),
			Permutation::FlipVertical => (ox, cy - oy - iy),
		};
		Some((i32::try_from(x).ok()?, i32::try_from(y).ok()?))
	}
}

/// One axis of [`Anchor9`]: where the old canvas sits inside the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Comp {
	Start,
	Center,
	End,
}

impl Comp {
	/// The offset of the old extent inside the new one. The centre rounds toward
	/// zero, so an odd pixel of growth (or of crop) is on the right/bottom, as
	/// Photoshop does. Saturates rather than wrapping: a canvas this large cannot
	/// exist (a two-billion-pixel canvas needs four trillion tiles).
	fn place(self, old: u32, new: u32) -> i32 {
		let delta = i64::from(new) - i64::from(old);
		let value = match self {
			Comp::Start => 0,
			Comp::Center => delta / 2,
			Comp::End => delta,
		};
		value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
	}
}

/// The nine anchor cells of Image ▸ Canvas Size (M6-T02).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor9 {
	TopLeft,
	Top,
	TopRight,
	Left,
	Center,
	Right,
	BottomLeft,
	Bottom,
	BottomRight,
}

impl Anchor9 {
	/// The Photoshop label of the cell (the dialog draws a 3 × 3 grid; the
	/// tooltip and the macro viewer use this).
	pub fn label(self) -> &'static str {
		match self {
			Anchor9::TopLeft => "Top Left",
			Anchor9::Top => "Top Centre",
			Anchor9::TopRight => "Top Right",
			Anchor9::Left => "Left Centre",
			Anchor9::Center => "Centre",
			Anchor9::Right => "Right Centre",
			Anchor9::BottomLeft => "Bottom Left",
			Anchor9::Bottom => "Bottom Centre",
			Anchor9::BottomRight => "Bottom Right",
		}
	}

	/// Where the old canvas's top-left corner lands in the new one.
	pub fn offset(self, old: (u32, u32), new: (u32, u32)) -> (i32, i32) {
		let (h, v) = match self {
			Anchor9::TopLeft => (Comp::Start, Comp::Start),
			Anchor9::Top => (Comp::Center, Comp::Start),
			Anchor9::TopRight => (Comp::End, Comp::Start),
			Anchor9::Left => (Comp::Start, Comp::Center),
			Anchor9::Center => (Comp::Center, Comp::Center),
			Anchor9::Right => (Comp::End, Comp::Center),
			Anchor9::BottomLeft => (Comp::Start, Comp::End),
			Anchor9::Bottom => (Comp::Center, Comp::End),
			Anchor9::BottomRight => (Comp::End, Comp::End),
		};
		(h.place(old.0, new.0), v.place(old.1, new.1))
	}
}

/// A 4 × 4 bicubic Bézier patch (Free Transform's Warp, D-053).
///
/// Parameter space `[0, 1]²` maps **from** [`BezierPatch::src_rect`] (source
/// image pixels) **to** the surface defined by [`BezierPatch::points`]
/// (destination document pixels). A uniform grid of control points gives the
/// bilinear identity, so an identity patch resamples exactly.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BezierPatch {
	/// Control points, row-major in `v` then `u`: `points[i * 4 + j]` is the
	/// control point at `(u = j/3, v = i/3)`.
	pub points: [[f64; 2]; 16],
	/// The source rectangle `[x0, y0, x1, y1]` in source image pixels.
	pub src_rect: [f64; 4],
}

impl BezierPatch {
	/// The identity patch over a `w × h` source rectangle: the surface is the
	/// destination rectangle `[0, w] × [0, h]`, so `destination = source`.
	pub fn identity(w: u32, h: u32) -> Self {
		Self::rect([0.0, 0.0, f64::from(w), f64::from(h)], [0.0, 0.0, f64::from(w), f64::from(h)])
	}

	/// A patch whose surface is the destination rectangle `dst` and whose
	/// parameter space maps from `src_rect`. With `dst` and `src_rect` equal
	/// this is the identity; with different sizes it is a pure scale+translate
	/// (which a warp may still want, e.g. a preset style).
	pub fn rect(dst: [f64; 4], src_rect: [f64; 4]) -> Self {
		let mut points = [[0.0; 2]; 16];
		for i in 0..4 {
			for j in 0..4 {
				let u = j as f64 / 3.0;
				let v = i as f64 / 3.0;
				points[i * 4 + j] = [dst[0] + u * (dst[2] - dst[0]), dst[1] + v * (dst[3] - dst[1])];
			}
		}
		Self { points, src_rect }
	}

	/// Move one control point (Warp handles). `i`/`j` are `0..=3`.
	pub fn set_point(&mut self, i: usize, j: usize, x: f64, y: f64) {
		self.points[i * 4 + j] = [x, y];
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn automatic_resolves_by_direction() {
		assert_eq!(Filter::BicubicAutomatic.resolve(0.25), Filter::BicubicSharper);
		assert_eq!(Filter::BicubicAutomatic.resolve(1.0), Filter::BicubicSmoother);
		assert_eq!(Filter::BicubicAutomatic.resolve(2.0), Filter::BicubicSmoother);
		assert_eq!(Filter::Bicubic.resolve(0.25), Filter::Bicubic);
	}

	#[test]
	fn a_quad_maps_the_rectangle_onto_its_corners() {
		let rect = [10.0, 20.0, 110.0, 70.0];
		let corners = [(10.0, 20.0), (110.0, 20.0), (110.0, 70.0), (10.0, 70.0)];
		// A parallelogram (rotated, sheared) is affine …
		let turned = [(50.0, 0.0), (150.0, 30.0), (170.0, 90.0), (70.0, 60.0)];
		let affine = Mapping::from_quad(rect, turned).expect("convex");
		assert!(matches!(affine, Mapping::Affine(_)));
		// … a trapezoid is not.
		let trapezoid = [(0.0, 0.0), (200.0, 0.0), (150.0, 100.0), (50.0, 100.0)];
		let projective = Mapping::from_quad(rect, trapezoid).expect("convex");
		assert!(matches!(projective, Mapping::Projective(_)));
		for (mapping, quad) in [(affine, turned), (projective, trapezoid)] {
			for (corner, expected) in corners.iter().zip(quad) {
				let p = mapping.forward_point(corner.0, corner.1).expect("finite");
				assert!(
					(p.0 - expected.0).abs() < 1e-9 && (p.1 - expected.1).abs() < 1e-9,
					"{corner:?} → {p:?}, want {expected:?}"
				);
			}
		}
		// The rectangle's own corners are the identity.
		assert_eq!(Mapping::from_quad(rect, corners), Some(Mapping::identity()));
		// A folded box (two corners swapped) and a collapsed one are refused.
		assert_eq!(Mapping::from_quad(rect, [(0.0, 0.0), (100.0, 100.0), (100.0, 0.0), (0.0, 100.0)]), None);
		assert_eq!(Mapping::from_quad(rect, [(0.0, 0.0), (0.0, 0.0), (100.0, 100.0), (0.0, 100.0)]), None);
		assert_eq!(Mapping::from_quad([0.0, 0.0, 0.0, 10.0], corners), None);
	}

	#[test]
	fn a_warps_box_is_its_control_points_box() {
		let mut patch = BezierPatch::rect([0.0, 0.0, 300.0, 200.0], [0.0, 0.0, 300.0, 200.0]);
		patch.set_point(1, 1, -40.5, 70.0);
		patch.set_point(3, 3, 330.0, 260.2);
		assert_eq!(dest_rect(&Mapping::Warp(patch), [0.0, 0.0, 300.0, 200.0]), Some(((-41, 0), (371, 261))));
	}

	#[test]
	fn integer_translations_are_detected() {
		assert_eq!(Mapping::translation(3.0, -4.0).integer_translation(), Some((3, -4)));
		assert_eq!(Mapping::translation(3.5, 0.0).integer_translation(), None);
		assert_eq!(Mapping::scale(1.0, 1.0).integer_translation(), Some((0, 0)));
		assert_eq!(Mapping::identity().integer_translation(), Some((0, 0)));
		assert_eq!(Mapping::rotation_about(0.3, 5.0, 5.0).integer_translation(), None);
	}

	#[test]
	fn mapping_round_trips_through_json() {
		for mapping in [
			Mapping::identity(),
			Mapping::affine(1.0, 0.2, -0.2, 1.0, 10.0, 20.0),
			Mapping::Projective([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0001, 0.0, 1.0]),
			Mapping::Warp(BezierPatch::identity(100, 50)),
		] {
			let json = serde_json::to_string(&mapping).unwrap();
			let back: Mapping = serde_json::from_str(&json).unwrap();
			assert_eq!(mapping, back, "{json}");
		}
	}

	#[test]
	fn quarter_turns_map_to_permutations() {
		assert_eq!(Permutation::from_quarter_turns(1), Some(Permutation::Rot90Cw));
		assert_eq!(Permutation::from_quarter_turns(2), Some(Permutation::Rot180));
		assert_eq!(Permutation::from_quarter_turns(3), Some(Permutation::Rot90Ccw));
		assert_eq!(Permutation::from_quarter_turns(-1), Some(Permutation::Rot90Ccw));
		assert_eq!(Permutation::from_quarter_turns(-2), Some(Permutation::Rot180));
		assert_eq!(Permutation::from_quarter_turns(-3), Some(Permutation::Rot90Cw));
		assert_eq!(Permutation::from_quarter_turns(0), None);
		assert_eq!(Permutation::from_quarter_turns(4), None);
		assert_eq!(Permutation::from_quarter_turns(-4), None);
		assert_eq!(Permutation::Rot90Cw.size((30, 10)), (10, 30));
		assert_eq!(Permutation::Rot180.size((30, 10)), (30, 10));
		assert_eq!(Permutation::FlipHorizontal.size((30, 10)), (30, 10));
	}

	/// The canvas-level permutation: where a canvas point moves to.
	fn canvas_forward(op: Permutation, (cw, ch): (u32, u32), (x, y): (i64, i64)) -> (i64, i64) {
		let (w, h) = (i64::from(cw), i64::from(ch));
		match op {
			Permutation::Rot90Cw => (h - 1 - y, x),
			Permutation::Rot90Ccw => (y, w - 1 - x),
			Permutation::Rot180 => (w - 1 - x, h - 1 - y),
			Permutation::FlipHorizontal => (w - 1 - x, y),
			Permutation::FlipVertical => (x, h - 1 - y),
		}
	}

	#[test]
	fn a_permutation_is_a_bijection_and_moves_the_offset_with_the_content() {
		let canvas = (8, 6);
		let image = (5, 3);
		let offset = (-2, 4);
		for op in [
			Permutation::Rot90Cw,
			Permutation::Rot90Ccw,
			Permutation::Rot180,
			Permutation::FlipHorizontal,
			Permutation::FlipVertical,
		] {
			let new_offset = op.offset(offset, image, canvas).expect("a small offset fits");
			let new_size = op.size(image);
			let mut seen = Vec::new();
			for y in 0..new_size.1 {
				for x in 0..new_size.0 {
					let src = op.source_pixel((x, y), image).expect("inside the permuted image");
					assert!(!seen.contains(&src), "{op:?}: {src:?} taken twice");
					seen.push(src);
					// The pixel keeps its canvas position: the destination pixel in
					// the new placement sits where the source pixel sat.
					let before = (i64::from(src.0) + i64::from(offset.0), i64::from(src.1) + i64::from(offset.1));
					let after = (i64::from(x) + i64::from(new_offset.0), i64::from(y) + i64::from(new_offset.1));
					assert_eq!(after, canvas_forward(op, canvas, before), "{op:?}: pixel {src:?}");
				}
			}
			assert_eq!(seen.len(), (image.0 * image.1) as usize, "{op:?} covers every source pixel exactly once");
		}
	}

	#[test]
	fn four_quarter_turns_return_to_the_start() {
		let image = (7, 4);
		for y in 0..image.1 {
			for x in 0..image.0 {
				let (mut pixel, mut size) = ((x, y), image);
				for _ in 0..4 {
					// Follow the pixel forwards: the destination it comes from.
					let dest = Permutation::Rot90Cw.size(size);
					let next = (0..dest.0)
						.flat_map(|dx| (0..dest.1).map(move |dy| (dx, dy)))
						.find(|&dst| Permutation::Rot90Cw.source_pixel(dst, size) == Some(pixel))
						.expect("every pixel has a destination");
					pixel = next;
					size = dest;
				}
				assert_eq!((pixel, size), ((x, y), image), "a full turn is the identity");
			}
		}
	}

	#[test]
	fn canvas_anchors_place_the_old_canvas() {
		let old = (100, 50);
		// Growth: the extra pixels go right/bottom for the centre.
		assert_eq!(Anchor9::TopLeft.offset(old, (200, 100)), (0, 0));
		assert_eq!(Anchor9::Center.offset(old, (201, 101)), (50, 25));
		assert_eq!(Anchor9::BottomRight.offset(old, (200, 100)), (100, 50));
		assert_eq!(Anchor9::Top.offset(old, (200, 100)), (50, 0));
		assert_eq!(Anchor9::Left.offset(old, (200, 100)), (0, 25));
		// Shrinking moves the origin the other way (negative offsets).
		assert_eq!(Anchor9::Center.offset(old, (100, 50)), (0, 0));
		assert_eq!(Anchor9::Center.offset((101, 51), (100, 50)), (0, 0), "an odd pixel of crop comes off the right");
		assert_eq!(Anchor9::Center.offset((103, 53), (100, 50)), (-1, -1));
		assert_eq!(Anchor9::BottomRight.offset(old, (40, 20)), (-60, -30));
		assert_eq!(Anchor9::Center.label(), "Centre");
	}

	#[test]
	fn translations_compose_on_the_right_side() {
		let rotation = Mapping::rotation_about(std::f64::consts::FRAC_PI_2, 10.0, 20.0);
		let moved = rotation.after_translation(100.0, -50.0);
		let point = moved.forward_point(3.0, 4.0).unwrap();
		let shifted = rotation.forward_point(103.0, -46.0).unwrap();
		assert!((point.0 - shifted.0).abs() < 1e-9 && (point.1 - shifted.1).abs() < 1e-9);
		let moved = rotation.after_destination_translation(-5.0, 7.0);
		let point = moved.forward_point(3.0, 4.0).unwrap();
		let base = rotation.forward_point(3.0, 4.0).unwrap();
		assert!((point.0 - (base.0 - 5.0)).abs() < 1e-9 && (point.1 - (base.1 + 7.0)).abs() < 1e-9);
		// A homography translates both ways too.
		let h = Mapping::Projective([1.0, 0.1, 5.0, 0.05, 1.2, -3.0, 0.0002, 0.0001, 1.0]);
		let p = h.after_translation(2.0, 3.0).forward_point(7.0, 11.0).unwrap();
		let q = h.forward_point(9.0, 14.0).unwrap();
		assert!((p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9);
		let p = h.after_destination_translation(4.0, -6.0).forward_point(7.0, 11.0).unwrap();
		let q = h.forward_point(7.0, 11.0).unwrap();
		assert!((p.0 - (q.0 + 4.0)).abs() < 1e-9 && (p.1 - (q.1 - 6.0)).abs() < 1e-9);
	}

	#[test]
	fn dest_rect_boxes_a_rotated_rectangle() {
		// A quarter turn about the origin maps [0, w] × [0, h] onto itself.
		let box_ = dest_rect(&Mapping::identity(), [0.0, 0.0, 30.0, 10.0]).unwrap();
		assert_eq!(box_, ((0, 0), (30, 10)));
		let half = Mapping::rotation_about(std::f64::consts::PI, 0.0, 0.0);
		let box_ = dest_rect(&half, [0.0, 0.0, 30.0, 10.0]).unwrap();
		assert_eq!(box_, ((-30, -10), (30, 10)));
		// A 45° turn grows the box to ⌈30·√2⌉ + 1 pixels.
		let diagonal = Mapping::rotation_about(std::f64::consts::FRAC_PI_4, 0.0, 0.0);
		let ((x, y), (w, h)) = dest_rect(&diagonal, [0.0, 0.0, 30.0, 10.0]).unwrap();
		assert_eq!((w, h), (30, 29), "ceil of the mapped box each way");
		assert_eq!((x, y), (-8, 0));
		// A warp is boxed by its control points (M6-T04).
		assert_eq!(
			dest_rect(&Mapping::Warp(BezierPatch::identity(10, 10)), [0.0, 0.0, 10.0, 10.0]),
			Some(((0, 0), (10, 10)))
		);
	}

	#[test]
	fn identity_patch_has_the_source_rect() {
		let patch = BezierPatch::identity(200, 100);
		assert_eq!(patch.src_rect, [0.0, 0.0, 200.0, 100.0]);
		assert_eq!(patch.points[0], [0.0, 0.0]);
		assert_eq!(patch.points[15], [200.0, 100.0]);
		// `points[5]` is the control point at (u, v) = (1/3, 1/3); compare with a
		// tolerance because `1/3 · 200` and `200 / 3` round differently.
		assert!((patch.points[5][0] - 200.0 / 3.0).abs() < 1e-9);
		assert!((patch.points[5][1] - 100.0 / 3.0).abs() < 1e-9);
	}
}
