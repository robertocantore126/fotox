//! Pixel operations a command needs but `fx-core` cannot implement (M4-T05).
//!
//! `Command::apply` lives here, while the algorithms live in crates that depend
//! on `fx-core` (`fx-ops` for filters, `fx-render` for compositing). The
//! dependency is inverted: `fx-core` defines [`PixelOps`], the engine
//! implements it and passes it in [`CommandContext::ops`](crate::CommandContext),
//! so every command — a live edit or a replayed macro — goes through
//! `Command::apply` (docs/tasks/HOWTO.md, R1a).

use fx_tiles::{TileStore, TiledImage};
use serde::{Deserialize, Serialize};

use crate::color::{BitDepth, ColorProfile, RenderingIntent};
use crate::command::CommandError;
use crate::document::Document;
use crate::layer::LayerId;
use crate::selection::{SelectModify, Selection, SelectionShape, WandParams};
use crate::transform::{Filter, Mapping, Permutation};

/// A destructive filter and its parameters. Serialised in commands, so macros
/// replay it; variant names are stable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FilterParams {
	/// Gaussian Blur. `radius` = the standard deviation σ in pixels (D-035),
	/// 0.1..=1000.
	GaussianBlur { radius: f32 },
	/// Unsharp Mask. `amount` in percent (1..=500), `radius` = σ of the blur in
	/// pixels (0.1..=1000), `threshold` in 8-bit levels (0..=255).
	UnsharpMask { amount: f32, radius: f32, threshold: u8 },
}

impl FilterParams {
	/// The History / menu name, as Photoshop writes it.
	pub fn label(&self) -> &'static str {
		match self {
			FilterParams::GaussianBlur { .. } => "Gaussian Blur",
			FilterParams::UnsharpMask { .. } => "Unsharp Mask",
		}
	}

	/// Check the parameter ranges (a command must refuse bad values, never
	/// clamp them silently).
	pub fn validate(&self) -> Result<(), CommandError> {
		let radius = |r: f32| {
			if (0.1..=1000.0).contains(&r) {
				Ok(())
			} else {
				Err(CommandError::InvalidValue {
					field: "radius",
					reason: format!("{r} is outside 0.1..=1000 px"),
				})
			}
		};
		match self {
			FilterParams::GaussianBlur { radius: r } => radius(*r),
			FilterParams::UnsharpMask { amount, radius: r, .. } => {
				radius(*r)?;
				if (1.0..=500.0).contains(amount) {
					Ok(())
				} else {
					Err(CommandError::InvalidValue {
						field: "amount",
						reason: format!("{amount} is outside 1..=500 %"),
					})
				}
			}
		}
	}
}

/// Pixel algorithms, implemented by the engine (`fx-engine/src/ops.rs`).
pub trait PixelOps: Send + Sync {
	/// `image` (a pixel layer's pixels at level 0, placed at `offset` in a
	/// document of size `canvas`) after `filter`. Only level 0 of the result
	/// is authoritative; its mips are left dirty.
	fn filter(&self, image: &TiledImage, offset: (i32, i32), canvas: (u32, u32), filter: &FilterParams, store: &TileStore) -> Result<TiledImage, CommandError>;

	/// The composite of `layers` of `doc` — an isolated group of them, each
	/// with its own blend mode, opacity, mask and clipping, adjustments baked
	/// in — as one pixel image of the document's size, at level 0. With
	/// `background`, the result is composited onto that opaque colour
	/// (Flatten's white). Used by merge, flatten and stamp (M4-T08).
	fn composite(&self, doc: &Document, layers: &[LayerId], background: Option<[u16; 4]>, store: &TileStore) -> Result<TiledImage, CommandError>;

	/// `image`'s pixels converted from `from` to `to` (Convert to Profile,
	/// M4-T03): level 0, alpha untouched.
	fn convert(&self, image: &TiledImage, conversion: &Conversion<'_>, store: &TileStore) -> Result<TiledImage, CommandError>;

	/// One straight RGBA16 colour converted (solid fill layers).
	fn convert_color(&self, rgba: [u16; 4], conversion: &Conversion<'_>) -> Result<[u16; 4], CommandError>;

	/// Rasterise `shape` (document pixels, fractional coordinates allowed)
	/// into a fresh selection coverage image (M5-T03). `anti_alias` off makes
	/// coverage ≥ 0.5 opaque, 0 otherwise. The result selects nothing when the
	/// shape lies outside the canvas.
	fn rasterise(&self, shape: &SelectionShape, size: (u32, u32), depth: BitDepth, anti_alias: bool, store: &TileStore) -> Result<Selection, CommandError>;

	/// `selection` after a reshape (M5-T03): feather, expand, contract, border
	/// or smooth. `None` when the result selects nothing.
	fn modify_selection(
		&self,
		selection: &Selection,
		op: &SelectModify,
		size: (u32, u32),
		depth: BitDepth,
		store: &TileStore,
	) -> Result<Option<Selection>, CommandError>;

	/// The pixels the Magic Wand selects in `doc` (M5-T04): the active layer's
	/// own pixels, or the composite with `sample_all_layers`. `None` when
	/// nothing matches.
	fn magic_wand(&self, doc: &Document, params: &WandParams, store: &TileStore) -> Result<Option<Selection>, CommandError>;

	/// `image` (a pixel layer's pixels, a mask, or a selection coverage),
	/// placed at `offset` in a document of size `canvas`, rotated or mirrored
	/// by `op` — M6-T02. A permutation is exact: every destination pixel is one
	/// source pixel, unchanged. Returns the new image and its new offset; the
	/// caller makes the coarser mip levels dirty.
	fn rotate(
		&self,
		_image: &TiledImage,
		_offset: (i32, i32),
		_canvas: (u32, u32),
		_op: Permutation,
		_store: &TileStore,
	) -> Result<(TiledImage, (i32, i32)), CommandError> {
		Err(CommandError::NotAllowed("rotating an image needs the engine's tile permutation".into()))
	}

	/// `image` resampled through `mapping` (source image pixels → destination
	/// image pixels) into an image of `size` pixels — M6-T02's Image Size and
	/// arbitrary rotation, M6-T04's Free Transform. Only level 0 of the result
	/// is authoritative; its mips are dirty.
	fn resample(&self, _image: &TiledImage, _mapping: Mapping, _size: (u32, u32), _filter: Filter, _store: &TileStore) -> Result<TiledImage, CommandError> {
		Err(CommandError::NotAllowed("resampling an image needs the engine's sampler".into()))
	}

	/// Replay a brush stroke on `layer` of `doc` (M5-T07): the layer's (or
	/// its mask's) new image and offset. The engine implements it with the
	/// brush engine of `fx-ops`, the same code its live strokes run.
	#[allow(clippy::too_many_arguments)]
	fn stroke(
		&self,
		_doc: &Document,
		_layer: LayerId,
		_target: crate::stroke::StrokeTarget,
		_tool: &crate::stroke::StrokeTool,
		_brush: &crate::stroke::BrushParams,
		_color: [u16; 4],
		_samples: &[crate::stroke::StrokeSample],
		_store: &TileStore,
	) -> Result<(TiledImage, (i32, i32)), CommandError> {
		Err(CommandError::NotAllowed("painting needs the engine's brush engine".into()))
	}

	/// What Edit ▸ Paste pastes (M5-T05): the engine's clipboard. `None` =
	/// empty.
	fn clipboard(&self) -> Option<crate::pixels::ClipboardImage> {
		None
	}
}

/// A colour conversion between two RGB spaces.
pub struct Conversion<'a> {
	pub from: &'a ColorProfile,
	pub to: &'a ColorProfile,
	pub intent: RenderingIntent,
	pub bpc: bool,
}
