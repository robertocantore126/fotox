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

/// What the Layer Comps panel does (M12-T06).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CompAction {
	New {
		name: String,
		visibility: bool,
		position: bool,
		appearance: bool,
	},
	Update {
		index: usize,
	},
	Apply {
		index: usize,
	},
	Delete {
		index: usize,
	},
	Rename {
		index: usize,
		name: String,
	},
}

pub(super) fn layer_comp(doc: &mut Document, action: &CompAction) -> Result<CommandEffect, CommandError> {
	let missing = || CommandError::NotAllowed("there is no such layer comp".into());
	let label = match action {
		CompAction::New {
			name,
			visibility,
			position,
			appearance,
		} => {
			let name = if name.trim().is_empty() {
				format!("Layer Comp {}", doc.comps.len() + 1)
			} else {
				name.clone()
			};
			let comp = crate::comps::LayerComp::capture(doc, name, *visibility, *position, *appearance);
			doc.comps.push(comp);
			doc.active_comp = Some(doc.comps.len() - 1);
			"New Layer Comp"
		}
		CompAction::Update { index } => {
			let old = doc.comps.get(*index).ok_or_else(missing)?.clone();
			let mut comp = crate::comps::LayerComp::capture(doc, old.name, old.visibility, old.position, old.appearance);
			comp.comment = old.comment;
			doc.comps[*index] = comp;
			"Update Layer Comp"
		}
		CompAction::Delete { index } => {
			if *index >= doc.comps.len() {
				return Err(missing());
			}
			doc.comps.remove(*index);
			doc.active_comp = None;
			"Delete Layer Comp"
		}
		CompAction::Rename { index, name } => {
			doc.comps.get_mut(*index).ok_or_else(missing)?.name = name.clone();
			"Rename Layer Comp"
		}
		CompAction::Apply { index } => {
			let comp = doc.comps.get(*index).ok_or_else(missing)?.clone();
			let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
			let mut changed = Vec::new();
			for state in &comp.states {
				let Some(layer) = doc.layer_mut(state.layer) else { continue };
				if comp.visibility {
					layer.visible = state.visible;
				}
				if comp.appearance {
					layer.opacity = state.opacity;
					layer.fill = state.fill;
					layer.blend = state.blend;
					if layer.styles != state.styles {
						layer.styles = state.styles.clone();
						layer.effects = match &layer.styles {
							Some(_) => crate::styles::EffectKind::ALL.iter().map(|_| TiledImage::derived(w, h, format)).collect(),
							None => Vec::new(),
						};
					}
				}
				if comp.position {
					match &mut layer.kind {
						LayerKind::Pixel { offset, .. } => {
							if let Some(o) = state.offset {
								*offset = o;
							}
						}
						LayerKind::Shape { transform, cache, .. } | LayerKind::Text { transform, cache, .. } => {
							if let Some(m) = state.matrix
								&& *transform != m
							{
								*transform = m;
								*cache = TiledImage::derived(w, h, format);
							}
						}
						LayerKind::Smart { smart, cache } => {
							if let Some(m) = state.mapping
								&& smart.transform != m
							{
								smart.transform = m;
								*cache = TiledImage::derived(w, h, format);
							}
						}
						_ => {}
					}
				}
				changed.push(state.layer);
			}
			doc.active_comp = Some(*index);
			return Ok(CommandEffect {
				label: "Apply Layer Comp".into(),
				pixels_changed: changed.clone(),
				props_changed: changed,
				..Default::default()
			});
		}
	};
	Ok(CommandEffect {
		label: label.into(),
		..Default::default()
	})
}

/// A closed rectangle path (an artboard's vector mask).
fn rect_path(rect: (i32, i32, u32, u32)) -> crate::path::Path {
	use crate::path::{Anchor, Path, PathOp, Subpath};
	let (x0, y0) = (f64::from(rect.0), f64::from(rect.1));
	let (x1, y1) = (x0 + f64::from(rect.2), y0 + f64::from(rect.3));
	Path {
		subpaths: vec![Subpath {
			anchors: vec![
				Anchor::corner((x0, y0)),
				Anchor::corner((x1, y0)),
				Anchor::corner((x1, y1)),
				Anchor::corner((x0, y1)),
			],
			closed: true,
			op: PathOp::Combine,
		}],
	}
}

/// Grow the canvas (right / down) so it holds `rect`, resetting every
/// canvas-sized derived cache. FAST: the canvas never shrinks, and an
/// artboard left of / above the origin is not supported (D-085's union).
fn grow_canvas_for(doc: &mut Document, rect: (i32, i32, u32, u32)) -> bool {
	let w = (i64::from(rect.0) + i64::from(rect.2)).max(i64::from(doc.width)) as u32;
	let h = (i64::from(rect.1) + i64::from(rect.3)).max(i64::from(doc.height)) as u32;
	if (w, h) == (doc.width, doc.height) {
		return false;
	}
	set_canvas(doc, w, h);
	let (format, gray) = (doc.color.depth.rgba_format(), crate::selection::gray_format(doc.color.depth));
	for id in layer_ids(doc) {
		let Some(layer) = doc.layer_mut(id) else { continue };
		if let Some(vm) = layer.vector_mask.as_mut() {
			vm.cache = TiledImage::derived(w, h, gray);
		}
		match &mut layer.kind {
			LayerKind::Smart { cache, .. } | LayerKind::FillLayer { cache, .. } => *cache = TiledImage::derived(w, h, format),
			_ => {}
		}
	}
	true
}

fn artboard_mask(doc: &Document, rect: (i32, i32, u32, u32)) -> crate::layer::VectorMask {
	crate::layer::VectorMask {
		path: rect_path(rect),
		enabled: true,
		feather: 0.0,
		density: 1.0,
		cache: TiledImage::derived(doc.width, doc.height, crate::selection::gray_format(doc.color.depth)),
	}
}

/// Layer ▸ New ▸ Artboard / Artboard from Layers, the Artboard tool (M12-T07).
pub(super) fn new_artboard(
	doc: &mut Document,
	rect: (i32, i32, u32, u32),
	name: Option<&str>,
	layers: &[LayerRef],
	background: Option<[u16; 4]>,
) -> Result<CommandEffect, CommandError> {
	if rect.2 == 0 || rect.3 == 0 || rect.0 < 0 || rect.1 < 0 {
		return Err(CommandError::NotAllowed("an artboard needs a size, inside the canvas's top-left".into()));
	}
	let ids = resolve_all(doc, layers, true)?;
	// Bottom → top, as stored.
	let mut order: Vec<LayerId> = doc.panel_order().into_iter().filter(|id| ids.contains(id)).collect();
	order.reverse();
	let children: Vec<Arc<Layer>> = order.iter().filter_map(|id| find_arc(&doc.layers, *id)).collect();
	for id in &order {
		remove_layer(doc, *id);
	}
	grow_canvas_for(doc, rect);
	let count = doc.layers.iter().filter(|l| l.artboard.is_some()).count();
	let id = doc.allocate_layer_id();
	let mut group = Layer::new(
		id,
		name.map_or_else(|| format!("Artboard {}", count + 1), str::to_owned),
		LayerKind::Group { children, expanded: true },
	);
	group.vector_mask = Some(artboard_mask(doc, rect));
	group.artboard = Some(crate::layer::Artboard { rect, background });
	doc.layers.push(Arc::new(group));
	doc.selected = vec![id];
	Ok(CommandEffect {
		label: "New Artboard".into(),
		structure_changed: true,
		..Default::default()
	})
}

/// Move / resize an artboard or change its background; a moved artboard
/// takes its layers along.
pub(super) fn set_artboard(
	doc: &mut Document,
	layer: &LayerRef,
	rect: (i32, i32, u32, u32),
	background: Option<[u16; 4]>,
) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let old = doc
		.layer(id)
		.and_then(|l| l.artboard.clone())
		.ok_or_else(|| CommandError::NotAllowed("the layer is not an artboard".into()))?;
	if rect.2 == 0 || rect.3 == 0 || rect.0 < 0 || rect.1 < 0 {
		return Err(CommandError::NotAllowed("an artboard needs a size, inside the canvas's top-left".into()));
	}
	let (dx, dy) = (rect.0 - old.rect.0, rect.1 - old.rect.1);
	if dx != 0 || dy != 0 {
		let children: Vec<LayerRef> = match &doc.layer(id).expect("resolved").kind {
			LayerKind::Group { children, .. } => children.iter().map(|c| LayerRef::Id(c.id)).collect(),
			_ => Vec::new(),
		};
		if !children.is_empty() {
			offset_layers(doc, &children, dx, dy)?;
		}
	}
	grow_canvas_for(doc, rect);
	let mask = artboard_mask(doc, rect);
	let target = doc.layer_mut(id).expect("resolved");
	target.vector_mask = Some(mask);
	target.artboard = Some(crate::layer::Artboard { rect, background });
	Ok(CommandEffect {
		label: "Artboard".into(),
		pixels_changed: vec![id],
		structure_changed: true,
		..Default::default()
	})
}
