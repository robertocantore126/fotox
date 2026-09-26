//! The engine side of M8's resources and actions: brush presets (T01),
//! patterns (T06), the History Brush source (T07).

use fx_protocol::EngineToUi;

use super::Engine;

/// Libraries the painting tools read.
pub(super) struct Resources {
	pub brushes: crate::brushes::Library,
}

impl Resources {
	pub fn load() -> Self {
		Self {
			brushes: crate::brushes::Library::load(),
		}
	}
}

/// The bytes of an uploaded file: `{ "data": "<base64 or data: URL>" }`.
fn upload(args: &serde_json::Value) -> Vec<u8> {
	let text = args.get("data").and_then(|v| v.as_str()).unwrap_or("");
	let text = text.split_once("base64,").map_or(text, |(_, b)| b);
	crate::b64::decode(text)
}

impl Engine {
	/// A dropped or opened `.abr` (and, T06, pattern file): `true` when the
	/// path was one.
	pub(super) fn open_resource(&mut self, path: &std::path::Path) -> bool {
		let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
		if ext.as_deref() != Some("abr") {
			return false;
		}
		// FAST: read on the engine thread (brush files are small).
		let data = match std::fs::read(path) {
			Ok(data) => data,
			Err(error) => {
				self.to_ui(&EngineToUi::Toast {
					text: format!("Could not read {}: {error}", path.display()),
				});
				return true;
			}
		};
		let args = serde_json::json!({ "data": crate::b64::encode(&data) });
		self.m8_action("brush:import-abr", &args);
		true
	}

	/// Send the libraries to the UI (after `hello` and on every change).
	pub(super) fn send_resources(&self) {
		self.to_ui(&EngineToUi::Brushes {
			presets: self.resources.brushes.infos(),
		});
	}

	/// An M8 action; `false` when `id` is not one.
	pub(super) fn m8_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		let index = || args.get("index").and_then(|v| v.as_u64()).map(|i| i as usize);
		match id {
			"brush:list" => {}
			"brush:import-abr" => {
				let data = upload(args);
				let text = match self.resources.brushes.import_abr(&data) {
					Ok(0) => "The brush file has no sampled tips Fotox can read".to_owned(),
					Ok(n) => {
						self.resources.brushes.save();
						format!("Imported {n} brushes")
					}
					Err(error) => format!("Could not import the brushes: {error}"),
				};
				self.to_ui(&EngineToUi::Toast { text });
			}
			"brush:save-preset" => {
				// The UI sends the current brush as a preset; a sampled tip
				// keeps the pixels of the preset it came from.
				let Some(mut preset) = args
					.get("preset")
					.and_then(|p| serde_json::from_value::<crate::brushes::Preset>(p.clone()).ok())
				else {
					return true;
				};
				let tip = args.get("tip").and_then(|v| v.as_u64()).unwrap_or(0);
				if tip != 0 {
					preset.tip = self
						.resources
						.brushes
						.presets
						.iter()
						.find(|p| crate::brushes::tip_id(p) == tip)
						.and_then(|p| p.tip.clone());
				}
				self.resources.brushes.presets.push(preset);
				self.resources.brushes.save();
			}
			"brush:rename-preset" => {
				let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
				if let Some(p) = index().and_then(|i| self.resources.brushes.presets.get_mut(i))
					&& !name.is_empty()
				{
					p.name = name;
					self.resources.brushes.save();
				}
			}
			"brush:delete-preset" => {
				if let Some(i) = index().filter(|&i| i < self.resources.brushes.presets.len()) {
					self.resources.brushes.presets.remove(i);
					self.resources.brushes.save();
				}
			}
			_ => return false,
		}
		self.send_resources();
		true
	}
}
