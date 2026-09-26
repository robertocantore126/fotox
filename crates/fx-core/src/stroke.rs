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
	/// The sampled tip (M8-T01): the id of a tip registered with
	/// `fx_ops::brush::tip::register`, 0 = the round computed tip.
	#[serde(default)]
	pub tip: u64,
	/// Brush Settings' dynamics (M8-T01).
	#[serde(default)]
	pub dynamics: Dynamics,
	/// The seed of every jitter of this stroke (set at pen-down and stored
	/// with the stroke, so live = replay).
	#[serde(default)]
	pub seed: u64,
}

/// What drives a dynamic (Photoshop's "Control" drop-downs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
	#[default]
	Off,
	/// Fades from full to the minimum over `Dynamics::fade_steps` dabs.
	Fade,
	PenPressure,
	PenTilt,
}

/// The Brush Settings sections of M8-T01: Shape Dynamics, Scattering,
/// Transfer and Color Dynamics. Jitters are `0..=1` (Photoshop's percent).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dynamics {
	pub size_jitter: f32,
	pub size_control: Control,
	/// `0..=1` of the diameter: the smallest a control shrinks the tip to.
	pub min_diameter: f32,
	/// `0..=1` of 360°.
	pub angle_jitter: f32,
	pub angle_control: Control,
	pub roundness_jitter: f32,
	pub roundness_control: Control,
	pub min_roundness: f32,
	/// Dabs a fade control lasts.
	pub fade_steps: u32,
	/// Scatter distance as a fraction of the diameter (`0..=10`).
	pub scatter: f32,
	pub scatter_both_axes: bool,
	/// Dabs per spacing interval, `1..=16`.
	pub count: u32,
	pub count_jitter: f32,
	pub opacity_jitter: f32,
	pub opacity_control: Control,
	pub flow_jitter: f32,
	pub flow_control: Control,
	/// Foreground/background jitter; the stroke's colour moves toward the
	/// background by a random amount.
	pub fg_bg_jitter: f32,
	pub hue_jitter: f32,
	pub saturation_jitter: f32,
	pub brightness_jitter: f32,
	/// Colour jitter per tip (Photoshop's "Apply Per Tip"). FAST: the stroke
	/// model paints one colour per stroke, so this is read as per stroke.
	pub per_tip: bool,
}

impl Default for Dynamics {
	fn default() -> Self {
		Self {
			size_jitter: 0.0,
			size_control: Control::Off,
			min_diameter: 0.0,
			angle_jitter: 0.0,
			angle_control: Control::Off,
			roundness_jitter: 0.0,
			roundness_control: Control::Off,
			min_roundness: 0.25,
			fade_steps: 25,
			scatter: 0.0,
			scatter_both_axes: false,
			count: 1,
			count_jitter: 0.0,
			opacity_jitter: 0.0,
			opacity_control: Control::Off,
			flow_jitter: 0.0,
			flow_control: Control::Off,
			fg_bg_jitter: 0.0,
			hue_jitter: 0.0,
			saturation_jitter: 0.0,
			brightness_jitter: 0.0,
			per_tip: false,
		}
	}
}

impl Dynamics {
	/// Whether anything varies from dab to dab.
	pub fn is_static(&self) -> bool {
		self.size_jitter == 0.0
			&& self.size_control == Control::Off
			&& self.angle_jitter == 0.0
			&& self.angle_control == Control::Off
			&& self.roundness_jitter == 0.0
			&& self.roundness_control == Control::Off
			&& self.scatter == 0.0
			&& self.count <= 1
			&& self.opacity_jitter == 0.0
			&& self.opacity_control == Control::Off
			&& self.flow_jitter == 0.0
			&& self.flow_control == Control::Off
	}
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
			tip: 0,
			dynamics: Dynamics::default(),
			seed: 0,
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
	/// Dodge (M8-T04): lighten the `range`; the brush's flow is the Exposure.
	Dodge {
		range: ToneRange,
		#[serde(default)]
		protect_tones: bool,
	},
	/// Burn (M8-T04): darken the `range`.
	Burn {
		range: ToneRange,
		#[serde(default)]
		protect_tones: bool,
	},
	/// Sponge (M8-T04): saturate or desaturate; the brush's flow is the Flow.
	Sponge {
		saturate: bool,
		#[serde(default)]
		vibrance: bool,
	},
	/// Blur (M8-T05): the layer (or composite) blurred, laid down by Strength.
	Blur {
		#[serde(default)]
		sample_all: bool,
	},
	/// Sharpen (M8-T05): an unsharp step.
	Sharpen {
		#[serde(default)]
		sample_all: bool,
		#[serde(default)]
		protect_detail: bool,
	},
	/// Smudge (M8-T05): drags the colour under the brush along the path.
	Smudge {
		#[serde(default)]
		finger_painting: bool,
		#[serde(default)]
		sample_all: bool,
	},
	/// The Pattern Stamp (M8-T06): a document pattern, tiled from `origin`.
	PatternStamp {
		pattern: u64,
		origin: (i64, i64),
		#[serde(default)]
		impressionist: bool,
	},
	/// The History Brush (M8-T07): the same layer in History panel row
	/// `state` (a snapshot, D-067), laid down through the mode.
	HistoryBrush { state: usize },
	/// The Art History Brush (M8-T07): stylised strokes coloured from the
	/// state.
	ArtHistory {
		state: usize,
		style: ArtStyle,
		/// Area diameter, pixels.
		area: f32,
		/// `0..=1`: strokes only where the state differs more than this.
		tolerance: f32,
	},
	/// The Color Replacement tool (M8-T08): the stroke colour blended by
	/// `mode` (Hue / Saturation / Color / Luminosity) where a pixel matches
	/// `sample` within `tolerance`.
	ColorReplace {
		sample: [u16; 3],
		tolerance: f32,
		mode: crate::blend::BlendMode,
	},
	/// The Background Eraser (M8-T02): erase what matches `sample` (straight
	/// 16-bit RGB) within `tolerance` (`0..=1`), keeping `protect`.
	BgEraser {
		sample: [u16; 3],
		tolerance: f32,
		#[serde(default)]
		protect: Option<[u16; 3]>,
	},
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
			StrokeTool::BgEraser { .. } => "Background Eraser",
			StrokeTool::Blur { .. } => "Blur Tool",
			StrokeTool::Sharpen { .. } => "Sharpen Tool",
			StrokeTool::Smudge { .. } => "Smudge Tool",
			StrokeTool::PatternStamp { .. } => "Pattern Stamp",
			StrokeTool::ColorReplace { .. } => "Color Replacement Tool",
			StrokeTool::HistoryBrush { .. } => "History Brush",
			StrokeTool::ArtHistory { .. } => "Art History Brush",
			StrokeTool::Dodge { .. } => "Dodge Tool",
			StrokeTool::Burn { .. } => "Burn Tool",
			StrokeTool::Sponge { .. } => "Sponge Tool",
		}
	}
}

/// The Art History Brush's Style (M8-T07).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtStyle {
	#[default]
	TightShort,
	TightMedium,
	TightLong,
	LooseMedium,
	LooseLong,
	Dab,
	TightCurl,
	TightCurlLong,
	LooseCurl,
	LooseCurlLong,
}

/// Dodge / Burn's Range (M8-T04).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneRange {
	Shadows,
	#[default]
	Midtones,
	Highlights,
}

/// What a stroke paints on: the layer's pixels or its mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeTarget {
	#[default]
	Pixels,
	Mask,
}
