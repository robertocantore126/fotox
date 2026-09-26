//! The Move tool (V), M7-T02.
//!
//! A drag moves the selected layers (or, with a pixel selection, the selected
//! pixels of the active layer). While dragging, the document is edited
//! outside the history (offsets and matrices only, cheap); the drop restores
//! it and returns **one** `OffsetLayers` command. Alt duplicates first,
//! Ctrl (or the Auto-Select option) picks the layer under the pointer, Shift
//! constrains to 0°/45°/90°. The arrows nudge 1 px (Shift: 10), a burst of
//! arrows being one history step.

use std::time::{Duration, Instant};

use fx_core::{Command, CommandContext, Document, LayerId, LayerKind, LayerRef, Mapping};
use fx_tiles::{TILE_SIZE, TileSlot, TileStore};

use super::{DocPointer, Tool, ToolContext, ToolResult};
use crate::{CursorShape, Modifiers, PointerKind};

const TOOL: &str = "move";
/// Arrow presses closer than this are one history step (the engine's merge window).
const NUDGE_BURST: Duration = Duration::from_millis(900);

struct Drag {
	start: (f64, f64),
	before: Document,
	/// Moving the selected pixels (a selection was up) instead of layers.
	pixels: bool,
	duplicate: bool,
	delta: (i32, i32),
	/// The layers selected when the drag started (after an auto-select).
	selected: Vec<LayerId>,
}

#[derive(Default)]
pub struct MoveTool {
	drag: Option<Drag>,
	nudge: Option<(Instant, i32, i32)>,
}

impl MoveTool {
	fn preview(&mut self, ctx: &mut ToolContext<'_>) {
		let Some(drag) = &self.drag else { return };
		*ctx.doc = drag.before.clone();
		if drag.pixels {
			// FAST: no live preview of a pixel move; the ants follow instead.
			return;
		}
		let _ = Command::OffsetLayers {
			layers: Vec::new(),
			dx: drag.delta.0,
			dy: drag.delta.1,
		}
		.apply(ctx.doc, &mut CommandContext { tiles: ctx.store, ops: None });
	}
}

impl Tool for MoveTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down => {
				let auto = event.modifiers.ctrl || ctx.settings.bool(TOOL, "Auto-Select").unwrap_or(false);
				if auto {
					let group = ctx.settings.string(TOOL, "Select").as_deref() == Some("Group");
					match layer_at(ctx.doc, ctx.store, event.x, event.y, group) {
						Some(id) => ctx.doc.selected = vec![id],
						None => return ToolResult::default(),
					}
				}
				if ctx.doc.selected.is_empty() {
					return ToolResult {
						info: Some("Select a layer to move".into()),
						..Default::default()
					};
				}
				let pixels = ctx.doc.selection.is_some();
				self.drag = Some(Drag {
					start: (event.x, event.y),
					before: ctx.doc.clone(),
					pixels,
					duplicate: event.modifiers.alt && !pixels,
					delta: (0, 0),
					selected: ctx.doc.selected.clone(),
				});
				ToolResult {
					cursor: Some(CursorShape::Move),
					doc_changed: auto,
					..Default::default()
				}
			}
			PointerKind::Move => {
				let Some(drag) = &mut self.drag else {
					return ToolResult::default();
				};
				let (mut dx, mut dy) = (event.x - drag.start.0, event.y - drag.start.1);
				if event.modifiers.shift {
					// 0°, 45° or 90°.
					let (ax, ay) = (dx.abs(), dy.abs());
					if ax > 2.0 * ay {
						dy = 0.0;
					} else if ay > 2.0 * ax {
						dx = 0.0;
					} else {
						let d = (ax + ay) / 2.0;
						dx = d * dx.signum();
						dy = d * dy.signum();
					}
				}
				let delta = (dx.round() as i32, dy.round() as i32);
				if delta == drag.delta {
					return ToolResult::default();
				}
				drag.delta = delta;
				let pixels = drag.pixels;
				self.preview(ctx);
				ToolResult {
					doc_changed: !pixels,
					redraw: true,
					cursor: Some(CursorShape::Move),
					status: Some(format!("Δx: {} px  Δy: {} px", delta.0, delta.1)),
					..Default::default()
				}
			}
			PointerKind::Up => {
				let Some(drag) = self.drag.take() else {
					return ToolResult::default();
				};
				*ctx.doc = drag.before;
				// An auto-select stays (it is the panel's selection, not a step).
				ctx.doc.selected = drag.selected;
				let mut result = ToolResult {
					doc_changed: true,
					redraw: true,
					status: Some(String::new()),
					..Default::default()
				};
				let (dx, dy) = drag.delta;
				if dx == 0 && dy == 0 {
					return result;
				}
				if drag.pixels {
					result.command = Some(Command::Transform {
						layer: LayerRef::Active,
						mapping: Box::new(Mapping::translation(f64::from(dx), f64::from(dy))),
						filter: fx_core::Filter::Nearest,
					});
					return result;
				}
				let offset = Command::OffsetLayers { layers: Vec::new(), dx, dy };
				if drag.duplicate {
					// FAST: two history steps (Duplicate, then Move).
					result.command = Some(Command::DuplicateLayers {
						layers: ctx.doc.selected.iter().map(|&id| LayerRef::Id(id)).collect(),
					});
					result.then = vec![offset];
				} else {
					result.command = Some(offset);
				}
				result
			}
			_ => ToolResult::default(),
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		let (step, arrow) = match key.strip_prefix("Shift+") {
			Some(arrow) => (10, arrow),
			None => (1, key),
		};
		let (dx, dy) = match arrow {
			"ArrowLeft" => (-step, 0),
			"ArrowRight" => (step, 0),
			"ArrowUp" => (0, -step),
			"ArrowDown" => (0, step),
			_ => return ToolResult::default(),
		};
		if ctx.doc.selected.is_empty() {
			return ToolResult::default();
		}
		let now = Instant::now();
		// The engine merges a repeat of the same edit by undoing the last one and
		// applying the new command instead, so a burst sends the running total.
		let (tx, ty) = match self.nudge {
			Some((at, x, y)) if now.duration_since(at) < NUDGE_BURST => (x + dx, y + dy),
			_ => (dx, dy),
		};
		self.nudge = Some((now, tx, ty));
		ToolResult {
			command: Some(Command::OffsetLayers {
				layers: Vec::new(),
				dx: tx,
				dy: ty,
			}),
			..Default::default()
		}
	}

	fn deactivate(&mut self, ctx: &mut ToolContext<'_>) -> ToolResult {
		match self.drag.take() {
			Some(drag) => {
				*ctx.doc = drag.before;
				ToolResult {
					doc_changed: true,
					..Default::default()
				}
			}
			None => ToolResult::default(),
		}
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Move
	}
}

/// The topmost visible layer whose pixel at `(x, y)` has alpha > 0, read
/// from the layers' own level-0 tiles (never a composite). With `group`, the
/// top-level group holding it.
pub fn layer_at(doc: &Document, store: &TileStore, x: f64, y: f64, group: bool) -> Option<LayerId> {
	if x < 0.0 || y < 0.0 {
		return None;
	}
	let (px, py) = (x.floor() as i64, y.floor() as i64);
	let mut hit = None;
	// `walk` is bottom → top: the last hit is the topmost one.
	doc.walk(|layer, _| {
		if !layer.visible {
			return;
		}
		let (image, offset) = match &layer.kind {
			LayerKind::Pixel { image, offset } => (image, (i64::from(offset.0), i64::from(offset.1))),
			LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } => (cache, (0, 0)),
			LayerKind::SolidFill { .. } => {
				hit = Some(layer.id);
				return;
			}
			_ => return,
		};
		let (ix, iy) = (px - offset.0, py - offset.1);
		if ix < 0 || iy < 0 || ix >= i64::from(image.width()) || iy >= i64::from(image.height()) {
			return;
		}
		let t = i64::from(TILE_SIZE);
		let (tx, ty) = ((ix / t) as u32, (iy / t) as u32);
		if image.is_derived() && image.is_dirty(0, tx, ty) {
			return; // FAST: a shape tile never drawn at level 0 is not picked
		}
		let alpha = match image.slot(0, tx, ty) {
			TileSlot::Empty => 0,
			TileSlot::Solid(v) => v.0[3],
			TileSlot::Data(handle) => match store.get(handle) {
				Ok(buffer) => {
					let i = ((iy % t) * t + ix % t) as usize;
					match buffer.format() {
						fx_tiles::PixelFormat::Rgba8 => u16::from(buffer.bytes()[i * 4 + 3]),
						fx_tiles::PixelFormat::Rgba16 => buffer.as_u16()[i * 4 + 3],
						_ => 0,
					}
				}
				Err(_) => 0,
			},
		};
		if alpha > 0 {
			hit = Some(layer.id);
		}
	});
	let id = hit?;
	if group {
		let path = doc.path_of(id)?;
		let root = doc.layers.get(*path.first()?)?;
		return Some(root.id);
	}
	Some(id)
}
