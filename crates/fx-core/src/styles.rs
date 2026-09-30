//! Layer styles (M6-T08, D-054): Drop Shadow, Outer Glow, Inner Shadow,
//! Color Overlay and Stroke; M12-T04 adds Inner Glow, Satin, Bevel & Emboss
//! (a shadow and a highlight pass), Gradient Overlay and Pattern Overlay.
//!
//! The parameters are the truth; each effect's pixels are a derived tile
//! cache on the layer ([`crate::Layer::effects`]), drawn by the engine from
//! the layer's alpha at the level being shown, like a shape's tiles.
//!
//! Lengths are document pixels at level 0; angles are degrees, Photoshop's
//! convention (0° = light from the right, 90° = from above, so the shadow
//! falls down).

use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;

/// The effects, in the order they are composited (bottom → top).
/// VERIFY: Photoshop's exact stacking of the interior effects.
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

	/// Index into [`crate::Layer::effects`].
	pub fn index(self) -> usize {
		self as usize
	}

	pub fn from_index(i: usize) -> Option<Self> {
		Self::ALL.get(i).copied()
	}

	/// Drawn under the layer's content (the rest are drawn over it).
	pub fn below_content(self) -> bool {
		matches!(self, EffectKind::DropShadow | EffectKind::OuterGlow)
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
/// pattern (Stroke's Fill Type; Outer Glow's gradient).
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

/// Photoshop's contour presets: a curve from 0..=1 to 0..=1 applied to an
/// effect's falloff (shadows, glows, satin), to a bevel's height profile, or
/// to its lighting (Gloss Contour).
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
}

impl Contour {
	pub const ALL: [Contour; 12] = [
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

	/// The curve at `t` (0..=1). VERIFY: shapes read off Photoshop's preset
	/// thumbnails, not its exact curves.
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
		};
		v.clamp(0.0, 1.0)
	}
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
}

fn yes() -> bool {
	true
}

/// Bevel & Emboss styles (M12-T04).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BevelStyle {
	InnerBevel,
	OuterBevel,
	Emboss,
	PillowEmboss,
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

/// A layer's styles: each effect is present (with `enabled` for the panel's
/// eye) or absent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerStyles {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub drop_shadow: Option<DropShadow>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub outer_glow: Option<OuterGlow>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub inner_shadow: Option<InnerShadow>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub color_overlay: Option<ColorOverlay>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stroke: Option<Stroke>,
	// M12-T04.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub bevel: Option<BevelEmboss>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub inner_glow: Option<InnerGlow>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub satin: Option<Satin>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub gradient_overlay: Option<GradientOverlay>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pattern_overlay: Option<PatternOverlay>,
	/// Blending Options ▸ Blend If (Gray): where this layer shows, by its own
	/// grey and by the grey under it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub blend_if: Option<BlendIf>,
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

/// What an M12 effect needs beyond a colour and a coverage.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum EffectExtra {
	#[default]
	None,
	/// Inner Glow from the centre instead of the edges.
	CenterGlow,
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
		gloss: Contour,
		anti_aliased: bool,
		texture: Option<BevelTexture>,
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
		matches!(self, EffectExtra::Gradient { align: true, .. } | EffectExtra::Pattern { align: true, .. })
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
}

impl LayerStyles {
	/// Whether any effect would draw.
	pub fn any_enabled(&self) -> bool {
		EffectKind::ALL.into_iter().any(|kind| self.effect(kind, 120.0).is_some())
	}

	/// The enabled effect `kind`, resolved with the document's global light.
	pub fn effect(&self, kind: EffectKind, global_light: f64) -> Option<EffectParams> {
		let offset = |angle: f64, global: bool, distance: f64| {
			let a = if global { global_light } else { angle }.to_radians();
			// Light from `a`: the shadow is cast the other way (y grows down).
			(-a.cos() * distance, a.sin() * distance)
		};
		match kind {
			EffectKind::DropShadow => self.drop_shadow.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: e.color,
				offset: offset(e.angle, e.use_global_light, e.distance),
				morph: e.size * e.spread.clamp(0.0, 100.0) / 100.0,
				blur: e.size * (1.0 - e.spread.clamp(0.0, 100.0) / 100.0),
				stroke: None,
				extra: EffectExtra::None,
			}),
			EffectKind::OuterGlow => self.outer_glow.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: e.color,
				offset: (0.0, 0.0),
				morph: e.size * e.spread.clamp(0.0, 100.0) / 100.0,
				blur: e.size * (1.0 - e.spread.clamp(0.0, 100.0) / 100.0),
				stroke: None,
				extra: e.fill.extra().unwrap_or_default(),
			}),
			EffectKind::InnerShadow => self.inner_shadow.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: e.color,
				offset: offset(e.angle, e.use_global_light, e.distance),
				morph: e.size * e.choke.clamp(0.0, 100.0) / 100.0,
				blur: e.size * (1.0 - e.choke.clamp(0.0, 100.0) / 100.0),
				stroke: None,
				extra: EffectExtra::None,
			}),
			EffectKind::ColorOverlay => self.color_overlay.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: e.color,
				offset: (0.0, 0.0),
				morph: 0.0,
				blur: 0.0,
				stroke: None,
				extra: EffectExtra::None,
			}),
			EffectKind::Stroke => self.stroke.as_ref().filter(|e| e.enabled && e.size > 0.0).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: e.color,
				offset: (0.0, 0.0),
				morph: 0.0,
				blur: 0.0,
				stroke: Some((e.size, e.position)),
				extra: e.fill.extra().unwrap_or_default(),
			}),
			EffectKind::InnerGlow => self.inner_glow.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: e.color,
				offset: (0.0, 0.0),
				morph: e.size * e.choke.clamp(0.0, 100.0) / 100.0,
				blur: e.size * (1.0 - e.choke.clamp(0.0, 100.0) / 100.0),
				stroke: None,
				extra: if e.source == GlowSource::Center { EffectExtra::CenterGlow } else { EffectExtra::None },
			}),
			EffectKind::Satin => self.satin.as_ref().filter(|e| e.enabled).map(|e| {
				let a = e.angle.to_radians();
				EffectParams {
					kind,
					blend: e.blend,
					opacity: e.opacity,
					color: e.color,
					offset: (0.0, 0.0),
					morph: 0.0,
					blur: e.size,
					stroke: None,
					extra: EffectExtra::Satin {
						offset: (a.cos() * e.distance, -a.sin() * e.distance),
						invert: e.invert,
					},
				}
			}),
			EffectKind::BevelShadow | EffectKind::BevelHighlight => self.bevel.as_ref().filter(|e| e.enabled && e.size > 0.0).map(|e| {
				let highlight = kind == EffectKind::BevelHighlight;
				let angle = if e.use_global_light { global_light } else { e.angle };
				EffectParams {
					kind,
					blend: if highlight { e.highlight_blend } else { e.shadow_blend },
					opacity: if highlight { e.highlight_opacity } else { e.shadow_opacity },
					color: if highlight { e.highlight_color } else { e.shadow_color },
					offset: (0.0, 0.0),
					morph: 0.0,
					blur: e.soften,
					stroke: None,
					extra: EffectExtra::Bevel {
						style: e.style,
						depth: e.depth,
						up: e.up,
						size: e.size,
						soften: e.soften,
						light: (angle.to_radians(), e.altitude.clamp(0.0, 90.0).to_radians()),
						highlight,
						technique: e.technique,
						contour: e.contour,
						gloss: e.gloss_contour,
						anti_aliased: e.anti_aliased,
						texture: e.texture.clone(),
					},
				}
			}),
			EffectKind::GradientOverlay => self.gradient_overlay.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: [65_535; 4],
				offset: (0.0, 0.0),
				morph: 0.0,
				blur: 0.0,
				stroke: None,
				extra: EffectExtra::Gradient {
					gradient: e.gradient.clone(),
					align: e.align,
				},
			}),
			EffectKind::PatternOverlay => self.pattern_overlay.as_ref().filter(|e| e.enabled).map(|e| EffectParams {
				kind,
				blend: e.blend,
				opacity: e.opacity,
				color: [65_535; 4],
				offset: (0.0, 0.0),
				morph: 0.0,
				blur: 0.0,
				stroke: None,
				extra: EffectExtra::Pattern {
					id: e.pattern,
					scale: e.scale,
					angle: e.angle,
					phase: e.phase,
					align: e.align,
				},
			}),
		}
	}

	/// The Noise (`0..=1`) of effect `kind`: the grain Photoshop mixes into
	/// shadows and glows.
	pub fn noise(&self, kind: EffectKind) -> f32 {
		let percent = match kind {
			EffectKind::DropShadow => self.drop_shadow.as_ref().map_or(0.0, |e| e.noise),
			EffectKind::OuterGlow => self.outer_glow.as_ref().map_or(0.0, |e| e.noise),
			EffectKind::InnerGlow => self.inner_glow.as_ref().map_or(0.0, |e| e.noise),
			_ => 0.0,
		};
		(percent / 100.0).clamp(0.0, 1.0) as f32
	}

	/// The Contour of effect `kind` (shadows, glows, satin), applied to its
	/// coverage.
	pub fn contour(&self, kind: EffectKind) -> Contour {
		match kind {
			EffectKind::DropShadow => self.drop_shadow.as_ref().map_or(Contour::Linear, |e| e.contour),
			EffectKind::InnerShadow => self.inner_shadow.as_ref().map_or(Contour::Linear, |e| e.contour),
			EffectKind::OuterGlow => self.outer_glow.as_ref().map_or(Contour::Linear, |e| e.contour),
			EffectKind::InnerGlow => self.inner_glow.as_ref().map_or(Contour::Linear, |e| e.contour),
			EffectKind::Satin => self.satin.as_ref().map_or(Contour::Linear, |e| e.contour),
			_ => Contour::Linear,
		}
	}

	/// How far outside a tile an effect reads the layer's alpha, level 0.
	pub fn reach(params: &EffectParams) -> f64 {
		let stroke = params.stroke.map_or(0.0, |(size, _)| size);
		let extra = match &params.extra {
			EffectExtra::Satin { offset, .. } => offset.0.abs().max(offset.1.abs()),
			EffectExtra::Bevel { size, .. } => size + 2.0,
			_ => 0.0,
		};
		params.offset.0.abs().max(params.offset.1.abs()) + params.morph.abs() + params.blur * 1.5 + stroke + extra + 2.0
	}
}
