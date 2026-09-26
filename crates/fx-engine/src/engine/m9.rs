//! The engine side of M9: channels and Quick Mask (T01), the selection
//! commands' UI actions, annotations (T08).

use std::collections::HashMap;

use fx_core::{Command, Document};
use fx_protocol::{DocId, EngineToUi};
use fx_tiles::{TILE_SIZE, TileSlot, TileStore};

use super::Engine;

#[derive(Default)]
pub(super) struct State {
	/// A cheap signature of each document's channel list, so thumbnails are
	/// rebuilt only when it changes.
	channels: HashMap<DocId, String>,
	/// Select and Mask's output, made once its job is done (M9-T05).
	output: Option<String>,
}

/// The channel list's signature: names, colours and the first slots.
fn signature(doc: &Document) -> String {
	let mut s = String::new();
	for c in &doc.channels {
		s.push_str(&format!("{}|{:?}|{}|", c.name, c.color, c.opacity));
		for (tx, ty, slot) in c.image.grid(0).non_empty().take(16) {
			s.push_str(&format!("{tx},{ty}:{slot:?};"));
		}
	}
	s.push_str(&format!("q{}", fx_core::command::m9::quick_mask_layer(doc).is_some()));
	s
}

/// A 48 × 48 grey thumbnail of a channel, sampled at level 0.
// FAST: point samples read up to 48² tiles of a big document.
fn thumbnail(image: &fx_tiles::TiledImage, store: &TileStore) -> Vec<u8> {
	let (w, h) = (image.width().max(1), image.height().max(1));
	let format = image.format();
	let mut cache: HashMap<(u32, u32), Option<std::sync::Arc<fx_tiles::TileBuffer>>> = HashMap::new();
	let mut out = Vec::with_capacity(48 * 48);
	for j in 0..48u32 {
		for i in 0..48u32 {
			let (x, y) = ((i * w / 48).min(w - 1), (j * h / 48).min(h - 1));
			let key = (x / TILE_SIZE, y / TILE_SIZE);
			let v = match image.slot(0, key.0, key.1) {
				TileSlot::Empty => 0.0,
				TileSlot::Solid(v) => f32::from(v.0[0]) / 65535.0,
				TileSlot::Data(handle) => {
					let buffer = cache.entry(key).or_insert_with(|| store.get(handle).ok());
					buffer
						.as_ref()
						.map_or(0.0, |b| fx_core::selection::gray_at(b, format, x % TILE_SIZE, y % TILE_SIZE))
				}
			};
			out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
		}
	}
	out
}

impl Engine {
	/// After an edit: resend the channel list when it changed, and the
	/// annotations with the samplers' current values.
	pub(super) fn after_edit_m9(&mut self, id: DocId) {
		self.send_annotations(id);
		self.send_paths(id);
		let Some(open) = self.docs.get(id) else { return };
		let sig = signature(&open.doc);
		if self.m9.channels.get(&id) == Some(&sig) {
			return;
		}
		self.m9.channels.insert(id, sig);
		self.send_channels(id);
	}

	/// Notes, counts and samplers with the samplers' values (M9-T08): read from
	/// the composite, averaged over the option bar's Sample Size.
	pub(super) fn send_annotations(&self, id: DocId) {
		let Some(open) = self.docs.get(id) else { return };
		let a = &open.doc.annotations;
		let area = match self.settings.string("sampler", "Sample Size").as_deref() {
			Some("3 by 3 Average") => 3,
			Some("5 by 5 Average") => 5,
			Some("11 by 11 Average") => 11,
			Some("31 by 31 Average") => 31,
			Some("51 by 51 Average") => 51,
			Some("101 by 101 Average") => 101,
			_ => 1,
		};
		// FAST: composited on the engine thread at every edit (≤ 10 points).
		let samples = a
			.samplers
			.iter()
			.map(|s| crate::tools::sample_pixel(&open.doc, s.x, s.y, area, None, &self.store).unwrap_or([0; 4]))
			.collect();
		self.to_ui(&EngineToUi::Annotations {
			doc: id,
			annotations: serde_json::to_value(a).unwrap_or_default(),
			samples,
		});
	}

	pub(super) fn send_channels(&self, id: DocId) {
		let Some(open) = self.docs.get(id) else { return };
		let channels = open
			.doc
			.channels
			.iter()
			.map(|c| {
				serde_json::json!({
					"name": c.name, "color": c.color, "opacity": c.opacity,
					"thumb": crate::b64::encode(&thumbnail(&c.image, &self.store)),
				})
			})
			.collect();
		self.to_ui(&EngineToUi::Channels {
			doc: id,
			channels,
			quick_mask: fx_core::command::m9::quick_mask_layer(&open.doc).is_some(),
		});
	}

	/// An M9 action; `false` when `id` is not one.
	pub(super) fn m9_action(&mut self, id: &str, _args: &serde_json::Value) -> bool {
		let Some(doc_id) = self.docs.active_id() else {
			return false;
		};
		match id {
			// Q (M9-T01): into or out of Quick Mask.
			"sel:quick-mask" => {
				let on = self.docs.get(doc_id).is_some_and(|o| fx_core::command::m9::quick_mask_layer(&o.doc).is_none());
				self.command(doc_id, Command::QuickMask { on });
				// Painting goes to the Quick Mask layer's mask.
				if let Some(open) = self.docs.get_mut(doc_id) {
					open.mask_target = if on { fx_core::command::m9::quick_mask_layer(&open.doc) } else { None };
				}
				self.after_edit(doc_id, true);
			}
			"channels:list" => self.send_channels(doc_id),
			// Select ▸ Grow / Similar (M9-T02) with the Magic Wand's options.
			"sel:grow" | "sel:similar" => {
				let tolerance = self.settings.number("magic-wand", "Tolerance").unwrap_or(32.0).clamp(0.0, 255.0);
				let sample_all = self.settings.bool("magic-wand", "Sample All Layers").unwrap_or(false);
				let select = if id == "sel:grow" {
					fx_core::select_ops::SelectOp::Grow { tolerance, sample_all }
				} else {
					fx_core::select_ops::SelectOp::Similar { tolerance, sample_all }
				};
				self.command(
					doc_id,
					Command::SelectBy {
						select,
						mode: fx_core::SelectMode::Replace,
					},
				);
			}
			"sel:transform" => self.start_selection_transform(doc_id),
			// The measuring tools' option-bar buttons (M9-T08) go to the active
			// tool as keys.
			"ruler:straighten" => {
				self.tool_key("Straighten");
			}
			"ruler:clear" | "notes:clear-all" | "count:reset" => {
				self.tool_key("Clear");
			}
			"count:new-group" => {
				self.tool_key("NewGroup");
			}
			"sampler:clear" => {
				let mut annotations = self.docs.get(doc_id).map(|o| o.doc.annotations.clone()).unwrap_or_default();
				annotations.samplers.clear();
				self.command(
					doc_id,
					Command::SetAnnotations {
						annotations,
						label: "Delete All Color Samplers".into(),
					},
				);
			}
			// The Notes panel's edits.
			"notes:set" => {
				let index = _args.get("index").and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize;
				let text = _args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_owned();
				let mut annotations = self.docs.get(doc_id).map(|o| o.doc.annotations.clone()).unwrap_or_default();
				if let Some(note) = annotations.notes.get_mut(index) {
					note.text = text;
					self.command(
						doc_id,
						Command::SetAnnotations {
							annotations,
							label: "Edit Note".into(),
						},
					);
				}
			}
			"notes:delete" => {
				let index = _args.get("index").and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize;
				let mut annotations = self.docs.get(doc_id).map(|o| o.doc.annotations.clone()).unwrap_or_default();
				if index < annotations.notes.len() {
					annotations.notes.remove(index);
					self.command(
						doc_id,
						Command::SetAnnotations {
							annotations,
							label: "Delete Note".into(),
						},
					);
				}
			}
			"select-mask:output" => {
				self.m9.output = _args.get("output").and_then(|v| v.as_str()).map(str::to_owned);
			}
			_ => return false,
		}
		true
	}
}

impl Engine {
	/// After a pixel job: Select and Mask's queued output (M9-T05).
	pub(super) fn after_job_m9(&mut self, doc_id: DocId) {
		let Some(output) = self.m9.output.take() else { return };
		let has_selection = self.docs.get(doc_id).is_some_and(|o| o.doc.selection.is_some());
		if !has_selection {
			return;
		}
		use fx_core::command::MaskFill;
		if output == "layer" {
			self.command(
				doc_id,
				Command::DuplicateLayers {
					layers: vec![fx_core::LayerRef::Active],
				},
			);
		}
		self.command(
			doc_id,
			Command::AddMask {
				layer: fx_core::LayerRef::Active,
				fill: MaskFill::RevealSelection,
			},
		);
	}

	/// Select ▸ Transform Selection (M9-T02): Free Transform's box over the
	/// selection's bounds, committing `Command::TransformSelection`.
	// FAST: no live preview of the transformed ants (the box only).
	fn start_selection_transform(&mut self, doc_id: DocId) {
		let filter = fx_core::Filter::Bilinear;
		let Some(open) = self.docs.get(doc_id) else { return };
		let size = (open.doc.width, open.doc.height);
		let Some(bounds) = open.doc.selection.as_ref().and_then(|s| s.canvas_bounds(size)) else {
			self.to_ui(&EngineToUi::Toast {
				text: "Transform Selection needs a selection".into(),
			});
			return;
		};
		let rect = [f64::from(bounds.0), f64::from(bounds.1), f64::from(bounds.2), f64::from(bounds.3)];
		let layer = open.doc.active_layer().unwrap_or(fx_core::LayerId(0));
		self.end_stroke();
		let mut session = crate::tools::transform::Session::new(layer, rect, crate::tools::transform::Mode::Free, filter);
		session.selection = true;
		let status = session.status();
		self.transform = Some((doc_id, session));
		self.to_ui(&EngineToUi::TransformBox { up: true });
		self.to_ui(&EngineToUi::ToolInfo { text: status });
		self.request_frame();
	}
}
