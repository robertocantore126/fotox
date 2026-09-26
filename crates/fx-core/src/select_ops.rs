//! Selections computed from the document's pixels (M9-T02..T06). The engine
//! computes them (`PixelOps::select_op`); `Command::SelectBy` combines the
//! result with the current selection.

use serde::{Deserialize, Serialize};

/// Color Range's Select menu (M9-T03).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeKind {
	#[default]
	Sampled,
	Reds,
	Yellows,
	Greens,
	Cyans,
	Blues,
	Magentas,
	Highlights,
	Midtones,
	Shadows,
	SkinTones,
	OutOfGamut,
}

/// Select and Mask's settings (M9-T05).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refine {
	/// Edge Detection radius, pixels.
	pub radius: f32,
	#[serde(default)]
	pub smart_radius: bool,
	/// `0..=100`.
	#[serde(default)]
	pub smooth: f32,
	/// Pixels.
	#[serde(default)]
	pub feather: f32,
	/// `0..=100` %.
	#[serde(default)]
	pub contrast: f32,
	/// `-100..=100` %.
	#[serde(default)]
	pub shift_edge: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectOp {
	/// Select ▸ Grow: the selection's neighbours within `tolerance` of its
	/// colours, contiguous (M9-T02).
	Grow { tolerance: f64, sample_all: bool },
	/// Select ▸ Similar: the same, everywhere (M9-T02).
	Similar { tolerance: f64, sample_all: bool },
	/// Select ▸ Color Range (M9-T03). `samples` are straight RGB `0..=1`
	/// (Sampled Colors); `fuzziness` `0..=200`; `range` (%) and `center`
	/// localize the clusters when set.
	ColorRange {
		range: RangeKind,
		#[serde(default)]
		samples: Vec<[f32; 3]>,
		fuzziness: f32,
		#[serde(default)]
		localized: Option<(f64, f64, f64)>,
		#[serde(default)]
		invert: bool,
		/// Highlights / Shadows / Midtones split points `0..=255`.
		#[serde(default)]
		low: f32,
		#[serde(default = "full")]
		high: f32,
	},
	/// Select ▸ Focus Area (M9-T04): `in_focus` `0..=1` (higher = only the
	/// sharpest), `noise` `0..=1`, `soften` on/off.
	FocusArea { in_focus: f32, noise: f32, soften: bool },
	/// Select and Mask (M9-T05) on the current selection.
	Refine(Refine),
	/// The Quick Selection tool's stroke (M9-T06): dabs `(x, y, radius)`.
	QuickSelect {
		dabs: Vec<(f64, f64, f64)>,
		sample_all: bool,
		enhance_edge: bool,
	},
}

fn full() -> f32 {
	255.0
}

impl SelectOp {
	pub fn label(&self) -> &'static str {
		match self {
			SelectOp::Grow { .. } => "Grow",
			SelectOp::Similar { .. } => "Similar",
			SelectOp::ColorRange { .. } => "Color Range",
			SelectOp::FocusArea { .. } => "Focus Area",
			SelectOp::Refine(_) => "Select and Mask",
			SelectOp::QuickSelect { .. } => "Quick Selection",
		}
	}
}
