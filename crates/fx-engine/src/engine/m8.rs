//! The engine side of M8's resources and actions: brush presets (T01),
//! patterns (T06), the History Brush source (T07).

use fx_core::{Command, LayerRef};
use fx_protocol::{DocId, EngineToUi};

use super::Engine;

/// Libraries the painting tools read.
pub(super) struct Resources {
	pub brushes: crate::brushes::Library,
	pub patterns: crate::patterns::Library,
	/// The pattern the tools and fills use (the Patterns panel's pick).
	pub current_pattern: Option<u64>,
	/// The History Brush's source row per document (M8-T07); 0 by default.
	pub history_source: std::collections::HashMap<DocId, usize>,
}

impl Resources {
	pub fn load() -> Self {
		let patterns = crate::patterns::Library::load();
		let current_pattern = patterns.patterns.first().map(|p| p.id);
		Self {
			brushes: crate::brushes::Library::load(),
			patterns,
			current_pattern,
			history_source: std::collections::HashMap::new(),
		}
	}
}

/// The pattern a command or stroke reads, if any.
fn pattern_of(command: &Command) -> Option<u64> {
	use fx_core::fill::{FillLayer, FillSource};
	match command {
		Command::BucketFill {
			source: FillSource::Pattern { pattern },
			..
		}
		| Command::FillPattern { pattern, .. }
		| Command::SetFillLayer {
			content: FillLayer::Pattern { pattern, .. },
			..
		}
		| Command::AddLayer {
			layer: fx_core::command::NewLayer::Fill {
				content: FillLayer::Pattern { pattern, .. },
			},
			..
		} => Some(*pattern),
		_ => None,
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
	pub(super) fn send_resources(&mut self) {
		self.to_ui(&EngineToUi::Brushes {
			presets: self.resources.brushes.infos(),
		});
		self.send_patterns();
		self.send_shapes();
	}

	fn send_patterns(&self) {
		self.to_ui(&EngineToUi::Patterns {
			patterns: self.resources.patterns.infos(),
			current: self.resources.current_pattern,
		});
	}

	/// Copy a library pattern into the document before a command or stroke
	/// reads it (M8-T06). FAST: outside the History (resources only grow).
	pub(super) fn ensure_pattern(&mut self, doc_id: DocId, pattern: u64) {
		let Some(p) = self.resources.patterns.get(pattern).cloned() else { return };
		if let Some(open) = self.docs.get_mut(doc_id)
			&& !open.doc.patterns.iter().any(|q| q.id == pattern)
		{
			open.doc.patterns.push(p);
		}
	}

	/// The hook `command` runs first (M8): patterns a command reads.
	pub(super) fn before_command(&mut self, doc_id: DocId, command: &Command) {
		if let Some(pattern) = pattern_of(command) {
			self.ensure_pattern(doc_id, pattern);
		}
	}

	/// Edit ▸ Define Pattern (M8-T06): the composite of the visible layers in
	/// the selection's bounds (the canvas without one), into the library and
	/// the document.
	fn define_pattern(&mut self, from_selection: bool) {
		let Some(doc_id) = self.docs.active_id() else { return };
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		// FAST: draws every dirty level-0 shape tile, not only those in the rect.
		crate::vector::prepare_level0(&mut open.doc, &store, None);
		let doc = open.doc.clone();
		let (x0, y0, x1, y1) = match (&doc.selection, from_selection) {
			(Some(selection), _) => match selection.canvas_bounds((doc.width, doc.height)) {
				Some(b) => b,
				None => return,
			},
			(None, _) => (0, 0, doc.width, doc.height),
		};
		let (w, h) = (x1 - x0, y1 - y0);
		let max = fx_core::pattern::MAX_SIDE;
		if w == 0 || h == 0 || w > max || h > max {
			self.to_ui(&EngineToUi::Toast {
				text: format!("A pattern is at most {max} × {max} px: select a smaller area"),
			});
			return;
		}
		// FAST: composited on the engine thread (bounded by the pattern size).
		let source = crate::stroke::CompositeTiles::new(doc, (*store).clone());
		let tile = fx_tiles::TILE_SIZE;
		let mut pixels = vec![[0u16; 4]; (w * h) as usize];
		let mut cache: std::collections::HashMap<(u32, u32), fx_ops::brush::stroke::SourceTile> = std::collections::HashMap::new();
		for y in 0..h {
			for x in 0..w {
				let (cx, cy) = (x0 + x, y0 + y);
				let key = (cx / tile, cy / tile);
				if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(key) {
					use fx_ops::brush::SourceTiles;
					match source.tile(key.0, key.1) {
						Ok(t) => {
							e.insert(t);
						}
						Err(_) => return,
					}
				}
				let p = cache[&key][((cy % tile) * tile + cx % tile) as usize];
				let a = p[3];
				let straight = if a > 0.0 { [p[0] / a, p[1] / a, p[2] / a, a] } else { [0.0; 4] };
				pixels[(y * w + x) as usize] = straight.map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16);
			}
		}
		let name = format!("Pattern {}", self.resources.patterns.patterns.len() + 1);
		let pattern = fx_core::pattern::Pattern::new(name, w, h, pixels);
		let id = self.resources.patterns.add(pattern.clone());
		self.resources.patterns.save();
		self.resources.current_pattern = Some(id);
		self.command(doc_id, Command::DefinePattern { pattern });
		self.send_patterns();
	}

	/// An M8 action; `false` when `id` is not one.
	pub(super) fn m8_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		let index = || args.get("index").and_then(|v| v.as_u64()).map(|i| i as usize);
		match id {
			// The UI opens the fill layer's dialog itself (M8-T03).
			"layer:content-options" => return true,
			"misc:define-pattern" | "misc:define-pattern-sel" => {
				self.define_pattern(id == "misc:define-pattern-sel");
				return true;
			}
			"pattern:use" => {
				let pattern = args.get("id").and_then(|v| v.as_u64());
				self.resources.current_pattern = pattern;
				if let (Some(pattern), Some(doc)) = (pattern, self.docs.active_id()) {
					self.ensure_pattern(doc, pattern);
				}
			}
			"pattern:import" => {
				let name = args
					.get("name")
					.and_then(|v| v.as_str())
					.unwrap_or("Pattern")
					.trim_end_matches(".png")
					.to_owned();
				match self.resources.patterns.import_png(&name, &upload(args)) {
					Ok(id) => {
						self.resources.current_pattern = Some(id);
						self.resources.patterns.save();
					}
					Err(error) => self.to_ui(&EngineToUi::Toast {
						text: format!("Could not import the pattern: {error}"),
					}),
				}
			}
			"pattern:delete" => {
				if let Some(pattern) = args.get("id").and_then(|v| v.as_u64()) {
					self.resources.patterns.patterns.retain(|p| p.id != pattern);
					self.resources.patterns.save();
				}
			}
			"pattern:rename" => {
				let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
				if let Some(pattern) = args.get("id").and_then(|v| v.as_u64())
					&& let Some(p) = self.resources.patterns.patterns.iter_mut().find(|p| p.id == pattern)
					&& !name.is_empty()
				{
					p.name = name;
					self.resources.patterns.save();
				}
			}
			// The History panel's source column (M8-T07).
			"hist:source" => {
				let Some(doc) = self.docs.active_id() else { return true };
				let row = args.get("row").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
				self.resources.history_source.insert(doc, row);
				self.to_ui(&EngineToUi::HistorySource { doc, state: Some(row) });
				return true;
			}
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

	/// The History / Art History Brush's stroke tool with the document's
	/// source row (M8-T07).
	pub(super) fn with_history_row(&self, doc: DocId, tool: fx_core::stroke::StrokeTool) -> fx_core::stroke::StrokeTool {
		use fx_core::stroke::StrokeTool;
		let row = self.resources.history_source.get(&doc).copied().unwrap_or(0);
		match tool {
			StrokeTool::HistoryBrush { .. } => StrokeTool::HistoryBrush { state: row },
			StrokeTool::ArtHistory { style, area, tolerance, .. } => StrokeTool::ArtHistory {
				state: row,
				style,
				area,
				tolerance,
			},
			other => other,
		}
	}

	/// Edit ▸ Fill with "Use: Pattern" (M8-T06), with the current pattern.
	pub(super) fn fill_pattern_command(&self, args: &serde_json::Value) -> Option<Command> {
		let pattern = self.resources.current_pattern?;
		let text = |key: &str| args.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
		Some(Command::FillPattern {
			layer: LayerRef::Active,
			pattern,
			mode: crate::tools::kinds::blend_mode(Some(text("mode"))),
			opacity: args
				.get("opacity")
				.and_then(serde_json::Value::as_f64)
				.map_or(1.0, |v| (v / 100.0).clamp(0.0, 1.0)),
			preserve_transparency: args.get("preserve").and_then(serde_json::Value::as_bool).unwrap_or(false),
		})
	}
}

/// The source of a History / Art History stroke (M8-T07): the same layer in
/// the chosen state, or Photoshop's refusal.
pub(super) fn history_source(
	open: &crate::documents::OpenDoc,
	layer: fx_core::LayerId,
	tool: &fx_core::stroke::StrokeTool,
	store: &fx_tiles::TileStore,
) -> Result<Option<std::sync::Arc<dyn fx_ops::brush::SourceTiles>>, String> {
	use fx_core::stroke::StrokeTool;
	let row = match tool {
		StrokeTool::HistoryBrush { state } | StrokeTool::ArtHistory { state, .. } => *state,
		_ => return Ok(None),
	};
	let Some(state) = open.history.state(row, &open.doc) else {
		return Err("Could not use the history brush because the source history state is gone".into());
	};
	if (state.width, state.height) != (open.doc.width, open.doc.height) {
		return Err("Could not use the history brush because the history state does not contain a corresponding layer (the canvas size does not match)".into());
	}
	match state.layer(layer).map(|l| &l.kind) {
		Some(fx_core::LayerKind::Pixel { image, offset }) => Ok(Some(std::sync::Arc::new(fx_ops::brush::LayerSource {
			image: image.clone(),
			offset: *offset,
			store: store.clone(),
		}))),
		_ => Err("Could not use the history brush because the history state does not contain a corresponding layer".into()),
	}
}
