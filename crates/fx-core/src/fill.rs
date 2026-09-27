//! What a fill paints (M8-T02/T06): a colour or a document pattern.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FillSource {
	/// Straight 16-bit RGBA.
	Color { rgba: [u16; 4] },
	/// A pattern of the document's `patterns` by id (M8-T06).
	Pattern { pattern: u64 },
}

/// The content of a fill layer (M8-T03/T06, D-065): drawn per tile and level
/// from these parameters, like a shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fill", rename_all = "snake_case")]
pub enum FillLayer {
	Gradient(crate::gradient::GradientLayer),
	/// A pattern of the document by id, scaled (percent) and turned
	/// (degrees) about `origin`, the document point where the pattern's own
	/// origin lies; `mirror` flips the pattern's x axis first. Origin and
	/// mirror let the fill follow the canvas (code review 2026-09-27 R03:
	/// Canvas Size, Crop and Flip move it with the pixels).
	Pattern {
		pattern: u64,
		scale: f64,
		angle: f64,
		#[serde(default, skip_serializing_if = "is_origin")]
		origin: [f64; 2],
		#[serde(default, skip_serializing_if = "std::ops::Not::not")]
		mirror: bool,
	},
}

fn is_origin(p: &[f64; 2]) -> bool {
	*p == [0.0, 0.0]
}

/// The linear map from pattern space to document space of a pattern fill:
/// `R(angle) · s · F`, with `F` the mirror (column-major `[a, b, c, d]`:
/// `x' = a·u + c·v`, `y' = b·u + d·v`).
pub fn pattern_matrix(scale: f64, angle: f64, mirror: bool) -> [f64; 4] {
	let s = (scale / 100.0).max(0.01);
	let (sn, c) = angle.to_radians().sin_cos();
	let f = if mirror { -1.0 } else { 1.0 };
	[c * s * f, sn * s * f, -sn * s, c * s]
}

/// The scale (percent), angle (degrees) and mirror nearest to the linear map
/// `m` (exact for a similarity; a non-uniform one keeps its mean scale).
pub fn pattern_parameters(m: [f64; 4]) -> (f64, f64, bool) {
	let [a, b, c, d] = m;
	let det = a * d - b * c;
	let mirror = det < 0.0;
	// Undo the mirror (negate the first column), then read the rotation.
	let (a, b) = if mirror { (-a, -b) } else { (a, b) };
	let angle = (b - c).atan2(a + d).to_degrees();
	(det.abs().sqrt() * 100.0, angle, mirror)
}

impl FillLayer {
	/// The History label of a new layer of this content.
	pub fn label(&self) -> &'static str {
		match self {
			FillLayer::Gradient(_) => "Gradient Fill",
			FillLayer::Pattern { .. } => "Pattern Fill",
		}
	}

	/// Straight RGBA at the document point `(x, y)` (a pixel centre); `(bx,
	/// by)` is the pixel, for the gradient's dither. `pattern` is the
	/// document's pattern of a pattern fill.
	pub fn color_at(
		&self,
		placed: Option<&crate::gradient::GradientFill>,
		pattern: Option<&crate::pattern::Pattern>,
		x: f64,
		y: f64,
		bx: i64,
		by: i64,
	) -> [f64; 4] {
		match self {
			FillLayer::Gradient(_) => placed.map_or([0.0; 4], |g| g.color_at_point(x, y, bx, by)),
			FillLayer::Pattern {
				scale, angle, origin, mirror, ..
			} => {
				let Some(p) = pattern else { return [0.0; 4] };
				let [a, b, c, d] = pattern_matrix(*scale, *angle, *mirror);
				let det = a * d - b * c;
				let (x, y) = (x - origin[0], y - origin[1]);
				let (u, v) = ((d * x - c * y) / det, (a * y - b * x) / det);
				p.sample(u, v)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_parameters_round_trip_through_the_matrix() {
		for (scale, angle, mirror) in [(100.0, 0.0, false), (50.0, 30.0, true), (250.0, -120.0, false), (100.0, 90.0, true)] {
			let (s, a, m) = pattern_parameters(pattern_matrix(scale, angle, mirror));
			assert!(
				(s - scale).abs() < 1e-9 && (a - angle).abs() < 1e-9 && m == mirror,
				"{scale} {angle} {mirror} → {s} {a} {m}"
			);
		}
	}
}
