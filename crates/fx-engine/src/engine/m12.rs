//! The engine side of M12: Smart Objects (T01, T02), Smart Filters (T03).

use std::sync::Arc;

use fx_core::{Command, LayerId, LayerKind, LayerRef};
use fx_protocol::{DocId, EngineToUi};

use super::Engine;
use crate::documents::OpenDoc;
use crate::vector;

impl Engine {
	pub(super) fn m12_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		let Some(doc_id) = self.docs.active_id() else {
			return false;
		};
		match id {
			"smart:convert" => {
				let ids = self.docs.get(doc_id).map(|o| o.doc.selected.clone()).unwrap_or_default();
				self.convert_to_smart(doc_id, &ids);
				true
			}
			"smart:copy" => {
				self.command(doc_id, Command::NewSmartObjectViaCopy { layer: LayerRef::Active });
				true
			}
			"smart:rasterize" | "raster:smart" => {
				if let Some(open) = self.docs.get_mut(doc_id) {
					vector::prepare_level0(&mut open.doc, &self.store, None);
				}
				self.command(
					doc_id,
					Command::Rasterize {
						layers: vec![LayerRef::Active],
					},
				);
				true
			}
			// Filter ▸ Convert for Smart Filters.
			"filter:smart" => {
				let ids = self.docs.get(doc_id).map(|o| o.doc.selected.clone()).unwrap_or_default();
				self.convert_to_smart(doc_id, &ids);
				true
			}
			// The Smart Filters dialog (M12-T03): the list's eyes, opacities,
			// deletions and order, all at once.
			"smart:filters-set" => {
				let Some((mut filters, _)) = self.smart_filters(doc_id) else { return true };
				let enabled = args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
				if let Some(rows) = args.get("filters").and_then(|v| v.as_array()) {
					for (f, row) in filters.iter_mut().zip(rows) {
						f.enabled = row.get("enabled").and_then(|v| v.as_bool()).unwrap_or(f.enabled);
						if let Some(o) = row.get("opacity").and_then(|v| v.as_f64()) {
							f.opacity = (o / 100.0).clamp(0.0, 1.0) as f32;
						}
					}
					let mut keep: Vec<(i64, fx_core::smart::SmartFilter)> = filters
						.into_iter()
						.zip(rows)
						.filter(|(_, row)| !row.get("delete").and_then(|v| v.as_bool()).unwrap_or(false))
						.enumerate()
						.map(|(i, (f, row))| (row.get("order").and_then(|v| v.as_i64()).unwrap_or(i as i64), f))
						.collect();
					keep.sort_by_key(|(o, _)| *o);
					filters = keep.into_iter().map(|(_, f)| f).collect();
				}
				self.command(
					doc_id,
					Command::SetSmartFilters {
						layer: LayerRef::Active,
						filters,
						enabled,
						label: "Smart Filters".into(),
					},
				);
				true
			}
			"smart:edit" => {
				self.edit_contents(doc_id);
				true
			}
			// Ctrl+S in an Edit Contents tab saves back into the parent (M12-T02).
			"doc:save" if self.smart_children.contains_key(&doc_id) => {
				self.save_contents(doc_id);
				true
			}
			_ => {
				let _ = args;
				false
			}
		}
	}

	/// Convert `ids` into one Smart Object (their shape / text tiles drawn
	/// first: the composite reads level 0).
	pub(super) fn convert_to_smart(&mut self, doc_id: DocId, ids: &[LayerId]) {
		if ids.is_empty() {
			self.to_ui(&EngineToUi::Toast {
				text: "Select the layers to convert".into(),
			});
			return;
		}
		if let Some(open) = self.docs.get_mut(doc_id) {
			vector::prepare_level0(&mut open.doc, &self.store, None);
		}
		self.command(
			doc_id,
			Command::ConvertToSmartObject {
				layers: ids.iter().map(|id| LayerRef::Id(*id)).collect(),
			},
		);
	}

	/// Layer ▸ Smart Objects ▸ Edit Contents: the source in its own tab.
	fn edit_contents(&mut self, doc_id: DocId) {
		let Some(open) = self.docs.get(doc_id) else { return };
		let Some(layer_id) = open.doc.active_layer() else { return };
		let Some(layer) = open.doc.layer(layer_id) else { return };
		let LayerKind::Smart { smart, .. } = &layer.kind else {
			self.to_ui(&EngineToUi::Toast {
				text: "Edit Contents works on a Smart Object".into(),
			});
			return;
		};
		// An already open tab for it comes to the front.
		if let Some((&child, _)) = self.smart_children.iter().find(|(_, v)| **v == (doc_id, layer_id)) {
			self.docs.activate(child);
			self.after_active_change();
			return;
		}
		let nested = (*smart.source.doc).clone();
		let name = format!("{}.psb", layer.name);
		let child = self.docs.allocate_id();
		let mut doc = OpenDoc::from_document(child, name, nested);
		if let Some(viewport) = self.virtual_view.viewport {
			doc.view.resize(viewport.width, viewport.height);
		}
		let info = doc.info();
		self.docs.add(doc);
		self.smart_children.insert(child, (doc_id, layer_id));
		self.to_ui(&EngineToUi::DocumentOpened { info });
		self.after_active_change();
	}

	/// Save an Edit Contents tab back into its parent: a new source, the
	/// cache redrawn, one "Edit Contents" step in the parent's history.
	fn save_contents(&mut self, child: DocId) {
		let Some(&(parent, layer_id)) = self.smart_children.get(&child) else { return };
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(child) else { return };
		vector::prepare_level0(&mut open.doc, &store, None);
		let nested = open.doc.clone();
		let roots: Vec<LayerId> = nested.layers.iter().map(|l| l.id).collect();
		let composite = match crate::export::composite_layers(&nested, &roots, None, &store, None) {
			Ok(image) => image,
			Err(error) => {
				self.to_ui(&EngineToUi::Error {
					text: format!("Edit Contents: {error}"),
				});
				return;
			}
		};
		open.dirty = false;
		let Some(open) = self.docs.get_mut(parent) else {
			self.to_ui(&EngineToUi::Toast {
				text: "The document of this Smart Object was closed".into(),
			});
			return;
		};
		let before = open.doc.clone();
		let (w, h, format) = (open.doc.width, open.doc.height, open.doc.color.depth.rgba_format());
		let Some(layer) = open.doc.layer_mut(layer_id) else { return };
		let LayerKind::Smart { smart, cache } = &mut layer.kind else { return };
		smart.source.doc = Arc::new(nested);
		smart.source.composite = composite;
		*cache = fx_tiles::TiledImage::derived(w, h, format);
		// FAST: the history entry's command is a placeholder (history is by
		// snapshots); instances sharing the source are not updated.
		open.history
			.record(before, Command::SelectLayers { layers: Vec::new() }, "Edit Contents".into());
		open.dirty = true;
		open.changed();
		self.after_edit(parent, true);
		self.refresh_thumbnail(parent, layer_id);
		self.to_ui(&EngineToUi::Toast {
			text: "The Smart Object was updated".into(),
		});
	}

	/// The active Smart Object's filters and stack eye.
	fn smart_filters(&self, doc_id: DocId) -> Option<(Vec<fx_core::smart::SmartFilter>, bool)> {
		let open = self.docs.get(doc_id)?;
		let layer = open.doc.layer(open.doc.active_layer()?)?;
		match &layer.kind {
			LayerKind::Smart { smart, .. } => Some((smart.filters.clone(), smart.filters_enabled)),
			_ => None,
		}
	}

	/// `ApplyFilter` on a Smart Object adds a Smart Filter instead.
	pub(super) fn smart_filter_rewrite(&self, doc_id: DocId, command: Command) -> Command {
		let Command::ApplyFilter { layer, filter } = &command else {
			return command;
		};
		let Some(open) = self.docs.get(doc_id) else { return command };
		let Ok(id) = fx_core::command::resolve(&open.doc, layer) else {
			return command;
		};
		let Some(LayerKind::Smart { smart, .. }) = open.doc.layer(id).map(|l| &l.kind) else {
			return command;
		};
		let mut filters = smart.filters.clone();
		filters.push(fx_core::smart::SmartFilter {
			filter: filter.clone(),
			enabled: true,
			mode: fx_core::BlendMode::Normal,
			opacity: 1.0,
		});
		Command::SetSmartFilters {
			layer: LayerRef::Id(id),
			filters,
			enabled: smart.filters_enabled,
			label: filter.label().to_owned(),
		}
	}
}
