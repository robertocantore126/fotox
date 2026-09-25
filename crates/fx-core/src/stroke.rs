//! What a brush stroke is, as data (M5-T06/T07): the brush, the tool and the
//! pointer samples. A recorded `Command::Stroke` replays exactly the pixels
//! the live stroke painted (the samples are stored after smoothing, so the
//! replay needs no smoothing state).

use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;

/// A round brush and how it paints (Photoshop's brush options).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushParams {
	/// Tip diameter in document pixels, `1..=5000`.
	pub diameter: f32,
	/// `0..=1`: the fully opaque core's share of the radius.
	pub hardness: f32,
	/// `0..=1`: 1 = a circle, less = an ellipse squashed along `angle`.
	pub roundness: f32,
	/// Degrees, counter-clockwise.
	pub angle: f32,
	/// Distance between dabs as a fraction of the current diameter
	/// (Photoshop's Spacing, `0.01..=10`; 0.25 = 25 %).
	pub spacing: f32,
	/// `0..=1`: the most any pixel of this stroke can change (a ceiling).
	pub opacity: f32,
	/// `0..=1`: how much each dab adds to the stroke's coverage.
	pub flow: f32,
	pub mode: BlendMode,
	/// Pen pressure scales the diameter (Photoshop's size pen button).
	#[serde(default)]
	pub pressure_size: bool,
	/// Pen pressure scales each dab's strength (the opacity pen button).
	#[serde(default)]
	pub pressure_opacity: bool,
}

impl Default for BrushParams {
	fn default() -> Self {
		Self {
			diameter: 40.0,
			hardness: 1.0,
			roundness: 1.0,
			angle: 0.0,
			spacing: 0.25,
			opacity: 1.0,
			flow: 1.0,
			mode: BlendMode::Normal,
			pressure_size: false,
			pressure_opacity: false,
		}
	}
}

impl BrushParams {
	/// The parameters clamped to their ranges (a command from a macro or the
	/// UI may carry anything).
	pub fn clamped(mut self) -> Self {
		let fix = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
		self.diameter = fix(self.diameter, 1.0, 5000.0, 40.0);
		self.hardness = fix(self.hardness, 0.0, 1.0, 1.0);
		self.roundness = fix(self.roundness, 0.01, 1.0, 1.0);
		self.angle = fix(self.angle, -360.0, 360.0, 0.0);
		self.spacing = fix(self.spacing, 0.01, 10.0, 0.25);
		self.opacity = fix(self.opacity, 0.0, 1.0, 1.0);
		self.flow = fix(self.flow, 0.0, 1.0, 1.0);
		self
	}
}

/// One pointer sample of a stroke, in document pixels (after smoothing).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeSample {
	pub x: f64,
	pub y: f64,
	/// `0..=1` (a mouse reports 1).
	pub pressure: f32,
	#[serde(default)]
	pub tilt_x: f32,
	#[serde(default)]
	pub tilt_y: f32,
	#[serde(default)]
	pub time_us: u64,
}

/// Which painting tool made a stroke (M5-T07/T08).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrokeTool {
	/// The Brush (B): the colour through the blend mode.
	Brush,
	/// The Pencil: hard, aliased edges.
	Pencil,
	/// The Eraser (E): alpha down to transparency (D-043).
	Eraser,
	/// The Clone Stamp (S): each pixel from `(x − dx, y − dy)` of the source
	/// (the document as it was when the stroke started, D-044).
	Clone { dx: f64, dy: f64, sample_all: bool },
	/// The Healing Brush (J): a clone, then at pen-up a Poisson blend that
	/// keeps the source's texture and the destination's shading (D-045).
	Heal { dx: f64, dy: f64, sample_all: bool },
	/// The Spot Healing Brush (J): the source is chosen near the stroke.
	SpotHeal,
}

impl StrokeTool {
	/// The History label (Photoshop's tool names).
	pub fn label(&self) -> &'static str {
		match self {
			StrokeTool::Brush => "Brush Tool",
			StrokeTool::Pencil => "Pencil",
			StrokeTool::Eraser => "Eraser",
			StrokeTool::Clone { .. } => "Clone Stamp",
			StrokeTool::Heal { .. } => "Healing Brush",
			StrokeTool::SpotHeal => "Spot Healing Brush",
		}
	}
}

/// What a stroke paints on: the layer's pixels or its mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeTarget {
	#[default]
	Pixels,
	Mask,
}
