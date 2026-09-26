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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OuterGlow {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	pub color: [u16; 4],
	pub spread: f64,
	pub size: f64,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InnerGlow {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	pub color: [u16; 4],
	pub choke: f64,
	pub size: f64,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientOverlay {
	pub enabled: bool,
	pub blend: BlendMode,
	pub opacity: f32,
	/// The gradient, angle, scale… placed over the canvas (FAST: Photoshop
	/// aligns it with the layer's bounds by default).
	pub gradient: crate::gradient::GradientLayer,
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
}

/// What an M12 effect needs beyond a colour and a coverage.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum EffectExtra {
	#[default]
	None,
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
	},
	Gradient(crate::gradient::GradientLayer),
	Pattern {
		id: u64,
		scale: f64,
	},
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
				extra: EffectExtra::None,
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
				extra: EffectExtra::None,
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
				extra: EffectExtra::None,
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
				extra: EffectExtra::Gradient(e.gradient.clone()),
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
				extra: EffectExtra::Pattern { id: e.pattern, scale: e.scale },
			}),
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
