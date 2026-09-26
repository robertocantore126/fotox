//! Layer styles (M6-T08, D-054): Drop Shadow, Outer Glow, Inner Shadow,
//! Color Overlay and Stroke.
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

/// The five effects, in the order they are composited (bottom → top).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
	DropShadow,
	OuterGlow,
	InnerShadow,
	ColorOverlay,
	Stroke,
}

impl EffectKind {
	pub const ALL: [EffectKind; 5] = [
		EffectKind::DropShadow,
		EffectKind::OuterGlow,
		EffectKind::InnerShadow,
		EffectKind::ColorOverlay,
		EffectKind::Stroke,
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
			}),
		}
	}

	/// How far outside a tile an effect reads the layer's alpha, level 0.
	pub fn reach(params: &EffectParams) -> f64 {
		let stroke = params.stroke.map_or(0.0, |(size, _)| size);
		params.offset.0.abs().max(params.offset.1.abs()) + params.morph.abs() + params.blur * 1.5 + stroke + 2.0
	}
}
