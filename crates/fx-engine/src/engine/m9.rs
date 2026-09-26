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
	/// After an edit: resend the channel list when it changed.
	pub(super) fn after_edit_m9(&mut self, id: DocId) {
		let Some(open) = self.docs.get(id) else { return };
		let sig = signature(&open.doc);
		if self.m9.channels.get(&id) == Some(&sig) {
			return;
		}
		self.m9.channels.insert(id, sig);
		self.send_channels(id);
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
			_ => return false,
		}
		true
	}
}

impl Engine {
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
