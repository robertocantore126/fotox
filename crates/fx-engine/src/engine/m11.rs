//! The engine side of M11: Edit ▸ Content-Aware Fill (T02), Content-Aware
//! Scale (T05), Liquify (T06), Puppet Warp (T07), Perspective Warp (T08).

use fx_core::command::m11::FillOutput;
use fx_core::{Command, LayerRef};
use fx_protocol::EngineToUi;

use super::Engine;

impl Engine {
	pub(super) fn m11_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		let Some(doc_id) = self.docs.active_id() else {
			return false;
		};
		let has_selection = self.docs.get(doc_id).is_some_and(|o| o.doc.selection.is_some());
		match id {
			// Edit ▸ Content-Aware Fill… and Edit ▸ Fill ▸ Use: Content-Aware.
			"edit:content-aware-fill" | "edit:fill" if id != "edit:fill" || args.get("use").and_then(|v| v.as_str()) == Some("Content-Aware") => {
				if !has_selection {
					self.to_ui(&EngineToUi::Toast {
						text: "Content-Aware Fill needs a selection".into(),
					});
					return true;
				}
				let output = match args.get("output").and_then(|v| v.as_str()) {
					Some("New Layer") => FillOutput::NewLayer,
					Some("Duplicate Layer") => FillOutput::Duplicate,
					_ => FillOutput::Current,
				};
				let seed = args.get("seed").and_then(|v| v.as_u64()).unwrap_or(1);
				self.command(
					doc_id,
					Command::ContentAwareFill {
						layer: LayerRef::Active,
						output,
						seed,
					},
				);
				true
			}
			// Filter ▸ Liquify (M11-T06).
			"warp:liquify" => {
				self.start_warp(Box::new(crate::tools::liquify::Liquify::default()));
				true
			}
			// Edit ▸ Puppet Warp (M11-T07): a mesh over the layer's content.
			"warp:puppet" => {
				let density = self.settings.string("_warp-puppet", "Density").unwrap_or_default();
				let expansion = self.settings.number("_warp-puppet", "Expansion").unwrap_or(2.0);
				match self.puppet_mesh(&density, expansion) {
					Ok(mesh) if !mesh.tris.is_empty() => self.start_warp(Box::new(crate::tools::puppet::Puppet::new(mesh))),
					Ok(_) => self.to_ui(&EngineToUi::Toast {
						text: "Puppet Warp needs a layer with pixels".into(),
					}),
					Err(text) => self.to_ui(&EngineToUi::Toast { text }),
				}
				true
			}
			// Edit ▸ Content-Aware Scale (M11-T05): percentages of the layer.
			"edit:content-aware-scale" => {
				let Some((w, h)) = self.docs.get(doc_id).and_then(|o| {
					let layer = o.doc.layer(o.doc.active_layer()?)?;
					match &layer.kind {
						fx_core::LayerKind::Pixel { image, .. } => Some((image.width(), image.height())),
						_ => None,
					}
				}) else {
					self.to_ui(&EngineToUi::Toast {
						text: "Content-Aware Scale works on a pixel layer".into(),
					});
					return true;
				};
				let pct = |k: &str| args.get(k).and_then(|v| v.as_f64()).unwrap_or(100.0) / 100.0;
				let width = (f64::from(w) * pct("width")).round().max(1.0) as u32;
				let height = (f64::from(h) * pct("height")).round().max(1.0) as u32;
				let protect = args.get("protect").and_then(|v| v.as_i64()).filter(|i| *i >= 0).map(|i| i as usize);
				self.command(
					doc_id,
					Command::ContentAwareScale {
						layer: LayerRef::Active,
						width,
						height,
						amount: pct("amount"),
						protect,
						protect_skin: args.get("skin").and_then(|v| v.as_bool()).unwrap_or(false),
					},
				);
				true
			}
			_ => false,
		}
	}
}

impl Engine {
	/// A warp session's option bar changed (M11): each value to the warp.
	pub(super) fn warp_options(&mut self, options: &serde_json::Value) {
		let Some((doc_id, session)) = &mut self.transform else { return };
		let doc_id = *doc_id;
		let Some(custom) = session.custom.as_mut() else { return };
		let mut changed = false;
		if let Some(map) = options.as_object() {
			for (key, value) in map {
				if custom.set_option(key, value) != crate::tools::transform::Update::None {
					changed = true;
				}
			}
		}
		if changed {
			self.transform_update(doc_id, crate::tools::transform::Update::Changed { dragging: false });
		}
	}

	/// Start a warp session (Liquify, Puppet Warp, Perspective Warp) on the
	/// active layer.
	pub(super) fn start_warp(&mut self, custom: Box<dyn crate::tools::transform::CustomWarp>) {
		let Some(doc_id) = self.docs.active_id() else { return };
		if self.transform.is_some() {
			return;
		}
		self.start_transform_with(doc_id, crate::tools::transform::Mode::Free, Some(custom));
	}

	/// Puppet Warp's mesh over the active layer's content (Density, Expansion).
	fn puppet_mesh(&self, density: &str, expansion: f64) -> Result<fx_core::warp_map::TriMesh, String> {
		use std::collections::HashMap;

		use fx_tiles::{TILE_SIZE, TileSlot};
		let open = self.docs.active_id().and_then(|id| self.docs.get(id)).ok_or("no document")?;
		let layer_id = open.doc.active_layer().ok_or("select a layer")?;
		let layer = open.doc.layer(layer_id).ok_or("select a layer")?;
		let fx_core::LayerKind::Pixel { image, offset } = &layer.kind else {
			return Err("Puppet Warp works on a pixel layer".into());
		};
		let rect = crate::transform_preview::start_rect(&open.doc, layer_id, &self.store)
			.map_err(|e| e.to_string())?
			.ok_or("the layer is empty")?;
		let size = (rect[2] - rect[0]).max(rect[3] - rect[1]);
		let cells = match density {
			"Fewer Points" => 20.0,
			"More Points" => 60.0,
			_ => 36.0,
		};
		let step = (size / cells).max(4.0);
		let e = expansion.max(0.0);
		let rect = [rect[0] - e, rect[1] - e, rect[2] + e, rect[3] + e];
		let tile = i64::from(TILE_SIZE);
		let mut cache: HashMap<(i64, i64), Option<Vec<[f32; 4]>>> = HashMap::new();
		let store = &self.store;
		let mut alpha = |x: f64, y: f64| -> f32 {
			let (lx, ly) = (x.floor() as i64 - i64::from(offset.0), y.floor() as i64 - i64::from(offset.1));
			if lx < 0 || ly < 0 || lx >= i64::from(image.width()) || ly >= i64::from(image.height()) {
				return 0.0;
			}
			let key = (lx / tile, ly / tile);
			let t = cache.entry(key).or_insert_with(|| match image.slot(0, key.0 as u32, key.1 as u32) {
				TileSlot::Empty => None,
				TileSlot::Solid(v) => Some(vec![v.0.map(|c| f32::from(c) / 65535.0); 1]),
				// FAST: RGBA layers only; an evicted tile reads transparent.
				TileSlot::Data(h) => store.get(h).ok().map(|b| fx_core::pixels::decode(&b, image.format())),
			});
			match t {
				None => 0.0,
				Some(p) if p.len() == 1 => p[0][3],
				Some(p) => p[((ly % tile) * tile + lx % tile) as usize][3],
			}
		};
		let mut covered = |x: f64, y: f64| -> bool {
			[(0.0, 0.0), (e, 0.0), (-e, 0.0), (0.0, e), (0.0, -e)]
				.iter()
				.any(|(dx, dy)| alpha(x + dx, y + dy) > 0.02)
		};
		Ok(fx_ops::puppet::build_mesh(rect, step, &mut covered))
	}
}
