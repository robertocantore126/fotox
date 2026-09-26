//! Layer Comps (M12-T06): named snapshots of the layers' visibility,
//! position and appearance, re-applied on demand.

use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::document::Document;
use crate::layer::{LayerId, LayerKind};
use crate::transform::Mapping;

/// What one layer looked like when the comp was recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompState {
	pub layer: LayerId,
	pub visible: bool,
	/// A pixel layer's offset.
	#[serde(default)]
	pub offset: Option<(i32, i32)>,
	/// A shape / text layer's matrix.
	#[serde(default)]
	pub matrix: Option<[f64; 6]>,
	/// A Smart Object's transform.
	#[serde(default)]
	pub mapping: Option<Mapping>,
	pub opacity: f32,
	pub fill: f32,
	pub blend: BlendMode,
	#[serde(default)]
	pub styles: Option<crate::styles::LayerStyles>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerComp {
	pub name: String,
	#[serde(default)]
	pub comment: String,
	/// Which aspects the comp applies.
	pub visibility: bool,
	pub position: bool,
	pub appearance: bool,
	pub states: Vec<CompState>,
}

impl LayerComp {
	/// Record the document as it is now.
	pub fn capture(doc: &Document, name: String, visibility: bool, position: bool, appearance: bool) -> Self {
		let mut states = Vec::new();
		doc.walk(|layer, _| {
			states.push(CompState {
				layer: layer.id,
				visible: layer.visible,
				offset: match &layer.kind {
					LayerKind::Pixel { offset, .. } => Some(*offset),
					_ => None,
				},
				matrix: layer.kind.transform(),
				mapping: match &layer.kind {
					LayerKind::Smart { smart, .. } => Some(smart.transform),
					_ => None,
				},
				opacity: layer.opacity,
				fill: layer.fill,
				blend: layer.blend,
				styles: layer.styles.clone(),
			});
		});
		Self {
			name,
			comment: String::new(),
			visibility,
			position,
			appearance,
			states,
		}
	}
}
