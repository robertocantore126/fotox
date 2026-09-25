//! Commands: the only way to change a [`Document`].
//!
//! Rules (docs/ARCHITECTURE.md §5):
//! * A command is plain data and serialises to JSON (`{"op": "...", ...}`).
//!   That JSON is also the macro/"action" format and the UI protocol payload.
//! * `apply` must be deterministic: same document + same command = same result.
//! * `apply` either fully succeeds or leaves the document untouched (validate
//!   first, then mutate).
//! * Heavy pixel work (brush strokes, filters) still goes through commands, but
//!   the command holds *parameters*, not pixels. The engine may run the pixel
//!   work on worker threads; the result is committed atomically.
//!
//! M2-T01 implements the layer stack: add, delete, duplicate, move, offset,
//! group, masks and adjustment parameters. Only `DeleteMask { apply: true }`
//! touches pixels; it runs tile by tile on the rayon pool and is committed
//! with one slot write per tile, so the document never holds a buffer
//! proportional to its size.

use std::sync::Arc;

use fx_tiles::{PixelFormat, PixelValue, TILE_PIXELS, TileBuffer, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::color::{ColorProfile, RenderingIntent};
use crate::document::{Document, NameKind};
use crate::layer::{Adjustment, Layer, LayerId, LayerKind, Mask};
use crate::ops::{FilterParams, PixelOps};
use crate::pixels::Placed;
use crate::selection::{self, SelectMode, SelectModify, Selection, SelectionShape, WandParams};
use crate::stroke::{BrushParams, StrokeSample, StrokeTarget, StrokeTool};
use crate::transform::{Anchor9, Filter, Mapping, Permutation, dest_rect};
use crate::vector::{Paint, StrokeStyle, VectorShape, document_box, grow_box};

/// How a command names a layer. Macros recorded on one document must replay
/// on another, so besides ids we support relative references.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerRef {
	Id(LayerId),
	/// The active (last selected) layer.
	Active,
	/// First layer with this exact name, searching top → bottom like Photoshop.
	Named(String),
}

/// Partial update of layer properties: `None` = leave unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerPropsPatch {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub visible: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub opacity: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fill: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub blend: Option<BlendMode>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub clipped: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub locked_pixels: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub locked_transparency: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub locked_position: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewLayer {
	/// Empty transparent pixel layer.
	Pixel,
	Group,
	Adjustment(Adjustment),
	SolidFill {
		rgba: [u16; 4],
	},
	/// A vector shape layer (M6-T06): the geometry is the layer's content, the
	/// tiles are rendered from it on demand at whatever level is drawn.
	Shape {
		shape: VectorShape,
		fill: Option<Paint>,
		stroke: Option<StrokeStyle>,
		transform: [f64; 6],
	},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskFill {
	RevealAll,
	HideAll,
	/// From the current pixel selection (M5).
	RevealSelection,
	/// The inverse of the current pixel selection (M5).
	HideSelection,
}

/// Every document mutation. Variants are added milestone by milestone; the
/// milestone that implements each one is noted. Keep variant names stable:
/// they are part of saved macros.
///
/// `MoveLayer`'s `index` is a position in the target list **after** the moved
/// layer has been taken out of it, bottom = 0, `index == len` = on top. That
/// is what a drag in the Layers panel means, and it makes moving a layer one
/// slot up or down the same command regardless of the current index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
	/// Replace the layer selection (Layers panel click). M2
	SelectLayers { layers: Vec<LayerRef> },
	/// Insert above the active layer (or at the top if none). M2
	AddLayer { layer: NewLayer, name: Option<String> },
	/// M2
	DeleteLayers { layers: Vec<LayerRef> },
	/// M2
	DuplicateLayers { layers: Vec<LayerRef> },
	/// Move to `index` inside `parent` (`None` = root), bottom = 0. M2
	MoveLayer { layer: LayerRef, parent: Option<LayerRef>, index: usize },
	/// Wrap the given layers in a new group. M2
	GroupLayers { layers: Vec<LayerRef>, name: Option<String> },
	/// Implemented (reference example). M2 extends validation.
	SetLayerProps { layer: LayerRef, props: LayerPropsPatch },
	/// Move a layer by whole pixels. Never rewrites pixels. M2
	OffsetLayer { layer: LayerRef, dx: i32, dy: i32 },
	/// M2
	AddMask { layer: LayerRef, fill: MaskFill },
	/// M2
	DeleteMask { layer: LayerRef, apply: bool },
	/// Change the parameters of an adjustment layer. M2
	SetAdjustment { layer: LayerRef, adjustment: Adjustment },
	/// Merge the given layers into one pixel layer (rasterises). The result
	/// takes the place, id and name of the bottom-most of them (Photoshop's
	/// Merge Down), Normal, 100 %. M4
	MergeLayers { layers: Vec<LayerRef> },
	/// Flatten the whole document into one "Background" pixel layer, the
	/// transparency filled with white; hidden layers are discarded. M4
	Flatten,
	/// A new pixel layer above the active one with the composite of every
	/// visible layer; nothing else changes. M4
	StampVisible,
	/// Give the document another profile without touching the numbers (the
	/// look changes). M4
	AssignProfile { profile: ColorProfile },
	/// Convert every pixel layer (level 0) and solid fill colour to `profile`,
	/// keeping the look. Masks and adjustment parameters stay as they are
	/// (Photoshop does the same). M4
	ConvertProfile {
		profile: ColorProfile,
		intent: RenderingIntent,
		bpc: bool,
	},
	/// Run a destructive filter on a pixel layer (level 0; the engine shows a
	/// live preview first). M4
	ApplyFilter { layer: LayerRef, filter: FilterParams },
	/// Combine a shape into the pixel selection (M5-T03). `feather` blurs the
	/// new shape's edge before combining.
	Select {
		shape: SelectionShape,
		mode: SelectMode,
		feather: f64,
		anti_alias: bool,
	},
	/// Select the whole canvas. M5
	SelectAll,
	/// Remove the pixel selection; `Reselect` brings it back. M5
	Deselect,
	/// Restore the selection the last `Deselect` removed. M5
	Reselect,
	/// Invert the pixel selection across the canvas. M5
	InvertSelection,
	/// Expand / contract / border / smooth / feather the selection. M5
	ModifySelection { modify: SelectModify },
	/// Move the selection outline by whole pixels (never rewrites tiles). M5
	OffsetSelection { dx: i32, dy: i32 },
	/// Select what the Magic Wand finds at a point, combined with the
	/// current selection by `mode`. M5
	MagicWand { params: WandParams, mode: SelectMode },
	/// Edit ▸ Fill a pixel layer through the selection (the whole canvas
	/// without one). `color` is straight 16-bit RGBA; the engine resolves the
	/// dialog's "Foreground Colour" etc. before sending. M5
	Fill {
		layer: LayerRef,
		color: [u16; 4],
		mode: BlendMode,
		/// `0..=1`.
		opacity: f64,
		preserve_transparency: bool,
	},
	/// Edit ▸ Clear (Delete): remove the selected pixels of a pixel layer.
	/// `cut` only changes the History label ("Cut"). M5
	Clear {
		layer: LayerRef,
		#[serde(default)]
		cut: bool,
	},
	/// Layer ▸ New ▸ Layer via Copy / via Cut (Ctrl+J / Shift+Ctrl+J): the
	/// selected pixels of the active layer on a new layer above it. M5
	LayerViaCopy { cut: bool },
	/// A brush stroke (M5-T07): the samples after smoothing, replayed through
	/// the brush engine; the live stroke painted exactly these pixels.
	Stroke {
		layer: LayerRef,
		#[serde(default)]
		target: StrokeTarget,
		tool: StrokeTool,
		brush: BrushParams,
		/// Straight 16-bit RGBA (for a mask, the grey is `color[0]`).
		color: [u16; 4],
		samples: Vec<StrokeSample>,
	},
	/// Edit ▸ Paste: the clipboard as a new layer above the active one. `in_place`
	/// keeps its canvas position; otherwise its bounds are centred on
	/// `center` (the view centre, when the source position is not visible),
	/// or kept where they were when `center` is `None`. M5
	Paste { in_place: bool, center: Option<(f64, f64)> },
	/// Image ▸ Rotate 90° CW / CCW / 180° (M6-T02): every pixel layer, mask and
	/// the selection is permuted **exactly** (no resampling) and its offset is
	/// recomputed so the content stays where it was relative to the canvas; the
	/// document's width and height swap for the odd turns. `quarter_turns` is 1
	/// (clockwise), 2 or 3; 0 (and ±4) is refused.
	RotateCanvas { quarter_turns: i8 },
	/// Image ▸ Flip Canvas Horizontal / Vertical (M6-T02): the same exact
	/// permutation as `RotateCanvas`.
	FlipCanvas { horizontal: bool },
	/// Image ▸ Rotate ▸ Arbitrary… (M6-T02): every pixel layer, mask and the
	/// selection is resampled and the canvas grows to the rotated bounding box.
	/// `angle_deg` is in degrees, clockwise (screen coordinates, y down); 0 is
	/// refused.
	RotateCanvasArbitrary { angle_deg: f64, filter: Filter },
	/// Image ▸ Canvas Size (M6-T02, Alt+Ctrl+C): only the document's size and
	/// every offset change — **no pixel is rewritten** (D-015), and content
	/// outside the new canvas is kept (it reappears if the canvas grows again).
	CanvasSize { width: u32, height: u32, anchor: Anchor9 },
	/// Image ▸ Image Size (M6-T02, Alt+Ctrl+I): with `resample` off only `ppi`
	/// changes (the pixel dimensions must stay); otherwise every pixel layer,
	/// mask and the selection is resampled with that filter, the offsets scale
	/// with it and `ppi` becomes the print resolution.
	ImageSize {
		width: u32,
		height: u32,
		ppi: f32,
		resample: Option<Filter>,
	},
	/// Image ▸ Crop, the Crop tool's ✓ and Trim (M6-T03): the canvas becomes
	/// `rect` (`(x, y, w, h)` in document pixels). With `angle_deg` 0 and
	/// `delete_cropped` off this is [`Command::CanvasSize`] — the offsets move,
	/// **no pixel is rewritten** and content outside the rectangle comes back if
	/// the canvas grows again (D-056's default). `delete_cropped` clips every
	/// pixel layer, mask and the selection to the rectangle first, so what falls
	/// outside it is gone for good. `angle_deg` (degrees, clockwise, about the
	/// rectangle's centre) **straightens** the image first: every pixel image is
	/// resampled through the turn, and the rectangle keeps what it shows.
	Crop {
		rect: (i32, i32, u32, u32),
		angle_deg: f64,
		delete_cropped: bool,
	},
	/// Edit ▸ Free Transform and Edit ▸ Transform ▸ … (M6-T04): the layer's
	/// pixels resampled through `mapping` (canvas pixels → canvas pixels) with
	/// `filter`. With a selection, only the selected pixels move: they are
	/// lifted off the layer, transformed and dropped back onto it, and the
	/// selection moves with them — one history step, as in Photoshop. Without
	/// one, the whole layer and its linked mask are transformed.
	Transform {
		layer: LayerRef,
		/// Boxed: a warp's 16 control points would make every command large.
		mapping: Box<Mapping>,
		filter: Filter,
	},
	/// Change a shape layer's geometry, paints or placement (M6-T06). Each
	/// field is a patch (`None` = leave alone); `fill: Some(None)` clears the
	/// fill. The shape's tile cache is invalidated, so the next frame draws the
	/// new geometry at the level on screen.
	SetShape {
		layer: LayerRef,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		shape: Option<VectorShape>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		fill: Option<Option<Paint>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		stroke: Option<Option<StrokeStyle>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		transform: Option<[f64; 6]>,
	},
	/// Layer ▸ Rasterize ▸ Shape / Layer (M6-T06): every named layer becomes a
	/// pixel layer holding what it drew, keeping its id, position in the stack,
	/// name, opacity, blend mode and mask (Photoshop keeps those too). Only
	/// non-pixel, non-group layers can be rasterised.
	Rasterize { layers: Vec<LayerRef> },
}

/// What a command changed. The engine uses it to invalidate render caches and
/// to decide which UI state to resend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandEffect {
	/// History label shown in the History panel, e.g. "Layer Properties".
	pub label: String,
	/// Layers whose *pixels* (or mask pixels) changed.
	pub pixels_changed: Vec<LayerId>,
	/// Layers whose compositing properties changed (opacity, blend, visibility, adjustment params, offset).
	pub props_changed: Vec<LayerId>,
	/// Layer tree structure changed (add/remove/reorder/group).
	pub structure_changed: bool,
	/// Only the layer selection changed: do not create an undo step.
	pub selection_only: bool,
	/// The command is a real history step but does not change the document
	/// (M5-T03: the pixel selection is recorded like Photoshop records it, yet
	/// it is not saved — D-028 — so making a selection must not dirty the
	/// document).
	pub history_only: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
	#[error("layer not found: {0:?}")]
	LayerNotFound(LayerRef),
	#[error("invalid value for {field}: {reason}")]
	InvalidValue { field: &'static str, reason: String },
	#[error("layer {0:?} is locked")]
	Locked(LayerId),
	#[error("operation not valid here: {0}")]
	NotAllowed(String),
	/// Reading the pixels of a layer being modified failed (scratch I/O,
	/// corrupt tile). Nothing was applied.
	#[error(transparent)]
	Tile(#[from] TileError),
}

/// Services a command may need while applying.
pub struct CommandContext<'a> {
	pub tiles: &'a TileStore,
	/// The engine's pixel algorithms (filters, compositing — `ops.rs`).
	/// `None` in `fx-core`'s own tests: commands that need it return
	/// `NotAllowed`.
	pub ops: Option<&'a dyn PixelOps>,
}

impl Command {
	/// Apply to `doc`. On error `doc` is unchanged.
	pub fn apply(&self, doc: &mut Document, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
		let effect = match self {
			Command::SelectLayers { layers } => select_layers(doc, layers),
			Command::AddLayer { layer, name } => add_layer(doc, layer, name.as_deref()),
			Command::DeleteLayers { layers } => delete_layers(doc, layers),
			Command::DuplicateLayers { layers } => duplicate_layers(doc, layers),
			Command::MoveLayer { layer, parent, index } => move_layer(doc, layer, parent.as_ref(), *index),
			Command::GroupLayers { layers, name } => group_layers(doc, layers, name.as_deref()),
			Command::SetLayerProps { layer, props } => set_layer_props(doc, layer, props),
			Command::OffsetLayer { layer, dx, dy } => offset_layer(doc, layer, *dx, *dy),
			Command::AddMask { layer, fill } => add_mask(doc, layer, *fill, ctx.tiles),
			Command::DeleteMask { layer, apply } => delete_mask(doc, layer, *apply, ctx.tiles),
			Command::SetAdjustment { layer, adjustment } => set_adjustment(doc, layer, adjustment),
			Command::ApplyFilter { layer, filter } => apply_filter(doc, layer, filter, ctx),
			Command::MergeLayers { layers } => merge_layers(doc, layers, ctx),
			Command::Flatten => flatten(doc, ctx),
			Command::StampVisible => stamp_visible(doc, ctx),
			Command::AssignProfile { profile } => assign_profile(doc, profile),
			Command::ConvertProfile { profile, intent, bpc } => convert_profile(doc, profile, *intent, *bpc, ctx),
			Command::Select {
				shape,
				mode,
				feather,
				anti_alias,
			} => select(doc, shape, *mode, *feather, *anti_alias, ctx),
			Command::SelectAll => select_all(doc),
			Command::Deselect => deselect(doc),
			Command::Reselect => reselect(doc),
			Command::InvertSelection => invert_selection(doc, ctx),
			Command::ModifySelection { modify } => modify_selection(doc, modify, ctx),
			Command::OffsetSelection { dx, dy } => offset_selection(doc, *dx, *dy),
			Command::MagicWand { params, mode } => magic_wand(doc, params, *mode, ctx),
			Command::Fill {
				layer,
				color,
				mode,
				opacity,
				preserve_transparency,
			} => fill(doc, layer, *color, *mode, *opacity, *preserve_transparency, ctx),
			Command::Clear { layer, cut } => clear(doc, layer, *cut, ctx),
			Command::LayerViaCopy { cut } => layer_via_copy(doc, *cut, ctx),
			Command::Paste { in_place, center } => paste(doc, *in_place, *center, ctx),
			Command::Stroke {
				layer,
				target,
				tool,
				brush,
				color,
				samples,
			} => stroke(doc, layer, *target, tool, brush, *color, samples, ctx),
			Command::RotateCanvas { quarter_turns } => rotate_canvas(doc, *quarter_turns, ctx),
			Command::FlipCanvas { horizontal } => permute_canvas(
				doc,
				if *horizontal {
					Permutation::FlipHorizontal
				} else {
					Permutation::FlipVertical
				},
				ctx,
			),
			Command::RotateCanvasArbitrary { angle_deg, filter } => rotate_canvas_arbitrary(doc, *angle_deg, *filter, ctx),
			Command::CanvasSize { width, height, anchor } => canvas_size(doc, *width, *height, *anchor, ctx.tiles),
			Command::ImageSize { width, height, ppi, resample } => image_size(doc, *width, *height, *ppi, *resample, ctx),
			Command::Crop {
				rect,
				angle_deg,
				delete_cropped,
			} => crop(doc, *rect, *angle_deg, *delete_cropped, ctx),
			Command::Transform { layer, mapping, filter } => transform_layer(doc, layer, **mapping, *filter, ctx),
			Command::SetShape {
				layer,
				shape,
				fill,
				stroke,
				transform,
			} => set_shape(doc, layer, shape.as_ref(), fill.as_ref(), stroke.as_ref(), *transform),
			Command::Rasterize { layers } => rasterize(doc, layers, ctx),
		}?;
		doc.revision += 1;
		Ok(effect)
	}
}

// ---------------------------------------------------------------------------
// The commands
// ---------------------------------------------------------------------------

fn select_layers(doc: &mut Document, layers: &[LayerRef]) -> Result<CommandEffect, CommandError> {
	let mut selected = Vec::with_capacity(layers.len());
	for layer in layers {
		// Validate every reference before changing anything.
		let id = resolve(doc, layer)?;
		if !selected.contains(&id) {
			selected.push(id);
		}
	}
	doc.selected = selected;
	Ok(CommandEffect {
		label: "Select Layers".into(),
		selection_only: true,
		..Default::default()
	})
}

fn add_layer(doc: &mut Document, new: &NewLayer, name: Option<&str>) -> Result<CommandEffect, CommandError> {
	validate_name(name)?;
	// The counter advances even when the caller supplies a name: it counts the
	// layers of that kind the document created, so the next default name is
	// never a duplicate of an earlier one.
	let default_name = doc.next_default_name(name_kind(new));
	let id = doc.allocate_layer_id();
	let kind = match new {
		// A pixel layer starts empty: no tile memory at all, whatever the size.
		NewLayer::Pixel => LayerKind::Pixel {
			image: TiledImage::new(doc.width, doc.height, doc.color.depth.rgba_format()),
			offset: (0, 0),
		},
		NewLayer::Group => LayerKind::Group {
			children: Vec::new(),
			expanded: true,
		},
		NewLayer::Adjustment(adjustment) => LayerKind::Adjustment(adjustment.clone()),
		NewLayer::SolidFill { rgba } => LayerKind::SolidFill { rgba: *rgba },
		// The cache holds no authority: every tile starts dirty and is drawn
		// from the geometry at the level being composited (M6-T06).
		NewLayer::Shape {
			shape,
			fill,
			stroke,
			transform,
		} => LayerKind::Shape {
			shape: shape.clone(),
			fill: *fill,
			stroke: stroke.clone(),
			transform: *transform,
			cache: TiledImage::derived(doc.width, doc.height, doc.color.depth.rgba_format()),
		},
	};
	let layer = Arc::new(Layer::new(id, name.unwrap_or(&default_name).to_owned(), kind));
	insert_above_active(doc, layer);
	doc.selected = vec![id];
	Ok(CommandEffect {
		label: add_label(new),
		structure_changed: true,
		..Default::default()
	})
}

fn delete_layers(doc: &mut Document, layers: &[LayerRef]) -> Result<CommandEffect, CommandError> {
	if layers.is_empty() {
		return Err(CommandError::NotAllowed("no layers to delete".into()));
	}
	// Deleting a group deletes its children, so a selected descendant needs no
	// separate removal.
	let ids = resolve_all(doc, layers, true)?;
	let mut gone = Vec::new();
	for &id in &ids {
		collect_subtree(doc.layer(id).expect("resolved id exists"), &mut gone);
	}
	// Selection rule first: it reads the tree we are about to change.
	let panel = doc.panel_order();
	doc.selected = selection_after_delete(&panel, &gone).into_iter().collect();
	for &id in &ids {
		remove_layer(doc, id);
	}
	Ok(CommandEffect {
		label: if ids.len() == 1 { "Delete Layer".into() } else { "Delete Layers".into() },
		structure_changed: true,
		..Default::default()
	})
}

fn duplicate_layers(doc: &mut Document, layers: &[LayerRef]) -> Result<CommandEffect, CommandError> {
	if layers.is_empty() {
		return Err(CommandError::NotAllowed("no layers to duplicate".into()));
	}
	let ids = resolve_all(doc, layers, true)?;
	// Top → bottom, so the copies keep the stack order of the originals.
	let panel = doc.panel_order();
	let mut copies = Vec::with_capacity(ids.len());
	for id in panel.into_iter().filter(|id| ids.contains(id)) {
		// `Layer::clone` copies the tree's handles, not its pixels, so the
		// copy shares every tile the original uses.
		let original = doc.layer(id).expect("resolved id exists").clone();
		let copy = deep_copy(doc, &original, format!("{} copy", original.name));
		copies.push(copy.id);
		let path = doc.path_of(id).expect("resolved id exists");
		let parent = id_at_path(doc, &path[..path.len() - 1]);
		// Directly above the original.
		children_mut(doc, parent).insert(path[path.len() - 1] + 1, copy);
	}
	doc.selected = copies;
	Ok(CommandEffect {
		label: if ids.len() == 1 {
			"Duplicate Layer".into()
		} else {
			"Duplicate Layers".into()
		},
		structure_changed: true,
		..Default::default()
	})
}

fn move_layer(doc: &mut Document, layer: &LayerRef, parent: Option<&LayerRef>, index: usize) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let parent_id = match parent {
		Some(reference) => {
			let parent_id = resolve(doc, reference)?;
			if !matches!(doc.layer(parent_id).expect("resolved id exists").kind, LayerKind::Group { .. }) {
				return Err(CommandError::InvalidValue {
					field: "parent",
					reason: format!("layer {parent_id:?} is not a group"),
				});
			}
			Some(parent_id)
		}
		None => None,
	};
	let path = doc.path_of(id).expect("resolved id exists");
	if let Some(parent_id) = parent_id {
		// A group moved into itself or into its own descendant would disappear
		// from the tree (and make `path_of` lie).
		let inside = parent_id == id || doc.path_of(parent_id).expect("resolved id exists").starts_with(&path);
		if inside {
			return Err(CommandError::NotAllowed("a layer cannot be moved into itself or one of its descendants".into()));
		}
	}
	let source_parent = id_at_path(doc, &path[..path.len() - 1]);
	let source_index = path[path.len() - 1];
	let same_list = source_parent == parent_id;
	// `index` counts the target list *without* the moved layer.
	let length = children_ref(doc, parent_id).len() - usize::from(same_list);
	if index > length {
		return Err(CommandError::InvalidValue {
			field: "index",
			reason: format!("{index} is past the end of the target list ({length} layers)"),
		});
	}
	let moved = children_mut(doc, source_parent).remove(source_index);
	children_mut(doc, parent_id).insert(index, moved);
	Ok(CommandEffect {
		label: "Move Layer".into(),
		structure_changed: true,
		..Default::default()
	})
}

fn group_layers(doc: &mut Document, layers: &[LayerRef], name: Option<&str>) -> Result<CommandEffect, CommandError> {
	if layers.is_empty() {
		return Err(CommandError::NotAllowed("no layers to group".into()));
	}
	validate_name(name)?;
	let ids = resolve_all(doc, layers, true)?;
	let panel = doc.panel_order();
	// Top → bottom; the group takes the place of the topmost one.
	let mut included: Vec<LayerId> = panel.into_iter().filter(|id| ids.contains(id)).collect();
	let topmost = included[0];
	let top = doc.path_of(topmost).expect("resolved id exists");
	let parent = id_at_path(doc, &top[..top.len() - 1]);
	let top_index = top[top.len() - 1];
	// Where the group lands: only the layers that stay behind, below the
	// topmost one, still count.
	let insert_at = children_ref(doc, parent)[..top_index].iter().filter(|layer| !ids.contains(&layer.id)).count();
	// Children are stored bottom → top.
	included.reverse();
	let mut children = Vec::with_capacity(included.len());
	for id in included {
		let path = doc.path_of(id).expect("resolved id exists");
		let list = children_mut(doc, id_at_path(doc, &path[..path.len() - 1]));
		children.push(list.remove(path[path.len() - 1]));
	}
	let id = doc.allocate_layer_id();
	let default_name = doc.next_default_name(NameKind::Group);
	let group = Arc::new(Layer::new(
		id,
		name.unwrap_or(&default_name).to_owned(),
		LayerKind::Group { children, expanded: true },
	));
	children_mut(doc, parent).insert(insert_at, group);
	doc.selected = vec![id];
	Ok(CommandEffect {
		label: "Group Layers".into(),
		structure_changed: true,
		..Default::default()
	})
}

fn set_layer_props(doc: &mut Document, layer: &LayerRef, props: &LayerPropsPatch) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	// M2 validation: an empty patch, an empty name, pass-through outside a
	// group and a clipping mask with nothing below it are all rejected before
	// the first write.
	if props == &LayerPropsPatch::default() {
		return Err(CommandError::InvalidValue {
			field: "props",
			reason: "no properties to set".into(),
		});
	}
	validate_unit("opacity", props.opacity)?;
	validate_unit("fill", props.fill)?;
	validate_name(props.name.as_deref())?;
	let target = doc.layer(id).expect("resolved id exists");
	if props.blend == Some(BlendMode::PassThrough) && !matches!(target.kind, LayerKind::Group { .. }) {
		return Err(CommandError::InvalidValue {
			field: "blend",
			reason: "pass-through is only valid for groups".into(),
		});
	}
	if props.clipped == Some(true) && doc.path_of(id).expect("resolved id exists").last() == Some(&0) {
		return Err(CommandError::NotAllowed("nothing below this layer to clip to".into()));
	}
	// Validate first, mutate after: every check above is done.
	let target = doc.layer_mut(id).expect("resolved id exists");
	if let Some(v) = &props.name {
		target.name = v.clone();
	}
	if let Some(v) = props.visible {
		target.visible = v;
	}
	if let Some(v) = props.opacity {
		target.opacity = v;
	}
	if let Some(v) = props.fill {
		target.fill = v;
	}
	if let Some(v) = props.blend {
		target.blend = v;
	}
	if let Some(v) = props.clipped {
		target.clipped = v;
	}
	if let Some(v) = props.locked_transparency {
		target.locked_transparency = v;
	}
	if let Some(v) = props.locked_pixels {
		target.locked_pixels = v;
	}
	if let Some(v) = props.locked_position {
		target.locked_position = v;
	}
	Ok(CommandEffect {
		label: "Layer Properties".into(),
		props_changed: vec![id],
		..Default::default()
	})
}

fn offset_layer(doc: &mut Document, layer: &LayerRef, dx: i32, dy: i32) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let target = doc.layer_mut(id).expect("resolved id exists");
	// Only pixel layers have an offset at all; a linked mask follows it (the
	// renderer places a linked mask at the layer's offset, an unlinked one at
	// the document origin — `fx-render::program`), so "move the mask too" here
	// means exactly "move the layer".
	let LayerKind::Pixel { offset, .. } = &mut target.kind else {
		return Err(CommandError::NotAllowed("only pixel layers have an offset".into()));
	};
	if target.locked_position {
		return Err(CommandError::Locked(id));
	}
	// All or nothing: compute both axes before writing either.
	let x = checked_offset(offset.0, dx)?;
	let y = checked_offset(offset.1, dy)?;
	*offset = (x, y);
	Ok(CommandEffect {
		label: "Offset".into(),
		props_changed: vec![id],
		..Default::default()
	})
}

fn add_mask(doc: &mut Document, layer: &LayerRef, fill: MaskFill, store: &TileStore) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let from_selection = match fill {
		MaskFill::RevealAll | MaskFill::HideAll => None,
		MaskFill::RevealSelection | MaskFill::HideSelection => {
			let Some(selection) = doc.selection.as_ref() else {
				return Err(CommandError::NotAllowed("nothing is selected".into()));
			};
			Some((selection.clone(), fill == MaskFill::HideSelection))
		}
	};
	let reveal = matches!(fill, MaskFill::RevealAll | MaskFill::RevealSelection);
	{
		let target = doc.layer(id).expect("resolved id exists");
		if matches!(target.kind, LayerKind::Group { .. }) {
			return Err(CommandError::NotAllowed("a group cannot have a pixel mask".into()));
		}
		if target.mask.is_some() {
			return Err(CommandError::NotAllowed("the layer already has a mask".into()));
		}
	}
	let format = doc.color.depth.gray_format();
	let image = match &from_selection {
		// A linked mask sits at the layer's offset (M5-T05).
		Some((selection, hide)) => {
			let origin = match &doc.layer(id).expect("resolved id exists").kind {
				LayerKind::Pixel { offset, .. } => *offset,
				_ => (0, 0),
			};
			crate::pixels::mask_from_selection(selection, (doc.width, doc.height), origin, (doc.width, doc.height), *hide, format, store)?
		}
		None => mask_image(doc.width, doc.height, format, reveal),
	};
	let mask = Mask {
		image,
		enabled: true,
		// Photoshop links a new mask to its layer.
		linked: true,
		// Outside the mask image: hidden for a selection mask (nothing outside
		// the canvas was selected), else the fill's value.
		outside_value: match (&from_selection, reveal) {
			(Some((_, hide)), _) => {
				if *hide {
					u16::MAX
				} else {
					0
				}
			}
			(None, true) => u16::MAX,
			(None, false) => 0,
		},
	};
	doc.layer_mut(id).expect("resolved id exists").mask = Some(mask);
	Ok(CommandEffect {
		label: "Add Layer Mask".into(),
		props_changed: vec![id],
		..Default::default()
	})
}

fn delete_mask(doc: &mut Document, layer: &LayerRef, apply: bool, store: &TileStore) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let target = doc.layer_mut(id).expect("resolved id exists");
	// Take the mask out; every error path below puts it back, so a failed
	// command leaves the document untouched.
	let Some(mask) = target.mask.take() else {
		return Err(CommandError::NotAllowed("the layer has no mask".into()));
	};
	if !apply {
		return Ok(delete_mask_effect(id, false));
	}
	let LayerKind::Pixel { image, offset } = &mut target.kind else {
		target.mask = Some(mask);
		return Err(CommandError::NotAllowed("only pixel layers can apply their mask".into()));
	};
	// The bake pairs layer tile (tx, ty) with mask tile (tx, ty), which is the
	// renderer's alignment only while both grids share an origin: a *linked*
	// mask is drawn at the layer's offset, an *unlinked* one at the document
	// origin (`fx-render::program`). Under an offset those two differ, so
	// refuse instead of baking the wrong pixels into the layer.
	if !mask.linked && *offset != (0, 0) {
		target.mask = Some(mask);
		return Err(CommandError::NotAllowed(
			"an unlinked mask on a layer with a non-zero offset cannot be applied yet".into(),
		));
	}
	// `apply` bakes the mask's pixels, so a *disabled* mask is applied too:
	// the flag governs live compositing, not the stored content.
	match bake_mask(image, &mask.image, store) {
		Ok(Some(baked)) => {
			*image = baked;
			Ok(delete_mask_effect(id, true))
		}
		// A fully revealing mask, or a layer with no pixels under it: nothing
		// changed, so `pixels_changed` stays empty.
		Ok(None) => Ok(delete_mask_effect(id, false)),
		Err(error) => {
			target.mask = Some(mask);
			Err(error)
		}
	}
}

fn set_adjustment(doc: &mut Document, layer: &LayerRef, adjustment: &Adjustment) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let target = doc.layer_mut(id).expect("resolved id exists");
	let LayerKind::Adjustment(current) = &mut target.kind else {
		return Err(CommandError::NotAllowed("only adjustment layers have adjustable parameters".into()));
	};
	// An adjustment layer's type is part of its identity: Photoshop's Curves
	// dialog cannot turn a Curves layer into a Levels layer.
	if std::mem::discriminant(&*current) != std::mem::discriminant(adjustment) {
		return Err(CommandError::InvalidValue {
			field: "adjustment",
			reason: format!(
				"a {} layer cannot take {} parameters",
				NameKind::of_adjustment(current).stem(),
				NameKind::of_adjustment(adjustment).stem()
			),
		});
	}
	*current = adjustment.clone();
	Ok(CommandEffect {
		label: NameKind::of_adjustment(adjustment).stem().to_owned(),
		props_changed: vec![id],
		..Default::default()
	})
}

/// `SetShape` (M6-T06): patch a shape layer's geometry, paints or placement.
/// The geometry is the truth, so nothing here touches pixels — but the tile
/// cache is invalidated, because everything it holds was drawn from the old
/// shape.
fn set_shape(
	doc: &mut Document,
	layer: &LayerRef,
	shape: Option<&VectorShape>,
	fill: Option<&Option<Paint>>,
	stroke: Option<&Option<StrokeStyle>>,
	transform: Option<[f64; 6]>,
) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	// Photoshop's History names the step after what changed.
	let label = match (shape.is_some(), fill.is_some(), stroke.is_some(), transform.is_some()) {
		(true, ..) => "Set Shape",
		(_, true, false, false) => "Fill",
		(_, false, true, false) => "Stroke",
		(_, false, false, true) => "Move",
		_ => "Set Shape",
	};
	// What the old geometry drew, to know what has to be drawn again: only the
	// box the old or the new outline reaches, not the whole document.
	let before = {
		let layer = doc.layer(id).ok_or(CommandError::LayerNotFound(LayerRef::Id(id)))?;
		let LayerKind::Shape { shape, stroke, transform, .. } = &layer.kind else {
			return Err(CommandError::NotAllowed("only shape layers have shape parameters".into()));
		};
		box_of(shape, *transform, stroke.as_ref())
	};
	let target = doc.layer_mut(id).ok_or(CommandError::LayerNotFound(LayerRef::Id(id)))?;
	let LayerKind::Shape {
		shape: current_shape,
		fill: current_fill,
		stroke: current_stroke,
		transform: current_transform,
		cache,
	} = &mut target.kind
	else {
		return Err(CommandError::NotAllowed("only shape layers have shape parameters".into()));
	};
	if let Some(shape) = shape {
		*current_shape = shape.clone();
	}
	if let Some(fill) = fill {
		*current_fill = *fill;
	}
	if let Some(stroke) = stroke {
		*current_stroke = stroke.clone();
	}
	if let Some(transform) = transform {
		if !transform.iter().all(|v| v.is_finite()) {
			return Err(CommandError::InvalidValue {
				field: "transform",
				reason: "the matrix has a non-finite component".into(),
			});
		}
		*current_transform = transform;
	}
	let after = box_of(current_shape, *current_transform, current_stroke.as_ref());
	cache.mark_rect_dirty([
		before[0].min(after[0]),
		before[1].min(after[1]),
		before[2].max(after[2]),
		before[3].max(after[3]),
	]);
	Ok(CommandEffect {
		label: label.into(),
		props_changed: vec![id],
		..Default::default()
	})
}

/// The document box a shape layer's content reaches, stroke included: a
/// stroke of `width` sticks out by up to that much.
fn box_of(shape: &VectorShape, transform: [f64; 6], stroke: Option<&StrokeStyle>) -> [f64; 4] {
	let reach = stroke.map_or(0.0, |stroke| stroke.width.max(0.0));
	grow_box(document_box(shape, transform), reach)
}

/// `Rasterize` (M6-T06): turn a generated layer into the pixels it drew.
/// The layer keeps its id, name, place, opacity, blend mode and mask.
fn rasterize(doc: &mut Document, layers: &[LayerRef], ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if layers.is_empty() {
		return Err(CommandError::NotAllowed("no layers to rasterise".into()));
	}
	let ids = resolve_all(doc, layers, true)?;
	for &id in &ids {
		let layer = doc.layer(id).ok_or(CommandError::LayerNotFound(LayerRef::Id(id)))?;
		match layer.kind {
			LayerKind::Pixel { .. } => return Err(CommandError::NotAllowed("the layer is already pixels".into())),
			LayerKind::Group { .. } => return Err(CommandError::NotAllowed("a group cannot be rasterised".into())),
			_ => {}
		}
	}
	// Composite each layer alone, with its own opacity, fill and blend mode
	// neutralised: those stay on the layer (Photoshop keeps them too).
	let mut flat = doc.clone();
	for &id in &ids {
		if let Some(layer) = flat.layer_mut(id) {
			layer.visible = true;
			layer.opacity = 1.0;
			layer.fill = 1.0;
			layer.blend = BlendMode::Normal;
		}
	}
	let mut images = Vec::with_capacity(ids.len());
	for &id in &ids {
		images.push((id, pixel_ops(ctx, "rasterising")?.composite(&flat, &[id], None, ctx.tiles)?));
	}
	let mut changed = Vec::with_capacity(ids.len());
	for (id, image) in images {
		if let Some(layer) = doc.layer_mut(id) {
			layer.kind = LayerKind::Pixel { image, offset: (0, 0) };
		}
		changed.push(id);
	}
	Ok(CommandEffect {
		label: if changed.len() == 1 {
			"Rasterize Layer".into()
		} else {
			"Rasterize Layers".into()
		},
		pixels_changed: changed.clone(),
		props_changed: changed,
		..Default::default()
	})
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a [`LayerRef`] to an existing id.
pub fn resolve(doc: &Document, layer: &LayerRef) -> Result<LayerId, CommandError> {
	let found = match layer {
		LayerRef::Id(id) => doc.layer(*id).map(|l| l.id),
		LayerRef::Active => doc.active_layer().filter(|id| doc.layer(*id).is_some()),
		LayerRef::Named(name) => {
			let mut hit = None;
			// walk is bottom → top; the last match is the top-most one
			doc.walk(|l, _| {
				if &l.name == name {
					hit = Some(l.id);
				}
			});
			hit
		}
	};
	found.ok_or_else(|| CommandError::LayerNotFound(layer.clone()))
}

/// Resolve a whole list: ids in first-seen order, duplicates dropped. With
/// `drop_descendants`, ids nested inside another resolved id are dropped as
/// well — the ancestor's edit already covers them.
fn resolve_all(doc: &Document, refs: &[LayerRef], drop_descendants: bool) -> Result<Vec<LayerId>, CommandError> {
	let mut ids: Vec<LayerId> = Vec::with_capacity(refs.len());
	for reference in refs {
		let id = resolve(doc, reference)?;
		if !ids.contains(&id) {
			ids.push(id);
		}
	}
	if drop_descendants && ids.len() > 1 {
		let paths: Vec<Vec<usize>> = ids.iter().map(|id| doc.path_of(*id).expect("resolved id exists")).collect();
		let mut kept = Vec::with_capacity(ids.len());
		for (index, path) in paths.iter().enumerate() {
			let nested = paths.iter().any(|other| other.len() < path.len() && path.starts_with(other));
			if !nested {
				kept.push(ids[index]);
			}
		}
		ids = kept;
	}
	Ok(ids)
}

/// The sibling list that contains the layer at `path` (the root list for `[]`).
fn children_ref(doc: &Document, parent: Option<LayerId>) -> &[Arc<Layer>] {
	match parent {
		None => &doc.layers,
		Some(id) => doc
			.layer(id)
			.expect("resolved id exists")
			.children()
			.expect("the caller validated that the parent is a group"),
	}
}

/// The sibling list to insert into: the root list, or a group's children.
/// `parent` must be [`None`] or a group id that exists.
fn children_mut(doc: &mut Document, parent: Option<LayerId>) -> &mut Vec<Arc<Layer>> {
	let Some(id) = parent else {
		return &mut doc.layers;
	};
	match &mut doc.layer_mut(id).expect("resolved id exists").kind {
		LayerKind::Group { children, .. } => children,
		_ => unreachable!("the caller validated that the parent is a group"),
	}
}

/// Id of the layer at `path`; `[]` (the root list itself) has none.
fn id_at_path(doc: &Document, path: &[usize]) -> Option<LayerId> {
	let mut layers = &doc.layers[..];
	let mut id = None;
	for &index in path {
		let layer = layers.get(index)?;
		id = Some(layer.id);
		layers = layer.children().unwrap_or_default();
	}
	id
}

/// `layer`'s id and the ids of everything inside it, depth first.
fn collect_subtree(layer: &Layer, out: &mut Vec<LayerId>) {
	out.push(layer.id);
	if let Some(children) = layer.children() {
		for child in children {
			collect_subtree(child, out);
		}
	}
}

/// Take the layer out of its parent list; its children go with it.
fn remove_layer(doc: &mut Document, id: LayerId) {
	let path = doc.path_of(id).expect("resolved id exists");
	let parent = id_at_path(doc, &path[..path.len() - 1]);
	children_mut(doc, parent).remove(path[path.len() - 1]);
}

/// Insert `layer` directly above the active layer — inside the group the
/// active layer lives in — or at the top of the document when nothing is
/// active (`docs/tasks/M2.md` M2-T01).
fn insert_above_active(doc: &mut Document, layer: Arc<Layer>) {
	let Some(path) = doc.active_layer().and_then(|id| doc.path_of(id)) else {
		doc.layers.push(layer);
		return;
	};
	let parent = id_at_path(doc, &path[..path.len() - 1]);
	let index = path[path.len() - 1] + 1;
	children_mut(doc, parent).insert(index, layer);
}

/// A copy of `layer` and its subtree: new ids, shared tile handles, renamed to
/// `name` (`docs/tasks/M2.md` M2-T01). Only the duplicated layer itself gets
/// the `" copy"` suffix — the children of a duplicated group keep their names,
/// like Photoshop. Tiles are immutable, so sharing them costs no pixel memory
/// and later edits copy-on-write only what they touch.
fn deep_copy(doc: &mut Document, layer: &Layer, name: String) -> Arc<Layer> {
	let mut copy = layer.clone();
	copy.id = doc.allocate_layer_id();
	copy.name = name;
	if let LayerKind::Group { children, expanded } = &layer.kind {
		let children = children.iter().map(|child| deep_copy(doc, child, child.name.clone())).collect();
		copy.kind = LayerKind::Group { children, expanded: *expanded };
	}
	Arc::new(copy)
}

/// The row Photoshop selects after rows disappear: the nearest remaining row
/// *below* the lowest deleted one, else the nearest one above the topmost
/// deleted row.
fn selection_after_delete(panel: &[LayerId], gone: &[LayerId]) -> Option<LayerId> {
	let deleted: Vec<usize> = panel.iter().enumerate().filter(|(_, id)| gone.contains(id)).map(|(index, _)| index).collect();
	let (first, last) = (*deleted.first()?, *deleted.last()?);
	panel[last + 1..]
		.iter()
		.find(|id| !gone.contains(id))
		.or_else(|| panel[..first].iter().rev().find(|id| !gone.contains(id)))
		.copied()
}

/// The mask tile grid for a new mask: `HideAll` leaves every slot empty (mask
/// value 0), `RevealAll` sets them all to `Solid(max)`. Both cost no tile
/// memory, and the mip levels are left dirty for the mip scheduler (M1-T05).
fn mask_image(width: u32, height: u32, format: PixelFormat, reveal: bool) -> TiledImage {
	let mut image = TiledImage::new(width, height, format);
	if reveal {
		let white = TileSlot::Solid(PixelValue::gray16(u16::MAX));
		let (cols, rows) = (image.grid(0).cols(), image.grid(0).rows());
		for ty in 0..rows {
			for tx in 0..cols {
				image.set_slot(tx, ty, white.clone());
			}
		}
	}
	image
}

/// Tile columns computed in parallel before their results are written back.
/// Bounds the buffers in flight (one 256² Rgba16 tile is 512 KB) instead of
/// collecting the whole layer.
const BAKE_COLUMNS: u32 = 16;

/// Multiply the alpha of `image` by `mask`, tile by tile on the rayon pool.
///
/// Returns a new image (so the caller can commit it in one write, or throw it
/// away on error), or `None` when the mask cannot change a single pixel.
fn bake_mask(image: &TiledImage, mask: &TiledImage, store: &TileStore) -> Result<Option<TiledImage>, CommandError> {
	let format = image.format();
	if !format.has_alpha() {
		return Err(CommandError::NotAllowed(format!(
			"layer tiles are {format:?}; only RGBA pixels can take a mask"
		)));
	}
	if image.width() != mask.width() || image.height() != mask.height() {
		return Err(CommandError::NotAllowed(format!(
			"the mask is {}×{} but the layer is {}×{}",
			mask.width(),
			mask.height(),
			image.width(),
			image.height()
		)));
	}
	let (cols, rows) = (image.grid(0).cols(), image.grid(0).rows());
	let mut baked = image.clone();
	let mut changed = false;
	for ty in 0..rows {
		for start in (0..cols).step_by(BAKE_COLUMNS as usize) {
			let end = (start + BAKE_COLUMNS).min(cols);
			let tiles: Result<Vec<(u32, TileBuffer)>, CommandError> = (start..end)
				.into_par_iter()
				.filter_map(|tx| match bake_tile(image.slot(0, tx, ty), mask.slot(0, tx, ty), format, store) {
					Ok(None) => None,
					Ok(Some(buffer)) => Some(Ok((tx, buffer))),
					Err(error) => Some(Err(error)),
				})
				.collect();
			for (tx, buffer) in tiles? {
				// `put_buffer` collapses a uniform result back to Empty/Solid
				// (no tile memory) and marks the mips above the tile dirty.
				baked.put_buffer(store, tx, ty, buffer);
				changed = true;
			}
		}
	}
	Ok(changed.then_some(baked))
}

/// One layer tile with the mask multiplied into its alpha, or `None` when the
/// mask cannot change it: it reveals everything, or the layer has no pixels
/// under it. Reads pixels, never writes to the store or the document.
fn bake_tile(layer: &TileSlot, mask: &TileSlot, format: PixelFormat, store: &TileStore) -> Result<Option<TileBuffer>, CommandError> {
	if matches!(mask, TileSlot::Solid(value) if value.0[0] == u16::MAX) {
		return Ok(None);
	}
	let pixels: Arc<TileBuffer> = match layer {
		// Zero alpha stays zero whatever the mask says.
		TileSlot::Empty => return Ok(None),
		// A uniform tile needs no store read; `filled` encodes it exactly.
		TileSlot::Solid(value) => Arc::new(TileBuffer::filled(format, *value)),
		TileSlot::Data(handle) => store.get(handle)?,
	};
	let mask = match mask {
		TileSlot::Empty => MaskSource::Zero,
		TileSlot::Solid(value) => MaskSource::Constant(value.0[0]),
		TileSlot::Data(handle) => {
			let buffer = store.get(handle)?;
			match buffer.format() {
				PixelFormat::Gray8 | PixelFormat::Gray16 => MaskSource::Tiles(buffer),
				other => {
					return Err(CommandError::NotAllowed(format!("layer masks are gray, not {other:?}")));
				}
			}
		}
	};
	Ok(Some(apply_mask_alpha(&pixels, &mask)))
}

/// Where one mask tile's values come from.
enum MaskSource {
	/// Mask value 0 (hidden) everywhere: an empty mask tile.
	Zero,
	/// The same value (16-bit scale) for the whole tile.
	Constant(u16),
	/// A stored mask tile (Gray8 or Gray16).
	Tiles(Arc<TileBuffer>),
}

impl MaskSource {
	/// Mask value at `pixel` (row-major index inside the tile), 0..=65535.
	fn at(&self, pixel: usize) -> u16 {
		match self {
			MaskSource::Zero => 0,
			MaskSource::Constant(value) => *value,
			MaskSource::Tiles(buffer) => gray16_at(buffer, pixel),
		}
	}
}

/// Every layer pixel with `alpha × mask`; colour is untouched (tiles are
/// straight alpha, `docs/DECISIONS.md` D-005).
///
/// A pixel whose alpha reaches 0 is cleared completely: the colour cannot
/// show through, and zero pixels let a fully hidden tile collapse back to
/// [`TileSlot::Empty`] instead of costing tile memory.
fn apply_mask_alpha(pixels: &TileBuffer, mask: &MaskSource) -> TileBuffer {
	let mut out = pixels.clone();
	match out.format() {
		PixelFormat::Rgba8 => {
			for (pixel, bytes) in out.bytes_mut().chunks_exact_mut(4).enumerate() {
				let m = mask.at(pixel);
				if m == u16::MAX {
					continue;
				}
				let alpha = scale_alpha8(bytes[3], m);
				if alpha == 0 {
					bytes.fill(0);
				} else {
					bytes[3] = alpha;
				}
			}
		}
		PixelFormat::Rgba16 => {
			let bytes = out.bytes_mut();
			for pixel in 0..TILE_PIXELS {
				let m = mask.at(pixel);
				if m == u16::MAX {
					continue;
				}
				let at = pixel * 8;
				let alpha = scale_alpha16(u16::from_ne_bytes([bytes[at + 6], bytes[at + 7]]), m);
				if alpha == 0 {
					bytes[at..at + 8].fill(0);
				} else {
					bytes[at + 6..at + 8].copy_from_slice(&alpha.to_ne_bytes());
				}
			}
		}
		other => unreachable!("layer pixels are RGBA, not {other:?} (checked in `bake_mask`)"),
	}
	out
}

/// A mask sample on the 16-bit scale. `buffer` is a `Gray8` or `Gray16` tile,
/// checked by [`bake_tile`].
fn gray16_at(buffer: &TileBuffer, pixel: usize) -> u16 {
	let bytes = buffer.bytes();
	match buffer.format() {
		PixelFormat::Gray8 => bytes[pixel] as u16 * 257,
		PixelFormat::Gray16 => u16::from_ne_bytes([bytes[pixel * 2], bytes[pixel * 2 + 1]]),
		other => unreachable!("mask tiles are gray, not {other:?} (checked in `bake_tile`)"),
	}
}

/// `alpha` (16-bit scale) × `mask` (16-bit scale), rounded half up.
fn scale_alpha16(alpha: u16, mask: u16) -> u16 {
	((alpha as u64 * mask as u64 * 2 + u16::MAX as u64) / (2 * u16::MAX as u64)) as u16
}

/// `alpha` is an 8-bit sample; `mask` is on the 16-bit scale. Rounded half up
/// on the 8-bit scale, so no second rounding is introduced on write.
fn scale_alpha8(alpha: u8, mask: u16) -> u8 {
	((alpha as u64 * mask as u64 * 2 + u16::MAX as u64) / (2 * u16::MAX as u64)) as u8
}

/// The effect of removing a mask: the layer's pixels changed only when the
/// mask was actually baked into them.
/// `ApplyFilter`: validate, then replace the layer's pixels with the
/// filtered image computed by the engine's [`PixelOps`].
fn apply_filter(doc: &mut Document, layer: &LayerRef, filter: &FilterParams, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	filter.validate()?;
	let id = resolve(doc, layer)?;
	let target = doc.layer(id).expect("resolved id exists");
	if target.locked_pixels {
		return Err(CommandError::Locked(id));
	}
	let LayerKind::Pixel { image, offset } = &target.kind else {
		return Err(CommandError::NotAllowed("the layer has no pixels; rasterise it first".into()));
	};
	let ops = ctx
		.ops
		.ok_or_else(|| CommandError::NotAllowed("filters need the engine's pixel operations".into()))?;
	let mut filtered = ops.filter(image, *offset, (doc.width, doc.height), filter, ctx.tiles)?;
	// With a selection the filter shows only through it (M5-T05).
	if let Some(selection) = &doc.selection {
		let before = crate::pixels::Placed { image, offset: *offset };
		filtered = crate::pixels::blend_through(before, &filtered, selection, (doc.width, doc.height), ctx.tiles)?;
	}
	if let LayerKind::Pixel { image, .. } = &mut doc.layer_mut(id).expect("resolved id exists").kind {
		*image = filtered;
	}
	Ok(CommandEffect {
		label: filter.label().into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

/// The engine's pixel operations, or the error a command without them gives.
fn pixel_ops<'a>(ctx: &CommandContext<'a>, what: &str) -> Result<&'a dyn PixelOps, CommandError> {
	ctx.ops
		.ok_or_else(|| CommandError::NotAllowed(format!("{what} needs the engine's pixel operations")))
}

/// `MergeLayers` (M4-T08): composite the layers, then replace the bottom-most
/// of them with the result and remove the others.
fn merge_layers(doc: &mut Document, layers: &[LayerRef], ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let ids = resolve_all(doc, layers, true)?;
	if ids.len() < 2 {
		return Err(CommandError::NotAllowed("select at least two layers to merge".into()));
	}
	let panel = doc.panel_order();
	let bottom = *ids.iter().max_by_key(|id| panel.iter().position(|p| p == *id)).expect("at least two ids");
	let image = pixel_ops(ctx, "merging")?.composite(doc, &ids, None, ctx.tiles)?;
	// Mutate only now: everything that can fail has run.
	let name = doc.layer(bottom).expect("resolved id exists").name.clone();
	for &id in ids.iter().filter(|&&id| id != bottom) {
		remove_layer(doc, id);
	}
	let path = doc.path_of(bottom).expect("the bottom layer stays");
	let parent = id_at_path(doc, &path[..path.len() - 1]);
	children_mut(doc, parent)[path[path.len() - 1]] = Arc::new(Layer::new(bottom, name, LayerKind::Pixel { image, offset: (0, 0) }));
	doc.selected = vec![bottom];
	Ok(CommandEffect {
		label: "Merge Layers".into(),
		pixels_changed: vec![bottom],
		structure_changed: true,
		..Default::default()
	})
}

/// `Flatten` (M4-T08): one "Background" layer with the visible composite on white.
fn flatten(doc: &mut Document, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if doc.layers.is_empty() {
		return Err(CommandError::NotAllowed("the document has no layers".into()));
	}
	let roots: Vec<LayerId> = doc.layers.iter().map(|l| l.id).collect();
	let image = pixel_ops(ctx, "flattening")?.composite(doc, &roots, Some([u16::MAX; 4]), ctx.tiles)?;
	let id = doc.allocate_layer_id();
	let mut layer = Layer::new(id, "Background", LayerKind::Pixel { image, offset: (0, 0) });
	layer.locked_position = true;
	doc.layers = vec![Arc::new(layer)];
	doc.selected = vec![id];
	Ok(CommandEffect {
		label: "Flatten Image".into(),
		pixels_changed: vec![id],
		structure_changed: true,
		..Default::default()
	})
}

/// `StampVisible` (M4-T08): a new layer with the composite of the visible layers.
fn stamp_visible(doc: &mut Document, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let roots: Vec<LayerId> = doc.layers.iter().map(|l| l.id).collect();
	if roots.is_empty() {
		return Err(CommandError::NotAllowed("the document has no layers".into()));
	}
	let image = pixel_ops(ctx, "stamping")?.composite(doc, &roots, None, ctx.tiles)?;
	add_layer(doc, &NewLayer::Pixel, None)?;
	let id = *doc.selected.first().expect("add_layer selects the new layer");
	if let LayerKind::Pixel { image: target, .. } = &mut doc.layer_mut(id).expect("just added").kind {
		*target = image;
	}
	Ok(CommandEffect {
		label: "Stamp Visible".into(),
		pixels_changed: vec![id],
		structure_changed: true,
		..Default::default()
	})
}

/// `AssignProfile` (M4-T03): metadata only.
fn assign_profile(doc: &mut Document, profile: &ColorProfile) -> Result<CommandEffect, CommandError> {
	doc.color.profile = profile.clone();
	Ok(CommandEffect {
		label: "Assign Profile".into(),
		// Every layer looks different on screen.
		props_changed: doc.panel_order(),
		..Default::default()
	})
}

/// `ConvertProfile` (M4-T03): the pixels of every pixel layer and the colour
/// of every solid fill, converted; then the document's profile.
fn convert_profile(
	doc: &mut Document,
	profile: &ColorProfile,
	intent: RenderingIntent,
	bpc: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let ops = pixel_ops(ctx, "converting")?;
	let from = doc.color.profile.clone();
	let conversion = crate::ops::Conversion {
		from: &from,
		to: profile,
		intent,
		bpc,
	};
	// Compute everything first (all or nothing), then install.
	let mut converted: Vec<(LayerId, LayerKind)> = Vec::new();
	let mut failure = None;
	doc.walk(|layer, _| {
		if failure.is_some() {
			return;
		}
		let kind = match &layer.kind {
			LayerKind::Pixel { image, offset } => ops
				.convert(image, &conversion, ctx.tiles)
				.map(|image| LayerKind::Pixel { image, offset: *offset }),
			LayerKind::SolidFill { rgba } => ops.convert_color(*rgba, &conversion).map(|rgba| LayerKind::SolidFill { rgba }),
			// A shape's own colours convert with it; the cache is redrawn from
			// the new geometry afterwards (M6-T06).
			LayerKind::Shape {
				shape,
				fill,
				stroke,
				transform,
				cache,
			} => {
				let fill = match fill {
					Some(paint) => match ops.convert_color(paint.rgba(), &conversion) {
						Ok(rgba) => Some(Paint::Solid { rgba }),
						Err(error) => {
							failure = Some(error);
							return;
						}
					},
					None => None,
				};
				let stroke = match stroke {
					Some(style) => match ops.convert_color(style.paint.rgba(), &conversion) {
						Ok(rgba) => Some(StrokeStyle {
							paint: Paint::Solid { rgba },
							..style.clone()
						}),
						Err(error) => {
							failure = Some(error);
							return;
						}
					},
					None => None,
				};
				let mut cache = cache.clone();
				cache.mark_all_dirty();
				Ok(LayerKind::Shape {
					shape: shape.clone(),
					fill,
					stroke,
					transform: *transform,
					cache,
				})
			}
			_ => return,
		};
		match kind {
			Ok(kind) => converted.push((layer.id, kind)),
			Err(error) => failure = Some(error),
		}
	});
	if let Some(error) = failure {
		return Err(error);
	}
	let changed: Vec<LayerId> = converted.iter().map(|(id, _)| *id).collect();
	for (id, kind) in converted {
		doc.layer_mut(id).expect("walked layer exists").kind = kind;
	}
	doc.color.profile = profile.clone();
	Ok(CommandEffect {
		label: "Convert to Profile".into(),
		pixels_changed: changed,
		props_changed: doc.panel_order(),
		..Default::default()
	})
}

fn delete_mask_effect(id: LayerId, pixels_changed: bool) -> CommandEffect {
	CommandEffect {
		label: "Delete Layer Mask".into(),
		pixels_changed: if pixels_changed { vec![id] } else { Vec::new() },
		props_changed: vec![id],
		..Default::default()
	}
}

/// The counter a new layer of `kind` takes its default name from.
fn name_kind(new: &NewLayer) -> NameKind {
	match new {
		NewLayer::Pixel => NameKind::Pixel,
		NewLayer::SolidFill { .. } => NameKind::SolidFill,
		NewLayer::Group => NameKind::Group,
		NewLayer::Adjustment(adjustment) => NameKind::of_adjustment(adjustment),
		// A rectangle is named "Rectangle 1", like Photoshop's tool, so the
		// counter follows the kind of shape drawn (M6-T06).
		NewLayer::Shape { shape, .. } => NameKind::of_shape_stem(shape.stem()),
	}
}

/// History label for `AddLayer`, in Photoshop's wording.
fn add_label(new: &NewLayer) -> String {
	match new {
		NewLayer::Pixel => "New Layer".into(),
		NewLayer::Group => "New Group".into(),
		NewLayer::SolidFill { .. } => "New Fill Layer".into(),
		NewLayer::Adjustment(adjustment) => {
			format!("New {} Adjustment Layer", NameKind::of_adjustment(adjustment).stem())
		}
		// Photoshop names the step after the shape: "New Rectangle" (M6-T06).
		NewLayer::Shape { shape, .. } => format!("New {}", shape.stem()),
	}
}

fn validate_unit(field: &'static str, value: Option<f32>) -> Result<(), CommandError> {
	match value {
		Some(v) if !(0.0..=1.0).contains(&v) || v.is_nan() => Err(CommandError::InvalidValue {
			field,
			reason: format!("{v} is outside 0..=1"),
		}),
		_ => Ok(()),
	}
}

/// Reject a name that would give the Layers panel an unreadable row.
fn validate_name(name: Option<&str>) -> Result<(), CommandError> {
	match name {
		Some(n) if n.trim().is_empty() => Err(CommandError::InvalidValue {
			field: "name",
			reason: "must not be empty".into(),
		}),
		_ => Ok(()),
	}
}

fn checked_offset(value: i32, delta: i32) -> Result<i32, CommandError> {
	value.checked_add(delta).ok_or_else(|| CommandError::InvalidValue {
		field: "offset",
		reason: format!("{value} + {delta} is outside the coordinate range"),
	})
}

// ---------------------------------------------------------------------------
// The pixel selection (M5-T03)
// ---------------------------------------------------------------------------

/// A selection command's effect: a real history step, but not a document
/// change (the selection is not saved — D-028).
fn selection_effect(label: &str) -> CommandEffect {
	CommandEffect {
		label: label.into(),
		history_only: true,
		..Default::default()
	}
}

/// The Photoshop History label of a rasterised shape.
fn shape_label(shape: &SelectionShape) -> &'static str {
	match shape {
		SelectionShape::Rect { .. } => "Rectangular Marquee",
		SelectionShape::Ellipse { .. } => "Elliptical Marquee",
		SelectionShape::Polygon { .. } => "Lasso",
		SelectionShape::RowPixel { .. } => "Single Row Marquee",
		SelectionShape::ColumnPixel { .. } => "Single Column Marquee",
	}
}

fn select(
	doc: &mut Document,
	shape: &SelectionShape,
	mode: SelectMode,
	feather: f64,
	anti_alias: bool,
	ctx: &mut CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if !feather.is_finite() || feather < 0.0 {
		return Err(CommandError::InvalidValue {
			field: "feather",
			reason: format!("{feather} is not a distance"),
		});
	}
	let size = (doc.width, doc.height);
	let depth = doc.color.depth;
	let Some(ops) = ctx.ops else {
		return Err(CommandError::NotAllowed("a selection needs the engine's rasteriser".into()));
	};
	let mut shape_selection = ops.rasterise(shape, size, depth, anti_alias, ctx.tiles)?;
	if feather > 0.0 {
		shape_selection = ops
			.modify_selection(&shape_selection, &SelectModify::Feather(feather), size, depth, ctx.tiles)?
			.unwrap_or_else(|| Selection::empty(size, depth));
	}
	doc.selection = selection::combine(size, doc.selection.as_ref(), &shape_selection, mode, ctx.tiles)?;
	Ok(selection_effect(shape_label(shape)))
}

fn magic_wand(doc: &mut Document, params: &WandParams, mode: SelectMode, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if !(0.0..=255.0).contains(&params.tolerance) {
		return Err(CommandError::InvalidValue {
			field: "tolerance",
			reason: format!("{} is not in 0..=255", params.tolerance),
		});
	}
	let Some(ops) = ctx.ops else {
		return Err(CommandError::NotAllowed("the Magic Wand needs the engine".into()));
	};
	let size = (doc.width, doc.height);
	let found = ops.magic_wand(doc, params, ctx.tiles)?;
	doc.selection = match found {
		Some(found) => selection::combine(size, doc.selection.as_ref(), &found, mode, ctx.tiles)?,
		// Nothing found: Replace and Intersect leave no selection, Add and
		// Subtract keep the current one.
		None => match mode {
			SelectMode::Replace | SelectMode::Intersect => None,
			SelectMode::Add | SelectMode::Subtract => doc.selection.take(),
		},
	};
	Ok(selection_effect("Magic Wand"))
}

/// The pixel layer a pixel edit targets: its id, image and offset. Locked
/// pixels and layers without pixels are refused.
fn pixel_target(doc: &Document, layer: &LayerRef) -> Result<(LayerId, TiledImage, (i32, i32)), CommandError> {
	let id = resolve(doc, layer)?;
	let target = doc.layer(id).expect("resolved id exists");
	if target.locked_pixels {
		return Err(CommandError::Locked(id));
	}
	match &target.kind {
		LayerKind::Pixel { image, offset } => Ok((id, image.clone(), *offset)),
		_ => Err(CommandError::NotAllowed("the layer has no pixels; rasterise it first".into())),
	}
}

/// Replace a pixel layer's image (and offset).
fn set_pixels(doc: &mut Document, id: LayerId, new_image: TiledImage, new_offset: (i32, i32)) {
	if let LayerKind::Pixel { image, offset } = &mut doc.layer_mut(id).expect("resolved id exists").kind {
		*image = new_image;
		*offset = new_offset;
	}
}

#[allow(clippy::too_many_arguments)]
fn fill(
	doc: &mut Document,
	layer: &LayerRef,
	color: [u16; 4],
	mode: BlendMode,
	opacity: f64,
	preserve_transparency: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if !(0.0..=1.0).contains(&opacity) {
		return Err(CommandError::InvalidValue {
			field: "opacity",
			reason: format!("{opacity} is not in 0..=1"),
		});
	}
	let (id, image, offset) = pixel_target(doc, layer)?;
	let locked_alpha = doc.layer(id).is_some_and(|l| l.locked_transparency);
	let spec = crate::pixels::FillSpec {
		color,
		mode,
		opacity,
		preserve_transparency: preserve_transparency || locked_alpha,
	};
	let placed = crate::pixels::Placed { image: &image, offset };
	let (filled, offset) = crate::pixels::fill(placed, doc.selection.as_ref(), (doc.width, doc.height), &spec, ctx.tiles)?;
	set_pixels(doc, id, filled, offset);
	Ok(CommandEffect {
		label: "Fill".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

fn clear(doc: &mut Document, layer: &LayerRef, cut: bool, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.as_ref() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let (id, image, offset) = pixel_target(doc, layer)?;
	let cleared = crate::pixels::clear(crate::pixels::Placed { image: &image, offset }, selection, (doc.width, doc.height), ctx.tiles)?;
	set_pixels(doc, id, cleared, offset);
	Ok(CommandEffect {
		label: if cut { "Cut".into() } else { "Clear".into() },
		pixels_changed: vec![id],
		..Default::default()
	})
}

fn layer_via_copy(doc: &mut Document, cut: bool, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let Some(active) = doc.active_layer() else {
		return Err(CommandError::NotAllowed("no active layer".into()));
	};
	let (id, image, offset) = if cut {
		pixel_target(doc, &LayerRef::Id(active))?
	} else {
		// Copying reads the pixels only: a locked layer may be copied.
		match &doc.layer(active).expect("the active layer exists").kind {
			LayerKind::Pixel { image, offset } => (active, image.clone(), *offset),
			_ => return Err(CommandError::NotAllowed("the layer has no pixels; rasterise it first".into())),
		}
	};
	let canvas = (doc.width, doc.height);
	let placed = crate::pixels::Placed { image: &image, offset };
	let taken = crate::pixels::extract(placed, Some(&selection), canvas, ctx.tiles)?;
	if taken.grid(0).non_empty().next().is_none() {
		return Err(CommandError::NotAllowed("the selected area is empty".into()));
	}
	if cut {
		let cleared = crate::pixels::clear(placed, &selection, canvas, ctx.tiles)?;
		set_pixels(doc, id, cleared, offset);
	}
	let name = doc.next_default_name(NameKind::Pixel);
	let new_id = doc.allocate_layer_id();
	let layer = Arc::new(Layer::new(new_id, name, LayerKind::Pixel { image: taken, offset }));
	insert_above_active(doc, layer);
	doc.selected = vec![new_id];
	Ok(CommandEffect {
		label: if cut { "Layer Via Cut".into() } else { "Layer Via Copy".into() },
		structure_changed: true,
		pixels_changed: if cut { vec![id] } else { Vec::new() },
		..Default::default()
	})
}

#[allow(clippy::too_many_arguments)]
fn stroke(
	doc: &mut Document,
	layer: &LayerRef,
	target: StrokeTarget,
	tool: &StrokeTool,
	brush: &BrushParams,
	color: [u16; 4],
	samples: &[StrokeSample],
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if samples.is_empty() {
		return Err(CommandError::NotAllowed("a stroke needs at least one sample".into()));
	}
	let id = resolve(doc, layer)?;
	let layer = doc.layer(id).expect("resolved id exists");
	if layer.locked_pixels {
		return Err(CommandError::Locked(id));
	}
	match target {
		StrokeTarget::Pixels if !matches!(layer.kind, LayerKind::Pixel { .. }) => {
			return Err(CommandError::NotAllowed("the layer has no pixels; rasterise it first".into()));
		}
		StrokeTarget::Mask if layer.mask.is_none() => return Err(CommandError::NotAllowed("the layer has no mask".into())),
		_ => {}
	}
	let (image, offset) = pixel_ops(ctx, "painting")?.stroke(doc, id, target, tool, brush, color, samples, ctx.tiles)?;
	let target_layer = doc.layer_mut(id).expect("resolved id exists");
	match target {
		StrokeTarget::Pixels => {
			if let LayerKind::Pixel {
				image: old,
				offset: old_offset,
			} = &mut target_layer.kind
			{
				*old = image;
				*old_offset = offset;
			}
		}
		StrokeTarget::Mask => {
			if let Some(mask) = &mut target_layer.mask {
				mask.image = image;
			}
		}
	}
	Ok(CommandEffect {
		label: tool.label().into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

fn paste(doc: &mut Document, in_place: bool, center: Option<(f64, f64)>, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let ops = pixel_ops(ctx, "Paste")?;
	let Some(clip) = ops.clipboard() else {
		return Err(CommandError::NotAllowed("the clipboard is empty".into()));
	};
	let image = crate::pixels::convert_depth(&clip.image, doc.color.depth.rgba_format(), ctx.tiles)?;
	let (bx0, by0, bx1, by1) = clip.bounds;
	let offset = match (in_place, center) {
		(false, Some((cx, cy))) => {
			let dx = (cx - f64::from(bx0 + bx1) / 2.0).round() as i32;
			let dy = (cy - f64::from(by0 + by1) / 2.0).round() as i32;
			(clip.offset.0.saturating_add(dx), clip.offset.1.saturating_add(dy))
		}
		_ => clip.offset,
	};
	let name = doc.next_default_name(NameKind::Pixel);
	let id = doc.allocate_layer_id();
	let layer = Arc::new(Layer::new(id, name, LayerKind::Pixel { image, offset }));
	insert_above_active(doc, layer);
	doc.selected = vec![id];
	Ok(CommandEffect {
		label: if in_place { "Paste in Place".into() } else { "Paste".into() },
		structure_changed: true,
		..Default::default()
	})
}

// ---------------------------------------------------------------------------
// Image geometry (M6-T02)
// ---------------------------------------------------------------------------

/// Every layer of `doc`, depth first (a group's children included).
fn layer_ids(doc: &Document) -> Vec<LayerId> {
	let mut ids = Vec::new();
	doc.walk(|layer, _| ids.push(layer.id));
	ids
}

/// Give the document a new canvas size and rebuild every shape layer's cache
/// for it (M6-T06). A shape's cache is derived from its geometry and is always
/// the size of the canvas, so a canvas-level command replaces it — the tiles
/// come back at the next draw, at whatever level is on screen. Returns the
/// layers whose cache was rebuilt.
fn set_canvas(doc: &mut Document, width: u32, height: u32) -> Vec<LayerId> {
	doc.width = width;
	doc.height = height;
	let format = doc.color.depth.rgba_format();
	let mut changed = Vec::new();
	for id in layer_ids(doc) {
		if let Some(layer) = doc.layer_mut(id)
			&& let LayerKind::Shape { cache, .. } = &mut layer.kind
		{
			if (cache.width(), cache.height()) == (width, height) {
				continue;
			}
			*cache = TiledImage::derived(width, height, format);
			changed.push(id);
		}
	}
	changed
}

/// Fold a canvas-level mapping into every shape layer's matrix (M6-T06): a
/// shape is placed by a matrix, so the canvas turning, flipping or scaling
/// turns, flips or scales the shape exactly, and nothing is resampled. Returns
/// the layers that changed; a mapping that is not affine (which no
/// canvas-level command produces) leaves them all alone.
fn map_shape_layers(doc: &mut Document, mapping: Mapping) -> Vec<LayerId> {
	let mut changed = Vec::new();
	for id in layer_ids(doc) {
		if let Some(layer) = doc.layer_mut(id)
			&& let LayerKind::Shape { transform, cache, .. } = &mut layer.kind
		{
			let Some(moved) = mapping.then_affine(*transform) else {
				continue;
			};
			if moved == *transform {
				continue;
			}
			*transform = moved;
			cache.mark_all_dirty();
			changed.push(id);
		}
	}
	changed
}

/// Image ▸ Rotate 90° CW / CCW / 180° (M6-T02).
fn rotate_canvas(doc: &mut Document, quarter_turns: i8, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let Some(op) = Permutation::from_quarter_turns(quarter_turns) else {
		return Err(CommandError::InvalidValue {
			field: "quarter_turns",
			reason: format!("{quarter_turns} is a whole number of full turns; 1, 2 or 3 (or -1…-3) turns the canvas"),
		});
	};
	permute_canvas(doc, op, ctx)
}

/// Where a layer's mask sits on the canvas: a linked mask of a pixel layer at
/// the layer's offset, every other mask at the origin (the renderer's rule,
/// `fx_render::program`).
fn mask_origin(layer: &Layer) -> (i32, i32) {
	match (&layer.kind, layer.mask.as_ref().is_some_and(|m| m.linked)) {
		(LayerKind::Pixel { offset, .. }, true) => *offset,
		_ => (0, 0),
	}
}

/// A mask's image that a geometry command left at canvas pixel `at`, moved to
/// where the mask must sit afterwards (`target`, see [`mask_origin`]), padded
/// with the mask's outside value.
fn mask_to(image: TiledImage, at: (i32, i32), target: (i32, i32), mask: &Mask, store: &TileStore) -> Result<TiledImage, CommandError> {
	if at == target {
		return Ok(image);
	}
	Ok(crate::pixels::place_at(
		&image,
		at,
		target,
		fx_tiles::PixelValue::gray16(mask.outside_value),
		store,
	)?)
}

/// Rotate or mirror the whole canvas (M6-T02): every pixel image is permuted
/// (bit-exact) and every offset recomputed, so the content stays where it was.
/// Nothing is resampled and nothing is interpolated, so the work is the tile
/// I/O of the images that really have pixels — an empty or uniform layer costs
/// nothing at all.
fn permute_canvas(doc: &mut Document, op: Permutation, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let ops = pixel_ops(ctx, op.label())?;
	let canvas = (doc.width, doc.height);
	let (dest_w, dest_h) = op.size(canvas);
	// Phase 1: compute every new image. The document stays untouched until all
	// of them are ready, so a corrupt tile or an offset that does not fit
	// leaves it exactly as it was (apply either fully succeeds or does nothing).
	let mut images: Vec<(LayerId, TiledImage, (i32, i32))> = Vec::new();
	let mut masks: Vec<(LayerId, TiledImage)> = Vec::new();
	for id in layer_ids(doc) {
		let Some(layer) = doc.layer(id) else { continue };
		let mut target = (0, 0);
		if let LayerKind::Pixel { image, offset } = &layer.kind {
			let (image, new_offset) = ops.rotate(image, *offset, canvas, op, ctx.tiles)?;
			images.push((id, image, new_offset));
			if layer.mask.as_ref().is_some_and(|m| m.linked) {
				target = new_offset;
			}
		}
		if let Some(mask) = &layer.mask {
			// The model stores no offset for a mask (see `mask_origin`): turn it
			// where it sits, then put it where it must sit afterwards.
			let (image, at) = ops.rotate(&mask.image, mask_origin(layer), canvas, op, ctx.tiles)?;
			masks.push((id, mask_to(image, at, target, mask, ctx.tiles)?));
		}
	}
	let selection = match &doc.selection {
		Some(selection) => Some(ops.rotate(&selection.image, selection.offset, canvas, op, ctx.tiles)?),
		None => None,
	};
	let reselect = match &doc.reselect {
		Some(selection) => Some(ops.rotate(&selection.image, selection.offset, canvas, op, ctx.tiles)?),
		None => None,
	};
	// Phase 2: commit.
	let mut changed = Vec::new();
	for (id, image, offset) in images {
		if let Some(layer) = doc.layer_mut(id)
			&& let LayerKind::Pixel { image: pixels, offset: at } = &mut layer.kind
		{
			*pixels = image;
			*at = offset;
		}
		changed.push(id);
	}
	for (id, image) in masks {
		if let Some(layer) = doc.layer_mut(id)
			&& let Some(mask) = &mut layer.mask
		{
			mask.image = image;
		}
		changed.push(id);
	}
	if let Some((image, offset)) = selection {
		doc.selection = Some(Selection { image, offset });
	}
	if let Some((image, offset)) = reselect {
		doc.reselect = Some(Selection { image, offset });
	}
	// Shape layers (M6-T06) carry a transform, not pixels: they turn with the
	// canvas without losing anything. Text layers follow in M6-T07.
	changed.extend(map_shape_layers(doc, op.mapping(canvas)));
	changed.extend(set_canvas(doc, dest_w, dest_h));
	changed.sort_unstable();
	changed.dedup();
	Ok(CommandEffect {
		label: op.label().into(),
		pixels_changed: changed,
		..Default::default()
	})
}

/// Image ▸ Rotate ▸ Arbitrary… (M6-T02): the canvas grows to the rotated
/// bounding box and every pixel image is resampled through the turn.
fn rotate_canvas_arbitrary(doc: &mut Document, angle_deg: f64, filter: Filter, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if !angle_deg.is_finite() {
		return Err(CommandError::InvalidValue {
			field: "angle_deg",
			reason: format!("{angle_deg} is not a number"),
		});
	}
	// Photoshop's Rotate Image dialog: the direction picks which way.
	let angle = angle_deg.rem_euclid(360.0);
	if angle == 0.0 {
		return Err(CommandError::InvalidValue {
			field: "angle_deg",
			reason: "0° leaves the canvas as it is".into(),
		});
	}
	let ops = pixel_ops(ctx, "Rotate Image")?;
	let canvas = (doc.width, doc.height);
	// Turn about the canvas centre (the same point in both the old and the new
	// canvas), then move the bounding box's top-left corner to the origin: the
	// new canvas is exactly the rotated canvas's box, with no wasted border.
	let turn = Mapping::rotation_about(angle.to_radians(), f64::from(canvas.0) / 2.0, f64::from(canvas.1) / 2.0);
	let Some(((left, top), size)) = dest_rect(&turn, [0.0, 0.0, f64::from(canvas.0), f64::from(canvas.1)]) else {
		return Err(CommandError::NotAllowed("the rotated canvas has no bounding box".into()));
	};
	let mapping = turn.after_destination_translation(-f64::from(left), -f64::from(top));
	let mut changed = resample_document(doc, ops, mapping, filter, ctx.tiles)?;
	changed.extend(set_canvas(doc, size.0, size.1));
	Ok(CommandEffect {
		label: "Rotate Image".into(),
		pixels_changed: changed,
		..Default::default()
	})
}

/// Image ▸ Canvas Size (M6-T02, D-015): the document's size and every offset
/// change — no pixel is rewritten, so content outside the new canvas is kept
/// and reappears if the canvas grows again.
fn canvas_size(doc: &mut Document, width: u32, height: u32, anchor: Anchor9, store: &TileStore) -> Result<CommandEffect, CommandError> {
	if width == 0 || height == 0 {
		return Err(CommandError::InvalidValue {
			field: if width == 0 { "width" } else { "height" },
			reason: "a canvas is at least 1 × 1 pixel".into(),
		});
	}
	let (dx, dy) = anchor.offset((doc.width, doc.height), (width, height));
	let (mut moved, masks) = shift_offsets(doc, dx, dy, store)?;
	moved.extend(set_canvas(doc, width, height));
	Ok(CommandEffect {
		label: "Canvas Size".into(),
		props_changed: moved,
		pixels_changed: masks,
		..Default::default()
	})
}

/// Move every layer offset, and the selection's, by `(dx, dy)` — Canvas Size
/// and Crop's placement change (M6-T02/T03). Every new offset is computed
/// before anything moves, so a document whose content cannot be expressed in
/// 32 bits is left exactly as it was. A mask that sits at the canvas origin
/// (unlinked, or on a group or an adjustment layer) has no offset to move: its
/// pixels move instead, and what goes past the new origin is dropped. Returns
/// the layers that moved and the layers whose mask pixels were moved.
fn shift_offsets(doc: &mut Document, dx: i32, dy: i32, store: &TileStore) -> Result<(Vec<LayerId>, Vec<LayerId>), CommandError> {
	let moved_offset = |offset: (i32, i32)| -> Result<(i32, i32), CommandError> { Ok((checked_offset(offset.0, dx)?, checked_offset(offset.1, dy)?)) };
	// Validate every new offset before moving anything.
	let mut moved = Vec::new();
	let mut masks = Vec::new();
	for id in layer_ids(doc) {
		let Some(layer) = doc.layer(id) else { continue };
		if let LayerKind::Pixel { offset, .. } = &layer.kind {
			moved.push((id, moved_offset(*offset)?));
		}
		if let Some(mask) = &layer.mask
			&& mask_origin(layer) == (0, 0)
			&& !(matches!(layer.kind, LayerKind::Pixel { .. }) && mask.linked)
		{
			masks.push((id, mask_to(mask.image.clone(), (dx, dy), (0, 0), mask, store)?));
		}
	}
	let selection = match &doc.selection {
		Some(selection) => Some(moved_offset(selection.offset)?),
		None => None,
	};
	let reselection = match &doc.reselect {
		Some(selection) => Some(moved_offset(selection.offset)?),
		None => None,
	};
	// Commit.
	for (id, offset) in &moved {
		if let Some(layer) = doc.layer_mut(*id)
			&& let LayerKind::Pixel { offset: at, .. } = &mut layer.kind
		{
			*at = *offset;
		}
	}
	if let (Some(selection), Some(offset)) = (doc.selection.as_mut(), selection) {
		selection.offset = offset;
	}
	if let (Some(selection), Some(offset)) = (doc.reselect.as_mut(), reselection) {
		selection.offset = offset;
	}
	// A shape layer has no offset to move: its matrix is translated instead
	// (M6-T06), which is the same thing without touching a single tile.
	let mut shapes = map_shape_layers(doc, Mapping::translation(f64::from(dx), f64::from(dy)));
	let mut masks_moved = Vec::new();
	for (id, image) in masks {
		if let Some(layer) = doc.layer_mut(id)
			&& let Some(mask) = &mut layer.mask
		{
			mask.image = image;
		}
		masks_moved.push(id);
	}
	shapes.extend(moved.into_iter().map(|(id, _)| id));
	Ok((shapes, masks_moved))
}

/// Keep only the pixels inside `rect`, for every pixel layer, mask and the
/// selection (M6-T03's Delete Cropped Pixels). Phase 1 computes every new
/// image, so a corrupt tile leaves the document untouched; the selection's
/// coverage outside the rectangle simply stops being selected.
fn clip_document(doc: &mut Document, rect: (i32, i32, u32, u32), store: &TileStore) -> Result<Vec<LayerId>, CommandError> {
	let mut images = Vec::new();
	let mut masks = Vec::new();
	for id in layer_ids(doc) {
		let Some(layer) = doc.layer(id) else { continue };
		if let LayerKind::Pixel { image, offset } = &layer.kind {
			images.push((id, crate::pixels::clip_to_rect(Placed { image, offset: *offset }, rect, store)?));
		}
		if let Some(mask) = &layer.mask {
			let placed = Placed {
				image: &mask.image,
				offset: mask_origin(layer),
			};
			masks.push((id, crate::pixels::clip_to_rect(placed, rect, store)?));
		}
	}
	let clip = |selection: &Selection| {
		crate::pixels::clip_to_rect(
			Placed {
				image: &selection.image,
				offset: selection.offset,
			},
			rect,
			store,
		)
	};
	let selection = doc.selection.as_ref().map(clip).transpose()?;
	let reselect = doc.reselect.as_ref().map(clip).transpose()?;
	// Commit.
	let mut changed = Vec::new();
	for (id, image) in images {
		if let Some(layer) = doc.layer_mut(id)
			&& let LayerKind::Pixel { image: pixels, .. } = &mut layer.kind
		{
			*pixels = image;
		}
		changed.push(id);
	}
	for (id, image) in masks {
		if let Some(layer) = doc.layer_mut(id)
			&& let Some(mask) = &mut layer.mask
		{
			mask.image = image;
		}
		changed.push(id);
	}
	if let (Some(selection), Some(image)) = (doc.selection.as_mut(), selection) {
		selection.image = image;
	}
	if let (Some(selection), Some(image)) = (doc.reselect.as_mut(), reselect) {
		selection.image = image;
	}
	Ok(changed)
}

/// An image and the canvas pixel of its (0, 0).
type PlacedImage = (TiledImage, (i32, i32));

/// The part of a placed image that holds tiles: the image cut to the box of
/// its non-empty tiles (whole tiles, so no pixel is rewritten), and where that
/// box sits. `None` for an image with no content. A transform then resamples
/// the content's box, not the whole layer (a small object on a canvas-sized
/// layer, M6-T04).
fn tight(image: &TiledImage, offset: (i32, i32), store: &TileStore) -> Result<Option<PlacedImage>, CommandError> {
	let mut bounds: Option<(u32, u32, u32, u32)> = None;
	for (tx, ty, _) in image.grid(0).non_empty() {
		bounds = Some(match bounds {
			None => (tx, ty, tx, ty),
			Some(b) => (b.0.min(tx), b.1.min(ty), b.2.max(tx), b.3.max(ty)),
		});
	}
	let Some((tx0, ty0, tx1, ty1)) = bounds else {
		return Ok(None);
	};
	let (x0, y0) = (tx0 * fx_tiles::TILE_SIZE, ty0 * fx_tiles::TILE_SIZE);
	let x1 = ((tx1 + 1) * fx_tiles::TILE_SIZE).min(image.width());
	let y1 = ((ty1 + 1) * fx_tiles::TILE_SIZE).min(image.height());
	let at = (checked_offset(offset.0, x0 as i32)?, checked_offset(offset.1, y0 as i32)?);
	if (x0, y0, x1, y1) == (0, 0, image.width(), image.height()) {
		return Ok(Some((image.clone(), offset)));
	}
	let cut = crate::pixels::place_in(image, offset, at, Some((x1 - x0, y1 - y0)), fx_tiles::PixelValue::TRANSPARENT, store)?;
	Ok(Some((cut, at)))
}

/// Edit ▸ Free Transform and the Transform submenu (M6-T04).
fn transform_layer(doc: &mut Document, layer: &LayerRef, mapping: Mapping, filter: Filter, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if !mapping.is_finite() {
		return Err(CommandError::InvalidValue {
			field: "mapping",
			reason: "the transform is not a finite mapping".into(),
		});
	}
	let id = resolve(doc, layer)?;
	let ops = pixel_ops(ctx, "Free Transform")?;
	let store = ctx.tiles;
	let canvas = (doc.width, doc.height);
	let target = doc.layer(id).expect("resolved id exists");
	let LayerKind::Pixel { image, offset } = &target.kind else {
		return Err(CommandError::NotAllowed("only a pixel layer can be transformed".into()));
	};
	if target.locked_position || target.locked_pixels {
		return Err(CommandError::Locked(id));
	}
	let offset = *offset;
	let mask = target.mask.as_ref().filter(|m| m.linked);
	// Phase 1: every new image, the document untouched.
	let (new_image, new_offset, new_mask, new_selection) = match &doc.selection {
		None => {
			let Some((content, at)) = tight(image, offset, store)? else {
				return Err(CommandError::NotAllowed("the layer is empty: there is nothing to transform".into()));
			};
			let (moved, moved_at) = resample_placed(ops, &content, at, mapping, filter, store)?;
			// The linked mask turns with its layer.
			let new_mask = match mask {
				Some(mask) => {
					let (image, at) = resample_placed(ops, &mask.image, offset, mapping, filter, store)?;
					Some(mask_to(image, at, moved_at, mask, store)?)
				}
				None => None,
			};
			(moved, moved_at, new_mask, None)
		}
		Some(selection) => {
			let placed = Placed { image, offset };
			let Some((x0, y0, x1, y1)) = selection.canvas_bounds(canvas) else {
				return Err(CommandError::NotAllowed("nothing is selected".into()));
			};
			// Lift the selected pixels (cut to the selection's bounds), leave a
			// hole, transform what was lifted and drop it back.
			let lifted = crate::pixels::extract(placed, Some(selection), canvas, store)?;
			let at = (x0 as i32, y0 as i32);
			let lifted = crate::pixels::place_in(&lifted, offset, at, Some((x1 - x0, y1 - y0)), fx_tiles::PixelValue::TRANSPARENT, store)?;
			let hole = crate::pixels::clear(placed, selection, canvas, store)?;
			let (moved, moved_at) = resample_placed(ops, &lifted, at, mapping, filter, store)?;
			let (merged, merged_at) = crate::pixels::over(
				Placed { image: &hole, offset },
				Placed {
					image: &moved,
					offset: moved_at,
				},
				store,
			)?;
			// The mask stays where it was: only the layer's origin moved.
			let new_mask = match mask {
				Some(mask) => Some(mask_to(mask.image.clone(), offset, merged_at, mask, store)?),
				None => None,
			};
			let (image, at) = resample_placed(ops, &selection.image, selection.offset, mapping, filter, store)?;
			(merged, merged_at, new_mask, Some(Selection { image, offset: at }))
		}
	};
	// Phase 2: commit.
	let target = doc.layer_mut(id).expect("resolved id exists");
	if let LayerKind::Pixel { image, offset } = &mut target.kind {
		*image = new_image;
		*offset = new_offset;
	}
	if let (Some(mask), Some(image)) = (target.mask.as_mut(), new_mask) {
		mask.image = image;
	}
	if let Some(selection) = new_selection {
		doc.selection = Some(selection);
	}
	Ok(CommandEffect {
		label: "Free Transform".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

/// Image ▸ Crop, the Crop tool and Trim (M6-T03). The four combinations the
/// Crop tool's option bar can ask for:
///
/// * no angle, no deletion — exactly Canvas Size; the content outside the
///   rectangle is kept (off-canvas) and returns if the canvas grows again.
/// * no angle, deletion — everything outside the rectangle is clipped away
///   first (every pixel layer, mask and the selection).
/// * an angle — the image is straightened: every pixel image is resampled
///   through a turn about the rectangle's centre, into the rectangle's own
///   frame, and the document becomes the rectangle. With `delete_cropped` the
///   straighten also clips what the turn brought into the frame from outside
///   the rectangle; without it those pixels stay, off-canvas.
fn crop(doc: &mut Document, rect: (i32, i32, u32, u32), angle_deg: f64, delete_cropped: bool, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if rect.2 == 0 || rect.3 == 0 {
		return Err(CommandError::InvalidValue {
			field: "rect",
			reason: "a canvas is at least 1 × 1 pixel".into(),
		});
	}
	if !angle_deg.is_finite() {
		return Err(CommandError::InvalidValue {
			field: "angle_deg",
			reason: format!("{angle_deg} is not a number"),
		});
	}
	let angle = angle_deg.rem_euclid(360.0);
	let mut pixels_changed = Vec::new();
	let mut props_changed = Vec::new();
	if angle != 0.0 {
		let ops = pixel_ops(ctx, "Crop")?;
		let (cx, cy) = (f64::from(rect.0) + f64::from(rect.2) / 2.0, f64::from(rect.1) + f64::from(rect.3) / 2.0);
		// Turn about the crop box's centre, then into the box's own frame: the
		// straightened images unfold from there, and the ones that end up in
		// the box are exactly the ones the user sees.
		let mapping = Mapping::rotation_about(angle.to_radians(), cx, cy).after_destination_translation(-f64::from(rect.0), -f64::from(rect.1));
		pixels_changed = resample_document(doc, ops, mapping, Filter::BicubicAutomatic, ctx.tiles)?;
		if delete_cropped {
			// The images are already in the box's frame: clip to the box.
			pixels_changed.extend(clip_document(doc, (0, 0, rect.2, rect.3), ctx.tiles)?);
		}
	} else {
		if delete_cropped {
			pixels_changed = clip_document(doc, rect, ctx.tiles)?;
		}
		let (moved, masks) = shift_offsets(doc, -rect.0, -rect.1, ctx.tiles)?;
		props_changed = moved;
		pixels_changed.extend(masks);
	}
	pixels_changed.sort_unstable();
	pixels_changed.dedup();
	props_changed.extend(set_canvas(doc, rect.2, rect.3));
	Ok(CommandEffect {
		label: "Crop".into(),
		pixels_changed,
		props_changed,
		..Default::default()
	})
}

/// Image ▸ Image Size (M6-T02).
fn image_size(
	doc: &mut Document,
	width: u32,
	height: u32,
	ppi: f32,
	resample: Option<Filter>,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if width == 0 || height == 0 {
		return Err(CommandError::InvalidValue {
			field: if width == 0 { "width" } else { "height" },
			reason: "an image is at least 1 × 1 pixel".into(),
		});
	}
	if !ppi.is_finite() || ppi <= 0.0 {
		return Err(CommandError::InvalidValue {
			field: "ppi",
			reason: format!("{ppi} is not a positive resolution"),
		});
	}
	let Some(filter) = resample else {
		// "Resample" off: the print size changes, the pixels do not.
		if (width, height) != (doc.width, doc.height) {
			return Err(CommandError::NotAllowed(
				"resampling is off: only the print resolution can change, so the pixel dimensions must stay".into(),
			));
		}
		doc.ppi = ppi;
		return Ok(CommandEffect {
			label: "Image Size".into(),
			..Default::default()
		});
	};
	let ops = pixel_ops(ctx, "Image Size")?;
	let mapping = Mapping::scale(f64::from(width) / f64::from(doc.width), f64::from(height) / f64::from(doc.height));
	let mut changed = resample_document(doc, ops, mapping, filter, ctx.tiles)?;
	changed.extend(set_canvas(doc, width, height));
	doc.ppi = ppi;
	Ok(CommandEffect {
		label: "Image Size".into(),
		pixels_changed: changed,
		..Default::default()
	})
}

/// Resample every pixel layer, mask and the selection through `mapping`, which
/// maps old canvas pixels to new ones (M6-T02's Image Size and arbitrary
/// rotation). Every image's size and offset come from its mapped bounding box,
/// so the layout scales with the canvas. Returns the layers that changed; the
/// caller sets the document's new size.
fn resample_document(doc: &mut Document, ops: &dyn PixelOps, mapping: Mapping, filter: Filter, store: &TileStore) -> Result<Vec<LayerId>, CommandError> {
	// Phase 1: every new image, while the document is still untouched.
	let mut images = Vec::new();
	let mut masks = Vec::new();
	for id in layer_ids(doc) {
		let Some(layer) = doc.layer(id) else { continue };
		let mut target = (0, 0);
		if let LayerKind::Pixel { image, offset } = &layer.kind {
			let (image, new_offset) = resample_placed(ops, image, *offset, mapping, filter, store)?;
			images.push((id, (image, new_offset)));
			if layer.mask.as_ref().is_some_and(|m| m.linked) {
				target = new_offset;
			}
		}
		if let Some(mask) = &layer.mask {
			// Resampled where it sits, then put where it must sit (see
			// `mask_origin`).
			let (image, at) = resample_placed(ops, &mask.image, mask_origin(layer), mapping, filter, store)?;
			masks.push((id, mask_to(image, at, target, mask, store)?));
		}
	}
	let selection = match &doc.selection {
		Some(selection) => Some(resample_placed(ops, &selection.image, selection.offset, mapping, filter, store)?),
		None => None,
	};
	let reselect = match &doc.reselect {
		Some(selection) => Some(resample_placed(ops, &selection.image, selection.offset, mapping, filter, store)?),
		None => None,
	};
	// Phase 2: commit.
	let mut changed = Vec::new();
	for (id, (image, offset)) in images {
		if let Some(layer) = doc.layer_mut(id)
			&& let LayerKind::Pixel { image: pixels, offset: at } = &mut layer.kind
		{
			*pixels = image;
			*at = offset;
		}
		changed.push(id);
	}
	for (id, image) in masks {
		if let Some(layer) = doc.layer_mut(id)
			&& let Some(mask) = &mut layer.mask
		{
			mask.image = image;
		}
		changed.push(id);
	}
	// Shape layers (M6-T06) go through the same mapping as a matrix instead of
	// as pixels: a straighten or a resize keeps them sharp.
	changed.extend(map_shape_layers(doc, mapping));
	if let Some((image, offset)) = selection {
		doc.selection = Some(Selection { image, offset });
	}
	if let Some((image, offset)) = reselect {
		doc.reselect = Some(Selection { image, offset });
	}
	changed.sort_unstable();
	changed.dedup();
	Ok(changed)
}

/// One placed image (`image` at `offset`) resampled through a canvas mapping:
/// the mapped bounding box becomes the image's new offset and size, and the
/// same mapping moved to that box maps its pixels (M6-T02; M6-T03's straighten
/// and M6-T04's Free Transform reuse it).
fn resample_placed(
	ops: &dyn PixelOps,
	image: &TiledImage,
	offset: (i32, i32),
	mapping: Mapping,
	filter: Filter,
	store: &TileStore,
) -> Result<(TiledImage, (i32, i32)), CommandError> {
	let placed = mapping.after_translation(f64::from(offset.0), f64::from(offset.1));
	let rect = [0.0, 0.0, f64::from(image.width()), f64::from(image.height())];
	let Some((new_offset, size)) = dest_rect(&placed, rect) else {
		return Err(CommandError::NotAllowed("this mapping cannot be resampled tile by tile".into()));
	};
	let local = placed.after_destination_translation(-f64::from(new_offset.0), -f64::from(new_offset.1));
	Ok((ops.resample(image, local, size, filter, store)?, new_offset))
}

fn select_all(doc: &mut Document) -> Result<CommandEffect, CommandError> {
	if doc.width == 0 || doc.height == 0 {
		return Err(CommandError::NotAllowed("the document is empty".into()));
	}
	doc.selection = Some(Selection::full((doc.width, doc.height), doc.color.depth));
	Ok(selection_effect("Select All"))
}

fn deselect(doc: &mut Document) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.take() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	doc.reselect = Some(selection);
	Ok(selection_effect("Deselect"))
}

fn reselect(doc: &mut Document) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.reselect.take() else {
		return Err(CommandError::NotAllowed("no deselected selection to restore".into()));
	};
	doc.selection = Some(selection);
	Ok(selection_effect("Reselect"))
}

fn invert_selection(doc: &mut Document, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	if doc.width == 0 || doc.height == 0 {
		return Err(CommandError::NotAllowed("the document is empty".into()));
	}
	let size = (doc.width, doc.height);
	let Some(source) = doc.selection.take() else {
		// Inverting nothing selects everything, like Photoshop.
		doc.selection = Some(Selection::full(size, doc.color.depth));
		return Ok(selection_effect("Inverse"));
	};
	doc.selection = selection::invert(size, &source, ctx.tiles)?;
	Ok(selection_effect("Inverse"))
}

fn modify_selection(doc: &mut Document, op: &SelectModify, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let size = (doc.width, doc.height);
	let depth = doc.color.depth;
	let Some(ops) = ctx.ops else {
		return Err(CommandError::NotAllowed("a selection change needs the engine's distance transform".into()));
	};
	let Some(current) = doc.selection.as_ref() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let modified = ops.modify_selection(current, op, size, depth, ctx.tiles)?;
	let label = match op {
		SelectModify::Expand(_) => "Expand Selection",
		SelectModify::Contract(_) => "Contract Selection",
		SelectModify::Border(_) => "Border Selection",
		SelectModify::Smooth(_) => "Smooth Selection",
		SelectModify::Feather(_) => "Feather Selection",
	};
	doc.selection = modified;
	Ok(selection_effect(label))
}

fn offset_selection(doc: &mut Document, dx: i32, dy: i32) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.as_mut() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let x = checked_offset(selection.offset.0, dx)?;
	let y = checked_offset(selection.offset.1, dy)?;
	selection.offset = (x, y);
	Ok(selection_effect("Move Selection"))
}

#[cfg(test)]
mod tests {
	use fx_tiles::{TILE_SIZE, TileStoreConfig};

	use super::*;
	use crate::color::{BitDepth, ColorProfile, DocumentColor};
	use crate::history::History;
	use crate::selection::{SelectMode, SelectModify, SelectionShape, gray_at, set_gray as set_coverage};

	/// Stands in for the engine's pixel operations: a composite is a solid
	/// image whose value counts the layers composited; a filter returns the
	/// image unchanged.
	struct FakeOps;

	impl PixelOps for FakeOps {
		fn filter(&self, image: &TiledImage, _: (i32, i32), _: (u32, u32), _: &FilterParams, _: &TileStore) -> Result<TiledImage, CommandError> {
			Ok(image.clone())
		}

		fn composite(&self, doc: &Document, layers: &[LayerId], background: Option<[u16; 4]>, _: &TileStore) -> Result<TiledImage, CommandError> {
			let mut image = TiledImage::new(doc.width, doc.height, doc.color.depth.rgba_format());
			let n = layers.len() as u16;
			let value = background.map_or([n, n, n, n], |_| [n, n, n, u16::MAX]);
			image.set_slot(0, 0, TileSlot::Solid(PixelValue(value)));
			Ok(image)
		}

		fn rotate(
			&self,
			image: &TiledImage,
			offset: (i32, i32),
			canvas: (u32, u32),
			op: Permutation,
			store: &TileStore,
		) -> Result<(TiledImage, (i32, i32)), CommandError> {
			// The same permutation arithmetic `fx_ops::permute` runs (that module
			// is tested on its own, and `fx-engine` tests the real one end to
			// end); this one works on the slots directly so the command tests can
			// see where content lands.
			let size = (image.width(), image.height());
			let dest = op.size(size);
			let format = image.format();
			let mut out = TiledImage::new(dest.0, dest.1, format);
			for ty in 0..out.grid(0).rows() {
				for tx in 0..out.grid(0).cols() {
					let mut buffer = TileBuffer::zeroed(format);
					let mut used = false;
					for y in 0..TILE_SIZE {
						for x in 0..TILE_SIZE {
							let (gx, gy) = (tx * TILE_SIZE + x, ty * TILE_SIZE + y);
							let Some(source) = op.source_pixel((gx, gy), size) else { continue };
							let px = read_slot(
								image.slot(0, source.0 / TILE_SIZE, source.1 / TILE_SIZE),
								format,
								store,
								source.0 % TILE_SIZE,
								source.1 % TILE_SIZE,
							);
							used = true;
							write_pixel(&mut buffer, format, x, y, px);
						}
					}
					if used {
						out.put_buffer(store, tx, ty, buffer);
					}
				}
			}
			let new_offset = op.offset(offset, size, canvas).ok_or(CommandError::InvalidValue {
				field: "offset",
				reason: "the rotated offset does not fit in 32 bits".into(),
			})?;
			Ok((out, new_offset))
		}

		fn resample(&self, image: &TiledImage, _: Mapping, size: (u32, u32), _: Filter, _: &TileStore) -> Result<TiledImage, CommandError> {
			// An empty image of the mapped size: these tests check the *geometry*
			// a command derives from `resample_placed` (which image size and
			// offset, the document's new size); the pixels are the sampler's job
			// (`fx-ops`, and the engine's end-to-end tests).
			Ok(TiledImage::new(size.0, size.1, image.format()))
		}

		fn convert(&self, image: &TiledImage, _: &crate::ops::Conversion<'_>, _: &TileStore) -> Result<TiledImage, CommandError> {
			Ok(image.clone())
		}

		fn convert_color(&self, rgba: [u16; 4], _: &crate::ops::Conversion<'_>) -> Result<[u16; 4], CommandError> {
			// "Converted": the red and blue channels swap, so the test can see it.
			Ok([rgba[2], rgba[1], rgba[0], rgba[3]])
		}

		fn rasterise(&self, shape: &SelectionShape, size: (u32, u32), depth: BitDepth, _: bool, store: &TileStore) -> Result<Selection, CommandError> {
			// Rectangles only: enough to drive the selection commands the tests
			// below exercise. The real rasteriser is tested in `fx-ops`.
			let (x0, y0, x1, y1) = match shape {
				SelectionShape::Rect { x, y, w, h } => (x.floor() as i64, y.floor() as i64, (x + w).ceil() as i64, (y + h).ceil() as i64),
				_ => return Ok(Selection::empty(size, depth)),
			};
			let format = depth.gray_format();
			let mut selection = Selection::empty(size, depth);
			let (tw, th) = (size.0.div_ceil(TILE_SIZE), size.1.div_ceil(TILE_SIZE));
			for ty in 0..th {
				for tx in 0..tw {
					let mut buffer = TileBuffer::zeroed(format);
					let mut used = false;
					for py in 0..TILE_SIZE {
						for px in 0..TILE_SIZE {
							let (x, y) = (i64::from(tx * TILE_SIZE + px), i64::from(ty * TILE_SIZE + py));
							if x >= x0 && x < x1 && y >= y0 && y < y1 {
								set_coverage(&mut buffer, format, px, py, 1.0);
								used = true;
							}
						}
					}
					if used {
						selection.image.put_buffer(store, tx, ty, buffer);
					}
				}
			}
			Ok(selection)
		}

		fn modify_selection(
			&self,
			selection: &Selection,
			_: &SelectModify,
			_: (u32, u32),
			_: BitDepth,
			_: &TileStore,
		) -> Result<Option<Selection>, CommandError> {
			// Identity: the commands only need to see their own plumbing here;
			// the reshaping algorithms are tested in `fx-ops`.
			Ok(Some(selection.clone()))
		}

		fn magic_wand(&self, doc: &Document, params: &WandParams, store: &TileStore) -> Result<Option<Selection>, CommandError> {
			// "Finds" a 10 × 10 square at the click; the flood is tested in `fx-ops`.
			let shape = SelectionShape::Rect {
				x: params.x.floor(),
				y: params.y.floor(),
				w: 10.0,
				h: 10.0,
			};
			self.rasterise(&shape, (doc.width, doc.height), doc.color.depth, true, store).map(Some)
		}
	}

	/// A document, the store its commands need and its history.
	///
	/// Layers are built *through the commands* (the only way to change a
	/// document, see the crate docs), so every test also exercises the command
	/// that built its fixture. Pixel content is written directly, the way an
	/// importer would.
	struct Fixture {
		doc: Document,
		store: TileStore,
		history: History,
	}

	impl Fixture {
		fn new() -> Self {
			Self {
				doc: Document::new(
					400,
					300,
					DocumentColor {
						depth: BitDepth::U16,
						profile: ColorProfile::Srgb,
					},
					72.0,
				),
				store: TileStore::new(TileStoreConfig::for_tests(std::env::temp_dir())).expect("test store"),
				history: History::default(),
			}
		}

		fn run(&mut self, command: Command) -> Result<CommandEffect, CommandError> {
			let mut ctx = CommandContext { tiles: &self.store, ops: None };
			self.history.execute(&mut self.doc, command, &mut ctx)
		}

		fn ok(&mut self, command: Command) -> CommandEffect {
			self.run(command.clone()).unwrap_or_else(|error| panic!("{command:?} failed: {error}"))
		}

		fn fail(&mut self, command: Command) -> CommandError {
			match self.run(command.clone()) {
				Err(error) => error,
				Ok(effect) => panic!("{command:?} unexpectedly succeeded: {effect:?}"),
			}
		}

		/// Run `command` with a fake [`PixelOps`] (the real one is the engine's).
		fn run_with_ops(&mut self, command: Command) -> Result<CommandEffect, CommandError> {
			let ops = FakeOps;
			let mut ctx = CommandContext {
				tiles: &self.store,
				ops: Some(&ops),
			};
			self.history.execute(&mut self.doc, command, &mut ctx)
		}

		fn ok_with_ops(&mut self, command: Command) -> CommandEffect {
			self.run_with_ops(command.clone()).unwrap_or_else(|error| panic!("{command:?} failed: {error}"))
		}

		fn fail_with_ops(&mut self, command: Command) -> CommandError {
			match self.run_with_ops(command.clone()) {
				Err(error) => error,
				Ok(effect) => panic!("{command:?} unexpectedly succeeded: {effect:?}"),
			}
		}

		/// Add a layer and return its id (`AddLayer` selects it).
		fn add(&mut self, new: NewLayer, name: &str) -> LayerId {
			self.ok(Command::AddLayer {
				layer: new,
				name: Some(name.into()),
			});
			self.doc.active_layer().expect("AddLayer selects the new layer")
		}

		fn add_pixel(&mut self, name: &str) -> LayerId {
			self.add(NewLayer::Pixel, name)
		}

		/// Add a layer with the document's own default name, which is what a tool
		/// does (the Shape tool names the layer after the shape it drew).
		fn add_default(&mut self, new: NewLayer) -> LayerId {
			self.ok(Command::AddLayer { layer: new, name: None });
			self.doc.active_layer().expect("AddLayer selects the new layer")
		}

		/// A shape layer as the Shape tool adds it (M6-T06): `shape` in local
		/// space, placed by `transform`, filled with one solid colour.
		fn add_shape(&mut self, shape: VectorShape, transform: [f64; 6]) -> LayerId {
			self.add_default(NewLayer::Shape {
				shape,
				fill: Some(Paint::Solid { rgba: [40_000, 0, 0, 65_535] }),
				stroke: None,
				transform,
			})
		}

		/// A shape layer's cache.
		fn cache(&self, id: LayerId) -> &TiledImage {
			match self.kind(id) {
				LayerKind::Shape { cache, .. } => cache,
				other => panic!("not a shape layer: {other:?}"),
			}
		}

		/// A shape layer's local → document matrix.
		fn matrix(&self, id: LayerId) -> [f64; 6] {
			match self.kind(id) {
				LayerKind::Shape { transform, .. } => *transform,
				other => panic!("not a shape layer: {other:?}"),
			}
		}

		/// The dirty tiles of a shape layer's level 0.
		fn dirty_shape_tiles(&self, id: LayerId) -> Vec<(u32, u32)> {
			self.cache(id).dirty_tiles(0).collect()
		}

		/// Pretend every tile of a shape layer's cache has been drawn: what is
		/// dirty after the next command is only what that command invalidated.
		fn clear_shape_cache(&mut self, id: LayerId) {
			let Some(layer) = self.doc.layer_mut(id) else { return };
			let LayerKind::Shape { cache, .. } = &mut layer.kind else { return };
			for level in 0..cache.level_count() {
				for ty in 0..cache.grid(level).rows() {
					for tx in 0..cache.grid(level).cols() {
						cache.set_derived_slot(level, tx, ty, TileSlot::Empty);
					}
				}
			}
		}

		fn group(&mut self, name: &str) -> LayerId {
			self.add(NewLayer::Group, name)
		}

		fn select(&mut self, ids: &[LayerId]) {
			self.ok(Command::SelectLayers {
				layers: ids.iter().copied().map(LayerRef::Id).collect(),
			});
		}

		/// Order the Layers panel draws: top → bottom, children under their group.
		fn panel(&self) -> Vec<LayerId> {
			self.doc.panel_order()
		}

		/// Depth-first stack order, bottom → top (nesting flattened).
		fn order(&self) -> Vec<LayerId> {
			let mut ids = Vec::new();
			self.doc.walk(|layer, _| ids.push(layer.id));
			ids
		}

		/// Panel order as names, which reads better in assertions.
		fn names(&self) -> Vec<&str> {
			self.panel().into_iter().map(|id| self.name(id)).collect()
		}

		fn children(&self, id: LayerId) -> &[Arc<Layer>] {
			self.layer(id).children().unwrap_or_default()
		}

		fn child_ids(&self, id: LayerId) -> Vec<LayerId> {
			self.children(id).iter().map(|layer| layer.id).collect()
		}

		fn layer(&self, id: LayerId) -> &Layer {
			self.doc.layer(id).expect("layer exists")
		}

		fn name(&self, id: LayerId) -> &str {
			&self.layer(id).name
		}

		fn kind(&self, id: LayerId) -> &LayerKind {
			&self.layer(id).kind
		}

		fn pixel(&self, id: LayerId) -> (&TiledImage, (i32, i32)) {
			match self.kind(id) {
				LayerKind::Pixel { image, offset } => (image, *offset),
				other => panic!("not a pixel layer: {other:?}"),
			}
		}

		fn slot(&self, id: LayerId, tx: u32, ty: u32) -> &TileSlot {
			self.pixel(id).0.slot(0, tx, ty)
		}

		fn mask_slot(&self, id: LayerId, tx: u32, ty: u32) -> &TileSlot {
			self.mask(id).image.slot(0, tx, ty)
		}

		fn mask(&self, id: LayerId) -> &Mask {
			self.layer(id).mask.as_ref().expect("layer has a mask")
		}

		/// One pixel of a pixel layer, as stored (16-bit scale).
		fn read_pixel(&self, id: LayerId, x: u32, y: u32) -> [u16; 4] {
			match self.slot(id, x / TILE_SIZE, y / TILE_SIZE) {
				TileSlot::Empty => [0; 4],
				TileSlot::Solid(value) => value.0,
				TileSlot::Data(handle) => {
					let buffer = self.store.get(handle).expect("tile is readable");
					pixel_at(&buffer, x % TILE_SIZE, y % TILE_SIZE)
				}
			}
		}

		/// Real pixel content in a tile, the way an importer writes it.
		fn paint(&mut self, id: LayerId, pixels: &[(u32, u32, [u16; 4])]) {
			let store = &self.store;
			let image = match &mut self.doc.layer_mut(id).expect("layer exists").kind {
				LayerKind::Pixel { image, .. } => image,
				other => panic!("not a pixel layer: {other:?}"),
			};
			let mut buffer = TileBuffer::zeroed(image.format());
			for &(x, y, value) in pixels {
				set_pixel(&mut buffer, x, y, value);
			}
			image.put_buffer(store, pixels[0].0 / TILE_SIZE, pixels[0].1 / TILE_SIZE, buffer);
		}

		/// Real mask content: a stored tile where `pixels` say, or a uniform
		/// `Solid` tile when the whole tile has the same value.
		fn paint_mask(&mut self, id: LayerId, tile: (u32, u32), pixels: &[(u32, u32, u16)]) {
			let store = &self.store;
			let image = &mut self.doc.layer_mut(id).expect("layer exists").mask.as_mut().expect("layer has a mask").image;
			let mut buffer = TileBuffer::zeroed(image.format());
			for &(x, y, gray) in pixels {
				set_gray(&mut buffer, x, y, gray);
			}
			image.put_buffer(store, tile.0, tile.1, buffer);
		}

		/// A whole mask tile of one value, kept as `Solid` (no tile memory).
		fn fill_mask(&mut self, id: LayerId, tile: (u32, u32), gray: u16) {
			let format = self.mask(id).image.format();
			let store = &self.store;
			let image = &mut self.doc.layer_mut(id).expect("layer exists").mask.as_mut().expect("layer has a mask").image;
			image.put_buffer(store, tile.0, tile.1, TileBuffer::filled(format, PixelValue::gray16(gray)));
		}

		/// Give a pixel layer an image of its own size (the fixture creates
		/// document-sized ones), the way an importer of a small image would.
		fn resize_image(&mut self, id: LayerId, width: u32, height: u32) {
			let format = self.pixel(id).0.format();
			if let LayerKind::Pixel { image, .. } = &mut self.doc.layer_mut(id).expect("layer exists").kind {
				*image = TiledImage::new(width, height, format);
			}
		}

		fn size(&self) -> (u32, u32) {
			(self.doc.width, self.doc.height)
		}

		fn selection(&self) -> &Selection {
			self.doc.selection.as_ref().expect("a selection is set")
		}

		/// One command with undo/redo assertions: after `undo` the document must
		/// be byte-identical to before, after `redo` to after.
		fn round_trip(&mut self, command: Command) {
			let before = format!("{:?}", self.doc);
			let effect = self.ok(command.clone());
			let after = format!("{:?}", self.doc);
			if effect.selection_only {
				return;
			}
			assert!(self.history.undo(&mut self.doc), "{command:?} is undoable");
			assert_eq!(format!("{:?}", self.doc), before, "undo restores the document after {command:?}");
			assert!(self.history.redo(&mut self.doc), "{command:?} is redoable");
			assert_eq!(format!("{:?}", self.doc), after, "redo re-applies {command:?}");
		}
	}

	/// Write one `Rgba16`/`Gray16` sample (native endian, like `fx-tiles`).
	fn set_pixel(buffer: &mut TileBuffer, x: u32, y: u32, value: [u16; 4]) {
		let at = ((y * TILE_SIZE + x) * 8) as usize;
		for (channel, sample) in value.iter().enumerate() {
			buffer.bytes_mut()[at + channel * 2..at + channel * 2 + 2].copy_from_slice(&sample.to_ne_bytes());
		}
	}

	fn set_gray(buffer: &mut TileBuffer, x: u32, y: u32, gray: u16) {
		let at = ((y * TILE_SIZE + x) * 2) as usize;
		buffer.bytes_mut()[at..at + 2].copy_from_slice(&gray.to_ne_bytes());
	}

	fn pixel_at(buffer: &TileBuffer, x: u32, y: u32) -> [u16; 4] {
		let at = ((y * TILE_SIZE + x) * 4) as usize;
		let samples = buffer.as_u16();
		[samples[at], samples[at + 1], samples[at + 2], samples[at + 3]]
	}

	/// One stored pixel of a slot, straight 16-bit (gray: channel 0).
	fn read_slot(slot: &TileSlot, format: PixelFormat, store: &TileStore, x: u32, y: u32) -> [u16; 4] {
		match slot {
			TileSlot::Empty => [0; 4],
			TileSlot::Solid(value) => value.0,
			TileSlot::Data(handle) => match store.get(handle) {
				Err(_) => [0; 4],
				Ok(buffer) => match format {
					PixelFormat::Rgba16 => pixel_at(&buffer, x, y),
					PixelFormat::Gray16 => [buffer.as_u16()[(y * TILE_SIZE + x) as usize], 0, 0, 0],
					_ => [0; 4],
				},
			},
		}
	}

	/// Write one pixel of a tile in the fixture formats (see [`read_slot`]).
	fn write_pixel(buffer: &mut TileBuffer, format: PixelFormat, x: u32, y: u32, px: [u16; 4]) {
		match format {
			PixelFormat::Rgba16 => set_pixel(buffer, x, y, px),
			PixelFormat::Gray16 => {
				let at = ((y * TILE_SIZE + x) * 2) as usize;
				buffer.bytes_mut()[at..at + 2].copy_from_slice(&px[0].to_ne_bytes());
			}
			_ => {}
		}
	}

	/// Tiles of a level-0 grid that hold real pixels (as opposed to empty or
	/// single-colour tiles, which cost nothing).
	fn data_tiles(image: &TiledImage) -> usize {
		let grid = image.grid(0);
		(0..grid.rows())
			.flat_map(|ty| (0..grid.cols()).map(move |tx| (tx, ty)))
			.filter(|&(tx, ty)| matches!(image.slot(0, tx, ty), TileSlot::Data(_)))
			.count()
	}

	const RED: [u16; 4] = [65535, 0, 0, 65535];

	#[test]
	fn json_shape_is_stable() {
		let cmd = Command::SetLayerProps {
			layer: LayerRef::Active,
			props: LayerPropsPatch {
				opacity: Some(0.5),
				blend: Some(BlendMode::Multiply),
				..Default::default()
			},
		};
		let json = serde_json::to_string(&cmd).unwrap();
		assert_eq!(json, r#"{"op":"set_layer_props","layer":"active","props":{"opacity":0.5,"blend":"multiply"}}"#);
		assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
	}

	/// A curves adjustment, for `NewLayer::Adjustment` and `SetAdjustment`.
	fn curves() -> Adjustment {
		Adjustment::Curves {
			channels: [vec![(0.0, 0.0), (1.0, 1.0)], Vec::new(), Vec::new(), Vec::new()],
		}
	}

	fn curves_with(last_y: f32) -> Adjustment {
		Adjustment::Curves {
			channels: [vec![(0.0, 0.0), (1.0, last_y)], Vec::new(), Vec::new(), Vec::new()],
		}
	}

	// -------------------------------------------------------------------
	// SelectLayers
	// -------------------------------------------------------------------

	#[test]
	fn select_layers_keeps_the_order_and_is_not_a_history_step() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let steps = f.history.labels().count();
		let effect = f.ok(Command::SelectLayers {
			layers: vec![LayerRef::Id(b), LayerRef::Id(a), LayerRef::Id(b)],
		});
		assert!(effect.selection_only);
		assert_eq!(f.doc.selected, vec![b, a], "order kept, duplicates dropped");
		assert_eq!(f.history.labels().count(), steps, "a selection change is not a history step");
	}

	#[test]
	fn select_layers_can_clear_the_selection() {
		let mut f = Fixture::new();
		f.add_pixel("A");
		f.select(&[]);
		assert!(f.doc.selected.is_empty());
		assert!(f.doc.active_layer().is_none());
	}

	#[test]
	fn select_layers_rejects_an_unknown_layer_and_keeps_the_selection() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.select(&[a]);
		let error = f.fail(Command::SelectLayers {
			layers: vec![LayerRef::Id(b), LayerRef::Id(LayerId(99))],
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
		assert_eq!(f.doc.selected, vec![a], "nothing changed");
	}

	// -------------------------------------------------------------------
	// AddLayer
	// -------------------------------------------------------------------

	#[test]
	fn add_layer_goes_directly_above_the_active_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.select(&[a]);
		let c = f.add_pixel("C");
		assert_eq!(f.order(), [a, c, b]);
		assert_eq!(f.doc.selected, vec![c], "the new layer is the only selected one");
	}

	#[test]
	fn add_layer_without_a_selection_goes_on_top() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.select(&[]);
		let c = f.add_pixel("C");
		assert_eq!(f.order(), [a, b, c]);
	}

	#[test]
	fn add_layer_above_a_selected_group_stays_at_its_level() {
		// M2-T01: "above the active layer, inside its group if the active layer
		// is in a group". A root-level group is not inside another group, so the
		// new layer goes above it (see the report).
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let g = f.group("G");
		f.select(&[g]);
		let n = f.add_pixel("New");
		assert_eq!(f.order(), [a, g, n]);
	}

	#[test]
	fn add_layer_goes_inside_the_group_the_active_layer_lives_in() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let g = f.doc.active_layer().expect("grouping selects the group");
		f.select(&[b]);
		let c = f.add_pixel("C");
		f.select(&[c]);
		let d = f.add_pixel("D");
		assert_eq!(f.panel(), [g, d, c, b, a], "D and C are children of G, above B");
		assert_eq!(f.child_ids(g), [b, c, d], "bottom → top");
	}

	#[test]
	fn add_layer_names_layers_like_photoshop_per_kind_and_per_document() {
		let mut f = Fixture::new();
		for new in [
			NewLayer::Pixel,
			NewLayer::Pixel,
			NewLayer::Group,
			NewLayer::SolidFill { rgba: [0; 4] },
			NewLayer::Adjustment(curves()),
			NewLayer::Adjustment(Adjustment::Invert),
		] {
			f.ok(Command::AddLayer { layer: new, name: None });
		}
		assert_eq!(
			f.names(),
			["Invert 1", "Curves 1", "Color Fill 1", "Group 1", "Layer 2", "Layer 1"],
			"top → bottom, one counter per kind"
		);
		// A second document starts its own counters.
		let mut other = Fixture::new();
		other.ok(Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		});
		assert_eq!(other.names(), ["Layer 1"]);
	}

	#[test]
	fn add_layer_keeps_counting_when_the_caller_names_the_layer() {
		let mut f = Fixture::new();
		f.add(NewLayer::Pixel, "Sky");
		f.ok(Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		});
		assert_eq!(f.name(f.doc.active_layer().expect("active")), "Layer 2");
	}

	#[test]
	fn add_layer_rejects_an_empty_name() {
		let mut f = Fixture::new();
		let error = f.fail(Command::AddLayer {
			layer: NewLayer::Pixel,
			name: Some("   ".into()),
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "name", .. }), "{error:?}");
		assert!(f.doc.layers.is_empty(), "nothing was added");
	}

	#[test]
	fn add_layer_creates_a_transparent_pixel_layer_of_the_document_size() {
		let mut f = Fixture::new();
		let effect = f.ok(Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		});
		assert_eq!(effect.label, "New Layer");
		assert!(effect.structure_changed && effect.props_changed.is_empty() && effect.pixels_changed.is_empty());
		let id = f.doc.active_layer().expect("active");
		let (image, offset) = f.pixel(id);
		assert_eq!((image.width(), image.height(), image.format()), (400, 300, PixelFormat::Rgba16));
		assert_eq!(offset, (0, 0));
		assert_eq!(data_tiles(image), 0, "an empty layer costs no tile memory");
		assert_eq!(f.read_pixel(id, 10, 10), [0; 4]);
	}

	#[test]
	fn add_layer_maps_every_new_layer_kind_through() {
		let mut f = Fixture::new();
		let effect = f.ok(Command::AddLayer {
			layer: NewLayer::SolidFill { rgba: [1, 2, 3, 4] },
			name: None,
		});
		assert_eq!(effect.label, "New Fill Layer");
		let fill = f.doc.active_layer().expect("active");
		assert!(matches!(f.kind(fill), LayerKind::SolidFill { rgba: [1, 2, 3, 4] }));

		let effect = f.ok(Command::AddLayer {
			layer: NewLayer::Group,
			name: None,
		});
		assert_eq!(effect.label, "New Group");
		let group = f.doc.active_layer().expect("active");
		assert!(matches!(f.kind(group), LayerKind::Group { children, expanded: true } if children.is_empty()));
		assert_eq!(f.layer(group).blend, BlendMode::PassThrough, "a new group passes through");

		let effect = f.ok(Command::AddLayer {
			layer: NewLayer::Adjustment(curves()),
			name: None,
		});
		assert_eq!(effect.label, "New Curves Adjustment Layer");
		let curves = f.doc.active_layer().expect("active");
		assert!(matches!(f.kind(curves), LayerKind::Adjustment(Adjustment::Curves { .. })));
	}

	// -------------------------------------------------------------------
	// DeleteLayers
	// -------------------------------------------------------------------

	#[test]
	fn delete_layers_selects_the_row_below() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let c = f.add_pixel("C");
		assert_eq!(f.panel(), [c, b, a]);
		let effect = f.ok(Command::DeleteLayers { layers: vec![LayerRef::Id(b)] });
		assert_eq!(effect.label, "Delete Layer");
		assert!(effect.structure_changed && effect.pixels_changed.is_empty());
		assert_eq!(f.panel(), [c, a]);
		assert_eq!(f.doc.selected, vec![a], "the row below the deleted one");
	}

	#[test]
	fn delete_layers_selects_the_row_above_when_the_bottom_most_goes() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::DeleteLayers { layers: vec![LayerRef::Id(a)] });
		assert_eq!(f.panel(), [b]);
		assert_eq!(f.doc.selected, vec![b]);
	}

	#[test]
	fn delete_layers_removes_several_rows_and_selects_the_row_below_the_lowest() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let c = f.add_pixel("C");
		let d = f.add_pixel("D");
		// panel: D, C, B, A
		let effect = f.ok(Command::DeleteLayers {
			layers: vec![LayerRef::Id(c), LayerRef::Id(b)],
		});
		assert_eq!(effect.label, "Delete Layers");
		assert_eq!(f.panel(), [d, a]);
		assert_eq!(f.doc.selected, vec![a], "the row below the lowest deleted one");
	}

	#[test]
	fn delete_layers_takes_a_groups_children_with_it() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let g = f.doc.active_layer().expect("active");
		f.select(&[b]);
		let c = f.add_pixel("C");
		assert_eq!(f.panel(), [g, c, b, a]);
		f.ok(Command::DeleteLayers { layers: vec![LayerRef::Id(g)] });
		assert_eq!(f.order(), [a], "the children went with the group");
		assert!(f.doc.layer(b).is_none() && f.doc.layer(c).is_none());
		assert_eq!(f.doc.selected, vec![a], "the row below the whole subtree");
	}

	#[test]
	fn delete_layers_ignores_a_descendant_of_a_deleted_group() {
		let mut f = Fixture::new();
		f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let g = f.doc.active_layer().expect("active");
		f.ok(Command::DeleteLayers {
			layers: vec![LayerRef::Id(g), LayerRef::Id(b)],
		});
		assert!(f.doc.layer(g).is_none() && f.doc.layer(b).is_none());
		assert_eq!(f.order().len(), 1);
	}

	#[test]
	fn delete_layers_can_empty_the_document() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::DeleteLayers {
			layers: vec![LayerRef::Id(a), LayerRef::Id(b)],
		});
		assert!(f.doc.layers.is_empty());
		assert!(f.doc.selected.is_empty());
	}

	#[test]
	fn delete_layers_rejects_an_empty_list_or_an_unknown_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let error = f.fail(Command::DeleteLayers { layers: Vec::new() });
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		let error = f.fail(Command::DeleteLayers {
			layers: vec![LayerRef::Id(a), LayerRef::Id(LayerId(99))],
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
		assert_eq!(f.order(), [a], "nothing was deleted");
	}

	// -------------------------------------------------------------------
	// DuplicateLayers
	// -------------------------------------------------------------------

	#[test]
	fn duplicate_layers_sits_above_the_original_with_a_copy_suffix() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let effect = f.ok(Command::DuplicateLayers { layers: vec![LayerRef::Id(a)] });
		assert_eq!(effect.label, "Duplicate Layer");
		assert!(effect.structure_changed && effect.pixels_changed.is_empty());
		let copy = f.doc.active_layer().expect("active");
		assert_ne!(copy, a);
		assert_eq!(f.name(copy), "A copy");
		assert_eq!(f.panel(), [b, copy, a], "the copy sits directly above its original");
		assert_eq!(f.doc.selected, vec![copy]);
	}

	#[test]
	fn duplicate_layers_share_tile_handles() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(3, 4, RED)]);
		let TileSlot::Data(before) = f.slot(a, 0, 0) else {
			panic!("the original has a stored tile")
		};
		let before = before.clone();
		f.ok(Command::DuplicateLayers { layers: vec![LayerRef::Id(a)] });
		let copy = f.doc.active_layer().expect("active");
		let TileSlot::Data(after) = f.slot(copy, 0, 0) else {
			panic!("the copy reads the same tile")
		};
		assert!(before.same_tile(after), "no pixel copy");
		assert_eq!(f.read_pixel(copy, 3, 4), RED);
	}

	#[test]
	fn duplicate_layers_deep_copies_a_group_with_fresh_ids() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let g = f.doc.active_layer().expect("active");
		f.ok(Command::DuplicateLayers { layers: vec![LayerRef::Id(g)] });
		let copy = f.doc.active_layer().expect("active");
		assert_ne!(copy, g);
		assert_eq!(f.name(copy), "G copy");
		assert_eq!(f.children(copy).len(), 1);
		let copy_child = f.children(copy)[0].id;
		assert_ne!(copy_child, b, "children get fresh ids");
		assert_eq!(f.name(copy_child), "B", "children keep their names, like Photoshop");
		assert_eq!(f.panel(), [copy, copy_child, g, b, a]);
	}

	#[test]
	fn duplicate_layers_rejects_an_empty_list_or_an_unknown_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let error = f.fail(Command::DuplicateLayers { layers: Vec::new() });
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		let error = f.fail(Command::DuplicateLayers {
			layers: vec![LayerRef::Id(LayerId(99))],
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
		assert_eq!(f.order(), [a]);
	}

	// -------------------------------------------------------------------
	// MoveLayer
	// -------------------------------------------------------------------

	#[test]
	fn move_layer_reorders_inside_a_list() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let c = f.add_pixel("C");
		let effect = f.ok(Command::MoveLayer {
			layer: LayerRef::Id(a),
			parent: None,
			index: 2,
		});
		assert_eq!(effect.label, "Move Layer");
		assert!(effect.structure_changed);
		assert_eq!(f.order(), [b, c, a], "A moved to the top");
		f.ok(Command::MoveLayer {
			layer: LayerRef::Id(a),
			parent: None,
			index: 0,
		});
		assert_eq!(f.order(), [a, b, c], "and back to the bottom");
		// An index at its own slot is a no-op, not a shift.
		f.ok(Command::MoveLayer {
			layer: LayerRef::Id(b),
			parent: None,
			index: 1,
		});
		assert_eq!(f.order(), [a, b, c]);
	}

	#[test]
	fn move_layer_moves_into_a_group() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let g = f.group("G");
		f.ok(Command::MoveLayer {
			layer: LayerRef::Id(a),
			parent: Some(LayerRef::Id(g)),
			index: 0,
		});
		assert_eq!(f.order(), [b, g, a]);
		assert_eq!(f.panel(), [g, a, b], "A is the group's only child");
	}

	#[test]
	fn move_layer_rejects_a_parent_that_is_not_a_group() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let error = f.fail(Command::MoveLayer {
			layer: LayerRef::Id(a),
			parent: Some(LayerRef::Id(b)),
			index: 0,
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "parent", .. }), "{error:?}");
		assert_eq!(f.order(), [a, b]);
	}

	#[test]
	fn move_layer_rejects_itself_and_its_own_descendants() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("Outer".into()),
		});
		let outer = f.doc.active_layer().expect("active");
		f.select(&[b]);
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("Inner".into()),
		});
		let inner = f.doc.active_layer().expect("active");
		assert_eq!(f.panel(), [outer, inner, b, a]);
		for parent in [outer, inner] {
			let error = f.fail(Command::MoveLayer {
				layer: LayerRef::Id(outer),
				parent: Some(LayerRef::Id(parent)),
				index: 0,
			});
			assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		}
		// A parent that is not a group at all is rejected first.
		let error = f.fail(Command::MoveLayer {
			layer: LayerRef::Id(outer),
			parent: Some(LayerRef::Id(b)),
			index: 0,
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "parent", .. }), "{error:?}");
		assert_eq!(f.order(), [a, outer, inner, b]);
	}

	#[test]
	fn move_layer_rejects_an_index_past_the_end_and_an_unknown_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let error = f.fail(Command::MoveLayer {
			layer: LayerRef::Id(a),
			parent: None,
			index: 2,
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "index", .. }), "{error:?}");
		let error = f.fail(Command::MoveLayer {
			layer: LayerRef::Id(LayerId(99)),
			parent: None,
			index: 0,
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
		assert_eq!(f.order(), [a, b], "nothing moved");
	}

	// -------------------------------------------------------------------
	// GroupLayers
	// -------------------------------------------------------------------

	#[test]
	fn group_layers_wraps_the_selection_at_the_topmost_position() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let c = f.add_pixel("C");
		let d = f.add_pixel("D");
		// bottom → top: A, B, C, D
		let effect = f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b), LayerRef::Id(c)],
			name: None,
		});
		assert_eq!(effect.label, "Group Layers");
		assert!(effect.structure_changed);
		let g = f.doc.active_layer().expect("active");
		assert_eq!(f.name(g), "Group 1");
		assert_eq!(f.panel(), [d, g, c, b, a], "the group takes C's place, the topmost selected");
		assert_eq!(f.child_ids(g), [b, c], "children keep their bottom → top order");
		assert_eq!(f.doc.selected, vec![g]);
	}

	#[test]
	fn group_layers_pulls_layers_out_of_other_groups_in_panel_order() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let g = f.doc.active_layer().expect("active");
		let c = f.add_pixel("C");
		// panel: C, G, B, A → group {C, A}, the topmost selected was C.
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(a), LayerRef::Id(c)],
			name: Some("Wrapped".into()),
		});
		let wrapped = f.doc.active_layer().expect("active");
		assert_eq!(f.panel(), [wrapped, c, a, g, b]);
		assert_eq!(f.child_ids(wrapped), [a, c], "bottom → top");
		assert_eq!(f.child_ids(g), [b], "B was not part of the selection");
	}

	#[test]
	fn group_layers_inside_a_group_creates_a_subgroup() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let g = f.doc.active_layer().expect("active");
		f.select(&[b]);
		let c = f.add_pixel("C");
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b), LayerRef::Id(c)],
			name: None,
		});
		let inner = f.doc.active_layer().expect("active");
		assert_eq!(f.panel(), [g, inner, c, b, a]);
		assert_eq!(f.child_ids(g), [inner], "the subgroup is G's only child");
	}

	#[test]
	fn group_layers_rejects_an_empty_selection_an_empty_name_and_an_unknown_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let error = f.fail(Command::GroupLayers {
			layers: Vec::new(),
			name: None,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		let error = f.fail(Command::GroupLayers {
			layers: vec![LayerRef::Id(a)],
			name: Some(String::new()),
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "name", .. }), "{error:?}");
		let error = f.fail(Command::GroupLayers {
			layers: vec![LayerRef::Id(LayerId(99))],
			name: None,
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
		assert_eq!(f.order(), [a], "nothing was grouped");
	}

	// -------------------------------------------------------------------
	// SetLayerProps
	// -------------------------------------------------------------------

	#[test]
	fn set_layer_props_applies_every_field() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let effect = f.ok(Command::SetLayerProps {
			layer: LayerRef::Id(a),
			props: LayerPropsPatch {
				name: Some("Sky".into()),
				visible: Some(false),
				opacity: Some(0.5),
				fill: Some(0.25),
				blend: Some(BlendMode::Multiply),
				clipped: Some(false),
				locked_pixels: Some(true),
				locked_transparency: Some(true),
				locked_position: Some(true),
			},
		});
		assert_eq!(effect.label, "Layer Properties");
		assert_eq!(effect.props_changed, vec![a]);
		let layer = f.layer(a);
		assert_eq!(layer.name, "Sky");
		assert!(!layer.visible);
		assert_eq!((layer.opacity, layer.fill), (0.5, 0.25));
		assert_eq!(layer.blend, BlendMode::Multiply);
		assert!(!layer.clipped);
		assert!(layer.locked_pixels && layer.locked_position && layer.locked_transparency);
	}

	#[test]
	fn set_layer_props_rejects_out_of_range_values_before_writing_anything() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		for (opacity, fill) in [(Some(1.5), None), (None, Some(-0.1)), (Some(f32::NAN), None)] {
			let error = f.fail(Command::SetLayerProps {
				layer: LayerRef::Id(a),
				props: LayerPropsPatch {
					name: Some("Sky".into()),
					opacity,
					fill,
					..Default::default()
				},
			});
			assert!(matches!(error, CommandError::InvalidValue { .. }), "{error:?}");
			assert_eq!(f.name(a), "A", "the valid field was not applied either");
		}
	}

	#[test]
	fn set_layer_props_rejects_an_empty_patch_or_an_empty_name() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let error = f.fail(Command::SetLayerProps {
			layer: LayerRef::Id(a),
			props: LayerPropsPatch::default(),
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "props", .. }), "{error:?}");
		let error = f.fail(Command::SetLayerProps {
			layer: LayerRef::Id(a),
			props: LayerPropsPatch {
				name: Some(" \t ".into()),
				..Default::default()
			},
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "name", .. }), "{error:?}");
		assert_eq!(f.name(a), "A");
	}

	#[test]
	fn set_layer_props_allows_pass_through_on_a_group_only() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let g = f.group("G");
		let error = f.fail(Command::SetLayerProps {
			layer: LayerRef::Id(a),
			props: LayerPropsPatch {
				blend: Some(BlendMode::PassThrough),
				..Default::default()
			},
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "blend", .. }), "{error:?}");
		f.ok(Command::SetLayerProps {
			layer: LayerRef::Id(g),
			props: LayerPropsPatch {
				blend: Some(BlendMode::PassThrough),
				..Default::default()
			},
		});
		assert_eq!(f.layer(g).blend, BlendMode::PassThrough);
	}

	#[test]
	fn set_layer_props_rejects_clipping_a_layer_with_nothing_below_it() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		let clip = |id: LayerId| Command::SetLayerProps {
			layer: LayerRef::Id(id),
			props: LayerPropsPatch {
				clipped: Some(true),
				..Default::default()
			},
		};
		let error = f.fail(clip(a));
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		f.ok(clip(b));
		assert!(f.layer(b).clipped && !f.layer(a).clipped);
		// The bottom layer of a group has nothing below it either.
		f.ok(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: Some("G".into()),
		});
		let error = f.fail(clip(b));
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
	}

	#[test]
	fn set_layer_props_rejects_an_unknown_layer() {
		let mut f = Fixture::new();
		let error = f.fail(Command::SetLayerProps {
			layer: LayerRef::Id(LayerId(99)),
			props: LayerPropsPatch {
				visible: Some(false),
				..Default::default()
			},
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
	}

	// -------------------------------------------------------------------
	// OffsetLayer
	// -------------------------------------------------------------------

	#[test]
	fn offset_layer_moves_the_layer_without_touching_tiles() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(3, 4, RED)]);
		let TileSlot::Data(before) = f.slot(a, 0, 0) else {
			panic!("the layer has a stored tile")
		};
		let before = before.clone();
		let effect = f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 12,
			dy: -7,
		});
		assert_eq!(effect.label, "Offset");
		assert_eq!(effect.props_changed, vec![a]);
		assert_eq!(f.pixel(a).1, (12, -7));
		let TileSlot::Data(after) = f.slot(a, 0, 0) else {
			panic!("still the same tile")
		};
		assert!(before.same_tile(after), "moving never rewrites pixels");
	}

	#[test]
	fn offset_layer_rejects_non_pixel_layers_and_locked_position() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let g = f.group("G");
		let error = f.fail(Command::OffsetLayer {
			layer: LayerRef::Id(g),
			dx: 1,
			dy: 1,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		f.ok(Command::SetLayerProps {
			layer: LayerRef::Id(a),
			props: LayerPropsPatch {
				locked_position: Some(true),
				..Default::default()
			},
		});
		let error = f.fail(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 1,
			dy: 1,
		});
		assert!(matches!(error, CommandError::Locked(id) if id == a), "{error:?}");
		assert_eq!(f.pixel(a).1, (0, 0), "the lock held");
	}

	#[test]
	fn offset_layer_rejects_an_overflow_and_an_unknown_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 1,
			dy: 0,
		});
		let error = f.fail(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: i32::MAX,
			dy: 0,
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "offset", .. }), "{error:?}");
		assert_eq!(f.pixel(a).1, (1, 0), "the x offset was not left half-applied");
		let error = f.fail(Command::OffsetLayer {
			layer: LayerRef::Id(LayerId(99)),
			dx: 1,
			dy: 1,
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
	}

	// -------------------------------------------------------------------
	// AddMask
	// -------------------------------------------------------------------

	#[test]
	fn add_mask_reveal_all_costs_no_tile_memory() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let effect = f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		assert_eq!(effect.label, "Add Layer Mask");
		assert_eq!(effect.props_changed, vec![a]);
		assert!(!effect.structure_changed && effect.pixels_changed.is_empty());
		let mask = f.mask(a);
		assert!(mask.enabled && mask.linked, "a new mask is enabled and linked");
		assert_eq!(mask.outside_value, u16::MAX);
		assert_eq!(mask.image.format(), PixelFormat::Gray16, "masks follow the document depth");
		assert_eq!(data_tiles(&mask.image), 0, "solid tiles only");
		assert!(matches!(f.mask_slot(a, 0, 0), TileSlot::Solid(v) if v.0[0] == u16::MAX));
		assert!(f.mask_slot(a, 0, 0).same_as(f.mask_slot(a, 1, 1)));
	}

	#[test]
	fn add_mask_hide_all_is_all_empty() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::HideAll,
		});
		let mask = f.mask(a);
		assert_eq!(mask.outside_value, 0);
		assert_eq!(data_tiles(&mask.image), 0);
		assert!(f.mask_slot(a, 0, 0).is_empty());
		assert!(f.mask_slot(a, 1, 1).is_empty());
	}

	#[test]
	fn add_mask_rejects_a_second_mask_a_group_and_a_selection() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let g = f.group("G");
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		let error = f.fail(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::HideAll,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		assert!(
			matches!(f.mask_slot(a, 0, 0), TileSlot::Solid(v) if v.0[0] == u16::MAX),
			"the first mask is untouched"
		);
		let error = f.fail(Command::AddMask {
			layer: LayerRef::Id(g),
			fill: MaskFill::RevealAll,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		let error = f.fail(Command::AddMask {
			layer: LayerRef::Id(g),
			fill: MaskFill::RevealSelection,
		});
		assert!(error.to_string().contains("nothing is selected"), "M5: {error:?}");
		assert!(f.layer(g).mask.is_none());
	}

	#[test]
	fn add_mask_works_on_fill_and_adjustment_layers() {
		let mut f = Fixture::new();
		let fill = f.add(NewLayer::SolidFill { rgba: [0; 4] }, "Fill");
		let curves = f.add(NewLayer::Adjustment(curves()), "Curves 1");
		f.ok(Command::AddMask {
			layer: LayerRef::Id(fill),
			fill: MaskFill::HideAll,
		});
		f.ok(Command::AddMask {
			layer: LayerRef::Id(curves),
			fill: MaskFill::RevealAll,
		});
		assert!(f.layer(fill).mask.is_some() && f.layer(curves).mask.is_some());
	}

	#[test]
	fn add_mask_rejects_an_unknown_layer() {
		let mut f = Fixture::new();
		let error = f.fail(Command::AddMask {
			layer: LayerRef::Id(LayerId(99)),
			fill: MaskFill::RevealAll,
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
	}

	// -------------------------------------------------------------------
	// DeleteMask
	// -------------------------------------------------------------------

	#[test]
	fn delete_mask_removes_it_without_touching_pixels() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(3, 4, RED)]);
		let TileSlot::Data(before) = f.slot(a, 0, 0) else {
			panic!("the layer has a stored tile")
		};
		let before = before.clone();
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::HideAll,
		});
		let effect = f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: false,
		});
		assert_eq!(effect.label, "Delete Layer Mask");
		assert_eq!(effect.props_changed, vec![a]);
		assert!(effect.pixels_changed.is_empty(), "no pixels were touched");
		assert!(!effect.structure_changed);
		assert!(f.layer(a).mask.is_none());
		let TileSlot::Data(after) = f.slot(a, 0, 0) else {
			panic!("still the same tile")
		};
		assert!(before.same_tile(after));
	}

	#[test]
	fn delete_mask_apply_multiplies_alpha_by_a_stored_mask() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(0, 0, [500, 600, 700, 65535])]);
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		// Half grey under the pixel, black elsewhere: a stored (non-uniform) tile.
		f.paint_mask(a, (0, 0), &[(0, 0, 32896)]);
		let effect = f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert_eq!(effect.pixels_changed, vec![a]);
		assert_eq!(effect.props_changed, vec![a]);
		assert!(f.layer(a).mask.is_none());
		assert_eq!(f.read_pixel(a, 0, 0), [500, 600, 700, 32896], "alpha × mask, colour untouched");
		assert_eq!(f.read_pixel(a, 1, 1), [0; 4], "transparent pixels stay transparent");
	}

	#[test]
	fn delete_mask_apply_scales_a_whole_tile_with_a_uniform_mask() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(0, 0, [10, 20, 30, 40000]), (5, 5, [1, 2, 3, 10000])]);
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		f.fill_mask(a, (0, 0), 32768);
		f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert_eq!(f.read_pixel(a, 0, 0), [10, 20, 30, scale_alpha16(40000, 32768)]);
		assert_eq!(f.read_pixel(a, 5, 5), [1, 2, 3, scale_alpha16(10000, 32768)]);
	}

	#[test]
	fn delete_mask_apply_of_a_revealing_mask_changes_no_pixel() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(3, 4, RED)]);
		let TileSlot::Data(before) = f.slot(a, 0, 0) else {
			panic!("the layer has a stored tile")
		};
		let before = before.clone();
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		let effect = f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert!(effect.pixels_changed.is_empty(), "a reveal-all mask cannot change a pixel");
		let TileSlot::Data(after) = f.slot(a, 0, 0) else {
			panic!("the tile is still there")
		};
		assert!(before.same_tile(after), "the tile was not rewritten");
	}

	#[test]
	fn delete_mask_apply_of_a_hiding_mask_empties_the_tiles() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(3, 4, RED)]);
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::HideAll,
		});
		let effect = f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert_eq!(effect.pixels_changed, vec![a]);
		assert!(f.slot(a, 0, 0).is_empty(), "alpha 0 collapses back to an empty tile");
		assert_eq!(data_tiles(f.pixel(a).0), 0);
	}

	#[test]
	fn delete_mask_apply_leaves_a_layer_without_pixels_alone() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		let effect = f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert!(effect.pixels_changed.is_empty());
		assert!(f.slot(a, 0, 0).is_empty());
	}

	#[test]
	fn delete_mask_apply_refuses_an_unlinked_mask_under_an_offset_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(0, 0, [500, 600, 700, 65535])]);
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		f.fill_mask(a, (0, 0), 32768);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 7,
			dy: -3,
		});

		// A *linked* mask moves with the layer, so its tiles stay paired with
		// the layer's and the bake goes through.
		f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert_eq!(f.read_pixel(a, 0, 0)[3], scale_alpha16(65535, 32768));

		// An *unlinked* mask stays at the document origin: the two grids no
		// longer line up, so the command refuses and changes nothing.
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		f.fill_mask(a, (0, 0), 32768);
		let mask = &mut f.doc.layer_mut(a).expect("layer exists").mask;
		mask.as_mut().expect("layer has a mask").linked = false;
		let error = f.fail(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		assert!(f.layer(a).mask.is_some(), "the failed command put the mask back");
		assert_eq!(f.read_pixel(a, 0, 0)[3], scale_alpha16(65535, 32768), "the mask was not baked a second time");

		// With no offset the two alignments coincide, so an unlinked mask can go.
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: -7,
			dy: 3,
		});
		f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert_eq!(f.read_pixel(a, 0, 0)[3], scale_alpha16(scale_alpha16(65535, 32768), 32768));
	}

	#[test]
	fn delete_mask_apply_rejects_a_layer_without_pixels_of_its_own() {
		let mut f = Fixture::new();
		let curves = f.add(NewLayer::Adjustment(curves()), "Curves 1");
		f.ok(Command::AddMask {
			layer: LayerRef::Id(curves),
			fill: MaskFill::RevealAll,
		});
		let error = f.fail(Command::DeleteMask {
			layer: LayerRef::Id(curves),
			apply: true,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		assert!(f.layer(curves).mask.is_some(), "the failed command put the mask back");
		f.ok(Command::DeleteMask {
			layer: LayerRef::Id(curves),
			apply: false,
		});
		assert!(f.layer(curves).mask.is_none());
	}

	#[test]
	fn delete_mask_rejects_a_layer_without_a_mask_and_an_unknown_layer() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let error = f.fail(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		let error = f.fail(Command::DeleteMask {
			layer: LayerRef::Id(LayerId(99)),
			apply: true,
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
	}

	// -------------------------------------------------------------------
	// SetAdjustment
	// -------------------------------------------------------------------

	#[test]
	fn set_adjustment_replaces_the_parameters() {
		let mut f = Fixture::new();
		let a = f.add(
			NewLayer::Adjustment(Adjustment::Exposure {
				exposure: 0.0,
				offset: 0.0,
				gamma: 1.0,
			}),
			"Exposure 1",
		);
		let effect = f.ok(Command::SetAdjustment {
			layer: LayerRef::Id(a),
			adjustment: Adjustment::Exposure {
				exposure: 1.5,
				offset: 0.1,
				gamma: 0.9,
			},
		});
		assert_eq!(effect.label, "Exposure");
		assert_eq!(effect.props_changed, vec![a]);
		assert!(matches!(f.kind(a), LayerKind::Adjustment(Adjustment::Exposure { exposure, .. }) if *exposure == 1.5));
	}

	#[test]
	fn set_adjustment_rejects_another_type_and_another_layer_kind() {
		let mut f = Fixture::new();
		let curves = f.add(NewLayer::Adjustment(curves()), "Curves 1");
		let pixel = f.add_pixel("A");
		let error = f.fail(Command::SetAdjustment {
			layer: LayerRef::Id(curves),
			adjustment: Adjustment::Invert,
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "adjustment", .. }), "{error:?}");
		let error = f.fail(Command::SetAdjustment {
			layer: LayerRef::Id(pixel),
			adjustment: Adjustment::Invert,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		assert!(matches!(f.kind(curves), LayerKind::Adjustment(Adjustment::Curves { .. })), "unchanged");
		let error = f.fail(Command::SetAdjustment {
			layer: LayerRef::Id(LayerId(99)),
			adjustment: Adjustment::Invert,
		});
		assert!(matches!(error, CommandError::LayerNotFound(_)), "{error:?}");
	}

	// -------------------------------------------------------------------
	// History
	// -------------------------------------------------------------------

	#[test]
	fn every_m2_command_round_trips_through_history() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		let b = f.add_pixel("B");
		f.round_trip(Command::SetLayerProps {
			layer: LayerRef::Id(a),
			props: LayerPropsPatch {
				opacity: Some(0.25),
				..Default::default()
			},
		});
		f.round_trip(Command::AddLayer {
			layer: NewLayer::Adjustment(curves()),
			name: None,
		});
		f.round_trip(Command::SetAdjustment {
			layer: LayerRef::Active,
			adjustment: curves_with(0.75),
		});
		f.round_trip(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 5,
			dy: -7,
		});
		f.round_trip(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		f.round_trip(Command::DuplicateLayers { layers: vec![LayerRef::Id(a)] });
		f.round_trip(Command::GroupLayers {
			layers: vec![LayerRef::Id(b)],
			name: None,
		});
		f.round_trip(Command::MoveLayer {
			layer: LayerRef::Id(a),
			parent: None,
			index: 0,
		});
		f.round_trip(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		f.round_trip(Command::DeleteLayers { layers: vec![LayerRef::Id(b)] });

		// A selection change is not a history step (it is restored with the
		// document it belongs to).
		let steps = f.history.labels().count();
		f.round_trip(Command::SelectLayers { layers: vec![LayerRef::Id(a)] });
		assert_eq!(f.history.labels().count(), steps);
	}

	#[test]
	fn undo_brings_back_the_pixels_and_the_mask_an_apply_baked() {
		let mut f = Fixture::new();
		let a = f.add_pixel("A");
		f.paint(a, &[(0, 0, [500, 600, 700, 65535])]);
		let TileSlot::Data(before) = f.slot(a, 0, 0) else {
			panic!("the layer has a stored tile")
		};
		let before = before.clone();
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		f.fill_mask(a, (0, 0), 32768);
		f.ok(Command::DeleteMask {
			layer: LayerRef::Id(a),
			apply: true,
		});
		assert_eq!(f.read_pixel(a, 0, 0)[3], scale_alpha16(65535, 32768));

		assert!(f.history.undo(&mut f.doc));
		assert_eq!(f.read_pixel(a, 0, 0)[3], 65535, "the baked alpha is gone again");
		let TileSlot::Data(after) = f.slot(a, 0, 0) else {
			panic!("the original tile came back")
		};
		assert!(before.same_tile(after), "undo restores the untouched tile");
		assert!(f.layer(a).mask.is_some(), "and the mask with it");

		assert!(f.history.redo(&mut f.doc));
		assert_eq!(f.read_pixel(a, 0, 0)[3], scale_alpha16(65535, 32768));
		assert!(f.layer(a).mask.is_none());
	}

	#[test]
	fn alpha_scaling_rounds_half_up() {
		assert_eq!(scale_alpha16(65535, 65535), 65535);
		assert_eq!(scale_alpha16(65535, 0), 0);
		assert_eq!(scale_alpha16(65535, 32768), 32768);
		assert_eq!(scale_alpha16(3, 32768), 2, "1.5 rounds up");
		assert_eq!(scale_alpha16(1, 32768), 1, "0.5 rounds up");
		assert_eq!(scale_alpha16(1, 21845), 0, "0.33 rounds down");
		assert_eq!(scale_alpha8(255, 65535), 255);
		assert_eq!(scale_alpha8(255, 32768), 128);
		assert_eq!(scale_alpha8(255, 0), 0);
		assert_eq!(scale_alpha8(1, 32768), 1);
	}

	#[test]
	fn merge_down_keeps_the_lower_layers_place_id_and_name() {
		let mut f = Fixture::new();
		let bottom = f.add(NewLayer::Pixel, "Bottom");
		let top = f.add(NewLayer::Pixel, "Top");
		let effect = f
			.run_with_ops(Command::MergeLayers {
				layers: vec![LayerRef::Id(top), LayerRef::Id(bottom)],
			})
			.unwrap();
		assert_eq!(effect.label, "Merge Layers");
		assert_eq!(f.doc.layers.len(), 1);
		let merged = &f.doc.layers[0];
		assert_eq!(
			(merged.id, merged.name.as_str(), merged.blend, merged.opacity),
			(bottom, "Bottom", BlendMode::Normal, 1.0)
		);
		assert_eq!(f.doc.selected, vec![bottom]);
		let LayerKind::Pixel { image, .. } = &merged.kind else {
			panic!("a pixel layer")
		};
		assert!(
			matches!(image.slot(0, 0, 0), TileSlot::Solid(v) if v.0[0] == 2),
			"the composite of the two layers"
		);
		// Undo restores both.
		f.history.undo(&mut f.doc);
		assert_eq!(f.doc.layers.len(), 2);
	}

	#[test]
	fn merge_needs_two_layers_and_the_engine() {
		let mut f = Fixture::new();
		let only = f.add(NewLayer::Pixel, "Only");
		let error = f
			.run_with_ops(Command::MergeLayers {
				layers: vec![LayerRef::Id(only)],
			})
			.unwrap_err();
		assert!(matches!(error, CommandError::NotAllowed(_)));
		let other = f.add(NewLayer::Pixel, "Other");
		let error = f.fail(Command::MergeLayers {
			layers: vec![LayerRef::Id(only), LayerRef::Id(other)],
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "without PixelOps: {error}");
		assert_eq!(f.doc.layers.len(), 2, "nothing changed");
	}

	#[test]
	fn flatten_leaves_one_opaque_background_and_stamp_adds_a_layer() {
		let mut f = Fixture::new();
		f.add(NewLayer::Pixel, "A");
		f.add(NewLayer::Pixel, "B");
		let effect = f.run_with_ops(Command::StampVisible).unwrap();
		assert_eq!(effect.label, "Stamp Visible");
		assert_eq!(f.doc.layers.len(), 3);
		let effect = f.run_with_ops(Command::Flatten).unwrap();
		assert_eq!(effect.label, "Flatten Image");
		assert_eq!(f.doc.layers.len(), 1);
		let background = &f.doc.layers[0];
		assert_eq!(background.name, "Background");
		assert!(background.locked_position);
		let LayerKind::Pixel { image, .. } = &background.kind else {
			panic!("a pixel layer")
		};
		assert!(
			matches!(image.slot(0, 0, 0), TileSlot::Solid(v) if v.0 == [3, 3, 3, u16::MAX]),
			"every layer, on an opaque background"
		);
	}

	#[test]
	fn assign_changes_only_the_profile_and_convert_converts_fills() {
		let mut f = Fixture::new();
		let fill = f.add(NewLayer::SolidFill { rgba: [100, 200, 300, 65535] }, "Fill");
		f.ok(Command::AssignProfile {
			profile: ColorProfile::AdobeRgb1998,
		});
		assert_eq!(f.doc.color.profile, ColorProfile::AdobeRgb1998);
		assert!(matches!(f.doc.layer(fill).unwrap().kind, LayerKind::SolidFill { rgba: [100, 200, 300, 65535] }));
		let effect = f
			.run_with_ops(Command::ConvertProfile {
				profile: ColorProfile::Srgb,
				intent: RenderingIntent::RelativeColorimetric,
				bpc: true,
			})
			.unwrap();
		assert_eq!(effect.label, "Convert to Profile");
		assert_eq!(f.doc.color.profile, ColorProfile::Srgb);
		assert!(matches!(f.doc.layer(fill).unwrap().kind, LayerKind::SolidFill { rgba: [300, 200, 100, 65535] }));
		f.history.undo(&mut f.doc);
		assert_eq!(f.doc.color.profile, ColorProfile::AdobeRgb1998, "undo restores the profile");
	}

	// -----------------------------------------------------------------------
	// The pixel selection (M5-T03)
	// -----------------------------------------------------------------------

	/// Coverage of the fixture's current selection at one document pixel.
	fn coverage(f: &Fixture, x: u32, y: u32) -> f32 {
		let Some(selection) = &f.doc.selection else {
			return 0.0;
		};
		let format = selection.image.format();
		let (ix, iy) = (i64::from(x) - i64::from(selection.offset.0), i64::from(y) - i64::from(selection.offset.1));
		if ix < 0 || iy < 0 || ix >= i64::from(selection.image.width()) || iy >= i64::from(selection.image.height()) {
			return 0.0;
		}
		match selection.image.slot(0, ix as u32 / TILE_SIZE, iy as u32 / TILE_SIZE) {
			TileSlot::Empty => 0.0,
			TileSlot::Solid(value) => f32::from(value.0[0]) / 65535.0,
			TileSlot::Data(handle) => gray_at(&f.store.get(handle).unwrap(), format, ix as u32 % TILE_SIZE, iy as u32 % TILE_SIZE),
		}
	}

	/// `Select` with a rectangle, the one shape [`FakeOps`] rasterises.
	fn rect_select(x: f64, y: f64, w: f64, h: f64, mode: SelectMode) -> Command {
		Command::Select {
			shape: SelectionShape::Rect { x, y, w, h },
			mode,
			feather: 0.0,
			anti_alias: true,
		}
	}

	#[test]
	fn a_rectangular_selection_is_one_history_step_that_undoes() {
		let mut f = Fixture::new();
		let effect = f.ok_with_ops(rect_select(10.0, 20.0, 30.0, 40.0, SelectMode::Replace));
		assert_eq!(effect.label, "Rectangular Marquee");
		// A real history step, but not a document change: the engine saves the
		// document only for the latter (D-028, `CommandEffect::history_only`).
		assert!(effect.history_only && !effect.selection_only);
		assert!(effect.pixels_changed.is_empty() && effect.props_changed.is_empty());
		assert_eq!(f.history.labels().collect::<Vec<_>>(), ["Rectangular Marquee"]);
		assert_eq!(coverage(&f, 15, 25), 1.0, "inside");
		assert_eq!(coverage(&f, 5, 25), 0.0, "left of the rectangle");
		assert_eq!(coverage(&f, 39, 59), 1.0, "the last row and column");
		assert_eq!(coverage(&f, 40, 20), 0.0, "just past the right edge");
		assert!(f.history.undo(&mut f.doc));
		assert!(f.doc.selection.is_none(), "undo takes the selection away");
		assert!(f.history.redo(&mut f.doc));
		assert_eq!(coverage(&f, 15, 25), 1.0, "redo brings it back");
	}

	#[test]
	fn selection_commands_need_the_engine_and_reject_bad_values() {
		let mut f = Fixture::new();
		let error = f.fail(rect_select(0.0, 0.0, 10.0, 10.0, SelectMode::Replace));
		assert!(matches!(error, CommandError::NotAllowed(_)), "without PixelOps: {error}");
		assert!(f.doc.selection.is_none(), "nothing was selected");
		let error = f.fail_with_ops(Command::Select {
			shape: SelectionShape::Rect {
				x: 0.0,
				y: 0.0,
				w: 1.0,
				h: 1.0,
			},
			mode: SelectMode::Replace,
			feather: -1.0,
			anti_alias: true,
		});
		assert!(matches!(error, CommandError::InvalidValue { field: "feather", .. }), "{error}");
		assert!(f.fail(Command::Deselect).to_string().contains("nothing is selected"));
		assert!(f.fail(Command::Reselect).to_string().contains("no deselected selection"));
		// These are pure model code: they need no PixelOps.
		f.ok(Command::SelectAll);
		let error = f.fail(Command::ModifySelection {
			modify: SelectModify::Expand(2.0),
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error}");
	}

	#[test]
	fn select_all_deselect_and_reselect_each_undo() {
		let mut f = Fixture::new();
		f.ok(Command::SelectAll);
		assert_eq!((coverage(&f, 0, 0), coverage(&f, 399, 299)), (1.0, 1.0), "the whole canvas");
		f.ok(Command::Deselect);
		assert!(f.doc.selection.is_none());
		assert!(f.doc.reselect.is_some(), "kept for Reselect");
		f.ok(Command::Reselect);
		assert_eq!(coverage(&f, 100, 100), 1.0);
		assert!(f.doc.reselect.is_none(), "restored, so there is nothing left to restore");
		// Walk the three steps back.
		assert!(f.history.undo(&mut f.doc), "Reselect");
		assert!(f.doc.selection.is_none());
		assert!(f.history.undo(&mut f.doc), "Deselect");
		assert_eq!(coverage(&f, 100, 100), 1.0);
		assert!(f.history.undo(&mut f.doc), "Select All");
		assert!(f.doc.selection.is_none());
		assert!(!f.history.can_undo(), "three commands, three steps");
	}

	#[test]
	fn inverting_selects_everything_but_the_old_selection() {
		let mut f = Fixture::new();
		// Inverting nothing selects everything, like Photoshop.
		assert_eq!(f.ok(Command::InvertSelection).label, "Inverse");
		assert_eq!(coverage(&f, 399, 299), 1.0);
		f.ok_with_ops(rect_select(0.0, 0.0, 100.0, 100.0, SelectMode::Replace));
		f.ok(Command::InvertSelection);
		assert_eq!(coverage(&f, 50, 50), 0.0, "the rectangle is gone");
		assert_eq!(coverage(&f, 150, 50), 1.0, "everything else is selected");
		assert_eq!(coverage(&f, 300, 250), 1.0, "to the far corner");
		f.history.undo(&mut f.doc);
		assert_eq!(coverage(&f, 50, 50), 1.0, "undo restores the rectangle");
		assert_eq!(coverage(&f, 150, 50), 0.0);
		// Inverting everything selected leaves an empty selection (the ants
		// disappear), like Photoshop's "no pixels are selected".
		f.ok(Command::SelectAll);
		f.ok(Command::InvertSelection);
		assert!(f.doc.selection.is_none(), "nothing is left selected");
	}

	#[test]
	fn a_moved_selection_keeps_its_coverage_and_inverts_in_document_space() {
		let mut f = Fixture::new();
		f.ok_with_ops(rect_select(0.0, 0.0, 100.0, 100.0, SelectMode::Replace));
		assert_eq!(f.ok(Command::OffsetSelection { dx: 5, dy: 7 }).label, "Move Selection");
		assert_eq!(f.doc.selection.as_ref().unwrap().offset, (5, 7), "the tiles never moved");
		assert_eq!(coverage(&f, 50, 50), 1.0, "the rectangle reads at its new place");
		assert_eq!(coverage(&f, 3, 3), 0.0, "and no longer at the old one");
		f.ok(Command::InvertSelection);
		assert_eq!(coverage(&f, 50, 50), 0.0);
		assert_eq!(coverage(&f, 3, 3), 1.0, "inverted in document space");
		assert_eq!(coverage(&f, 104, 106), 0.0, "the moved rectangle's last pixel");
		assert_eq!(coverage(&f, 105, 106), 1.0, "just past it");
	}

	#[test]
	fn a_new_shape_combines_with_the_current_selection_by_mode() {
		let mut f = Fixture::new();
		f.ok_with_ops(rect_select(0.0, 0.0, 100.0, 100.0, SelectMode::Replace));
		f.ok_with_ops(rect_select(50.0, 0.0, 100.0, 100.0, SelectMode::Add));
		assert_eq!((coverage(&f, 25, 25), coverage(&f, 75, 25), coverage(&f, 120, 25)), (1.0, 1.0, 1.0));
		f.ok_with_ops(rect_select(50.0, 0.0, 100.0, 100.0, SelectMode::Subtract));
		assert_eq!((coverage(&f, 25, 25), coverage(&f, 75, 25), coverage(&f, 120, 25)), (1.0, 0.0, 0.0));
		f.ok_with_ops(rect_select(0.0, 0.0, 60.0, 60.0, SelectMode::Intersect));
		assert_eq!((coverage(&f, 25, 25), coverage(&f, 75, 25)), (1.0, 0.0));
		f.ok_with_ops(rect_select(200.0, 200.0, 50.0, 50.0, SelectMode::Replace));
		assert_eq!((coverage(&f, 225, 225), coverage(&f, 25, 25)), (1.0, 0.0), "Replace drops the old one");
		assert_eq!(f.history.labels().collect::<Vec<_>>(), ["Rectangular Marquee"; 5], "one step each");
	}

	#[test]
	fn every_selection_command_round_trips_through_json() {
		let commands = vec![
			rect_select(1.5, 2.5, 3.0, 4.0, SelectMode::Add),
			Command::Select {
				shape: SelectionShape::Ellipse {
					x: 0.0,
					y: 1.0,
					w: 10.0,
					h: 20.0,
				},
				mode: SelectMode::Subtract,
				feather: 2.5,
				anti_alias: false,
			},
			Command::Select {
				shape: SelectionShape::Polygon {
					points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 8.0)],
				},
				mode: SelectMode::Intersect,
				feather: 0.0,
				anti_alias: true,
			},
			Command::Select {
				shape: SelectionShape::RowPixel { y: 3.0 },
				mode: SelectMode::Replace,
				feather: 0.0,
				anti_alias: true,
			},
			Command::Select {
				shape: SelectionShape::ColumnPixel { x: 3.0 },
				mode: SelectMode::Replace,
				feather: 0.0,
				anti_alias: true,
			},
			Command::SelectAll,
			Command::Deselect,
			Command::Reselect,
			Command::InvertSelection,
			Command::ModifySelection {
				modify: SelectModify::Expand(2.0),
			},
			Command::ModifySelection {
				modify: SelectModify::Contract(2.0),
			},
			Command::ModifySelection {
				modify: SelectModify::Border(4.0),
			},
			Command::ModifySelection {
				modify: SelectModify::Smooth(1.5),
			},
			Command::ModifySelection {
				modify: SelectModify::Feather(3.0),
			},
			Command::OffsetSelection { dx: -2, dy: 5 },
		];
		for command in commands {
			let json = serde_json::to_string(&command).unwrap();
			assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command, "{json}");
		}
		// Pin the wire shapes: they are part of saved macros and of the
		// `command` message the UI sends (docs/PROTOCOL.md §4).
		assert_eq!(serde_json::to_string(&Command::SelectAll).unwrap(), r#"{"op":"select_all"}"#);
		assert_eq!(
			serde_json::to_string(&rect_select(1.5, 2.5, 3.0, 4.0, SelectMode::Add)).unwrap(),
			r#"{"op":"select","shape":{"shape":"rect","x":1.5,"y":2.5,"w":3.0,"h":4.0},"mode":"add","feather":0.0,"anti_alias":true}"#
		);
		assert_eq!(
			serde_json::to_string(&Command::ModifySelection {
				modify: SelectModify::Feather(3.0)
			})
			.unwrap(),
			r#"{"op":"modify_selection","modify":{"kind":"feather","px":3.0}}"#
		);
	}

	#[test]
	fn every_image_geometry_command_round_trips_through_json() {
		let commands = vec![
			Command::RotateCanvas { quarter_turns: 1 },
			Command::RotateCanvas { quarter_turns: 3 },
			Command::FlipCanvas { horizontal: true },
			Command::FlipCanvas { horizontal: false },
			Command::RotateCanvasArbitrary {
				angle_deg: 12.5,
				filter: Filter::BicubicAutomatic,
			},
			Command::CanvasSize {
				width: 800,
				height: 600,
				anchor: Anchor9::Center,
			},
			Command::ImageSize {
				width: 800,
				height: 600,
				ppi: 300.0,
				resample: Some(Filter::BicubicSharper),
			},
			Command::ImageSize {
				width: 800,
				height: 600,
				ppi: 300.0,
				resample: None,
			},
			Command::Crop {
				rect: (10, -20, 400, 300),
				angle_deg: 0.0,
				delete_cropped: false,
			},
			Command::Transform {
				layer: LayerRef::Active,
				// Values JSON holds exactly (serde_json's float parse is not bit-exact).
				mapping: Box::new(Mapping::affine(0.5, 0.25, -0.25, 0.5, 10.0, 20.0)),
				filter: Filter::Bicubic,
			},
			Command::Crop {
				rect: (0, 0, 400, 300),
				angle_deg: -7.5,
				delete_cropped: true,
			},
		];
		for command in commands {
			let json = serde_json::to_string(&command).unwrap();
			assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command, "{json}");
		}
		// Pin the wire shapes: `dlg:rotate-arbitrary`, `dlg:canvas-size` and
		// `dlg:image-size` send exactly these (docs/PROTOCOL.md §4).
		assert_eq!(
			serde_json::to_string(&Command::RotateCanvas { quarter_turns: 1 }).unwrap(),
			r#"{"op":"rotate_canvas","quarter_turns":1}"#
		);
		assert_eq!(
			serde_json::to_string(&Command::FlipCanvas { horizontal: true }).unwrap(),
			r#"{"op":"flip_canvas","horizontal":true}"#
		);
		assert_eq!(
			serde_json::to_string(&Command::RotateCanvasArbitrary {
				angle_deg: 12.5,
				filter: Filter::Bicubic
			})
			.unwrap(),
			r#"{"op":"rotate_canvas_arbitrary","angle_deg":12.5,"filter":"bicubic"}"#
		);
		assert_eq!(
			serde_json::to_string(&Command::CanvasSize {
				width: 800,
				height: 600,
				anchor: Anchor9::TopLeft
			})
			.unwrap(),
			r#"{"op":"canvas_size","width":800,"height":600,"anchor":"top_left"}"#
		);
		assert_eq!(
			serde_json::to_string(&Command::ImageSize {
				width: 800,
				height: 600,
				ppi: 300.0,
				resample: Some(Filter::Lanczos3)
			})
			.unwrap(),
			r#"{"op":"image_size","width":800,"height":600,"ppi":300.0,"resample":"lanczos3"}"#
		);
		// "Resample" off: the print resolution changes, the pixels do not.
		assert_eq!(
			serde_json::to_string(&Command::ImageSize {
				width: 800,
				height: 600,
				ppi: 300.0,
				resample: None
			})
			.unwrap(),
			r#"{"op":"image_size","width":800,"height":600,"ppi":300.0,"resample":null}"#
		);
		// The crop tool's ✓ and Image ▸ Crop send this (docs/PROTOCOL.md §4).
		assert_eq!(
			serde_json::to_string(&Command::Crop {
				rect: (60, 70, 200, 150),
				angle_deg: 0.0,
				delete_cropped: false
			})
			.unwrap(),
			r#"{"op":"crop","rect":[60,70,200,150],"angle_deg":0.0,"delete_cropped":false}"#
		);
	}

	#[test]
	fn quarter_turns_move_the_image_and_its_offset_and_come_back() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		f.resize_image(a, 100, 60);
		f.paint(a, &[(0, 0, [111, 222, 333, 65_535]), (99, 59, [10, 20, 30, 65_535])]);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 50,
			dy: 40,
		});
		assert_eq!(f.size(), (400, 300));

		f.ok_with_ops(Command::RotateCanvas { quarter_turns: 1 });
		assert_eq!(f.size(), (300, 400), "the odd quarter turn swaps the axes");
		assert_eq!(f.pixel(a).1, (200, 50), "the offset follows the content");
		assert_eq!(f.pixel(a).0.width(), 60);
		assert_eq!(f.pixel(a).0.height(), 100);
		// A clockwise turn puts the image's bottom-left pixel at its top-left.
		assert_eq!(f.read_pixel(a, 59, 0), [111, 222, 333, 65_535]);
		assert_eq!(f.read_pixel(a, 0, 0), [0; 4], "the old top-left column is now the bottom row");

		// Four quarter turns are the original document: size, offset and tiles.
		for _ in 0..3 {
			f.ok_with_ops(Command::RotateCanvas { quarter_turns: 1 });
		}
		assert_eq!(f.size(), (400, 300));
		assert_eq!(f.pixel(a).1, (50, 40));
		assert_eq!((f.pixel(a).0.width(), f.pixel(a).0.height()), (100, 60));
		assert_eq!(f.read_pixel(a, 0, 0), [111, 222, 333, 65_535]);
		assert_eq!(f.read_pixel(a, 99, 59), [10, 20, 30, 65_535], "every pixel came home");
		// The turns are history steps, and undo walks them back.
		assert_eq!(f.history.labels().count(), 6, "AddLayer, OffsetLayer and the four turns");
	}

	#[test]
	fn flipping_the_canvas_mirrors_the_content_and_undoes_itself() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		f.resize_image(a, 100, 60);
		f.paint(a, &[(0, 0, [100, 0, 0, 65_535]), (99, 59, [0, 100, 0, 65_535])]);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 50,
			dy: 40,
		});

		f.ok_with_ops(Command::FlipCanvas { horizontal: true });
		assert_eq!(f.size(), (400, 300), "a flip keeps the canvas size");
		assert_eq!(f.pixel(a).1, (250, 40));
		assert_eq!(f.read_pixel(a, 99, 0), [100, 0, 0, 65_535], "left ↔ right");
		assert_eq!(f.read_pixel(a, 0, 59), [0, 100, 0, 65_535]);

		f.ok_with_ops(Command::FlipCanvas { horizontal: true });
		assert_eq!(f.pixel(a).1, (50, 40));
		assert_eq!(f.read_pixel(a, 0, 0), [100, 0, 0, 65_535], "twice is the original");
		assert_eq!(f.read_pixel(a, 99, 59), [0, 100, 0, 65_535]);
	}

	#[test]
	fn canvas_size_moves_offsets_and_rewrites_no_pixel() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		f.resize_image(a, 100, 60);
		f.paint(a, &[(0, 0, [7, 8, 9, 65_535])]);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 50,
			dy: 40,
		});
		f.ok(Command::SelectAll);
		let tile = f.slot(a, 0, 0).clone();

		let effect = f.ok(Command::CanvasSize {
			width: 600,
			height: 500,
			anchor: Anchor9::Center,
		});
		assert_eq!(f.size(), (600, 500));
		assert_eq!(f.pixel(a).1, (150, 140), "the layer moved by the anchor's +100");
		assert_eq!(f.selection().offset, (100, 100), "the selection moved with it");
		assert!(f.slot(a, 0, 0).same_as(&tile), "no tile was rewritten (D-015)");
		assert_eq!(effect.props_changed, vec![a], "an offset change is a property change");

		// And back: the reverse anchor restores the document exactly.
		f.ok(Command::CanvasSize {
			width: 400,
			height: 300,
			anchor: Anchor9::Center,
		});
		assert_eq!(f.size(), (400, 300));
		assert_eq!(f.pixel(a).1, (50, 40));
		assert_eq!(f.selection().offset, (0, 0));
		assert!(f.slot(a, 0, 0).same_as(&tile));
	}

	#[test]
	fn image_size_without_resampling_only_changes_the_resolution() {
		let mut f = Fixture::new();
		f.ok(Command::ImageSize {
			width: 400,
			height: 300,
			ppi: 200.0,
			resample: None,
		});
		assert_eq!(f.doc.ppi, 200.0);
		let error = f.fail(Command::ImageSize {
			width: 200,
			height: 150,
			ppi: 300.0,
			resample: None,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
		assert_eq!(f.doc.ppi, 200.0, "nothing was applied");
	}

	#[test]
	fn image_size_scales_every_offset_the_masks_and_the_selection() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		f.resize_image(a, 100, 60);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 40,
			dy: 20,
		});
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::RevealAll,
		});
		f.ok(Command::SelectAll);

		let effect = f.ok_with_ops(Command::ImageSize {
			width: 200,
			height: 150,
			ppi: 144.0,
			resample: Some(Filter::Bicubic),
		});
		assert_eq!(f.size(), (200, 150));
		assert_eq!(f.doc.ppi, 144.0);
		assert!(effect.pixels_changed.contains(&a));
		// Half scale about the canvas origin: the 100 × 60 image at (40, 20)
		// becomes 50 × 30 at (20, 10).
		assert_eq!(f.pixel(a).1, (20, 10));
		assert_eq!((f.pixel(a).0.width(), f.pixel(a).0.height()), (50, 30));
		let mask = f.mask(a).image.clone();
		assert_eq!((mask.width(), mask.height()), (200, 150), "a document-sized mask scales with the canvas");
		assert_eq!((f.selection().image.width(), f.selection().image.height()), (200, 150));
	}

	#[test]
	fn crop_without_deleting_keeps_the_pixels_and_rewrites_nothing() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		f.resize_image(a, 100, 60);
		f.paint(a, &[(0, 0, [7, 8, 9, 65_535])]);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 50,
			dy: 40,
		});
		f.ok(Command::SelectAll);
		let tile = f.slot(a, 0, 0).clone();

		let effect = f.ok(Command::Crop {
			rect: (60, 70, 200, 150),
			angle_deg: 0.0,
			delete_cropped: false,
		});
		assert_eq!(effect.label, "Crop");
		assert_eq!(f.size(), (200, 150));
		assert_eq!(f.pixel(a).1, (-10, -30), "the layer moved with the canvas");
		assert_eq!(f.selection().offset, (-60, -70), "and so did the selection");
		assert!(f.slot(a, 0, 0).same_as(&tile), "no pixel was rewritten (D-015)");
		assert_eq!(effect.props_changed, vec![a], "an offset change is a property change");
		assert!(effect.pixels_changed.is_empty());
		assert_eq!(f.read_pixel(a, 0, 0), [7, 8, 9, 65_535], "the pixels are where they were");

		// Cropping back from the old canvas's own corner is the identity.
		f.ok(Command::Crop {
			rect: (-60, -70, 400, 300),
			angle_deg: 0.0,
			delete_cropped: false,
		});
		assert_eq!(f.size(), (400, 300));
		assert_eq!(f.pixel(a).1, (50, 40));
		assert_eq!(f.selection().offset, (0, 0));
		assert!(f.slot(a, 0, 0).same_as(&tile), "still the same tile handle");
	}

	#[test]
	fn crop_with_delete_leaves_nothing_outside_the_rectangle() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		// Two pixels in the first tile, one in the second.
		f.paint(a, &[(10, 20, [1, 2, 3, 65_535]), (200, 100, [4, 5, 6, 65_535])]);
		f.paint(a, &[(300, 10, [7, 8, 9, 65_535])]);
		f.ok(Command::SelectAll);

		let effect = f.ok(Command::Crop {
			rect: (50, 50, 200, 150),
			angle_deg: 0.0,
			delete_cropped: true,
		});
		assert_eq!(f.size(), (200, 150));
		assert_eq!(effect.pixels_changed, vec![a]);
		assert_eq!(f.pixel(a).1, (-50, -50), "the offsets still move");
		assert_eq!(f.read_pixel(a, 200, 100), [4, 5, 6, 65_535], "inside the rectangle");
		assert_eq!(f.read_pixel(a, 10, 20), [0; 4], "outside it the pixel is gone");
		assert_eq!(f.read_pixel(a, 300, 10), [0; 4], "a whole tile outside was dropped");
		assert!(matches!(f.slot(a, 1, 0), TileSlot::Empty));
		// The selection is clipped too, and then moves with the canvas: what the
		// rectangle kept is selected over the whole new canvas, and the coverage
		// outside it is gone.
		let selection = f.selection();
		assert_eq!(selection.offset, (-50, -50), "it moved with the canvas");
		assert!(matches!(selection.image.slot(0, 0, 0), TileSlot::Data(_)), "the crossed tile was rewritten");
		assert!(matches!(selection.image.slot(0, 1, 0), TileSlot::Empty), "a tile below the rectangle went");
		assert!(matches!(selection.image.slot(1, 0, 0), TileSlot::Empty), "and one to its right");
		assert_eq!(f.doc.width, 200, "the selection is read in the new canvas's frame");
	}

	#[test]
	fn a_straighten_turns_the_images_and_makes_the_document_the_rectangle() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		let b = f.add_pixel("Layer 2");
		f.resize_image(b, 100, 60);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(b),
			dx: 40,
			dy: 20,
		});

		let effect = f.ok_with_ops(Command::Crop {
			rect: (100, 80, 200, 150),
			angle_deg: 10.0,
			delete_cropped: false,
		});
		assert_eq!(f.size(), (200, 150), "the canvas is the rectangle");
		assert!(effect.pixels_changed.contains(&a) && effect.pixels_changed.contains(&b));
		assert!(effect.props_changed.is_empty(), "a turn carries the offsets in the image box");
		// A 10° turn about the rectangle's centre, then into the rectangle's
		// frame: the 100 × 60 image at (40, 20) unfolds into its mapped box.
		let (image, offset) = f.pixel(b);
		assert_eq!((offset, (image.width(), image.height())), ((-45, -86), (110, 77)));
	}

	#[test]
	fn an_arbitrary_rotation_grows_the_canvas_to_the_rotated_box() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		let effect = f.ok_with_ops(Command::RotateCanvasArbitrary {
			angle_deg: 90.0,
			filter: Filter::Bicubic,
		});
		assert_eq!(effect.label, "Rotate Image");
		assert_eq!(f.size(), (300, 400));
		// A document-sized layer covers the whole new canvas, from the origin.
		assert_eq!(f.pixel(a).1, (0, 0));
		assert_eq!((f.pixel(a).0.width(), f.pixel(a).0.height()), (300, 400));

		// A 45° turn grows the box by ⌈400·sin45 + 300·cos45⌉ each way.
		let mut g = Fixture::new();
		g.add_pixel("Layer 1");
		g.ok_with_ops(Command::RotateCanvasArbitrary {
			angle_deg: 45.0,
			filter: Filter::Bilinear,
		});
		assert_eq!(g.size(), (496, 496));
	}

	#[test]
	fn geometry_commands_refuse_nonsense_and_change_nothing() {
		let mut f = Fixture::new();
		f.add_pixel("Layer 1");
		let before = format!("{:?}", f.doc);
		let steps = f.history.labels().count();
		for command in [
			Command::RotateCanvas { quarter_turns: 0 },
			Command::RotateCanvas { quarter_turns: 4 },
			Command::CanvasSize {
				width: 0,
				height: 100,
				anchor: Anchor9::Center,
			},
			Command::CanvasSize {
				width: 100,
				height: 0,
				anchor: Anchor9::Center,
			},
			Command::ImageSize {
				width: 100,
				height: 100,
				ppi: 0.0,
				resample: Some(Filter::Nearest),
			},
			Command::ImageSize {
				width: 100,
				height: 100,
				ppi: f32::NAN,
				resample: None,
			},
			Command::ImageSize {
				width: 0,
				height: 100,
				ppi: 72.0,
				resample: None,
			},
			Command::RotateCanvasArbitrary {
				angle_deg: 0.0,
				filter: Filter::Bicubic,
			},
			Command::RotateCanvasArbitrary {
				angle_deg: f64::NAN,
				filter: Filter::Bicubic,
			},
			Command::Crop {
				rect: (0, 0, 0, 100),
				angle_deg: 0.0,
				delete_cropped: true,
			},
			Command::Crop {
				rect: (0, 0, 100, 0),
				angle_deg: 0.0,
				delete_cropped: false,
			},
			Command::Crop {
				rect: (0, 0, 100, 100),
				angle_deg: f64::NAN,
				delete_cropped: false,
			},
		] {
			let error = f.fail_with_ops(command.clone());
			assert!(matches!(error, CommandError::InvalidValue { .. }), "{command:?}: {error:?}");
		}
		assert_eq!(format!("{:?}", f.doc), before);
		assert_eq!(f.history.labels().count(), steps, "a refused command is no history step");
	}

	#[test]
	fn geometry_commands_round_trip_through_history() {
		let mut f = Fixture::new();
		let a = f.add_pixel("Layer 1");
		f.resize_image(a, 100, 60);
		f.paint(a, &[(3, 4, [1, 2, 3, 65_535])]);
		f.ok(Command::OffsetLayer {
			layer: LayerRef::Id(a),
			dx: 10,
			dy: 20,
		});
		let ops = FakeOps;
		for command in [
			Command::RotateCanvas { quarter_turns: 1 },
			Command::FlipCanvas { horizontal: false },
			Command::CanvasSize {
				width: 500,
				height: 400,
				anchor: Anchor9::BottomRight,
			},
			Command::ImageSize {
				width: 200,
				height: 150,
				ppi: 72.0,
				resample: Some(Filter::Bilinear),
			},
			Command::RotateCanvasArbitrary {
				angle_deg: 30.0,
				filter: Filter::Bicubic,
			},
			Command::Crop {
				rect: (60, 70, 200, 150),
				angle_deg: 0.0,
				delete_cropped: true,
			},
		] {
			let before = format!("{:?}", f.doc);
			let mut ctx = CommandContext {
				tiles: &f.store,
				ops: Some(&ops),
			};
			f.history
				.execute(&mut f.doc, command.clone(), &mut ctx)
				.unwrap_or_else(|e| panic!("{command:?}: {e}"));
			let after = format!("{:?}", f.doc);
			assert_ne!(before, after, "{command:?} changed the document");
			assert!(f.history.undo(&mut f.doc), "{command:?} is undoable");
			assert_eq!(format!("{:?}", f.doc), before, "undo restores {command:?}");
			assert!(f.history.redo(&mut f.doc), "{command:?} is redoable");
			assert_eq!(format!("{:?}", f.doc), after, "redo replays {command:?}");
		}
	}

	#[test]
	fn a_shape_layer_carries_a_document_sized_derived_cache() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 10.0],
		);
		let cache = f.cache(id);
		assert!(cache.is_derived(), "a shape's pixels are drawn from its geometry, never stored");
		assert_eq!((cache.width(), cache.height()), (400, 300), "the cache is the canvas");
		assert_eq!(f.dirty_shape_tiles(id).len(), 4, "a new shape has every tile to draw");
	}

	#[test]
	fn a_shape_layer_is_named_after_its_shape() {
		let mut f = Fixture::new();
		let rect = f.add_shape(
			VectorShape::Rect {
				w: 10.0,
				h: 10.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
		);
		let rounded = f.add_shape(
			VectorShape::Rect {
				w: 10.0,
				h: 10.0,
				radii: [3.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
		);
		let star = f.add_shape(VectorShape::Polygon { sides: 5, star_inset: 0.5 }, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
		let ellipse = f.add_shape(VectorShape::Ellipse { w: 10.0, h: 10.0 }, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
		assert_eq!(f.name(rect), "Rectangle 1", "Photoshop names the layer after the tool");
		assert_eq!(f.name(rounded), "Rounded Rectangle 1");
		assert_eq!(f.name(star), "Star 1");
		assert_eq!(f.name(ellipse), "Ellipse 1", "each kind has its own counter");
	}

	#[test]
	fn set_shape_dirties_only_the_tiles_the_outline_reaches() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 10.0],
		);
		f.clear_shape_cache(id);
		assert!(f.dirty_shape_tiles(id).is_empty());
		// A fill has no effect on the box: only the tile the shape sits in.
		f.ok(Command::SetShape {
			layer: LayerRef::Id(id),
			shape: None,
			fill: Some(Some(Paint::Solid { rgba: [0, 60_000, 0, 65_535] })),
			stroke: None,
			transform: None,
		});
		assert_eq!(f.dirty_shape_tiles(id), vec![(0, 0)], "a 100 × 50 rect at (10, 10) is in tile (0, 0)");
		// Moving it dirties where it was and where it goes — nothing else.
		f.clear_shape_cache(id);
		f.ok(Command::SetShape {
			layer: LayerRef::Id(id),
			shape: None,
			fill: None,
			stroke: None,
			transform: Some([1.0, 0.0, 0.0, 1.0, 300.0, 250.0]),
		});
		let mut dirty = f.dirty_shape_tiles(id);
		dirty.sort_unstable();
		assert_eq!(dirty, vec![(0, 0), (1, 1)], "the old tile and the new one");
	}

	#[test]
	fn set_shape_only_takes_a_shape_layer() {
		let mut f = Fixture::new();
		let pixel = f.add_pixel("Layer 1");
		let error = f.fail(Command::SetShape {
			layer: LayerRef::Id(pixel),
			shape: None,
			fill: Some(None),
			stroke: None,
			transform: None,
		});
		assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
	}

	#[test]
	fn rasterising_a_shape_layer_turns_it_into_pixels_and_keeps_the_rest() {
		let mut f = Fixture::new();
		let id = f.add_shape(VectorShape::Ellipse { w: 40.0, h: 40.0 }, [1.0, 0.0, 0.0, 1.0, 5.0, 5.0]);
		f.ok(Command::SetLayerProps {
			layer: LayerRef::Id(id),
			props: LayerPropsPatch {
				opacity: Some(0.5),
				blend: Some(BlendMode::Multiply),
				..Default::default()
			},
		});
		let before = f.layer(id).clone();
		let effect = f.ok_with_ops(Command::Rasterize {
			layers: vec![LayerRef::Id(id)],
		});
		assert_eq!(effect.label, "Rasterize Layer");
		assert_eq!(effect.pixels_changed, vec![id]);
		assert_eq!(effect.props_changed, vec![id]);
		let (image, offset) = f.pixel(id);
		assert_eq!(offset, (0, 0), "a rasterised layer sits at the canvas origin");
		assert_eq!((image.width(), image.height()), (400, 300));
		assert_eq!(
			(f.name(id), f.layer(id).opacity, f.layer(id).blend),
			(before.name.as_str(), 0.5, BlendMode::Multiply)
		);
		assert_eq!(f.layer(id).id, id, "the layer keeps its identity");
	}

	#[test]
	fn rasterising_refuses_pixels_a_group_and_nothing_at_all() {
		let mut f = Fixture::new();
		let pixel = f.add_pixel("Layer 1");
		let group = f.group("Group 1");
		for (layers, expected) in [
			(vec![LayerRef::Id(pixel)], "already pixels"),
			(vec![LayerRef::Id(group)], "cannot be rasterised"),
			(Vec::new(), "no layers"),
		] {
			let error = f.fail_with_ops(Command::Rasterize { layers });
			assert!(matches!(error, CommandError::NotAllowed(_)), "{error:?}");
			assert!(error.to_string().contains(expected), "{error}");
		}
	}

	#[test]
	fn a_shape_layer_turns_with_the_canvas() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
		);
		f.ok_with_ops(Command::RotateCanvas { quarter_turns: 1 });
		// A quarter turn clockwise sends (x, y) to (h − 1 − y, x) in a 400 × 300
		// canvas, so the shape's origin lands at (279, 10) — and the canvas is
		// now 300 × 400.
		assert_eq!(f.matrix(id), [0.0, 1.0, -1.0, 0.0, 279.0, 10.0]);
		assert_eq!((f.cache(id).width(), f.cache(id).height()), (300, 400), "the cache follows the canvas");
		assert_eq!(f.dirty_shape_tiles(id).len(), 4, "2 × 2 tiles at the new size");
	}

	#[test]
	fn a_shape_layer_flips_with_the_canvas() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
		);
		f.ok_with_ops(Command::FlipCanvas { horizontal: true });
		// A horizontal flip sends x to w − 1 − x: the matrix becomes a mirror.
		assert_eq!(f.matrix(id), [-1.0, 0.0, 0.0, 1.0, 389.0, 20.0]);
		assert_eq!((f.cache(id).width(), f.cache(id).height()), (400, 300), "a flip keeps the size");
	}

	#[test]
	fn a_shape_layer_scales_with_image_size() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
		);
		f.ok_with_ops(Command::ImageSize {
			width: 200,
			height: 150,
			ppi: 72.0,
			resample: Some(Filter::Bilinear),
		});
		assert_eq!(f.matrix(id), [0.5, 0.0, 0.0, 0.5, 5.0, 10.0], "half the canvas, half the placement");
		assert_eq!(f.dirty_shape_tiles(id).len(), 1, "200 × 150 is one tile");
	}

	#[test]
	fn a_shape_layer_crops_and_canvas_sizes_with_the_canvas() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
		);
		let shape_before = format!("{:?}", f.kind(id));
		f.ok_with_ops(Command::Crop {
			rect: (60, 70, 200, 150),
			angle_deg: 0.0,
			delete_cropped: false,
		});
		// The crop box's top-left becomes the origin: the shape moves by it and
		// its geometry is untouched (a shape is not rasterised by a crop).
		assert_eq!(f.matrix(id), [1.0, 0.0, 0.0, 1.0, -50.0, -50.0]);
		assert_eq!(format!("{:?}", f.kind(id)), shape_before, "crop must not change the geometry");
		assert_eq!((f.cache(id).width(), f.cache(id).height()), (200, 150));
		f.ok_with_ops(Command::CanvasSize {
			width: 300,
			height: 250,
			anchor: Anchor9::TopLeft,
		});
		assert_eq!(f.matrix(id), [1.0, 0.0, 0.0, 1.0, -50.0, -50.0], "an anchored canvas grows on one side");
		assert_eq!((f.cache(id).width(), f.cache(id).height()), (300, 250));
	}

	#[test]
	fn a_shape_layer_straightens_with_a_cropped_canvas() {
		let mut f = Fixture::new();
		let id = f.add_shape(
			VectorShape::Rect {
				w: 100.0,
				h: 50.0,
				radii: [0.0; 4],
			},
			[1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
		);
		f.ok_with_ops(Command::Crop {
			rect: (0, 0, 400, 300),
			angle_deg: 90.0,
			delete_cropped: false,
		});
		// A quarter turn about the crop box's centre (200, 150) maps (x, y) to
		// (350 − y, x − 50): the same mapping the pixels were resampled through.
		let m = f.matrix(id);
		assert!((m[0]).abs() < 1e-9 && (m[1] - 1.0).abs() < 1e-9, "a quarter turn: {m:?}");
		assert!((m[2] + 1.0).abs() < 1e-9 && m[3].abs() < 1e-9, "and no scale: {m:?}");
		assert!((m[4] - 330.0).abs() < 1e-9 && (m[5] + 40.0).abs() < 1e-9, "the placement turns too: {m:?}");
		// A local point of the shape: (100, 50) is the far corner of a 100 × 50
		// rect, and lands where (10 + 100, 20 + 50) = (110, 70) goes — (280, 60).
		let (x, y) = (m[0] * 100.0 + m[2] * 50.0 + m[4], m[1] * 100.0 + m[3] * 50.0 + m[5]);
		assert!((x - 280.0).abs() < 1e-9 && (y - 60.0).abs() < 1e-9, "the far corner lands at ({x}, {y})");
	}

	/// The shape layers of a document, in the document's own order.
	fn shape_layers(doc: &Document) -> Vec<LayerId> {
		let mut ids = Vec::new();
		doc.walk(|layer, _| {
			if matches!(layer.kind, LayerKind::Shape { .. }) {
				ids.push(layer.id);
			}
		});
		ids
	}

	#[test]
	fn a_document_without_shapes_keeps_no_shape_state() {
		let mut f = Fixture::new();
		f.add_pixel("Layer 1");
		f.ok(Command::RotateCanvas { quarter_turns: 2 });
		assert!(shape_layers(&f.doc).is_empty());
	}
}

#[cfg(test)]
mod using_the_selection_tests {
	//! Fill, Clear, Layer via Copy/Cut, masks from the selection and a filter
	//! limited by it (M5-T05), through the commands.

	use fx_tiles::{TILE_SIZE, TileStoreConfig};

	use super::*;
	use crate::color::{BitDepth, ColorProfile, DocumentColor};
	use crate::history::History;

	struct Ops;

	impl PixelOps for Ops {
		fn filter(&self, image: &TiledImage, _: (i32, i32), _: (u32, u32), _: &FilterParams, _: &TileStore) -> Result<TiledImage, CommandError> {
			// "Filtered" = every tile painted solid green.
			let mut out = image.clone();
			for ty in 0..image.grid(0).rows() {
				for tx in 0..image.grid(0).cols() {
					out.set_slot(tx, ty, TileSlot::Solid(PixelValue([0, 65535, 0, 65535])));
				}
			}
			Ok(out)
		}
		fn composite(&self, _: &Document, _: &[LayerId], _: Option<[u16; 4]>, _: &TileStore) -> Result<TiledImage, CommandError> {
			unreachable!()
		}
		fn convert(&self, _: &TiledImage, _: &crate::ops::Conversion<'_>, _: &TileStore) -> Result<TiledImage, CommandError> {
			unreachable!()
		}
		fn convert_color(&self, _: [u16; 4], _: &crate::ops::Conversion<'_>) -> Result<[u16; 4], CommandError> {
			unreachable!()
		}
		fn rasterise(&self, shape: &SelectionShape, size: (u32, u32), depth: BitDepth, _: bool, store: &TileStore) -> Result<Selection, CommandError> {
			let SelectionShape::Rect { x, y, w, h } = shape else { unreachable!() };
			let mut selection = Selection::empty(size, depth);
			for ty in 0..size.1.div_ceil(TILE_SIZE) {
				for tx in 0..size.0.div_ceil(TILE_SIZE) {
					let mut buffer = TileBuffer::zeroed(depth.gray_format());
					for py in 0..TILE_SIZE {
						for px in 0..TILE_SIZE {
							let (cx, cy) = (f64::from(tx * TILE_SIZE + px), f64::from(ty * TILE_SIZE + py));
							if cx >= *x && cx < x + w && cy >= *y && cy < y + h {
								selection::set_gray(&mut buffer, depth.gray_format(), px, py, 1.0);
							}
						}
					}
					selection.image.put_buffer(store, tx, ty, buffer);
				}
			}
			Ok(selection)
		}
		fn modify_selection(&self, s: &Selection, _: &SelectModify, _: (u32, u32), _: BitDepth, _: &TileStore) -> Result<Option<Selection>, CommandError> {
			Ok(Some(s.clone()))
		}
		fn magic_wand(&self, _: &Document, _: &WandParams, _: &TileStore) -> Result<Option<Selection>, CommandError> {
			Ok(None)
		}
		fn clipboard(&self) -> Option<crate::pixels::ClipboardImage> {
			let mut image = TiledImage::new(300, 300, PixelFormat::Rgba16);
			image.set_slot(0, 0, TileSlot::Solid(PixelValue([100, 200, 300, 65535])));
			Some(crate::pixels::ClipboardImage {
				image,
				offset: (10, 20),
				bounds: (10, 20, 266, 276),
			})
		}
	}

	struct F {
		doc: Document,
		store: TileStore,
		history: History,
	}

	impl F {
		fn new() -> Self {
			let dir = std::env::temp_dir().join("fx-core-using-selection-tests");
			std::fs::create_dir_all(&dir).unwrap();
			Self {
				doc: Document::new(
					600,
					400,
					DocumentColor {
						depth: BitDepth::U16,
						profile: ColorProfile::Srgb,
					},
					72.0,
				),
				store: TileStore::new(TileStoreConfig::for_tests(dir)).unwrap(),
				history: History::default(),
			}
		}

		fn run(&mut self, command: Command) -> Result<CommandEffect, CommandError> {
			let mut ctx = CommandContext {
				tiles: &self.store,
				ops: Some(&Ops),
			};
			self.history.execute(&mut self.doc, command, &mut ctx)
		}

		fn ok(&mut self, command: Command) -> CommandEffect {
			self.run(command.clone()).unwrap_or_else(|e| panic!("{command:?}: {e}"))
		}

		fn layer(&mut self) -> LayerId {
			self.ok(Command::AddLayer {
				layer: NewLayer::Pixel,
				name: None,
			});
			self.doc.active_layer().unwrap()
		}

		fn select(&mut self, x: f64, y: f64, w: f64, h: f64) {
			self.ok(Command::Select {
				shape: SelectionShape::Rect { x, y, w, h },
				mode: SelectMode::Replace,
				feather: 0.0,
				anti_alias: true,
			});
		}

		/// Straight RGBA16 of `layer` at canvas pixel `(x, y)`.
		fn px(&self, layer: LayerId, x: i32, y: i32) -> [u16; 4] {
			let LayerKind::Pixel { image, offset } = &self.doc.layer(layer).unwrap().kind else {
				panic!()
			};
			let (lx, ly) = (x - offset.0, y - offset.1);
			if lx < 0 || ly < 0 || lx as u32 >= image.width() || ly as u32 >= image.height() {
				return [0; 4];
			}
			let (lx, ly) = (lx as u32, ly as u32);
			match image.slot(0, lx / TILE_SIZE, ly / TILE_SIZE) {
				TileSlot::Empty => [0; 4],
				TileSlot::Solid(v) => v.0,
				TileSlot::Data(h) => {
					let b = self.store.get(h).unwrap();
					let i = (((ly % TILE_SIZE) * TILE_SIZE + lx % TILE_SIZE) * 4) as usize;
					let s = &b.as_u16()[i..i + 4];
					[s[0], s[1], s[2], s[3]]
				}
			}
		}
	}

	fn red_fill(layer: LayerId) -> Command {
		Command::Fill {
			layer: LayerRef::Id(layer),
			color: [65535, 0, 0, 65535],
			mode: BlendMode::Normal,
			opacity: 1.0,
			preserve_transparency: false,
		}
	}

	#[test]
	fn fill_paints_the_selection_and_undoes() {
		let mut f = F::new();
		let a = f.layer();
		f.select(100.0, 100.0, 50.0, 50.0);
		assert_eq!(f.ok(red_fill(a)).label, "Fill");
		assert_eq!(f.px(a, 120, 120), [65535, 0, 0, 65535]);
		assert_eq!(f.px(a, 90, 120), [0; 4], "outside the selection");
		f.history.undo(&mut f.doc);
		assert_eq!(f.px(a, 120, 120), [0; 4]);
		// Without a selection: the whole canvas.
		f.ok(Command::Deselect);
		f.ok(red_fill(a));
		assert_eq!(f.px(a, 599, 399), [65535, 0, 0, 65535]);
	}

	#[test]
	fn preserve_transparency_leaves_empty_pixels_empty() {
		let mut f = F::new();
		let a = f.layer();
		f.select(0.0, 0.0, 10.0, 10.0);
		f.ok(red_fill(a));
		f.ok(Command::Deselect);
		f.ok(Command::Fill {
			layer: LayerRef::Id(a),
			color: [0, 0, 65535, 65535],
			mode: BlendMode::Normal,
			opacity: 1.0,
			preserve_transparency: true,
		});
		assert_eq!(f.px(a, 5, 5), [0, 0, 65535, 65535], "painted pixels take the colour");
		assert_eq!(f.px(a, 50, 50)[3], 0, "transparent pixels stay transparent");
	}

	#[test]
	fn clear_and_cut_remove_the_selected_pixels() {
		let mut f = F::new();
		let a = f.layer();
		f.ok(red_fill(a));
		f.select(0.0, 0.0, 300.0, 100.0);
		assert_eq!(
			f.ok(Command::Clear {
				layer: LayerRef::Id(a),
				cut: false
			})
			.label,
			"Clear"
		);
		assert_eq!(f.px(a, 10, 10)[3], 0);
		assert_eq!(f.px(a, 10, 150)[3], 65535);
		f.ok(Command::Deselect);
		let error = f
			.run(Command::Clear {
				layer: LayerRef::Id(a),
				cut: true,
			})
			.unwrap_err();
		assert!(error.to_string().contains("nothing is selected"));
	}

	#[test]
	fn layer_via_copy_and_cut_lift_the_selected_pixels() {
		let mut f = F::new();
		let a = f.layer();
		f.ok(red_fill(a));
		f.select(200.0, 100.0, 64.0, 32.0);
		assert_eq!(f.ok(Command::LayerViaCopy { cut: false }).label, "Layer Via Copy");
		let copy = f.doc.active_layer().unwrap();
		assert_ne!(copy, a);
		assert_eq!(f.px(copy, 210, 110), [65535, 0, 0, 65535]);
		assert_eq!(f.px(copy, 190, 110)[3], 0, "only the selection");
		assert_eq!(f.px(a, 210, 110)[3], 65535, "copy leaves the source");
		f.doc.selected = vec![a];
		assert_eq!(f.ok(Command::LayerViaCopy { cut: true }).label, "Layer Via Cut");
		assert_eq!(f.px(a, 210, 110)[3], 0, "cut clears the source");
		assert_eq!(f.doc.layers.len(), 3);
	}

	#[test]
	fn masks_reveal_or_hide_the_selection() {
		let mut f = F::new();
		let a = f.layer();
		f.select(0.0, 0.0, 256.0, 400.0);
		f.ok(Command::AddMask {
			layer: LayerRef::Id(a),
			fill: MaskFill::HideSelection,
		});
		let mask = f.doc.layer(a).unwrap().mask.as_ref().unwrap();
		assert!(matches!(mask.image.slot(0, 0, 0), TileSlot::Empty), "hidden where selected");
		assert!(matches!(mask.image.slot(0, 1, 0), TileSlot::Solid(_)), "revealed elsewhere");
	}

	#[test]
	fn a_filter_shows_only_through_the_selection() {
		let mut f = F::new();
		let a = f.layer();
		f.ok(red_fill(a));
		f.select(0.0, 0.0, 100.0, 100.0);
		f.ok(Command::ApplyFilter {
			layer: LayerRef::Id(a),
			filter: FilterParams::GaussianBlur { radius: 1.0 },
		});
		assert_eq!(f.px(a, 50, 50), [0, 65535, 0, 65535], "filtered inside");
		assert_eq!(f.px(a, 150, 50), [65535, 0, 0, 65535], "untouched outside");
	}

	#[test]
	fn paste_centres_the_clipboard_or_keeps_its_place() {
		let mut f = F::new();
		f.layer();
		assert_eq!(f.ok(Command::Paste { in_place: true, center: None }).label, "Paste in Place");
		let pasted = f.doc.active_layer().unwrap();
		assert_eq!(f.px(pasted, 10, 20), [100, 200, 300, 65535]);
		f.ok(Command::Paste {
			in_place: false,
			center: Some((300.0, 200.0)),
		});
		let centred = f.doc.active_layer().unwrap();
		// Bounds (10, 20)–(266, 276) centred on (300, 200): moved by (162, 52).
		assert_eq!(f.px(centred, 172, 72), [100, 200, 300, 65535]);
		assert_eq!(f.px(centred, 171, 72)[3], 0);
	}
}
