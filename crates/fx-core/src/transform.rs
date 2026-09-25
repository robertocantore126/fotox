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
