//! [`Document`] → protocol layer list: the data the Layers panel draws.
//!
//! `fx-core` stores a document **bottom → top** (the order the compositor
//! blends in): `Document::layers` runs from the bottom-most root layer to the
//! top-most, and a group's `children` likewise. The Layers panel draws the
//! other way round, so every sibling list is walked back to front here. A
//! group is emitted *before* its children (the panel shows the group row above
//! them), which is what makes the flat list + `depth` a complete tree.
//!
//! Only `Layer`'s structure, names and flags are read: no tile is touched, so
//! this is O(layers), not O(pixels) — safe to call for every `layers` message
//! on a 30 000² document.

use std::sync::Arc;

use fx_core::{Document, Layer, LayerId, LayerKind};
use fx_protocol::{LayerInfo, LayerInfoKind};

/// Flatten `doc` into [`LayerInfo`]s in Layers-panel order, **top → bottom**,
/// each group immediately followed by its children (docs/PROTOCOL.md §5).
///
/// `depth` is the group nesting level: `0` for root layers, `+1` per group.
/// `selected` mirrors [`Document::selected`]. The list length equals the
/// number of layers, so no buffer proportional to the document *pixels* is
/// ever allocated.
pub fn layer_infos(doc: &Document) -> Vec<LayerInfo> {
	let mut out = Vec::new();
	flatten(&doc.layers, 0, &doc.selected, &mut out);
	out
}

/// Append `layers` (bottom → top) top-first, then recurse into groups.
fn flatten(layers: &[Arc<Layer>], depth: u32, selected: &[LayerId], out: &mut Vec<LayerInfo>) {
	for layer in layers.iter().rev() {
		out.push(layer_info(layer, depth, selected.contains(&layer.id)));
		if let LayerKind::Group { children, .. } = &layer.kind {
			flatten(children, depth + 1, selected, out);
		}
	}
}

fn layer_info(layer: &Layer, depth: u32, selected: bool) -> LayerInfo {
	LayerInfo {
		id: layer.id,
		name: layer.name.clone(),
		kind: layer_kind(&layer.kind),
		depth,
		visible: layer.visible,
		opacity: layer.opacity,
		fill: layer.fill,
		blend: layer.blend,
		clipped: layer.clipped,
		// A mask that is present but disabled still gets a row indicator.
		has_mask: layer.mask.is_some(),
		// The protocol carries one lock flag for `fx-core`'s two locks
		// (`locked_pixels`, `locked_position`); it reads as "locked in some
		// way", like Photoshop's row lock icon. See the report.
		locked: layer.locked_pixels || layer.locked_position || layer.locked_transparency,
		locked_pixels: layer.locked_pixels,
		locked_transparency: layer.locked_transparency,
		edit_mask: false,
		locked_position: layer.locked_position,
		// Only groups can collapse; for anything else the flag means nothing.
		expanded: matches!(&layer.kind, LayerKind::Group { expanded: true, .. }),
		selected,
		adjustment: match &layer.kind {
			LayerKind::Adjustment(adjustment) => Some(adjustment.clone()),
			_ => None,
		},
		fill_color: match &layer.kind {
			LayerKind::SolidFill { rgba } => Some(*rgba),
			// A shape layer shows its fill in the panel, so its row reads like
			// a fill layer's until the thumbnail arrives (M6-T06).
			LayerKind::Shape { fill, .. } => fill.map(|paint| paint.rgba()),
			_ => None,
		},
		styles: layer.styles.clone(),
		fill_layer: match &layer.kind {
			LayerKind::FillLayer { content, .. } => Some(content.clone()),
			_ => None,
		},
	}
}

fn layer_kind(kind: &LayerKind) -> LayerInfoKind {
	match kind {
		LayerKind::Pixel { .. } => LayerInfoKind::Pixel,
		LayerKind::Group { .. } => LayerInfoKind::Group,
		LayerKind::Adjustment(_) => LayerInfoKind::Adjustment,
		LayerKind::SolidFill { .. } => LayerInfoKind::SolidFill,
		LayerKind::Shape { .. } => LayerInfoKind::Shape,
		LayerKind::Text { .. } => LayerInfoKind::Text,
		LayerKind::FillLayer { .. } => LayerInfoKind::FillLayer,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_core::{Adjustment, BitDepth, BlendMode, ColorProfile, DocumentColor};
	use fx_tiles::{PixelFormat, TiledImage};

	fn pixel_layer(id: u64, name: &str) -> Arc<Layer> {
		Arc::new(Layer::new(
			LayerId(id),
			name,
			LayerKind::Pixel {
				image: TiledImage::new(100, 80, PixelFormat::Rgba16),
				offset: (0, 0),
			},
		))
	}

	fn group(id: u64, name: &str, expanded: bool, children: Vec<Arc<Layer>>) -> Arc<Layer> {
		Arc::new(Layer::new(LayerId(id), name, LayerKind::Group { children, expanded }))
	}

	fn solid(id: u64, name: &str) -> Arc<Layer> {
		Arc::new(Layer::new(LayerId(id), name, LayerKind::SolidFill { rgba: [0, 0, 0, 65535] }))
	}

	fn adjustment(id: u64, name: &str) -> Arc<Layer> {
		Arc::new(Layer::new(LayerId(id), name, LayerKind::Adjustment(Adjustment::Invert)))
	}

	fn new_doc() -> Document {
		Document::new(
			100,
			80,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			72.0,
		)
	}

	/// Bottom → top: `A`, `Group( B, Inner( Curves ) )`, `Pixel`.
	fn doc() -> Document {
		let mut doc = new_doc();
		let inner = group(4, "Inner", false, vec![adjustment(5, "Curves")]);
		doc.layers = vec![solid(1, "A"), group(2, "Group", true, vec![solid(3, "B"), inner]), pixel_layer(6, "Pixel")];
		doc
	}

	fn ids(infos: &[LayerInfo]) -> Vec<u64> {
		infos.iter().map(|i| i.id.0).collect()
	}

	#[test]
	fn panel_order_is_top_to_bottom_with_depths() {
		// Panel draws the top root layer first, then its children, down to the
		// bottom root layer; a nested group is one level deeper again.
		let infos = layer_infos(&doc());
		assert_eq!(ids(&infos), [6, 2, 4, 5, 3, 1]);
		assert_eq!(
			infos.iter().map(|i| i.depth).collect::<Vec<_>>(),
			[0, 0, 1, 2, 1, 0],
			"depth = group nesting level"
		);
	}

	#[test]
	fn every_layer_kind_maps_to_its_protocol_kind() {
		let infos = layer_infos(&doc());
		let kind_of = |id: u64| infos.iter().find(|i| i.id == LayerId(id)).unwrap().kind.clone();
		assert_eq!(kind_of(1), LayerInfoKind::SolidFill);
		assert_eq!(kind_of(2), LayerInfoKind::Group);
		assert_eq!(kind_of(5), LayerInfoKind::Adjustment);
		assert_eq!(kind_of(6), LayerInfoKind::Pixel);
	}

	#[test]
	fn flags_and_properties_come_across() {
		let mut doc = doc();
		let layer = doc.layer_mut(LayerId(1)).unwrap();
		layer.visible = false;
		layer.opacity = 0.5;
		layer.fill = 0.25;
		layer.blend = BlendMode::Multiply;
		layer.clipped = true;
		layer.locked_position = true;
		layer.mask = Some(fx_core::Mask {
			image: TiledImage::new(100, 80, PixelFormat::Gray16),
			enabled: false,
			linked: true,
			outside_value: 0,
		});

		let infos = layer_infos(&doc);
		let a = &infos[5]; // bottom-most layer, last in panel order
		assert_eq!(a.id, LayerId(1));
		assert_eq!(a.name, "A");
		assert!(!a.visible);
		assert_eq!(a.opacity, 0.5);
		assert_eq!(a.fill, 0.25);
		assert_eq!(a.blend, BlendMode::Multiply);
		assert!(a.clipped);
		assert!(a.has_mask, "a disabled mask is still shown in the row");
		assert!(a.locked, "a position lock alone lights the lock flag");
		assert!(!a.expanded, "expanded is a group-only flag");

		assert!(infos.iter().find(|i| i.id == LayerId(5)).unwrap().name == "Curves");
	}

	#[test]
	fn pixel_lock_also_sets_locked() {
		let mut doc = doc();
		doc.layer_mut(LayerId(6)).unwrap().locked_pixels = true;
		let infos = layer_infos(&doc);
		assert!(infos[0].locked);
		assert!(!infos[1].locked);
	}

	#[test]
	fn expanded_comes_from_the_group_only() {
		let infos = layer_infos(&doc());
		let expanded = |id: u64| infos.iter().find(|i| i.id == LayerId(id)).unwrap().expanded;
		assert!(expanded(2), "a group with expanded = true");
		assert!(!expanded(4), "a group with expanded = false");
		assert!(!expanded(1) && !expanded(6), "non-groups never expand");
	}

	#[test]
	fn selection_mirrors_the_document() {
		let mut doc = doc();
		doc.selected = vec![LayerId(3), LayerId(6)];
		let infos = layer_infos(&doc);
		let selected = infos.iter().filter(|i| i.selected).map(|i| i.id.0).collect::<Vec<_>>();
		assert_eq!(selected, [6, 3], "every selected layer is flagged, order is panel order");
	}

	#[test]
	fn selection_of_a_deleted_layer_is_ignored() {
		let mut doc = doc();
		doc.selected = vec![LayerId(99)];
		assert!(layer_infos(&doc).iter().all(|i| !i.selected));
	}

	#[test]
	fn empty_document_yields_no_layers() {
		assert!(layer_infos(&new_doc()).is_empty());
	}

	#[test]
	fn group_without_children_is_a_single_row() {
		let mut doc = new_doc();
		doc.layers = vec![group(1, "Empty group", true, Vec::new()), solid(2, "B")];
		let infos = layer_infos(&doc);
		assert_eq!(ids(&infos), [2, 1]);
		assert_eq!(infos[1].depth, 0);
	}
}
