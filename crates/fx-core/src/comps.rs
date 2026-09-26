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

/// A user slice (M12-T08): a named canvas rectangle exported as one image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slice {
	pub name: String,
	/// `(x, y, width, height)`, document pixels.
	pub rect: (i32, i32, u32, u32),
}

/// The auto slices that fill the canvas around the user slices (Photoshop's
/// grey slices): the grid of every user-slice edge, minus the covered cells.
/// FAST: cells are not merged into bigger rectangles.
pub fn auto_slices(slices: &[Slice], size: (u32, u32)) -> Vec<(i32, i32, u32, u32)> {
	let (w, h) = (size.0 as i32, size.1 as i32);
	let mut xs = vec![0, w];
	let mut ys = vec![0, h];
	for s in slices {
		xs.extend([s.rect.0.clamp(0, w), (s.rect.0 + s.rect.2 as i32).clamp(0, w)]);
		ys.extend([s.rect.1.clamp(0, h), (s.rect.1 + s.rect.3 as i32).clamp(0, h)]);
	}
	xs.sort_unstable();
	xs.dedup();
	ys.sort_unstable();
	ys.dedup();
	let mut out = Vec::new();
	for y in ys.windows(2) {
		for x in xs.windows(2) {
			let (cx, cy) = ((x[0] + x[1]) / 2, (y[0] + y[1]) / 2);
			let covered = slices
				.iter()
				.any(|s| cx >= s.rect.0 && cy >= s.rect.1 && cx < s.rect.0 + s.rect.2 as i32 && cy < s.rect.1 + s.rect.3 as i32);
			if !covered && x[1] > x[0] && y[1] > y[0] {
				out.push((x[0], y[0], (x[1] - x[0]) as u32, (y[1] - y[0]) as u32));
			}
		}
	}
	out
}
