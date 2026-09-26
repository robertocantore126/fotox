//! M12's layer-model commands: Smart Objects (T01..T03).

use super::*;
use crate::smart::{SmartFilter, SmartObject, SmartSource, new_uid};

/// `outer ∘ inner` for affine / projective mappings (`None` for a warp or a
/// custom mesh: FAST, those are refused on a Smart Object).
pub fn compose(outer: Mapping, inner: Mapping) -> Option<Mapping> {
	fn matrix(m: Mapping) -> Option<[f64; 9]> {
		match m {
			Mapping::Affine([a, b, c, d, e, f]) => Some([a, c, e, b, d, f, 0.0, 0.0, 1.0]),
			Mapping::Projective(m) => Some(m),
			_ => None,
		}
	}
	let (a, b) = (matrix(outer)?, matrix(inner)?);
	let mut m = [0.0; 9];
	for r in 0..3 {
		for c in 0..3 {
			m[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
		}
	}
	if m[6] == 0.0 && m[7] == 0.0 && m[8] != 0.0 {
		let s = m[8];
		return Some(Mapping::Affine([m[0] / s, m[3] / s, m[1] / s, m[4] / s, m[2] / s, m[5] / s]));
	}
	Some(Mapping::Projective(m))
}

/// A Smart Object layer around `source` (identity transform).
pub fn smart_layer(doc: &mut Document, name: String, source: SmartSource, transform: Mapping) -> Arc<Layer> {
	let id = doc.allocate_layer_id();
	Arc::new(Layer::new(
		id,
		name,
		LayerKind::Smart {
			smart: SmartObject {
				source,
				transform,
				filters: Vec::new(),
				filters_enabled: true,
			},
			cache: TiledImage::derived(doc.width, doc.height, doc.color.depth.rgba_format()),
		},
	))
}

/// Layer ▸ Smart Objects ▸ Convert to Smart Object (M12-T01): the layers
/// become the embedded document of one Smart Object, in place of the top one.
/// FAST: the nested document has the parent's canvas (not the layers' union).
pub(super) fn convert_to_smart(doc: &mut Document, layers: &[LayerRef], ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let ids = resolve_all(doc, layers, true)?;
	if ids.is_empty() {
		return Err(CommandError::NotAllowed("select the layers to convert".into()));
	}
	let ops = pixel_ops(ctx, "Convert to Smart Object")?;
	// Bottom → top, as a document stores them.
	let mut order: Vec<LayerId> = doc.panel_order().into_iter().filter(|id| ids.contains(id)).collect();
	order.reverse();
	let top = *order.last().expect("not empty");
	let mut nested = Document::new(doc.width, doc.height, doc.color.clone(), doc.ppi);
	let (next_id, counters) = doc.id_state();
	nested = nested.with_id_state(next_id, counters);
	nested.layers = order.iter().filter_map(|id| find_arc(&doc.layers, *id)).collect();
	nested.selected = vec![top];
	let composite = ops.composite(&nested, &order, None, ctx.tiles)?;
	let name = doc.layer(top).map(|l| l.name.clone()).unwrap_or_default();
	let source = SmartSource {
		doc: Arc::new(nested),
		composite,
		linked: None,
		linked_mtime: None,
		uid: new_uid(),
	};
	let layer = smart_layer(doc, name, source, Mapping::identity());
	let new_id = layer.id;
	let path = doc.path_of(top).expect("resolved");
	let parent = id_at_path(doc, &path[..path.len() - 1]);
	children_mut(doc, parent)[path[path.len() - 1]] = layer;
	for id in order.iter().filter(|id| **id != top) {
		remove_layer(doc, *id);
	}
	doc.selected = vec![new_id];
	Ok(CommandEffect {
		label: "Convert to Smart Object".into(),
		structure_changed: true,
		..Default::default()
	})
}

fn find_arc(layers: &[Arc<Layer>], id: LayerId) -> Option<Arc<Layer>> {
	for layer in layers {
		if layer.id == id {
			return Some(layer.clone());
		}
		if let LayerKind::Group { children, .. } = &layer.kind
			&& let Some(found) = find_arc(children, id)
		{
			return Some(found);
		}
	}
	None
}

/// Free Transform of a Smart Object (M12-T01): only the transform changes.
pub(super) fn transform_smart(doc: &mut Document, id: LayerId, mapping: Mapping) -> Result<CommandEffect, CommandError> {
	let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
	let layer = doc.layer_mut(id).expect("resolved");
	if layer.locked_position {
		return Err(CommandError::Locked(id));
	}
	let LayerKind::Smart { smart, cache } = &mut layer.kind else {
		return Err(CommandError::NotAllowed("not a Smart Object".into()));
	};
	let Some(new) = compose(mapping, smart.transform) else {
		return Err(CommandError::NotAllowed(
			"this warp cannot be applied to a Smart Object yet; rasterise it first".into(),
		));
	};
	smart.transform = new;
	*cache = TiledImage::derived(w, h, format);
	Ok(CommandEffect {
		label: "Free Transform".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

/// Layer ▸ Smart Objects ▸ New Smart Object via Copy: a duplicate with its
/// own source (Duplicate Layer shares it).
pub(super) fn new_smart_via_copy(doc: &mut Document, layer: &LayerRef) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let original = doc.layer(id).expect("resolved").clone();
	let LayerKind::Smart { smart, .. } = &original.kind else {
		return Err(CommandError::NotAllowed("the layer is not a Smart Object".into()));
	};
	let mut source = smart.source.clone();
	source.uid = new_uid();
	let mut copy = (*smart_layer(doc, format!("{} copy", original.name), source, smart.transform)).clone();
	if let LayerKind::Smart { smart: s, .. } = &mut copy.kind {
		s.filters = smart.filters.clone();
		s.filters_enabled = smart.filters_enabled;
	}
	copy.opacity = original.opacity;
	copy.fill = original.fill;
	copy.blend = original.blend;
	let new_id = copy.id;
	doc.selected = vec![id];
	insert_above_active(doc, Arc::new(copy));
	doc.selected = vec![new_id];
	Ok(CommandEffect {
		label: "New Smart Object via Copy".into(),
		structure_changed: true,
		..Default::default()
	})
}

/// Smart Filters (M12-T03): the whole list replaced (add, edit, toggle,
/// reorder, delete all go through this), and the stack's eye.
pub(super) fn set_smart_filters(
	doc: &mut Document,
	layer: &LayerRef,
	filters: &[SmartFilter],
	enabled: bool,
	label: &str,
) -> Result<CommandEffect, CommandError> {
	let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
	let id = resolve(doc, layer)?;
	let target = doc.layer_mut(id).expect("resolved");
	let LayerKind::Smart { smart, cache } = &mut target.kind else {
		return Err(CommandError::NotAllowed(
			"Smart Filters need a Smart Object: Filter ▸ Convert for Smart Filters".into(),
		));
	};
	smart.filters = filters.to_vec();
	smart.filters_enabled = enabled;
	*cache = TiledImage::derived(w, h, format);
	Ok(CommandEffect {
		label: label.into(),
		pixels_changed: vec![id],
		props_changed: vec![id],
		..Default::default()
	})
}
