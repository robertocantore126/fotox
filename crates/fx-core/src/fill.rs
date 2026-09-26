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
	/// (degrees) about the canvas origin.
	Pattern {
		pattern: u64,
		scale: f64,
		angle: f64,
	},
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
			FillLayer::Pattern { scale, angle, .. } => {
				let Some(p) = pattern else { return [0.0; 4] };
				let s = (scale / 100.0).max(0.01);
				let a = (-angle).to_radians();
				let (c, sn) = (a.cos(), a.sin());
				let (u, v) = ((x * c - y * sn) / s, (x * sn + y * c) / s);
				p.sample(u, v)
			}
		}
	}
}
