use std::sync::Arc;

use fx_tiles::TiledImage;
use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;

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
