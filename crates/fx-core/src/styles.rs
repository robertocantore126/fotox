//! Layer styles (M6-T08, D-054): Drop Shadow, Outer Glow, Inner Shadow,
//! Color Overlay and Stroke; M12-T04 adds Inner Glow, Satin, Bevel & Emboss
//! (a shadow and a highlight pass), Gradient Overlay and Pattern Overlay.
//!
//! The parameters are the truth; each effect's pixels are a derived tile
//! cache on the layer ([`crate::Layer::effects`]), drawn by the engine from
//! the layer's alpha at the level being shown, like a shape's tiles.
//!
//! As in Photoshop, Drop Shadow, Inner Shadow, Color Overlay, Gradient
//! Overlay and Stroke may be applied several times (the dialog's **+**): each
//! effect is a list, the first entry at the top of the dialog's list and on
//! top of the others when composited. [`LayerStyles::slots`] flattens the
//! lists into the order the caches follow.
//!
//! Lengths are document pixels at level 0; angles are degrees, Photoshop's
//! convention (0° = light from the right, 90° = from above, so the shadow
//! falls down).

use serde::{Deserialize, Deserializer, Serialize};

use crate::blend::BlendMode;

/// The effects, in the order they are composited (bottom → top), which is
/// Photoshop's dialog list read from the bottom up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
	DropShadow,
	OuterGlow,
	PatternOverlay,
	GradientOverlay,
	ColorOverlay,
	Satin,
	InnerGlow,
	InnerShadow,
	Stroke,
	BevelShadow,
	BevelHighlight,
}

impl EffectKind {
	pub const ALL: [EffectKind; 11] = [
		EffectKind::DropShadow,
		EffectKind::OuterGlow,
		EffectKind::PatternOverlay,
		EffectKind::GradientOverlay,
		EffectKind::ColorOverlay,
		EffectKind::Satin,
		EffectKind::InnerGlow,
		EffectKind::InnerShadow,
		EffectKind::Stroke,
		EffectKind::BevelShadow,
		EffectKind::BevelHighlight,
	];

	/// Drawn under the layer's content (the rest are drawn over it).
	pub fn below_content(self) -> bool {
		matches!(self, EffectKind::DropShadow | EffectKind::OuterGlow)
	}

	/// Whether Photoshop lets this effect be applied more than once.
	pub fn stackable(self) -> bool {
		matches!(
			self,
			EffectKind::DropShadow | EffectKind::InnerShadow | EffectKind::ColorOverlay | EffectKind::GradientOverlay | EffectKind::Stroke
		)
	}
}

/// One effect present on a layer: its kind and its place in that kind's
/// list (0 = the top one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectSlot {
	pub kind: EffectKind,
	pub instance: usize,
}

/// The document's Global Light: the angle and altitude every effect with Use
/// Global Light follows (Layer ▸ Layer Style ▸ Global Light).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalLight {
	pub angle: f64,
	pub altitude: f64,
}

impl Default for GlobalLight {
	fn default() -> Self {
		Self { angle: 120.0, altitude: 30.0 }
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokePosition {
	Outside,
	Inside,
	Center,
}

/// What an effect is filled with: its colour (the default), a gradient or a
/// pattern (Stroke's Fill Type; the glows' gradient).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectFill {
	#[default]
	Color,
	Gradient {
		gradient: crate::gradient::GradientLayer,
		#[serde(default)]
		align: bool,
	},
	Pattern {
		pattern: u64,
		#[serde(default = "hundred")]
		scale: f64,
		#[serde(default)]
		angle: f64,
		#[serde(default)]
		phase: (f64, f64),
		#[serde(default)]
		align: bool,
	},
}

fn hundred() -> f64 {
	100.0
}

fn fifty() -> f64 {
	50.0
}

fn yes() -> bool {
	true
}

fn is_true(v: &bool) -> bool {
	*v
}

impl EffectFill {
	/// The drawing extra this fill needs (`None` for a plain colour).
	fn extra(&self) -> Option<EffectExtra> {
		match self {
			EffectFill::Color => None,
			EffectFill::Gradient { gradient, align } => Some(EffectExtra::Gradient {
				gradient: gradient.clone(),
				align: *align,
			}),
			EffectFill::Pattern { pattern, scale, angle, phase, align } => Some(EffectExtra::Pattern {
				id: *pattern,
				scale: *scale,
				angle: *angle,
				phase: *phase,
				align: *align,
			}),
		}
	}

	/// A glow's gradient: coloured along the glow's falloff, not placed on
	/// the canvas (Photoshop's glow gradient runs from the edge outward).
	fn glow_gradient(&self) -> Option<(crate::gradient::Gradient, bool)> {
		match self {
			EffectFill::Gradient { gradient, .. } => Some((gradient.gradient.clone(), gradient.reverse)),
			_ => None,
		}
	}
}

/// A glow's Technique: Softer blurs the matte; Precise measures the distance
/// to the edge, so hard shapes keep their corners.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlowTechnique {
	#[default]
	Softer,
	Precise,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DropShadow {
	pub enabled: bool,
	pub blend: BlendMode,
	pub color: [u16; 4],
	pub opacity: f32,
	pub angle: f64,
	pub use_global_light: bool,
	pub distance: f64,
	/// Percent of `size` that is dilation instead of blur (Photoshop's Spread).
	pub spread: f64,
	pub size: f64,
	/// Percent of grain mixed into the shadow (Photoshop's Noise).
	#[serde(default)]
	pub noise: f64,
	/// Contour: how the effect's falloff is shaped (Linear = as computed).
	#[serde(default)]
	pub contour: Contour,
	#[serde(default)]
	pub anti_aliased: bool,
	/// Layer Knocks Out Drop Shadow: the shadow does not show through the
	/// layer's own (semi-transparent) pixels. On by default, as in Photoshop.
	#[serde(default = "yes")]
	pub knocks_out: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OuterGlow {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	pub color: [u16; 4],
	pub spread: f64,
	pub size: f64,
	#[serde(default)]
	pub noise: f64,
	/// A gradient instead of the colour.
	#[serde(default)]
	pub fill: EffectFill,
	/// Contour: how the effect's falloff is shaped (Linear = as computed).
	#[serde(default)]
	pub contour: Contour,
	#[serde(default)]
	pub anti_aliased: bool,
	#[serde(default)]
	pub technique: GlowTechnique,
	/// Percent of the falloff the contour spans.
	#[serde(default = "fifty")]
	pub range: f64,
	/// Percent: how much the gradient's colour is scattered.
	#[serde(default)]
	pub jitter: f64,
}

/// Where an Inner Glow starts (Photoshop's Source).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlowSource {
	/// From the edges inward.
	#[default]
	Edge,
	/// From the centre outward.
	Center,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InnerShadow {
	pub enabled: bool,
	pub blend: BlendMode,
	pub color: [u16; 4],
	pub opacity: f32,
	pub angle: f64,
	pub use_global_light: bool,
	pub distance: f64,
	/// Percent of `size` that is erosion instead of blur (Photoshop's Choke).
	pub choke: f64,
	pub size: f64,
	/// Contour: how the effect's falloff is shaped (Linear = as computed).
	#[serde(default)]
	pub contour: Contour,
	#[serde(default)]
	pub noise: f64,
	#[serde(default)]
	pub anti_aliased: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorOverlay {
	pub enabled: bool,
	pub blend: BlendMode,
	pub color: [u16; 4],
	pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
	pub enabled: bool,
	pub size: f64,
	pub position: StrokePosition,
	pub blend: BlendMode,
	pub opacity: f32,
	pub color: [u16; 4],
	/// Fill Type: Color, Gradient or Pattern.
	#[serde(default)]
	pub fill: EffectFill,
}

/// The most points a custom contour keeps (Photoshop's editor allows about
/// as many before they crowd the 0..255 square).
pub const CONTOUR_POINTS: usize = 16;

/// Photoshop's contour presets, or a curve drawn in the Contour Editor: a
/// map from 0..=1 to 0..=1 applied to an effect's falloff (shadows, glows,
/// satin), to a bevel's height profile, or to its lighting (Gloss Contour).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Contour {
	#[default]
	Linear,
	Cone,
	ConeInverted,
	CoveDeep,
	CoveShallow,
	Gaussian,
	HalfRound,
	Ring,
	RingDouble,
	RollingSlope,
	RoundedSteps,
	Sawtooth,
	/// A curve from the Contour Editor: `len` points `[input, output]` on
	/// 0..=255, sorted by input, joined by a smooth (Catmull-Rom) curve.
	Custom {
		points: [[u8; 2]; CONTOUR_POINTS],
		len: u8,
	},
}

impl Contour {
	/// The presets, in Photoshop's picker order.
	pub const PRESETS: [Contour; 12] = [
		Contour::Linear,
		Contour::Cone,
		Contour::ConeInverted,
		Contour::CoveDeep,
		Contour::CoveShallow,
		Contour::Gaussian,
		Contour::HalfRound,
		Contour::Ring,
		Contour::RingDouble,
		Contour::RollingSlope,
		Contour::RoundedSteps,
		Contour::Sawtooth,
	];

	/// A custom contour through `points` (0..=255 each; sorted, clipped to
	/// [`CONTOUR_POINTS`], at least the two ends).
	pub fn custom(points: &[[u8; 2]]) -> Contour {
		let mut sorted: Vec<[u8; 2]> = points.to_vec();
		sorted.sort_by_key(|p| p[0]);
		sorted.dedup_by_key(|p| p[0]);
		sorted.truncate(CONTOUR_POINTS);
		if sorted.len() < 2 {
			return Contour::Linear;
		}
		let mut out = [[0u8; 2]; CONTOUR_POINTS];
		out[..sorted.len()].copy_from_slice(&sorted);
		Contour::Custom {
			points: out,
			len: sorted.len() as u8,
		}
	}

	/// The curve at `t` (0..=1). VERIFY: the presets' shapes are read off
	/// Photoshop's preset thumbnails, not its exact curves.
	pub fn apply(self, t: f64) -> f64 {
		let t = t.clamp(0.0, 1.0);
		let smooth = |x: f64| x * x * (3.0 - 2.0 * x);
		let peak = |c: f64, w: f64| (-((t - c) / w).powi(2)).exp();
		let v = match self {
			Contour::Linear => t,
			Contour::Cone => 1.0 - (2.0 * t - 1.0).abs(),
			Contour::ConeInverted => (2.0 * t - 1.0).abs(),
			Contour::CoveDeep => t.powi(3),
			Contour::CoveShallow => t.powf(1.6),
			Contour::Gaussian => smooth(t),
			Contour::HalfRound => (1.0 - (1.0 - t).powi(2)).sqrt(),
			Contour::Ring => peak(0.5, 0.18),
			Contour::RingDouble => peak(0.3, 0.1).max(peak(0.75, 0.1)),
			Contour::RollingSlope => (1.0 - t + 0.2 * (2.0 * std::f64::consts::PI * t).sin()).clamp(0.0, 1.0),
			Contour::RoundedSteps => {
				let n = 4.0;
				((t * n).floor() + smooth((t * n).fract())) / n
			}
			Contour::Sawtooth => (t * 3.0).fract(),
			Contour::Custom { points, len } => custom_curve(&points[..usize::from(len).clamp(2, CONTOUR_POINTS)], t),
		};
		v.clamp(0.0, 1.0)
	}

	/// [`apply`](Self::apply), anti-aliased: the curve averaged over a small
	/// window, so steps and sharp peaks (Rounded Steps, Sawtooth, a custom
	/// corner) do not print as hard rings.
	pub fn apply_smooth(self, t: f64, anti_aliased: bool) -> f64 {
		if !anti_aliased || self == Contour::Linear {
			return self.apply(t);
		}
		const H: f64 = 0.012;
		(self.apply(t - H) + 2.0 * self.apply(t) + self.apply(t + H)) / 4.0
	}
}

/// A Catmull-Rom curve through `points` (0..=255, sorted by input), flat
/// beyond the first and last.
fn custom_curve(points: &[[u8; 2]], t: f64) -> f64 {
	// Sorted with one point per input, whatever a file holds (the editor and
	// `Contour::custom` already send them so).
	let mut p: Vec<(f64, f64)> = points.iter().map(|q| (f64::from(q[0]) / 255.0, f64::from(q[1]) / 255.0)).collect();
	p.sort_by(|a, b| a.0.total_cmp(&b.0));
	p.dedup_by(|a, b| a.0 == b.0);
	let n = p.len();
	if n < 2 {
		return p.first().map_or(t, |q| q.1);
	}
	if t <= p[0].0 {
		return p[0].1;
	}
	if t >= p[n - 1].0 {
		return p[n - 1].1;
	}
	let i = p.windows(2).position(|w| t >= w[0].0 && t <= w[1].0).unwrap_or(0);
	let (p1, p2) = (p[i], p[i + 1]);
	let p0 = if i > 0 { p[i - 1] } else { (2.0 * p1.0 - p2.0, 2.0 * p1.1 - p2.1) };
	let p3 = if i + 2 < n { p[i + 2] } else { (2.0 * p2.0 - p1.0, 2.0 * p2.1 - p1.1) };
	let span = (p2.0 - p1.0).max(1e-9);
	let u = (t - p1.0) / span;
	// Tangents in output per unit of `u` (the spans may differ in width).
	let m1 = (p2.1 - p0.1) / (p2.0 - p0.0).max(1e-9) * span;
	let m2 = (p3.1 - p1.1) / (p3.0 - p1.0).max(1e-9) * span;
	let (u2, u3) = (u * u, u * u * u);
	(2.0 * u3 - 3.0 * u2 + 1.0) * p1.1 + (u3 - 2.0 * u2 + u) * m1 + (-2.0 * u3 + 3.0 * u2) * p2.1 + (u3 - u2) * m2
}

/// Bevel & Emboss ▸ Technique.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BevelTechnique {
	#[default]
	Smooth,
	ChiselHard,
	ChiselSoft,
}

/// Bevel & Emboss ▸ Texture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BevelTexture {
	pub pattern: u64,
	/// Percent.
	#[serde(default = "hundred")]
	pub scale: f64,
	/// Percent, -1000..=1000.
	#[serde(default = "hundred")]
	pub depth: f64,
	#[serde(default)]
	pub invert: bool,
	/// Link with Layer: the pattern moves with the layer's box.
	#[serde(default = "yes")]
	pub align: bool,
	/// Where the pattern starts (Snap to Origin resets it), document pixels.
	#[serde(default)]
	pub phase: (f64, f64),
}

/// Bevel & Emboss styles (M12-T04).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BevelStyle {
	InnerBevel,
	OuterBevel,
	Emboss,
	PillowEmboss,
	/// Embosses the layer's Stroke effect (nothing without one).
	StrokeEmboss,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BevelEmboss {
	pub enabled: bool,
	pub style: BevelStyle,
	/// Percent (1..=1000).
	pub depth: f64,
	/// Direction Up (true) / Down.
	pub up: bool,
	pub size: f64,
	pub soften: f64,
	pub angle: f64,
	pub use_global_light: bool,
	/// Degrees above the horizon.
	pub altitude: f64,
	pub highlight_blend: BlendMode,
	pub highlight_color: [u16; 4],
	pub highlight_opacity: f32,
	pub shadow_blend: BlendMode,
	pub shadow_color: [u16; 4],
	pub shadow_opacity: f32,
	/// Technique: Smooth, Chisel Hard, Chisel Soft.
	#[serde(default)]
	pub technique: BevelTechnique,
	/// Structure ▸ Contour: the height profile across the bevel.
	#[serde(default)]
	pub contour: Contour,
	/// Contour ▸ Range: percent of the bevel the contour spans.
	#[serde(default = "hundred")]
	pub contour_range: f64,
	#[serde(default)]
	pub contour_anti_aliased: bool,
	/// Shading ▸ Gloss Contour: how the lighting maps to highlight and shadow.
	#[serde(default)]
	pub gloss_contour: Contour,
	#[serde(default)]
	pub anti_aliased: bool,
	/// Texture: a pattern pressed into the surface.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub texture: Option<BevelTexture>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InnerGlow {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	pub color: [u16; 4],
	pub choke: f64,
	pub size: f64,
	#[serde(default)]
	pub noise: f64,
	#[serde(default)]
	pub source: GlowSource,
	/// Contour: how the effect's falloff is shaped (Linear = as computed).
	#[serde(default)]
	pub contour: Contour,
	/// A gradient instead of the colour.
	#[serde(default)]
	pub fill: EffectFill,
	#[serde(default)]
	pub anti_aliased: bool,
	#[serde(default)]
	pub technique: GlowTechnique,
	#[serde(default = "fifty")]
	pub range: f64,
	#[serde(default)]
	pub jitter: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Satin {
	pub enabled: bool,
	pub blend: BlendMode,
	pub color: [u16; 4],
	pub opacity: f32,
	pub angle: f64,
	pub distance: f64,
	pub size: f64,
	pub invert: bool,
	/// Contour: how the effect's falloff is shaped (Linear = as computed).
	#[serde(default)]
	pub contour: Contour,
	#[serde(default)]
	pub anti_aliased: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientOverlay {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	/// The gradient, angle, scale… placed over the canvas, or over the
	/// layer's bounds with `align`.
	pub gradient: crate::gradient::GradientLayer,
	/// Photoshop's "Align with Layer": the gradient spans the layer's
	/// content box instead of the canvas (the default in Photoshop; false
	/// for styles saved before it existed).
	#[serde(default)]
	pub align: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternOverlay {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	/// A document pattern's id (`Document::patterns`).
	pub pattern: u64,
	/// Percent.
	pub scale: f64,
	/// Degrees, counter-clockwise.
	#[serde(default)]
	pub angle: f64,
	/// Offset of the pattern's origin, document pixels (Photoshop's phase,
	/// what dragging the pattern in the dialog changes).
	#[serde(default)]
	pub phase: (f64, f64),
	/// Photoshop's "Link with Layer": the pattern's origin is the layer's
	/// top-left corner instead of the canvas's.
	#[serde(default)]
	pub align: bool,
}

impl Default for BevelEmboss {
	fn default() -> Self {
		Self {
			enabled: true,
			style: BevelStyle::InnerBevel,
			depth: 100.0,
			up: true,
			size: 5.0,
			soften: 0.0,
			angle: 120.0,
			use_global_light: true,
			altitude: 30.0,
			highlight_blend: BlendMode::Screen,
			highlight_color: [65_535; 4],
			highlight_opacity: 0.75,
			shadow_blend: BlendMode::Multiply,
			shadow_color: [0, 0, 0, 65_535],
			shadow_opacity: 0.75,
			technique: BevelTechnique::Smooth,
			contour: Contour::Linear,
			contour_range: 100.0,
			contour_anti_aliased: false,
			gloss_contour: Contour::Linear,
			anti_aliased: false,
			texture: None,
		}
	}
}

impl Default for InnerGlow {
	fn default() -> Self {
		Self {
			enabled: true,
			blend: BlendMode::Screen,
			opacity: 0.75,
			color: [65_535, 65_535, 48_830, 65_535],
			choke: 0.0,
			size: 5.0,
			noise: 0.0,
			source: GlowSource::Edge,
			contour: Contour::Linear,
			fill: EffectFill::Color,
			anti_aliased: false,
			technique: GlowTechnique::Softer,
			range: 50.0,
			jitter: 0.0,
		}
	}
}

impl Default for Satin {
	fn default() -> Self {
		Self {
			enabled: true,
			blend: BlendMode::Multiply,
			color: [0, 0, 0, 65_535],
			opacity: 0.5,
			angle: 19.0,
			distance: 11.0,
			size: 14.0,
			invert: true,
			contour: Contour::Linear,
			anti_aliased: false,
		}
	}
}

impl Default for DropShadow {
	fn default() -> Self {
		Self {
			enabled: true,
			blend: BlendMode::Multiply,
			color: [0, 0, 0, 65_535],
			opacity: 0.75,
			angle: 120.0,
			use_global_light: true,
			distance: 5.0,
			spread: 0.0,
			size: 5.0,
			noise: 0.0,
			contour: Contour::Linear,
			anti_aliased: false,
			knocks_out: true,
		}
	}
}

impl Default for OuterGlow {
	fn default() -> Self {
		Self {
			enabled: true,
			blend: BlendMode::Screen,
			opacity: 0.75,
			color: [65_535, 65_535, 48_830, 65_535],
			spread: 0.0,
			size: 5.0,
			noise: 0.0,
			fill: EffectFill::Color,
			contour: Contour::Linear,
			anti_aliased: false,
			technique: GlowTechnique::Softer,
			range: 50.0,
			jitter: 0.0,
		}
	}
}

impl Default for InnerShadow {
	fn default() -> Self {
		Self {
			enabled: true,
			blend: BlendMode::Multiply,
			color: [0, 0, 0, 65_535],
			opacity: 0.75,
			angle: 120.0,
			use_global_light: true,
			distance: 5.0,
			choke: 0.0,
			size: 5.0,
			contour: Contour::Linear,
			noise: 0.0,
			anti_aliased: false,
		}
	}
}

impl Default for ColorOverlay {
	fn default() -> Self {
		Self {
			enabled: true,
			blend: BlendMode::Normal,
			color: [65_535, 0, 0, 65_535],
			opacity: 1.0,
		}
	}
}

impl Default for Stroke {
	fn default() -> Self {
		Self {
			enabled: true,
			size: 3.0,
			position: StrokePosition::Outside,
			blend: BlendMode::Normal,
			opacity: 1.0,
			color: [0, 0, 0, 65_535],
			fill: EffectFill::Color,
		}
	}
}

/// An effect list read from either form: a list (since the dialog's **+**),
/// a single effect (older files and styles), or nothing.
fn one_or_many<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de>,
{
	#[derive(Deserialize)]
	#[serde(untagged)]
	enum OneOrMany<T> {
		Many(Vec<T>),
		One(T),
	}
	Ok(match Option::<OneOrMany<T>>::deserialize(d)? {
		None => Vec::new(),
		Some(OneOrMany::Many(v)) => v,
		Some(OneOrMany::One(t)) => vec![t],
	})
}

fn all_channels() -> [bool; 3] {
	[true; 3]
}

fn all_on(c: &[bool; 3]) -> bool {
	*c == [true; 3]
}

/// A layer's styles: each effect is a list (usually of one), each entry with
/// `enabled` for the panel's eye; plus the Blending Options that belong to
/// the style in Photoshop's descriptor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerStyles {
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub drop_shadow: Vec<DropShadow>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub outer_glow: Vec<OuterGlow>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub inner_shadow: Vec<InnerShadow>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub color_overlay: Vec<ColorOverlay>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub stroke: Vec<Stroke>,
	// M12-T04.
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub bevel: Vec<BevelEmboss>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub inner_glow: Vec<InnerGlow>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub satin: Vec<Satin>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub gradient_overlay: Vec<GradientOverlay>,
	#[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
	pub pattern_overlay: Vec<PatternOverlay>,
	/// Blending Options ▸ Blend If (Gray): where this layer shows, by its own
	/// grey and by the grey under it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub blend_if: Option<BlendIf>,
	/// Blending Options ▸ Channels: R, G, B. An unticked channel keeps the
	/// backdrop's value (the layer and its effects leave it alone).
	#[serde(default = "all_channels", skip_serializing_if = "all_on")]
	pub channels: [bool; 3],
	/// Blend Interior Effects as Group: the overlays, satin, inner glow and
	/// inner shadow are blended with the content first, and the layer's blend
	/// mode applies to the result.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub interior_as_group: bool,
	/// Layer Mask Hides Effects: the effects are drawn from the unmasked
	/// content and then hidden by the mask. Off (Photoshop's default), they
	/// follow the masked shape and the mask does not cut them.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub layer_mask_hides: bool,
	/// Vector Mask Hides Effects, likewise for the vector mask.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub vector_mask_hides: bool,
	/// The Layers panel's "Effects" eye (Layer ▸ Layer Style ▸ Hide All
	/// Effects): off hides every effect and keeps them.
	#[serde(default = "yes", skip_serializing_if = "is_true")]
	pub effects_visible: bool,
}

impl Default for LayerStyles {
	fn default() -> Self {
		Self {
			drop_shadow: Vec::new(),
			outer_glow: Vec::new(),
			inner_shadow: Vec::new(),
			color_overlay: Vec::new(),
			stroke: Vec::new(),
			bevel: Vec::new(),
			inner_glow: Vec::new(),
			satin: Vec::new(),
			gradient_overlay: Vec::new(),
			pattern_overlay: Vec::new(),
			blend_if: None,
			channels: [true; 3],
			interior_as_group: false,
			layer_mask_hides: false,
			vector_mask_hides: false,
			effects_visible: true,
		}
	}
}

/// Blend If's two sliders, each `[black low, black high, white low, white
/// high]` on 0..=255: below the black pair and above the white pair the layer
/// does not show; between the split halves (Alt-drag in Photoshop) it fades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlendIf {
	pub this_layer: [u8; 4],
	pub underlying: [u8; 4],
}

impl Default for BlendIf {
	fn default() -> Self {
		Self {
			this_layer: [0, 0, 255, 255],
			underlying: [0, 0, 255, 255],
		}
	}
}

impl BlendIf {
	/// Whether it changes anything (both sliders fully open).
	pub fn is_identity(&self) -> bool {
		self.this_layer == [0, 0, 255, 255] && self.underlying == [0, 0, 255, 255]
	}

	/// How much of the layer shows for a grey `v` (0..=1) on one slider.
	pub fn factor(range: [u8; 4], v: f64) -> f64 {
		let v = v.clamp(0.0, 1.0) * 255.0;
		let [b0, b1, w0, w1] = range.map(f64::from);
		let black = if v < b0 {
			0.0
		} else if v >= b1 {
			1.0
		} else {
			(v - b0) / (b1 - b0).max(1e-9)
		};
		let white = if v > w1 {
			0.0
		} else if v <= w0 {
			1.0
		} else {
			(w1 - v) / (w1 - w0).max(1e-9)
		};
		black * white
	}
}

/// What an effect needs beyond a colour and a coverage.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum EffectExtra {
	#[default]
	None,
	/// A glow: from the centre instead of the edges (Inner Glow's Source), and
	/// the gradient coloured along the falloff (`reverse` flips it).
	Glow {
		center: bool,
		gradient: Option<(crate::gradient::Gradient, bool)>,
	},
	/// Satin: the shift of the two copies (document pixels) and Invert.
	Satin {
		offset: (f64, f64),
		invert: bool,
	},
	/// Bevel & Emboss: the pass (highlight or shadow) of this style.
	Bevel {
		style: BevelStyle,
		depth: f64,
		up: bool,
		size: f64,
		soften: f64,
		/// Light direction: azimuth and altitude, radians.
		light: (f64, f64),
		highlight: bool,
		technique: BevelTechnique,
		contour: Contour,
		/// Contour ▸ Range, 0..=1.
		contour_range: f64,
		contour_anti_aliased: bool,
		gloss: Contour,
		anti_aliased: bool,
		texture: Option<BevelTexture>,
		/// Stroke Emboss: the stroke the bevel follows (size, position).
		stroke: Option<(f64, StrokePosition)>,
	},
	Gradient {
		gradient: crate::gradient::GradientLayer,
		align: bool,
	},
	Pattern {
		id: u64,
		scale: f64,
		angle: f64,
		phase: (f64, f64),
		align: bool,
	},
}

impl EffectExtra {
	/// Whether the effect is placed relative to the layer's bounds.
	pub fn aligned(&self) -> bool {
		matches!(
			self,
			EffectExtra::Gradient { align: true, .. } | EffectExtra::Pattern { align: true, .. } | EffectExtra::Bevel { texture: Some(BevelTexture { align: true, .. }), .. }
		)
	}
}

/// How an effect's falloff is finished: its contour, grain, glow technique
/// and range, and the drop shadow's knockout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
	pub contour: Contour,
	pub anti_aliased: bool,
	/// 0..=1.
	pub noise: f32,
	/// Glows: the part of the falloff (0..=1) the contour spans.
	pub range: f64,
	/// Glows: gradient scatter, 0..=1.
	pub jitter: f64,
	/// Glows: Precise instead of Softer.
	pub precise: bool,
	/// Drop Shadow: Layer Knocks Out Drop Shadow.
	pub knocks_out: bool,
}

impl Default for Quality {
	fn default() -> Self {
		Self {
			contour: Contour::Linear,
			anti_aliased: false,
			noise: 0.0,
			range: 1.0,
			jitter: 0.0,
			precise: false,
			knocks_out: false,
		}
	}
}

/// The parameters of one enabled effect, resolved for drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectParams {
	pub kind: EffectKind,
	pub blend: BlendMode,
	pub opacity: f32,
	pub color: [u16; 4],
	/// Shift of the source alpha, document pixels (shadows).
	pub offset: (f64, f64),
	/// Dilation (> 0) or erosion (< 0) radius before the blur.
	pub morph: f64,
	/// Blur radius (Photoshop's "Size" minus the spread part).
	pub blur: f64,
	pub stroke: Option<(f64, StrokePosition)>,
	pub extra: EffectExtra,
	pub quality: Quality,
}

fn percent(v: f64) -> f64 {
	v.clamp(0.0, 100.0) / 100.0
}

impl LayerStyles {
	/// Whether any effect would draw.
	pub fn any_enabled(&self) -> bool {
		self.slots().into_iter().any(|slot| self.effect_at(slot, GlobalLight::default()).is_some())
	}

	/// Whether the style holds any effect at all (enabled or not).
	pub fn has_effects(&self) -> bool {
		EffectKind::ALL.into_iter().any(|kind| self.count(kind) > 0)
	}

	/// How many instances of `kind` the layer has (the bevel's two passes
	/// share its list).
	pub fn count(&self, kind: EffectKind) -> usize {
		match kind {
			EffectKind::DropShadow => self.drop_shadow.len(),
			EffectKind::OuterGlow => self.outer_glow.len(),
			EffectKind::PatternOverlay => self.pattern_overlay.len(),
			EffectKind::GradientOverlay => self.gradient_overlay.len(),
			EffectKind::ColorOverlay => self.color_overlay.len(),
			EffectKind::Satin => self.satin.len(),
			EffectKind::InnerGlow => self.inner_glow.len(),
			EffectKind::InnerShadow => self.inner_shadow.len(),
			EffectKind::Stroke => self.stroke.len(),
			EffectKind::BevelShadow | EffectKind::BevelHighlight => self.bevel.len(),
		}
	}

	/// Every effect present, enabled or not, in compositing order (bottom →
	/// top): the layer's effect cache `i` draws `slots()[i]`. Within a kind
	/// the list's first entry is composited last, on top.
	pub fn slots(&self) -> Vec<EffectSlot> {
		let mut out = Vec::new();
		for kind in EffectKind::ALL {
			for instance in (0..self.count(kind)).rev() {
				out.push(EffectSlot { kind, instance });
			}
		}
		out
	}

	/// Empty effect caches for these styles, one per slot.
	pub fn caches(&self, width: u32, height: u32, format: fx_tiles::PixelFormat) -> Vec<fx_tiles::TiledImage> {
		(0..self.slots().len()).map(|_| fx_tiles::TiledImage::derived(width, height, format)).collect()
	}

	/// Only the effect in `slot`, drawn plainly: enabled, Normal, fully
	/// opaque (Create Layers renders each effect this way, then gives the new
	/// layer the effect's mode and opacity). A bevel keeps only the pass
	/// `slot` names; Stroke Emboss keeps the strokes it follows, invisible.
	pub fn only(&self, slot: EffectSlot) -> LayerStyles {
		let i = slot.instance;
		let mut out = LayerStyles {
			layer_mask_hides: self.layer_mask_hides,
			vector_mask_hides: self.vector_mask_hides,
			..LayerStyles::default()
		};
		match slot.kind {
			EffectKind::DropShadow => out.drop_shadow = self.drop_shadow.get(i).map(|e| DropShadow { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::OuterGlow => out.outer_glow = self.outer_glow.get(i).map(|e| OuterGlow { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::InnerShadow => out.inner_shadow = self.inner_shadow.get(i).map(|e| InnerShadow { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::ColorOverlay => out.color_overlay = self.color_overlay.get(i).map(|e| ColorOverlay { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::Stroke => out.stroke = self.stroke.get(i).map(|e| Stroke { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::InnerGlow => out.inner_glow = self.inner_glow.get(i).map(|e| InnerGlow { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::Satin => out.satin = self.satin.get(i).map(|e| Satin { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::GradientOverlay => out.gradient_overlay = self.gradient_overlay.get(i).map(|e| GradientOverlay { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::PatternOverlay => out.pattern_overlay = self.pattern_overlay.get(i).map(|e| PatternOverlay { enabled: true, blend: BlendMode::Normal, opacity: 1.0, ..e.clone() }).into_iter().collect(),
			EffectKind::BevelShadow | EffectKind::BevelHighlight => {
				let highlight = slot.kind == EffectKind::BevelHighlight;
				out.bevel = self
					.bevel
					.get(i)
					.map(|e| BevelEmboss {
						enabled: true,
						highlight_blend: BlendMode::Normal,
						shadow_blend: BlendMode::Normal,
						highlight_opacity: if highlight { 1.0 } else { 0.0 },
						shadow_opacity: if highlight { 0.0 } else { 1.0 },
						..e.clone()
					})
					.into_iter()
					.collect();
				if out.bevel.first().is_some_and(|b| b.style == BevelStyle::StrokeEmboss) {
					out.stroke = self.stroke.iter().map(|s| Stroke { opacity: 0.0, ..s.clone() }).collect();
				}
			}
		}
		out
	}

	/// Photoshop's name for a layer made from effect `slot` of `layer`.
	pub fn layer_name(&self, slot: EffectSlot, layer: &str) -> String {
		let what = match slot.kind {
			EffectKind::DropShadow => "Drop Shadow",
			EffectKind::OuterGlow => "Outer Glow",
			EffectKind::InnerShadow => "Inner Shadow",
			EffectKind::InnerGlow => "Inner Glow",
			EffectKind::Satin => "Satin",
			EffectKind::ColorOverlay => "Color Fill",
			EffectKind::GradientOverlay => "Gradient Fill",
			EffectKind::PatternOverlay => "Pattern Fill",
			EffectKind::BevelShadow => "Bevel Shadows",
			EffectKind::BevelHighlight => "Bevel Highlights",
			EffectKind::Stroke => match self.stroke.get(slot.instance).map(|s| s.position) {
				Some(StrokePosition::Inside) => "Inner Stroke",
				_ => "Outer Stroke",
			},
		};
		format!("{layer}'s {what}")
	}

	/// Whether a layer made from `slot` stays inside the layer's shape
	/// (Create Layers clips those to the layer, as Photoshop does).
	pub fn stays_inside(&self, slot: EffectSlot) -> bool {
		match slot.kind {
			EffectKind::DropShadow | EffectKind::OuterGlow => false,
			EffectKind::Stroke => self.stroke.get(slot.instance).is_some_and(|s| s.position == StrokePosition::Inside),
			EffectKind::BevelShadow | EffectKind::BevelHighlight => self.bevel.get(slot.instance).is_some_and(|b| b.style == BevelStyle::InnerBevel),
			_ => true,
		}
	}

	/// The ids of every pattern the style draws with: Pattern Overlay, a
	/// pattern-filled stroke or glow, Bevel ▸ Texture (the engine copies them
	/// from the library into the document before the style is set).
	pub fn patterns(&self) -> Vec<u64> {
		let fill = |f: &EffectFill| match f {
			EffectFill::Pattern { pattern, .. } => Some(*pattern),
			_ => None,
		};
		let mut out: Vec<u64> = self.pattern_overlay.iter().map(|e| e.pattern).collect();
		out.extend(self.stroke.iter().filter_map(|e| fill(&e.fill)));
		out.extend(self.outer_glow.iter().filter_map(|e| fill(&e.fill)));
		out.extend(self.inner_glow.iter().filter_map(|e| fill(&e.fill)));
		out.extend(self.bevel.iter().filter_map(|e| e.texture.as_ref().map(|t| t.pattern)));
		out.sort_unstable();
		out.dedup();
		out
	}

	/// Every colour the style holds (for colour-mode conversions).
	pub fn colors_mut(&mut self, f: &mut dyn FnMut(&mut [u16; 4])) {
		self.drop_shadow.iter_mut().for_each(|e| f(&mut e.color));
		self.outer_glow.iter_mut().for_each(|e| f(&mut e.color));
		self.inner_shadow.iter_mut().for_each(|e| f(&mut e.color));
		self.color_overlay.iter_mut().for_each(|e| f(&mut e.color));
		self.stroke.iter_mut().for_each(|e| f(&mut e.color));
		self.inner_glow.iter_mut().for_each(|e| f(&mut e.color));
		self.satin.iter_mut().for_each(|e| f(&mut e.color));
		for e in &mut self.bevel {
			f(&mut e.highlight_color);
			f(&mut e.shadow_color);
		}
	}

	/// Scale Effects: every size and distance times `factor` (Layer ▸ Layer
	/// Style ▸ Scale Effects, and scaling a layer with its styles). A
	/// non-finite factor leaves the style as it is.
	pub fn scale(&mut self, factor: f64) {
		if !factor.is_finite() {
			return;
		}
		let k = factor.max(0.0);
		for e in &mut self.drop_shadow {
			e.distance *= k;
			e.size *= k;
		}
		for e in &mut self.inner_shadow {
			e.distance *= k;
			e.size *= k;
		}
		for e in &mut self.outer_glow {
			e.size *= k;
		}
		for e in &mut self.inner_glow {
			e.size *= k;
		}
		for e in &mut self.stroke {
			e.size *= k;
		}
		for e in &mut self.satin {
			e.distance *= k;
			e.size *= k;
		}
		for e in &mut self.bevel {
			e.size *= k;
			e.soften *= k;
			if let Some(t) = &mut e.texture {
				t.scale *= k;
			}
		}
		for e in &mut self.gradient_overlay {
			e.gradient.scale = (e.gradient.scale * k).clamp(1.0, 1000.0);
		}
		for e in &mut self.pattern_overlay {
			e.scale = (e.scale * k).clamp(1.0, 1000.0);
		}
	}

	/// The effect in `slot` if it is enabled (and effects are shown),
	/// resolved with the document's Global Light.
	pub fn effect_at(&self, slot: EffectSlot, light: GlobalLight) -> Option<EffectParams> {
		if !self.effects_visible {
			return None;
		}
		let offset = |angle: f64, global: bool, distance: f64| {
			let a = if global { light.angle } else { angle }.to_radians();
			// Light from `a`: the shadow is cast the other way (y grows down).
			(-a.cos() * distance, a.sin() * distance)
		};
		let kind = slot.kind;
		let i = slot.instance;
		let base = |blend, opacity, color| EffectParams {
			kind,
			blend,
			opacity,
			color,
			offset: (0.0, 0.0),
			morph: 0.0,
			blur: 0.0,
			stroke: None,
			extra: EffectExtra::None,
			quality: Quality::default(),
		};
		match kind {
			EffectKind::DropShadow => self.drop_shadow.get(i).filter(|e| e.enabled).map(|e| EffectParams {
				offset: offset(e.angle, e.use_global_light, e.distance),
				morph: e.size * percent(e.spread),
				blur: e.size * (1.0 - percent(e.spread)),
				quality: Quality {
					contour: e.contour,
					anti_aliased: e.anti_aliased,
					noise: percent(e.noise) as f32,
					knocks_out: e.knocks_out,
					..Quality::default()
				},
				..base(e.blend, e.opacity, e.color)
			}),
			EffectKind::OuterGlow => self.outer_glow.get(i).filter(|e| e.enabled).map(|e| EffectParams {
				morph: e.size * percent(e.spread),
				blur: e.size * (1.0 - percent(e.spread)),
				extra: EffectExtra::Glow {
					center: false,
					gradient: e.fill.glow_gradient(),
				},
				quality: Quality {
					contour: e.contour,
					anti_aliased: e.anti_aliased,
					noise: percent(e.noise) as f32,
					range: percent(e.range).max(0.01),
					jitter: percent(e.jitter),
					precise: e.technique == GlowTechnique::Precise,
					knocks_out: false,
				},
				..base(e.blend, e.opacity, e.color)
			}),
			EffectKind::InnerShadow => self.inner_shadow.get(i).filter(|e| e.enabled).map(|e| EffectParams {
				offset: offset(e.angle, e.use_global_light, e.distance),
				morph: e.size * percent(e.choke),
				blur: e.size * (1.0 - percent(e.choke)),
				quality: Quality {
					contour: e.contour,
					anti_aliased: e.anti_aliased,
					noise: percent(e.noise) as f32,
					..Quality::default()
				},
				..base(e.blend, e.opacity, e.color)
			}),
			EffectKind::ColorOverlay => self.color_overlay.get(i).filter(|e| e.enabled).map(|e| base(e.blend, e.opacity, e.color)),
			EffectKind::Stroke => self.stroke.get(i).filter(|e| e.enabled && e.size > 0.0).map(|e| EffectParams {
				stroke: Some((e.size, e.position)),
				extra: e.fill.extra().unwrap_or_default(),
				..base(e.blend, e.opacity, e.color)
			}),
			EffectKind::InnerGlow => self.inner_glow.get(i).filter(|e| e.enabled).map(|e| EffectParams {
				morph: e.size * percent(e.choke),
				blur: e.size * (1.0 - percent(e.choke)),
				extra: EffectExtra::Glow {
					center: e.source == GlowSource::Center,
					gradient: e.fill.glow_gradient(),
				},
				quality: Quality {
					contour: e.contour,
					anti_aliased: e.anti_aliased,
					noise: percent(e.noise) as f32,
					range: percent(e.range).max(0.01),
					jitter: percent(e.jitter),
					precise: e.technique == GlowTechnique::Precise,
					knocks_out: false,
				},
				..base(e.blend, e.opacity, e.color)
			}),
			EffectKind::Satin => self.satin.get(i).filter(|e| e.enabled).map(|e| {
				let a = e.angle.to_radians();
				EffectParams {
					blur: e.size,
					extra: EffectExtra::Satin {
						offset: (a.cos() * e.distance, -a.sin() * e.distance),
						invert: e.invert,
					},
					quality: Quality {
						contour: e.contour,
						anti_aliased: e.anti_aliased,
						..Quality::default()
					},
					..base(e.blend, e.opacity, e.color)
				}
			}),
			EffectKind::BevelShadow | EffectKind::BevelHighlight => self.bevel.get(i).filter(|e| e.enabled && e.size > 0.0).and_then(|e| {
				// Stroke Emboss follows the (top) enabled stroke; nothing without one.
				let stroke = self.stroke.iter().find(|s| s.enabled && s.size > 0.0).map(|s| (s.size, s.position));
				if e.style == BevelStyle::StrokeEmboss && stroke.is_none() {
					return None;
				}
				let highlight = kind == EffectKind::BevelHighlight;
				let (angle, altitude) = if e.use_global_light { (light.angle, light.altitude) } else { (e.angle, e.altitude) };
				Some(EffectParams {
					blur: e.soften,
					extra: EffectExtra::Bevel {
						style: e.style,
						depth: e.depth,
						up: e.up,
						size: e.size,
						soften: e.soften,
						light: (angle.to_radians(), altitude.clamp(0.0, 90.0).to_radians()),
						highlight,
						technique: e.technique,
						contour: e.contour,
						contour_range: percent(e.contour_range).max(0.01),
						contour_anti_aliased: e.contour_anti_aliased,
						gloss: e.gloss_contour,
						anti_aliased: e.anti_aliased,
						texture: e.texture.clone(),
						stroke: if e.style == BevelStyle::StrokeEmboss { stroke } else { None },
					},
					..if highlight {
						base(e.highlight_blend, e.highlight_opacity, e.highlight_color)
					} else {
						base(e.shadow_blend, e.shadow_opacity, e.shadow_color)
					}
				})
			}),
			EffectKind::GradientOverlay => self.gradient_overlay.get(i).filter(|e| e.enabled).map(|e| EffectParams {
				extra: EffectExtra::Gradient {
					gradient: e.gradient.clone(),
					align: e.align,
				},
				..base(e.blend, e.opacity, [65_535; 4])
			}),
			EffectKind::PatternOverlay => self.pattern_overlay.get(i).filter(|e| e.enabled).map(|e| EffectParams {
				extra: EffectExtra::Pattern {
					id: e.pattern,
					scale: e.scale,
					angle: e.angle,
					phase: e.phase,
					align: e.align,
				},
				..base(e.blend, e.opacity, [65_535; 4])
			}),
		}
	}

	/// How far outside a tile an effect reads the layer's alpha, level 0.
	pub fn reach(params: &EffectParams) -> f64 {
		let stroke = params.stroke.map_or(0.0, |(size, _)| size);
		let extra = match &params.extra {
			EffectExtra::Satin { offset, .. } => offset.0.abs().max(offset.1.abs()),
			EffectExtra::Bevel { size, stroke, .. } => size + 2.0 + stroke.map_or(0.0, |s| s.0),
			_ => 0.0,
		};
		params.offset.0.abs().max(params.offset.1.abs()) + params.morph.abs() + params.blur * 1.5 + stroke + extra + 2.0
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_single_effect_and_a_list_both_read() {
		let one: LayerStyles = serde_json::from_str(r#"{"drop_shadow": {"enabled": true, "blend": "multiply", "color": [0,0,0,65535], "opacity": 0.5, "angle": 90, "use_global_light": false, "distance": 4, "spread": 0, "size": 3}}"#).unwrap();
		assert_eq!(one.drop_shadow.len(), 1);
		assert!(one.drop_shadow[0].knocks_out, "the default is Photoshop's");
		let json = serde_json::to_string(&LayerStyles {
			drop_shadow: vec![DropShadow::default(), DropShadow { distance: 9.0, ..DropShadow::default() }],
			..LayerStyles::default()
		})
		.unwrap();
		let two: LayerStyles = serde_json::from_str(&json).unwrap();
		assert_eq!(two.drop_shadow.len(), 2);
		assert_eq!(two.drop_shadow[1].distance, 9.0);
		assert!(!json.contains("channels") && !json.contains("effects_visible"), "defaults stay out of the file: {json}");
	}

	#[test]
	fn slots_put_the_first_instance_on_top() {
		let styles = LayerStyles {
			stroke: vec![Stroke::default(), Stroke::default()],
			drop_shadow: vec![DropShadow::default()],
			bevel: vec![BevelEmboss::default()],
			..LayerStyles::default()
		};
		let slots = styles.slots();
		assert_eq!(slots.len(), 5, "{slots:?}");
		assert_eq!(slots[0], EffectSlot { kind: EffectKind::DropShadow, instance: 0 });
		let strokes: Vec<usize> = slots.iter().filter(|s| s.kind == EffectKind::Stroke).map(|s| s.instance).collect();
		assert_eq!(strokes, vec![1, 0], "stroke 0 (the top of the list) composites last");
		assert_eq!(slots.last().unwrap().kind, EffectKind::BevelHighlight);
	}

	#[test]
	fn hidden_effects_draw_nothing() {
		let mut styles = LayerStyles {
			color_overlay: vec![ColorOverlay::default()],
			..LayerStyles::default()
		};
		assert!(styles.any_enabled());
		styles.effects_visible = false;
		assert!(!styles.any_enabled());
	}

	#[test]
	fn a_custom_contour_passes_through_its_points() {
		let c = Contour::custom(&[[0, 0], [128, 255], [255, 0]]);
		assert!(c.apply(0.0).abs() < 1e-9);
		assert!((c.apply(128.0 / 255.0) - 1.0).abs() < 1e-9);
		assert!(c.apply(1.0).abs() < 1e-9);
		assert!(c.apply(0.25) > 0.3 && c.apply(0.25) < 1.0);
		let json = serde_json::to_string(&c).unwrap();
		assert_eq!(serde_json::from_str::<Contour>(&json).unwrap(), c);
	}

	#[test]
	fn stroke_emboss_needs_a_stroke() {
		let mut styles = LayerStyles {
			bevel: vec![BevelEmboss {
				style: BevelStyle::StrokeEmboss,
				..BevelEmboss::default()
			}],
			..LayerStyles::default()
		};
		let slot = EffectSlot { kind: EffectKind::BevelShadow, instance: 0 };
		assert!(styles.effect_at(slot, GlobalLight::default()).is_none());
		styles.stroke.push(Stroke::default());
		assert!(styles.effect_at(slot, GlobalLight::default()).is_some());
	}

	#[test]
	fn scale_effects_scales_sizes_and_distances() {
		let mut styles = LayerStyles {
			drop_shadow: vec![DropShadow::default()],
			stroke: vec![Stroke::default()],
			..LayerStyles::default()
		};
		styles.scale(2.0);
		assert_eq!(styles.drop_shadow[0].distance, 10.0);
		assert_eq!(styles.drop_shadow[0].size, 10.0);
		assert_eq!(styles.stroke[0].size, 6.0);
	}
}
