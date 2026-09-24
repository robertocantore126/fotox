use std::sync::Arc;

use fx_tiles::TiledImage;

use crate::color::DocumentColor;
use crate::layer::{Adjustment, Layer, LayerId, LayerKind};

/// An open image. Cheap to clone: see crate docs.
#[derive(Clone, Debug)]
pub struct Document {
	pub width: u32,
	pub height: u32,
	pub color: DocumentColor,
	/// Pixels per inch, metadata only (print size).
	pub ppi: f32,
	/// Root layers, bottom → top.
	pub layers: Vec<Arc<Layer>>,
	/// Selected layers in the Layers panel; the last one is the "active" layer.
	pub selected: Vec<LayerId>,
	/// Pixel selection (Gray, document size). `None` = nothing selected. (M5)
	pub selection: Option<TiledImage>,
	/// Incremented by every applied command. Used for cache keys and UI sync.
	pub revision: u64,
	next_id: u64,
	/// How many layers of each kind this document has created, for the
	/// Photoshop-style default names (`"Layer 1"`, `"Group 2"`, `"Curves 1"`).
	name_counters: [u32; NameKind::COUNT],
}

/// Counter key for default layer names: one counter per key, per document.
///
/// Adjustment layers count per adjustment type, like Photoshop (`"Curves 1"`,
/// `"Levels 1"`), so a document can have a `Curves 1` and a `Levels 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameKind {
	Pixel,
	SolidFill,
	Group,
	BrightnessContrast,
	Levels,
	Curves,
	Exposure,
	HueSaturation,
	Invert,
}

impl NameKind {
	const COUNT: usize = 9;

	/// The name Photoshop gives the first layer of this kind; the counter is
	/// appended ("Curves 1").
	pub(crate) fn stem(self) -> &'static str {
		match self {
			NameKind::Pixel => "Layer",
			NameKind::SolidFill => "Color Fill",
			NameKind::Group => "Group",
			NameKind::BrightnessContrast => "Brightness/Contrast",
			NameKind::Levels => "Levels",
			NameKind::Curves => "Curves",
			NameKind::Exposure => "Exposure",
			NameKind::HueSaturation => "Hue/Saturation",
			NameKind::Invert => "Invert",
		}
	}

	pub(crate) fn of_adjustment(adjustment: &Adjustment) -> Self {
		match adjustment {
			Adjustment::BrightnessContrast { .. } => NameKind::BrightnessContrast,
			Adjustment::Levels { .. } => NameKind::Levels,
			Adjustment::Curves { .. } => NameKind::Curves,
			Adjustment::Exposure { .. } => NameKind::Exposure,
			Adjustment::HueSaturation { .. } => NameKind::HueSaturation,
			Adjustment::Invert => NameKind::Invert,
		}
	}

	fn index(self) -> usize {
		self as usize
	}
}

impl Document {
	/// A document with no layers.
	pub fn new(width: u32, height: u32, color: DocumentColor, ppi: f32) -> Self {
		Self {
			width,
			height,
			color,
			ppi,
			layers: Vec::new(),
			selected: Vec::new(),
			selection: None,
			revision: 0,
			next_id: 1,
			name_counters: [0; NameKind::COUNT],
		}
	}

	pub fn allocate_layer_id(&mut self) -> LayerId {
		let id = LayerId(self.next_id);
		self.next_id += 1;
		id
	}

	pub fn active_layer(&self) -> Option<LayerId> {
		self.selected.last().copied()
	}

	/// The next Photoshop-style default name for a layer of `kind` (`"Layer 3"`).
	///
	/// The counters live in the document, so numbering is per document and is
	/// restored by undo along with everything else. A counter counts the layers
	/// of its kind that were *created*, whether or not the command supplied its
	/// own name — Photoshop keeps counting too (`Add Layer`, rename, `Add Layer`
	/// gives `Layer 2`).
	pub(crate) fn next_default_name(&mut self, kind: NameKind) -> String {
		let counter = &mut self.name_counters[kind.index()];
		// Layers are bounded by memory long before u32 wraps; saturate rather
		// than wrap so a name can never repeat.
		*counter = counter.saturating_add(1);
		format!("{} {}", kind.stem(), counter)
	}

	/// Layer ids in the order the Layers panel draws them: **top → bottom**, a
	/// group immediately followed by its children (the flat list + `LayerInfo`
	/// `depth` is the tree, docs/PROTOCOL.md §5). The exact reverse of [`walk`].
	///
	/// [`walk`]: Document::walk
	pub fn panel_order(&self) -> Vec<LayerId> {
		fn go(layers: &[Arc<Layer>], out: &mut Vec<LayerId>) {
			for layer in layers.iter().rev() {
				out.push(layer.id);
				if let Some(children) = layer.children() {
					go(children, out);
				}
			}
		}
		let mut out = Vec::new();
		go(&self.layers, &mut out);
		out
	}

	/// Indices from the root to the layer: `[3]` = fourth root layer,
	/// `[3, 0]` = first child of that group.
	pub fn path_of(&self, id: LayerId) -> Option<Vec<usize>> {
		fn search(layers: &[Arc<Layer>], id: LayerId, path: &mut Vec<usize>) -> bool {
			for (i, layer) in layers.iter().enumerate() {
				path.push(i);
				if layer.id == id {
					return true;
				}
				if let Some(children) = layer.children()
					&& search(children, id, path)
				{
					return true;
				}
				path.pop();
			}
			false
		}
		let mut path = Vec::new();
		search(&self.layers, id, &mut path).then_some(path)
	}

	pub fn layer(&self, id: LayerId) -> Option<&Layer> {
		let path = self.path_of(id)?;
		let mut layers = &self.layers;
		let (last, parents) = path.split_last()?;
		for &i in parents {
			layers = match &layers[i].kind {
				LayerKind::Group { children, .. } => children,
				_ => unreachable!("path goes through a non-group"),
			};
		}
		Some(&layers[*last])
	}

	/// Mutable access. Copies-on-write every `Arc` on the path (only those),
	/// so snapshots held elsewhere are unaffected.
	pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
		let path = self.path_of(id)?;
		let (last, parents) = path.split_last()?;
		let mut layers = &mut self.layers;
		for &i in parents {
			layers = match &mut Arc::make_mut(&mut layers[i]).kind {
				LayerKind::Group { children, .. } => children,
				_ => unreachable!("path goes through a non-group"),
			};
		}
		Some(Arc::make_mut(&mut layers[*last]))
	}

	/// The sibling list that contains the layer at `path` (root list for `[i]`).
	pub fn siblings_mut(&mut self, path: &[usize]) -> &mut Vec<Arc<Layer>> {
		let mut layers = &mut self.layers;
		for &i in &path[..path.len().saturating_sub(1)] {
			layers = match &mut Arc::make_mut(&mut layers[i]).kind {
				LayerKind::Group { children, .. } => children,
				_ => panic!("path goes through a non-group"),
			};
		}
		layers
	}

	/// Visit every layer, depth first, bottom → top, with its depth.
	pub fn walk(&self, mut visit: impl FnMut(&Layer, usize)) {
		fn go(layers: &[Arc<Layer>], depth: usize, visit: &mut impl FnMut(&Layer, usize)) {
			for layer in layers {
				visit(layer, depth);
				if let Some(children) = layer.children() {
					go(children, depth + 1, visit);
				}
			}
		}
		go(&self.layers, 0, &mut visit);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::color::{BitDepth, ColorProfile};

	fn doc() -> Document {
		let mut doc = Document::new(
			1000,
			800,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			300.0,
		);
		let a = doc.allocate_layer_id();
		let g = doc.allocate_layer_id();
		let b = doc.allocate_layer_id();
		doc.layers.push(Arc::new(Layer::new(a, "A", LayerKind::SolidFill { rgba: [0, 0, 0, 65535] })));
		let child = Arc::new(Layer::new(b, "B", LayerKind::SolidFill { rgba: [65535; 4] }));
		doc.layers.push(Arc::new(Layer::new(
			g,
			"Group",
			LayerKind::Group {
				children: vec![child],
				expanded: true,
			},
		)));
		doc
	}

	#[test]
	fn find_nested_layer() {
		let doc = doc();
		assert_eq!(doc.path_of(LayerId(3)), Some(vec![1, 0]));
		assert_eq!(doc.layer(LayerId(3)).unwrap().name, "B");
		assert!(doc.layer(LayerId(99)).is_none());
	}

	#[test]
	fn panel_order_is_top_to_bottom_with_children_in_place() {
		let doc = doc();
		// bottom → top: A, Group{ B }; the panel draws Group, B, A.
		assert_eq!(doc.panel_order(), vec![LayerId(2), LayerId(3), LayerId(1)]);
		assert!(empty_doc().panel_order().is_empty());
	}

	#[test]
	fn name_counters_are_per_kind_and_per_document() {
		let mut doc = empty_doc();
		assert_eq!(doc.next_default_name(NameKind::Pixel), "Layer 1");
		assert_eq!(doc.next_default_name(NameKind::Pixel), "Layer 2");
		assert_eq!(doc.next_default_name(NameKind::Group), "Group 1");
		assert_eq!(doc.next_default_name(NameKind::HueSaturation), "Hue/Saturation 1");
		assert_eq!(doc.next_default_name(NameKind::Pixel), "Layer 3");
		// A second document starts over.
		assert_eq!(empty_doc().next_default_name(NameKind::Pixel), "Layer 1");
	}

	#[test]
	fn every_name_kind_has_a_counter() {
		assert_eq!(
			NameKind::Invert as usize,
			NameKind::COUNT - 1,
			"a new NameKind must be added before Invert, or COUNT must grow"
		);
	}

	fn empty_doc() -> Document {
		Document::new(
			10,
			10,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		)
	}

	#[test]
	fn edits_do_not_touch_snapshots() {
		let mut doc = doc();
		let snapshot = doc.clone();
		doc.layer_mut(LayerId(3)).unwrap().name = "renamed".into();
		assert_eq!(snapshot.layer(LayerId(3)).unwrap().name, "B");
		assert_eq!(doc.layer(LayerId(3)).unwrap().name, "renamed");
		// the untouched root layer is still shared
		assert!(Arc::ptr_eq(&snapshot.layers[0], &doc.layers[0]));
	}
}
