//! The engine side of M10: the Paths panel's list and actions (T01),
//! Stroke Path.

use fx_core::path::PathTarget;
use fx_core::stroke::{BrushParams, StrokeSample, StrokeTarget, StrokeTool};
use fx_core::{Command, LayerRef};
use fx_protocol::{DocId, EngineToUi};

use super::Engine;

fn target_json(t: Option<PathTarget>) -> serde_json::Value {
	serde_json::to_value(t).unwrap_or_default()
}

fn target_of(args: &serde_json::Value) -> Option<PathTarget> {
	args.get("target").and_then(|t| serde_json::from_value(t.clone()).ok())
}

impl Engine {
	pub(super) fn send_paths(&self, id: DocId) {
		let Some(open) = self.docs.get(id) else { return };
		self.to_ui(&EngineToUi::Paths {
			doc: id,
			work: open.doc.work_path.is_some(),
			paths: open.doc.paths.iter().map(|p| p.name.clone()).collect(),
			active: target_json(open.doc.active_path),
		});
	}

	/// The brush of a painting tool's option bar, for Stroke Path.
	fn brush_of(&self, tool: &str) -> BrushParams {
		let s = &self.settings;
		let percent = |key: &str, default: f64| (s.number(tool, key).unwrap_or(default) / 100.0).clamp(0.0, 1.0) as f32;
		BrushParams {
			diameter: s.number(tool, "Size").unwrap_or(if tool == "pencil" { 3.0 } else { 20.0 }).clamp(1.0, 5000.0) as f32,
			hardness: percent("Hardness", 100.0),
			opacity: percent("Opacity", 100.0),
			flow: percent("Flow", 100.0),
			mode: crate::tools::kinds::blend_mode(s.string(tool, "Mode")),
			..BrushParams::default()
		}
	}

	/// Stroke Path (M10-T01): the path fed to the brush engine as a stroke
	/// of the chosen tool, with or without simulated pressure.
	fn stroke_path(&mut self, doc_id: DocId, target: PathTarget, tool_id: &str, simulate_pressure: bool) {
		let Some(open) = self.docs.get(doc_id) else { return };
		let Some(path) = open.doc.path(target).cloned() else {
			self.to_ui(&EngineToUi::Toast {
				text: "Select a path in the Paths panel first".into(),
			});
			return;
		};
		let (tool, color) = match tool_id {
			"pencil" => (StrokeTool::Pencil, self.settings.fg),
			"eraser" => (StrokeTool::Eraser, self.settings.bg),
			_ => (StrokeTool::Brush, self.settings.fg),
		};
		let mut brush = self.brush_of(tool_id);
		brush.pressure_size = simulate_pressure;
		// One stroke per subpath.
		for (points, closed, _) in path.flatten(0.5) {
			let mut pts = points;
			if closed && let Some(&first) = pts.first() {
				pts.push(first);
			}
			let n = pts.len().max(2) as f32;
			let samples: Vec<StrokeSample> = pts
				.iter()
				.enumerate()
				.map(|(i, p)| {
					// Simulated pressure: thin at both ends.
					let t = i as f32 / (n - 1.0);
					let pressure = if simulate_pressure { (std::f32::consts::PI * t).sin().max(0.05) } else { 1.0 };
					StrokeSample {
						x: p.0,
						y: p.1,
						pressure,
						tilt_x: 0.0,
						tilt_y: 0.0,
						time_us: 0,
					}
				})
				.collect();
			if samples.is_empty() {
				continue;
			}
			self.command(
				doc_id,
				Command::Stroke {
					layer: LayerRef::Active,
					target: StrokeTarget::Pixels,
					tool,
					brush,
					color,
					samples,
				},
			);
		}
	}

	/// An M10 action; `false` when `id` is not one.
	/// Layer ▸ Vector Mask (M10-T06).
	fn vector_mask_action(&mut self, doc_id: DocId, id: &str) {
		use fx_core::path::{Anchor, Path, Subpath};
		use fx_core::select_ops::VectorMaskSpec;
		let Some(open) = self.docs.get(doc_id) else { return };
		let doc = &open.doc;
		let Some(layer) = doc.active_layer().and_then(|l| doc.layer(l)) else { return };
		let current = layer.vector_mask.as_ref().map(|v| VectorMaskSpec {
			path: v.path.clone(),
			enabled: v.enabled,
			feather: v.feather,
			density: v.density,
		});
		let canvas = || {
			let (w, h) = (f64::from(doc.width), f64::from(doc.height));
			Path {
				subpaths: vec![Subpath {
					anchors: vec![
						Anchor::corner((0.0, 0.0)),
						Anchor::corner((w, 0.0)),
						Anchor::corner((w, h)),
						Anchor::corner((0.0, h)),
					],
					closed: true,
					op: Default::default(),
				}],
			}
		};
		let spec = |path: Path| VectorMaskSpec {
			path,
			enabled: true,
			feather: 0.0,
			density: 1.0,
		};
		let (mask, label) = match id {
			"vmask:reveal-all" => (Some(spec(canvas())), "Add Vector Mask"),
			"vmask:hide-all" => (Some(spec(Path::default())), "Add Vector Mask"),
			"vmask:current-path" => {
				let Some(path) = doc.active_path.and_then(|t| doc.path(t)).or(doc.work_path.as_ref()).cloned() else {
					self.to_ui(&EngineToUi::Toast {
						text: "Select a path in the Paths panel first".into(),
					});
					return;
				};
				(Some(spec(path)), "Add Vector Mask")
			}
			"vmask:delete" => (None, "Delete Vector Mask"),
			"vmask:toggle" => {
				let Some(mut c) = current else { return };
				c.enabled = !c.enabled;
				let label = if c.enabled { "Enable Vector Mask" } else { "Disable Vector Mask" };
				(Some(c), label)
			}
			// The path goes to the Work Path for the pen tools to edit.
			"vmask:edit" => {
				let Some(c) = current else { return };
				self.command(
					doc_id,
					Command::SetPath {
						target: PathTarget::Work,
						path: c.path,
						name: None,
						label: "Work Path".into(),
					},
				);
				return;
			}
			_ => return,
		};
		self.command(
			doc_id,
			Command::SetVectorMask {
				layer: LayerRef::Active,
				mask,
				label: label.into(),
			},
		);
	}

	pub(super) fn m10_action(&mut self, id: &str, args: &serde_json::Value) -> bool {
		if id.starts_with("vmask:") {
			if let Some(doc_id) = self.docs.active_id() {
				self.vector_mask_action(doc_id, id);
			}
			return true;
		}
		if !id.starts_with("path:") {
			return false;
		}
		let Some(doc_id) = self.docs.active_id() else { return true };
		let active = self.docs.get(doc_id).and_then(|o| o.doc.active_path);
		let target = target_of(args).or(active).unwrap_or(PathTarget::Work);
		match id {
			"path:stroke" => {
				let tool = args.get("tool").and_then(|v| v.as_str()).unwrap_or("brush").to_owned();
				let pressure = args.get("pressure").and_then(|v| v.as_bool()).unwrap_or(false);
				self.stroke_path(doc_id, target, &tool, pressure);
			}
			"path:fill" => {
				let source = match (args.get("use").and_then(|v| v.as_str()), self.resources.current_pattern) {
					(Some("Pattern"), Some(pattern)) => fx_core::fill::FillSource::Pattern { pattern },
					(Some("Background Colour"), _) => fx_core::fill::FillSource::Color { rgba: self.settings.bg },
					_ => fx_core::fill::FillSource::Color { rgba: self.settings.fg },
				};
				self.command(
					doc_id,
					Command::FillPath {
						target,
						source,
						mode: fx_core::BlendMode::Normal,
						opacity: 1.0,
					},
				);
			}
			"path:to-selection" => {
				let mode = match args.get("mode").and_then(|v| v.as_str()) {
					Some("add") => fx_core::SelectMode::Add,
					Some("subtract") => fx_core::SelectMode::Subtract,
					Some("intersect") => fx_core::SelectMode::Intersect,
					_ => fx_core::SelectMode::Replace,
				};
				let feather = args.get("feather").and_then(|v| v.as_f64()).unwrap_or(0.0);
				self.command(
					doc_id,
					Command::PathToSelection {
						target,
						feather,
						anti_alias: true,
						mode,
					},
				);
			}
			"path:from-selection" => {
				let tolerance = args.get("tolerance").and_then(|v| v.as_f64()).unwrap_or(2.0);
				self.command(doc_id, Command::SelectionToPath { tolerance });
			}
			"path:save" => {
				let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
				self.command(doc_id, Command::SaveWorkPath { name });
			}
			"path:new" => {
				let n = self.docs.get(doc_id).map_or(0, |o| o.doc.paths.len());
				self.command(
					doc_id,
					Command::SetPath {
						target: PathTarget::Saved(n),
						path: Default::default(),
						name: None,
						label: "New Path".into(),
					},
				);
			}
			"path:delete" => self.command(doc_id, Command::DeletePath { target }),
			"path:rename" => {
				if let PathTarget::Saved(index) = target {
					let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
					self.command(doc_id, Command::RenamePath { index, name });
				}
			}
			"path:select" => {
				let t = target_of(args);
				self.command(doc_id, Command::SelectPath { target: t });
				self.send_paths(doc_id);
				self.request_frame();
			}
			_ => return false,
		}
		true
	}
}
