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

use fx_tiles::TileStore;
use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::document::Document;
use crate::layer::{Adjustment, LayerId};

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

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CommandError {
	#[error("layer not found: {0:?}")]
	LayerNotFound(LayerRef),
	#[error("invalid value for {field}: {reason}")]
	InvalidValue { field: &'static str, reason: String },
	#[error("layer {0:?} is locked")]
	Locked(LayerId),
	#[error("operation not valid here: {0}")]
	NotAllowed(String),
}

/// Services a command may need while applying.
pub struct CommandContext<'a> {
	pub tiles: &'a TileStore,
}

impl Command {
	pub fn apply(&self, doc: &mut Document, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
		let _ = &ctx;
		let effect = match self {
			Command::SetLayerProps { layer, props } => {
				let id = resolve(doc, layer)?;
				validate_unit("opacity", props.opacity)?;
				validate_unit("fill", props.fill)?;
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
				CommandEffect {
					label: "Layer Properties".into(),
					props_changed: vec![id],
					..Default::default()
				}
			}
			other => todo!("{other:?}: see docs/tasks for the milestone that implements it"),
		};
		doc.revision += 1;
		Ok(effect)
	}
}

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

fn validate_unit(field: &'static str, value: Option<f32>) -> Result<(), CommandError> {
	match value {
		Some(v) if !(0.0..=1.0).contains(&v) || v.is_nan() => Err(CommandError::InvalidValue {
			field,
			reason: format!("{v} is outside 0..=1"),
		}),
		_ => Ok(()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
}
