use serde::{Deserialize, Serialize};

/// Photoshop's blend modes, in Photoshop's menu order.
///
/// The exact formulas (and their reference CPU implementation used to test
/// the GPU shaders) are specified in docs/BLEND_MODES.md and implemented in
/// `fx-render` (`blend.rs`, `gpu/composite.wgsl`). Do not invent formulas: match that document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
	/// Groups only: children blend directly into what is below the group.
	PassThrough,
	#[default]
	Normal,
	Dissolve,
	// darken
	Darken,
	Multiply,
	ColorBurn,
	LinearBurn,
	DarkerColor,
	// lighten
	Lighten,
	Screen,
	ColorDodge,
	LinearDodge,
	LighterColor,
	// contrast
	Overlay,
	SoftLight,
	HardLight,
	VividLight,
	LinearLight,
	PinLight,
	HardMix,
	// inversion
	Difference,
	Exclusion,
	Subtract,
	Divide,
	// component
	Hue,
	Saturation,
	Color,
	Luminosity,
}

impl BlendMode {
	/// Stable numeric id shared with the WGSL shaders (`blend.wgsl`). Never reorder.
	pub fn shader_id(self) -> u32 {
		self as u32
	}

	/// Separable modes can be computed per channel; the rest need the whole RGB triple.
	pub fn is_separable(self) -> bool {
		!matches!(
			self,
			BlendMode::DarkerColor | BlendMode::LighterColor | BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity
		)
	}
}
