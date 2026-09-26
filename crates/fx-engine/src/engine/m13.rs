//! The engine side of M13: the AI model manager's actions (T01).
//!
//! FAST: no Preferences page yet — `ai:status` says what is installed,
//! `ai:download {id}` fetches a model on a background thread (the result is
//! logged; no progress bar), `ai:delete {id}` removes it.

use fx_protocol::EngineToUi;

use super::Engine;

impl Engine {
	pub(super) fn m13_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		let model = || args.get("id").and_then(|v| v.as_str()).and_then(fx_ai::models::by_id);
		match id {
			"ai:status" => {
				let runtime = if fx_ai::runtime::available() {
					"ONNX Runtime found"
				} else {
					"ONNX Runtime missing"
				};
				let models: Vec<String> = fx_ai::models::ALL
					.iter()
					.map(|m| {
						format!(
							"{}: {} ({} MB)",
							m.name,
							if m.installed() { "installed" } else { "not installed" },
							m.bytes() / 1_000_000
						)
					})
					.collect();
				self.to_ui(&EngineToUi::Toast {
					text: format!("{runtime} · {} · folder {}", models.join(" · "), fx_ai::models::models_dir().display()),
				});
				true
			}
			"ai:download" => {
				let Some(spec) = model() else { return true };
				self.to_ui(&EngineToUi::Toast {
					text: format!("Downloading {} ({} MB)…", spec.name, spec.bytes() / 1_000_000),
				});
				std::thread::spawn(move || match fx_ai::models::download(&spec, &mut |_, _| true) {
					Ok(()) => tracing::info!("model {} installed", spec.id),
					Err(error) => tracing::warn!("model {}: {error}", spec.id),
				});
				true
			}
			"ai:delete" => {
				let Some(spec) = model() else { return true };
				let text = match fx_ai::models::delete(&spec) {
					Ok(()) => format!("{} deleted", spec.name),
					Err(error) => error.to_string(),
				};
				self.to_ui(&EngineToUi::Toast { text });
				true
			}
			_ => false,
		}
	}
}
