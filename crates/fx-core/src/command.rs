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
use crate::document::{Document, NameKind};
use crate::layer::{Adjustment, Layer, LayerId, LayerKind, Mask};
use crate::ops::{FilterParams, PixelOps};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskFill {
	RevealAll,
	HideAll,
	/// From the current pixel selection (M5).
	RevealSelection,
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
	/// Merge the given layers into one pixel layer (rasterises). M4
	MergeLayers { layers: Vec<LayerRef> },
	/// Flatten the whole document into one pixel layer. M4
	Flatten,
	/// Run a destructive filter on a pixel layer (level 0; the engine shows a
	/// live preview first). M4
	ApplyFilter { layer: LayerRef, filter: FilterParams },
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
			Command::AddMask { layer, fill } => add_mask(doc, layer, *fill),
			Command::DeleteMask { layer, apply } => delete_mask(doc, layer, *apply, ctx.tiles),
			Command::SetAdjustment { layer, adjustment } => set_adjustment(doc, layer, adjustment),
			Command::ApplyFilter { layer, filter } => apply_filter(doc, layer, filter, ctx),
			other => todo!("{other:?}: see docs/tasks for the milestone that implements it"),
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

fn add_mask(doc: &mut Document, layer: &LayerRef, fill: MaskFill) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let reveal = match fill {
		MaskFill::RevealAll => true,
		MaskFill::HideAll => false,
		MaskFill::RevealSelection => {
			return Err(CommandError::NotAllowed("a mask from the pixel selection arrives with selections (M5)".into()));
		}
	};
	{
		let target = doc.layer(id).expect("resolved id exists");
		if matches!(target.kind, LayerKind::Group { .. }) {
			return Err(CommandError::NotAllowed("a group cannot have a pixel mask".into()));
		}
		if target.mask.is_some() {
			return Err(CommandError::NotAllowed("the layer already has a mask".into()));
		}
	}
	let mask = Mask {
		image: mask_image(doc.width, doc.height, doc.color.depth.gray_format(), reveal),
		enabled: true,
		// Photoshop links a new mask to its layer.
		linked: true,
		// The mask covers the document, so no pixel is "outside" it; keep the
		// value consistent with the fill anyway.
		outside_value: if reveal { u16::MAX } else { 0 },
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
	let filtered = ops.filter(image, *offset, (doc.width, doc.height), filter, ctx.tiles)?;
	if let LayerKind::Pixel { image, .. } = &mut doc.layer_mut(id).expect("resolved id exists").kind {
		*image = filtered;
	}
	Ok(CommandEffect {
		label: filter.label().into(),
		pixels_changed: vec![id],
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

#[cfg(test)]
mod tests {
	use fx_tiles::{TILE_SIZE, TileStoreConfig};

	use super::*;
	use crate::color::{BitDepth, ColorProfile, DocumentColor};
	use crate::history::History;

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
		assert!(layer.locked_pixels && layer.locked_position);
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
		assert!(matches!(error, CommandError::NotAllowed(_)), "M5: {error:?}");
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
}
