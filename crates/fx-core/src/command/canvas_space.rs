//! Canvas-level geometry for everything that is not a layer's pixel image
//! (code review 2026-09-27 R03). Rotate, flip, Canvas Size, Crop, Image Size
//! and Perspective Crop move the pixels themselves; this module moves the rest
//! of the document with them, so a mixed document turns as one.
//!
//! What follows the canvas mapping:
//! * shape and text matrices, Smart Object transforms (composed);
//! * vector-mask paths, the work path and saved paths;
//! * gradient fills (centre and angle), pattern fills (angle, for turns);
//! * artboard and slice rectangles (their mapped bounding box);
//! * notes, count markers and colour samplers;
//! * guides, when they stay horizontal or vertical (an arbitrary turn leaves
//!   them where they were);
//! * layer comps' matrices and Smart Object transforms; a comp's pixel-layer
//!   offset follows a whole-pixel translation and is forgotten otherwise (the
//!   pixels it placed were rewritten).
//!
//! Every derived cache these touch is marked dirty; [`set_canvas`] then gives
//! each canvas-sized cache the new size. Alpha channels are pixels and are
//! moved by the raster helpers, like masks at the canvas origin.

use super::*;

/// The document's non-pixel geometry after a canvas mapping, computed while
/// the document is untouched (phase 1) and installed with the pixels (phase 2).
pub(super) struct SpacePlan {
	layers: Vec<LayerPatch>,
	guides: Vec<crate::document::Guide>,
	work_path: Option<crate::path::Path>,
	paths: Vec<crate::path::NamedPath>,
	slices: Vec<crate::comps::Slice>,
	annotations: crate::annotations::Annotations,
	comps: Vec<crate::comps::LayerComp>,
}

struct LayerPatch {
	id: LayerId,
	/// A generated layer's new kind (shape, text, fill, Smart Object).
	kind: Option<LayerKind>,
	vector_mask: Option<crate::layer::VectorMask>,
	artboard: Option<crate::layer::Artboard>,
}

/// Where `mapping` sends a point; a point it cannot evaluate stays.
fn point(mapping: &Mapping, p: (f64, f64)) -> (f64, f64) {
	mapping.forward_point(p.0, p.1).unwrap_or(p)
}

/// The linear part of an affine mapping.
fn linear(mapping: &Mapping) -> Option<[f64; 4]> {
	match mapping {
		Mapping::Affine([a, b, c, d, _, _]) => Some([*a, *b, *c, *d]),
		_ => None,
	}
}

/// A rectangle's mapped bounding box.
fn rect(mapping: &Mapping, r: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
	let box_ = [f64::from(r.0), f64::from(r.1), f64::from(r.0) + f64::from(r.2), f64::from(r.1) + f64::from(r.3)];
	match dest_rect(mapping, box_) {
		Some(((x, y), (w, h))) => (x, y, w, h),
		None => r,
	}
}

impl SpacePlan {
	/// The plan for `mapping` (old canvas pixels → new canvas pixels), where
	/// the old canvas is `old` and the new one `new`.
	pub(super) fn new(doc: &Document, mapping: Mapping, old: (u32, u32), new: (u32, u32)) -> Result<Self, CommandError> {
		let lin = linear(&mapping);
		let scale = lin.map_or(1.0, |[a, b, c, d]| (a * d - b * c).abs().sqrt());
		// A direction (y down) turned by the mapping, as Photoshop's
		// counter-clockwise angle in degrees.
		let angle = |degrees: f64| -> f64 {
			let Some([a, b, c, d]) = lin else { return degrees };
			let (s, co) = degrees.to_radians().sin_cos();
			let (x, y) = (co, -s);
			let (x2, y2) = (a * x + c * y, b * x + d * y);
			(-y2).atan2(x2).to_degrees()
		};
		let old_centre = (f64::from(old.0) / 2.0, f64::from(old.1) / 2.0);
		let new_centre = (f64::from(new.0) / 2.0, f64::from(new.1) / 2.0);
		let map_path = |p: &crate::path::Path| p.map(|q| point(&mapping, q));

		let mut layers = Vec::new();
		for id in layer_ids(doc) {
			let layer = doc.layer(id).expect("walked layer exists");
			// Exhaustive: a new layer kind must decide how it follows the canvas.
			let kind = match &layer.kind {
				LayerKind::Pixel { .. } | LayerKind::Group { .. } | LayerKind::Adjustment(_) | LayerKind::SolidFill { .. } => None,
				LayerKind::Shape { .. } | LayerKind::Text { .. } => {
					let mut kind = layer.kind.clone();
					if let Some((transform, cache)) = kind.derived_placement() {
						// A projective mapping cannot be held by a matrix: such
						// a layer was wrapped in a Smart Object first
						// (`wrap_for_projection`).
						if let Some(moved) = mapping.then_affine(*transform) {
							*transform = moved;
						}
						cache.mark_all_dirty();
					}
					Some(kind)
				}
				LayerKind::FillLayer { content, cache } => {
					let content = match content {
						crate::fill::FillLayer::Gradient(g) => {
							let mut g = g.clone();
							let c = point(&mapping, (old_centre.0 + g.offset.0, old_centre.1 + g.offset.1));
							g.offset = (c.0 - new_centre.0, c.1 - new_centre.1);
							g.angle = angle(g.angle);
							crate::fill::FillLayer::Gradient(g)
						}
						// Pattern space → document space is `origin + M·p`; the
						// canvas mapping `L·x + t` makes it `L·origin + t + L·M·p`
						// (exact for turns, flips and uniform scales).
						crate::fill::FillLayer::Pattern {
							pattern,
							scale: s,
							angle: a,
							origin,
							mirror,
						} => {
							let (scale, angle, mirror) = match lin {
								Some([la, lb, lc, ld]) => {
									let [ma, mb, mc, md] = crate::fill::pattern_matrix(*s, *a, *mirror);
									crate::fill::pattern_parameters([la * ma + lc * mb, lb * ma + ld * mb, la * mc + lc * md, lb * mc + ld * md])
								}
								None => (*s, *a, *mirror),
							};
							let origin = point(&mapping, (origin[0], origin[1]));
							crate::fill::FillLayer::Pattern {
								pattern: *pattern,
								scale,
								angle,
								origin: [origin.0, origin.1],
								mirror,
							}
						}
					};
					let mut cache = cache.clone();
					cache.mark_all_dirty();
					Some(LayerKind::FillLayer { content, cache })
				}
				LayerKind::Smart { smart, cache } => {
					let Some(transform) = m12::compose(mapping, smart.transform) else {
						return Err(CommandError::NotAllowed(format!(
							"the Smart Object \"{}\" has a warp that cannot follow the canvas; rasterise it first",
							layer.name
						)));
					};
					let mut smart = smart.clone();
					smart.transform = transform;
					let mut cache = cache.clone();
					cache.mark_all_dirty();
					Some(LayerKind::Smart { smart, cache })
				}
			};
			let vector_mask = layer.vector_mask.as_ref().map(|vm| {
				let mut vm = vm.clone();
				vm.path = map_path(&vm.path);
				vm.feather *= scale;
				vm.cache.mark_all_dirty();
				vm
			});
			let artboard = layer.artboard.as_ref().map(|a| crate::layer::Artboard {
				rect: rect(&mapping, a.rect),
				background: a.background,
			});
			layers.push(LayerPatch {
				id,
				kind,
				vector_mask,
				artboard,
			});
		}

		let guides = doc
			.guides
			.iter()
			.map(|g| {
				let Some([a, b, c, d]) = lin else { return *g };
				let flat = |v: f64| v.abs() < 1e-9;
				let (p, direction) = if g.vertical {
					((g.position, 0.0), (c, d))
				} else {
					((0.0, g.position), (a, b))
				};
				let q = point(&mapping, p);
				if flat(direction.0) {
					crate::document::Guide { vertical: true, position: q.0 }
				} else if flat(direction.1) {
					crate::document::Guide {
						vertical: false,
						position: q.1,
					}
				} else {
					*g
				}
			})
			.collect();

		let mut annotations = doc.annotations.clone();
		for note in &mut annotations.notes {
			(note.x, note.y) = point(&mapping, (note.x, note.y));
		}
		for group in &mut annotations.counts {
			for p in &mut group.points {
				*p = point(&mapping, *p);
			}
		}
		for sampler in &mut annotations.samplers {
			(sampler.x, sampler.y) = point(&mapping, (sampler.x, sampler.y));
		}

		let whole_translation = match mapping {
			Mapping::Affine([a, b, c, d, e, f]) if (a, b, c, d) == (1.0, 0.0, 0.0, 1.0) && e.fract() == 0.0 && f.fract() == 0.0 => Some((e as i32, f as i32)),
			_ => None,
		};
		let mut comps = doc.comps.clone();
		for comp in &mut comps {
			for state in &mut comp.states {
				state.offset = match (state.offset, whole_translation) {
					(Some((x, y)), Some((dx, dy))) => Some((x.saturating_add(dx), y.saturating_add(dy))),
					_ => None,
				};
				if let Some(m) = state.matrix {
					state.matrix = mapping.then_affine(m).or(Some(m));
				}
				if let Some(m) = state.mapping {
					state.mapping = m12::compose(mapping, m).or(Some(m));
				}
			}
		}

		Ok(Self {
			layers,
			guides,
			work_path: doc.work_path.as_ref().map(map_path),
			paths: doc
				.paths
				.iter()
				.map(|p| crate::path::NamedPath {
					name: p.name.clone(),
					path: map_path(&p.path),
				})
				.collect(),
			slices: doc
				.slices
				.iter()
				.map(|s| crate::comps::Slice {
					name: s.name.clone(),
					rect: rect(&mapping, s.rect),
				})
				.collect(),
			annotations,
			comps,
		})
	}

	/// Install the plan. Returns the layers whose geometry changed.
	pub(super) fn apply(self, doc: &mut Document) -> Vec<LayerId> {
		let mut changed = Vec::new();
		for patch in self.layers {
			let Some(layer) = doc.layer_mut(patch.id) else { continue };
			let mut touched = false;
			if let Some(kind) = patch.kind {
				layer.kind = kind;
				touched = true;
			}
			if patch.vector_mask.is_some() {
				layer.vector_mask = patch.vector_mask;
				touched = true;
			}
			if patch.artboard.is_some() {
				layer.artboard = patch.artboard;
				touched = true;
			}
			if touched {
				changed.push(patch.id);
			}
		}
		doc.guides = self.guides;
		doc.work_path = self.work_path;
		doc.paths = self.paths;
		doc.slices = self.slices;
		doc.annotations = self.annotations;
		doc.comps = self.comps;
		changed
	}
}

/// Before a projective canvas mapping (Perspective Crop): every shape and
/// text layer becomes a Smart Object around its own content, so the
/// homography can be kept on it — an affine placement cannot hold one. The
/// layer keeps its id, name, opacity, blend mode, masks and styles; its
/// content, neutral (as Rasterize keeps it), becomes the embedded document at
/// the old canvas size. Returns the layers wrapped; nothing for an affine
/// mapping.
pub(super) fn wrap_for_projection(doc: &mut Document, mapping: &Mapping, ops: &dyn PixelOps, store: &TileStore) -> Result<Vec<LayerId>, CommandError> {
	if matches!(mapping, Mapping::Affine(_)) {
		return Ok(Vec::new());
	}
	let ids: Vec<LayerId> = layer_ids(doc)
		.into_iter()
		.filter(|id| {
			doc.layer(*id)
				.is_some_and(|l| matches!(l.kind, LayerKind::Shape { .. } | LayerKind::Text { .. }))
		})
		.collect();
	let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
	let mut wrapped = Vec::with_capacity(ids.len());
	for id in ids {
		let content = content_alone(doc, id).ok_or(CommandError::LayerNotFound(LayerRef::Id(id)))?;
		let (next_id, counters) = doc.id_state();
		let mut nested = Document::new(w, h, doc.color.clone(), doc.ppi).with_id_state(next_id, counters);
		nested.layers = content.layers;
		nested.selected = vec![id];
		let composite = ops.composite(&nested, &[id], None, store)?;
		let smart = crate::smart::SmartObject {
			source: crate::smart::SmartSource {
				doc: Arc::new(nested),
				composite,
				linked: None,
				linked_mtime: None,
				uid: crate::smart::new_uid(),
			},
			transform: Mapping::identity(),
			filters: Vec::new(),
			filters_enabled: true,
		};
		if let Some(layer) = doc.layer_mut(id) {
			layer.kind = LayerKind::Smart {
				smart,
				cache: TiledImage::derived(w, h, format),
			};
			wrapped.push(id);
		}
	}
	Ok(wrapped)
}

/// The alpha channels (canvas-origin grey images) after `f` moves each one.
pub(super) fn map_channels(doc: &Document, mut f: impl FnMut(&TiledImage) -> Result<TiledImage, CommandError>) -> Result<Vec<TiledImage>, CommandError> {
	doc.channels.iter().map(|c| f(&c.image)).collect()
}

/// Install [`map_channels`]'s result.
pub(super) fn set_channels(doc: &mut Document, images: Vec<TiledImage>) {
	for (channel, image) in doc.channels.iter_mut().zip(images) {
		channel.image = image;
	}
}
