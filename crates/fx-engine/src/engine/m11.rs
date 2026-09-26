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
