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
			_ => false,
		}
	}
}
