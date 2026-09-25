use std::sync::Arc;

use fx_tiles::TiledImage;
use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::vector::{Paint, StrokeStyle, VectorShape};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u64);

/// A layer mask or vector-free raster mask. Gray pixels, same size as the
/// document, `0` = hidden, max = visible.
#[derive(Clone, Debug)]
pub struct Mask {
	pub image: TiledImage,
	pub enabled: bool,
	/// Mask stays put when the layer moves (Photoshop's unlinked mask).
	pub linked: bool,
	/// Value for pixels outside the mask image (Photoshop "default colour").
	pub outside_value: u16,
}

/// Non-destructive adjustments. Evaluated lazily by the renderer, per tile,
/// at the viewed mip level. Parameters are plain data so they serialise into
/// the document, macros and the UI protocol.
///
/// Milestone M2 implements `Levels`, `Curves`, `HueSaturation`,
/// `BrightnessContrast`, `Exposure`, `Invert`. Others follow in M4.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Adjustment {
	BrightnessContrast {
		brightness: f32,
		contrast: f32,
		legacy: bool,
	},
	/// Input black/white/gamma and output black/white, per channel (0 = composite, 1..=3 = R,G,B).
	Levels {
		channels: [LevelsChannel; 4],
	},
	/// Control points (x, y) in 0..=1 per channel (0 = composite, 1..=3 = R,G,B).
	Curves {
		channels: [Vec<(f32, f32)>; 4],
	},
	Exposure {
		exposure: f32,
		offset: f32,
		gamma: f32,
	},
	/// Master only for M2; per-range editing (reds, yellows, ...) in M4.
	HueSaturation {
		hue: f32,
		saturation: f32,
		lightness: f32,
		colorize: bool,
	},
	Invert,
	/// `levels` tone levels per channel, 2..=255 (M4-T07).
	Posterize {
		levels: u8,
	},
	/// White where the luminance reaches `level`/255, black below; 1..=255 (M4-T07).
	Threshold {
		level: u8,
	},
	/// The luminance mapped through a gradient (stops sorted by position,
	/// 0..=1; colours straight 0..=1); `reverse` flips it (M4-T07).
	GradientMap {
		stops: Vec<GradientStop>,
		reverse: bool,
	},
	/// Each output channel as a mix of the input channels, in percent:
	/// `[red, green, blue, constant]` (−200..=200). `monochrome`: `red` for
	/// all three (M4-T07). Identity: red `[100, 0, 0, 0]`, green `[0, 100, 0, 0]`,
	/// blue `[0, 0, 100, 0]`.
	ChannelMixer {
		red: [f32; 4],
		green: [f32; 4],
		blue: [f32; 4],
		monochrome: bool,
	},
	/// A coloured filter: `color` straight 0..=1, `density` 0..=1 (M4-T07).
	PhotoFilter {
		color: [f32; 3],
		density: f32,
		preserve_luminosity: bool,
	},
	/// Cyan–red, magenta–green and yellow–blue shifts per tone range, each
	/// −100..=100 (M4-T07).
	ColorBalance {
		shadows: [f32; 3],
		midtones: [f32; 3],
		highlights: [f32; 3],
		preserve_luminosity: bool,
	},
	/// Vibrance and saturation, −100..=100 each (M4-T07).
	Vibrance {
		vibrance: f32,
		saturation: f32,
	},
	/// Grey from per-hue weights in percent (−200..=300; Photoshop's defaults
	/// 40, 60, 40, 60, 20, 80), optionally tinted with `tint_hue` (degrees)
	/// and `tint_saturation` (percent) (M4-T07).
	BlackWhite {
		reds: f32,
		yellows: f32,
		greens: f32,
		cyans: f32,
		blues: f32,
		magentas: f32,
		tint: bool,
		tint_hue: f32,
		tint_saturation: f32,
	},
}

/// One colour stop of a Gradient Map.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
	/// 0..=1 along the gradient.
	pub position: f32,
	/// Straight RGB, 0..=1.
	pub color: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelsChannel {
	pub in_black: f32,
	pub in_white: f32,
	pub gamma: f32,
	pub out_black: f32,
	pub out_white: f32,
}

impl Default for LevelsChannel {
	fn default() -> Self {
		Self {
			in_black: 0.0,
			in_white: 1.0,
			gamma: 1.0,
			out_black: 0.0,
			out_white: 1.0,
		}
	}
}

#[derive(Clone, Debug)]
pub enum LayerKind {
	/// Raster content. `offset` is added to tile positions at composite time,
	/// so moving a layer never rewrites pixels (see ARCHITECTURE.md §4.3).
	Pixel {
		image: TiledImage,
		offset: (i32, i32),
	},
	/// Children are ordered bottom → top.
	Group {
		children: Vec<Arc<Layer>>,
		expanded: bool,
	},
	Adjustment(Adjustment),
	SolidFill {
		rgba: [u16; 4],
	},
	/// A vector shape (M6-T06): geometry plus the tiles rendered from it on
	/// demand at the level being drawn. The geometry is the truth — editing it
	/// is lossless (D-055) — and `cache` is derived data, rebuilt by the
	/// engine's vector scheduler whenever the program key changes.
	Shape {
		shape: VectorShape,
		fill: Option<Paint>,
		stroke: Option<StrokeStyle>,
		/// Local → document, `[a, b, c, d, e, f]` (see `crate::vector`).
		transform: [f64; 6],
		/// Document-sized, one level per document mip level.
		cache: TiledImage,
	},
}

#[derive(Clone, Debug)]
pub struct Layer {
	pub id: LayerId,
	pub name: String,
	pub visible: bool,
	/// 0.0 ..= 1.0
	pub opacity: f32,
	/// 0.0 ..= 1.0, like Photoshop's "Fill" (affects content, not layer styles).
	pub fill: f32,
	pub blend: BlendMode,
	/// Clipped to the layer below (clipping mask).
	pub clipped: bool,
	pub locked_pixels: bool,
	/// Photoshop's "Lock transparent pixels" (D-049): painting and fills keep
	/// every pixel's alpha.
	pub locked_transparency: bool,
	pub locked_position: bool,
	pub mask: Option<Mask>,
	pub kind: LayerKind,
}

impl Layer {
	pub fn new(id: LayerId, name: impl Into<String>, kind: LayerKind) -> Self {
		let blend = if matches!(kind, LayerKind::Group { .. }) {
			BlendMode::PassThrough
		} else {
			BlendMode::Normal
		};
		Self {
			id,
			name: name.into(),
			visible: true,
			opacity: 1.0,
			fill: 1.0,
			blend,
			clipped: false,
			locked_pixels: false,
			locked_transparency: false,
			locked_position: false,
			mask: None,
			kind,
		}
	}

	pub fn children(&self) -> Option<&[Arc<Layer>]> {
		match &self.kind {
			LayerKind::Group { children, .. } => Some(children),
			_ => None,
		}
	}
}
