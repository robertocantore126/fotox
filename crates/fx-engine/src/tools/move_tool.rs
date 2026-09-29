//! The Move tool (V), M7-T02.
//!
//! A drag moves the selected layers (or, with a pixel selection, the selected
//! pixels of the active layer). While dragging, the document is edited
//! outside the history (offsets and matrices only, cheap); the drop restores
//! it and returns **one** history step. The arrows nudge 1 px (Shift: 10),
//! a burst of arrows being one history step.
//!
//! Modifiers, as in Photoshop:
//! * Ctrl inverts the Auto-Select option while held (on → off, off → on), so
//!   the active layer can be dragged when another one covers the pointer.
//! * Shift+click (Auto-Select) adds the layer under the pointer to the
//!   selection, or takes it out when it was in; once dragging, Shift
//!   constrains to 0°/45°/90°.
//! * Alt duplicates: the layers, or with a pixel selection the selected
//!   pixels, in one history step.
//!
//! Auto-Select clicks through what cannot be moved: hidden layers, fully
//! locked ones (Rob's rule: a locked layer is invisible to tools, the Layers
//! panel is the way to reach it) and position-locked ones. Clicking a layer
//! that is already part of a multi-selection keeps the selection.

use std::time::{Duration, Instant};

use fx_core::{Command, CommandContext, Document, LayerId, LayerKind, LayerRef};
use fx_tiles::TileStore;

use super::{DocPointer, Tool, ToolContext, ToolResult};
use crate::{CursorShape, Modifiers, PointerKind};

const TOOL: &str = "move";
/// Arrow presses closer than this are one history step (the engine's merge window).
const NUDGE_BURST: Duration = Duration::from_millis(900);
/// Auto-Select ignores pixels fainter than this (a soft brush's halo, a
/// texture's haze), so they do not steal clicks from what is under them.
/// Photoshop's own threshold is not known; this one is a choice.
pub const PICK_ALPHA: f64 = 10.0 / 255.0;

struct Drag {
	start: (f64, f64),
	before: Document,
	/// Moving the selected pixels (a selection was up) instead of layers.
	pixels: bool,
	duplicate: bool,
	delta: (i32, i32),
	/// The layers selected when the drag started (after an auto-select).
	selected: Vec<LayerId>,
	/// Shift+click on a layer that was already selected: it leaves the
	/// selection on release, if the pointer did not move.
	deselect_on_click: Option<LayerId>,
}

struct Nudge {
	at: Instant,
	total: (i32, i32),
	/// What the burst moves; another selection starts a new burst.
	layers: Vec<LayerId>,
	pixels: bool,
}

#[derive(Default)]
pub struct MoveTool {
	drag: Option<Drag>,
	nudge: Option<Nudge>,
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

/// Why the current target cannot move, as a toast; `None` when it can.
fn refusal(doc: &Document, pixels: bool) -> Option<String> {
	if pixels {
		let id = doc.active_layer()?;
		let layer = doc.layer(id)?;
		if !matches!(layer.kind, LayerKind::Pixel { .. }) {
			return Some(format!("“{}” has no pixels to move: rasterize it, or deselect to move the whole layer", layer.name));
		}
		let locks = doc.locks(id);
		return (locks.pixels || locks.position).then(|| locked_message(doc, id));
	}
	let mut moving = Vec::new();
	for &id in &doc.selected {
		collect(doc, id, &mut moving);
	}
	moving.into_iter().find(|&id| doc.locks(id).position).map(|id| locked_message(doc, id))
}

fn collect(doc: &Document, id: LayerId, out: &mut Vec<LayerId>) {
	out.push(id);
	if let Some(children) = doc.layer(id).and_then(|l| l.children()) {
		for child in children {
			collect(doc, child.id, out);
		}
	}
}

fn locked_message(doc: &Document, id: LayerId) -> String {
	let name = doc.layer(id).map_or("The layer", |l| l.name.as_str());
	format!("“{name}” is locked: unlock it in the Layers panel to move it")
}

impl Tool for MoveTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down => {
				self.nudge = None;
				// On unless the option bar says off (the app starts with it on);
				// Ctrl inverts it while held.
				let setting = ctx.settings.bool(TOOL, "Auto-Select").unwrap_or(true);
				let auto = setting != event.modifiers.ctrl;
				let mut deselect_on_click = None;
				let mut selection_changed = false;
				if auto {
					let group = ctx.settings.string(TOOL, "Select").as_deref() == Some("Group");
					let Some(id) = layer_at(ctx.doc, ctx.store, event.x, event.y, group) else {
						// Nothing movable under the pointer: keep the selection.
						return ToolResult {
							status: Some("Nothing to move here (hidden and locked layers are skipped)".into()),
							..Default::default()
						};
					};
					let already = ctx.doc.with_ancestors(id).iter().any(|l| ctx.doc.selected.contains(&l.id));
					if event.modifiers.shift {
						if ctx.doc.selected.contains(&id) {
							deselect_on_click = Some(id);
						} else {
							ctx.doc.selected.push(id);
							selection_changed = true;
						}
					} else if !already {
						ctx.doc.selected = vec![id];
						selection_changed = true;
					}
				}
				if ctx.doc.selected.is_empty() {
					return ToolResult {
						info: Some("Select a layer to move".into()),
						..Default::default()
					};
				}
				let pixels = ctx.doc.selection.is_some();
				if deselect_on_click.is_none()
					&& let Some(text) = refusal(ctx.doc, pixels)
				{
					return ToolResult {
						info: Some(text),
						doc_changed: selection_changed,
						..Default::default()
					};
				}
				self.drag = Some(Drag {
					start: (event.x, event.y),
					before: ctx.doc.clone(),
					pixels,
					duplicate: event.modifiers.alt,
					delta: (0, 0),
					selected: ctx.doc.selected.clone(),
					deselect_on_click,
				});
				ToolResult {
					cursor: Some(CursorShape::Move),
					doc_changed: selection_changed,
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
					if let Some(id) = drag.deselect_on_click
						&& ctx.doc.selected.len() > 1
					{
						ctx.doc.selected.retain(|&s| s != id);
					}
					return result;
				}
				if drag.deselect_on_click.is_some()
					&& let Some(text) = refusal(ctx.doc, drag.pixels)
				{
					result.info = Some(text);
					return result;
				}
				result.command = Some(if drag.pixels {
					// Wrapped so the drop is its own step: a bare `MovePixels`
					// would fold into an arrow nudge made just before.
					Command::Sequence {
						commands: vec![Command::MovePixels { dx, dy, copy: drag.duplicate }],
						label: if drag.duplicate { "Duplicate Pixels".into() } else { "Move".into() },
					}
				} else if drag.duplicate {
					Command::Sequence {
						commands: vec![
							Command::DuplicateLayers {
								layers: ctx.doc.selected.iter().map(|&id| LayerRef::Id(id)).collect(),
							},
							// The copies are the selection now.
							Command::OffsetLayers { layers: Vec::new(), dx, dy },
						],
						label: "Duplicate and Move".into(),
					}
				} else {
					Command::OffsetLayers { layers: Vec::new(), dx, dy }
				});
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
		// With a pixel selection the arrows move the selected pixels, as a drag does.
		let pixels = ctx.doc.selection.is_some();
		if let Some(text) = refusal(ctx.doc, pixels) {
			return ToolResult {
				info: Some(text),
				..Default::default()
			};
		}
		let now = Instant::now();
		let layers = ctx.doc.selected.clone();
		// The engine merges a repeat of the same edit by undoing the last one and
		// applying the new command instead, so a burst sends the running total.
		let total = match &self.nudge {
			Some(n) if now.duration_since(n.at) < NUDGE_BURST && n.layers == layers && n.pixels == pixels => (n.total.0 + dx, n.total.1 + dy),
			_ => (dx, dy),
		};
		self.nudge = Some(Nudge {
			at: now,
			total,
			layers: layers.clone(),
			pixels,
		});
		let command = if pixels {
			Command::MovePixels {
				dx: total.0,
				dy: total.1,
				copy: false,
			}
		} else {
			// Named layers: the engine merges only nudges of the same layers.
			Command::OffsetLayers {
				layers: layers.into_iter().map(LayerRef::Id).collect(),
				dx: total.0,
				dy: total.1,
			}
		};
		ToolResult {
			command: Some(command),
			..Default::default()
		}
	}

	fn deactivate(&mut self, ctx: &mut ToolContext<'_>) -> ToolResult {
		self.nudge = None;
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

/// What Auto-Select picks at `(x, y)`: the topmost layer showing a pixel
/// there ([`crate::derived::alpha_at`]: through its masks, its groups'
/// masks and opacity, its clipping base) that the Move tool can move.
/// Hidden, fully locked and position-locked layers are clicked through, and
/// adjustment layers are never picked. With `group`, the top-level group
/// holding the layer.
pub fn layer_at(doc: &Document, store: &TileStore, x: f64, y: f64, group: bool) -> Option<LayerId> {
	let id = doc.panel_order().into_iter().find(|&id| {
		let Some(layer) = doc.layer(id) else { return false };
		!matches!(layer.kind, LayerKind::Group { .. } | LayerKind::Adjustment(_))
			&& !doc.tool_ignored(id)
			&& !doc.locks(id).position
			&& crate::derived::alpha_at(doc, store, id, x, y) >= PICK_ALPHA
	})?;
	if group {
		let path = doc.path_of(id)?;
		let root = doc.layers.get(*path.first()?)?;
		return Some(root.id);
	}
	Some(id)
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use fx_core::{Command, CommandContext, Layer, LayerId, LayerKind, SelectionShape};
	use fx_tiles::{PixelFormat, PixelValue, TileSlot, TiledImage};

	use super::*;
	use crate::tools::testing::Fixture;

	const PHOTO: LayerId = LayerId(1);
	const TEXTURE: LayerId = LayerId(2);
	const OTHER: LayerId = LayerId(3);

	/// A 512 × 512 pixel layer, opaque in the tiles `keep` says (256² each).
	fn pixels(id: LayerId, name: &str, keep: impl Fn(u32, u32) -> bool) -> Layer {
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba8);
		for ty in 0..2 {
			for tx in 0..2 {
				if keep(tx, ty) {
					image.set_slot(tx, ty, TileSlot::Solid(PixelValue([40000, 20000, 10000, 65535])));
				}
			}
		}
		Layer::new(id, name, LayerKind::Pixel { image, offset: (0, 0) })
	}

	fn lock_all(layer: &mut Layer) {
		layer.locked_pixels = true;
		layer.locked_position = true;
		layer.locked_transparency = true;
	}

	/// "photo" in the top-left tile, "other" in the bottom-right one, and on
	/// top a full-canvas Screen "texture" (Rob's case, 2026-09-29).
	fn fixture(locked: bool) -> Fixture {
		let mut f = Fixture::new("move-locks", (512, 512), 1.0);
		f.doc.layers.push(Arc::new(pixels(PHOTO, "photo", |x, y| (x, y) == (0, 0))));
		f.doc.layers.push(Arc::new(pixels(OTHER, "other", |x, y| (x, y) == (1, 1))));
		let mut texture = pixels(TEXTURE, "texture", |_, _| true);
		texture.blend = fx_core::BlendMode::Screen;
		if locked {
			lock_all(&mut texture);
		}
		f.doc.layers.push(Arc::new(texture));
		f.doc.selected = vec![OTHER];
		// The ids above were set by hand: new layers (duplicates) start past them.
		let (_, names) = f.doc.id_state();
		f.doc = f.doc.clone().with_id_state(100, names);
		f
	}

	fn run(f: &mut Fixture, result: ToolResult) -> fx_core::CommandEffect {
		let command = result.command.expect("the tool sent a command");
		command.apply(&mut f.doc, &mut CommandContext { tiles: &f.store, ops: Some(&f.ops) }).expect("the command applies")
	}

	fn offset(f: &Fixture, id: LayerId) -> (i32, i32) {
		match &f.doc.layer(id).unwrap().kind {
			LayerKind::Pixel { offset, .. } => *offset,
			_ => panic!("not a pixel layer"),
		}
	}

	const NONE: Modifiers = Modifiers {
		shift: false,
		ctrl: false,
		alt: false,
		space: false,
	};

	#[test]
	fn auto_select_clicks_through_a_locked_layer() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		let result = f.drag(&mut tool, &[(100.0, 100.0), (110.0, 105.0)], NONE);
		assert_eq!(f.doc.selected, vec![PHOTO], "the photo under the locked texture");
		run(&mut f, result);
		assert_eq!(offset(&f, PHOTO), (10, 5));
		assert_eq!(offset(&f, TEXTURE), (0, 0));
	}

	#[test]
	fn unlocked_the_texture_is_what_a_click_picks() {
		let mut tool = MoveTool::default();
		let mut f = fixture(false);
		f.drag(&mut tool, &[(100.0, 100.0), (110.0, 100.0)], NONE);
		assert_eq!(f.doc.selected, vec![TEXTURE]);
	}

	#[test]
	fn a_locked_group_hides_its_children_from_auto_select() {
		let mut tool = MoveTool::default();
		let mut f = fixture(false);
		let texture = f.doc.layers.pop().unwrap();
		let mut group = Layer::new(
			LayerId(9),
			"locked group",
			LayerKind::Group {
				children: vec![texture],
				expanded: true,
			},
		);
		lock_all(&mut group);
		f.doc.layers.push(Arc::new(group));
		assert!(f.doc.tool_ignored(TEXTURE), "the group's lock covers its child");
		f.drag(&mut tool, &[(100.0, 100.0), (110.0, 100.0)], NONE);
		assert_eq!(f.doc.selected, vec![PHOTO]);
	}

	#[test]
	fn a_click_on_only_locked_pixels_keeps_the_selection() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		// (300, 100): only the texture is there.
		let result = f.drag(&mut tool, &[(300.0, 100.0), (320.0, 100.0)], NONE);
		assert!(result.command.is_none(), "nothing moves");
		assert_eq!(f.doc.selected, vec![OTHER], "the selection is kept");
		assert_eq!(offset(&f, OTHER), (0, 0));
	}

	#[test]
	fn ctrl_turns_auto_select_off_while_held() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		let ctrl = Modifiers { ctrl: true, ..NONE };
		let result = f.drag(&mut tool, &[(100.0, 100.0), (120.0, 100.0)], ctrl);
		assert_eq!(f.doc.selected, vec![OTHER], "Ctrl: the active layer moves, whatever is under the pointer");
		run(&mut f, result);
		assert_eq!(offset(&f, OTHER), (20, 0));
		// Auto-Select off in the bar: Ctrl turns it on.
		f.options(TOOL, serde_json::json!({ "Auto-Select": false }));
		f.drag(&mut tool, &[(100.0, 100.0), (101.0, 100.0)], ctrl);
		assert_eq!(f.doc.selected, vec![PHOTO]);
	}

	#[test]
	fn a_click_on_a_layer_of_a_multi_selection_keeps_it() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		f.doc.selected = vec![PHOTO, OTHER];
		let result = f.drag(&mut tool, &[(100.0, 100.0), (130.0, 100.0)], NONE);
		assert_eq!(f.doc.selected, vec![PHOTO, OTHER]);
		run(&mut f, result);
		assert_eq!(offset(&f, PHOTO), (30, 0));
		assert_eq!(offset(&f, OTHER), (30, 0));
	}

	#[test]
	fn shift_click_adds_and_removes() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		let shift = Modifiers { shift: true, ..NONE };
		f.drag(&mut tool, &[(100.0, 100.0)], shift);
		assert_eq!(f.doc.selected, vec![OTHER, PHOTO]);
		f.drag(&mut tool, &[(100.0, 100.0)], shift);
		assert_eq!(f.doc.selected, vec![OTHER]);
	}

	#[test]
	fn alt_drag_duplicates_in_one_step() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		let result = f.drag(&mut tool, &[(100.0, 100.0), (150.0, 100.0)], Modifiers { alt: true, ..NONE });
		let effect = run(&mut f, result);
		assert_eq!(effect.label, "Duplicate and Move");
		assert_eq!(offset(&f, PHOTO), (0, 0), "the original stays");
		let copy = *f.doc.selected.last().unwrap();
		assert_ne!(copy, PHOTO);
		assert_eq!(offset(&f, copy), (50, 0), "the copy moved");
	}

	#[test]
	fn a_locked_layer_selected_in_the_panel_refuses_to_move() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		f.doc.selected = vec![TEXTURE];
		f.options(TOOL, serde_json::json!({ "Auto-Select": false }));
		let down = f.pointer(&mut tool, PointerKind::Down, 100.0, 100.0, NONE);
		assert!(down.info.as_deref().is_some_and(|t| t.contains("texture")), "{:?}", down.info);
		let result = f.drag(&mut tool, &[(100.0, 100.0), (150.0, 100.0)], NONE);
		assert!(result.command.is_none());
		assert!(f.key(&mut tool, "ArrowRight").command.is_none(), "a nudge is refused too");
	}

	#[test]
	fn with_a_selection_arrows_and_alt_drag_move_pixels() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		f.doc.selected = vec![PHOTO];
		f.select(SelectionShape::Rect {
			x: 0.0,
			y: 0.0,
			w: 64.0,
			h: 64.0,
		});
		let nudge = f.key(&mut tool, "ArrowRight");
		assert!(
			matches!(nudge.command, Some(Command::MovePixels { dx: 1, dy: 0, copy: false })),
			"{:?}",
			nudge.command
		);
		let result = f.drag(&mut tool, &[(10.0, 10.0), (310.0, 10.0)], Modifiers { alt: true, ..NONE });
		let effect = run(&mut f, result);
		assert_eq!(effect.label, "Duplicate Pixels");
		let composite = |x: f64, y: f64| crate::derived::alpha_at(&f.doc, &f.store, PHOTO, x, y);
		assert!(composite(10.0, 10.0) > 0.5, "the original pixels stay");
		assert!(composite(320.0, 10.0) > 0.5, "the copy landed 300 px right");
		assert!(composite(320.0, 200.0) < 0.01, "only the selected 64² were copied");
	}

	#[test]
	fn nudges_of_other_layers_do_not_share_a_burst() {
		let mut tool = MoveTool::default();
		let mut f = fixture(true);
		f.doc.selected = vec![PHOTO];
		f.key(&mut tool, "ArrowRight");
		let second = f.key(&mut tool, "ArrowRight");
		assert!(matches!(&second.command, Some(Command::OffsetLayers { layers, dx: 2, .. }) if layers == &vec![LayerRef::Id(PHOTO)]));
		f.doc.selected = vec![OTHER];
		let third = f.key(&mut tool, "ArrowRight");
		assert!(matches!(&third.command, Some(Command::OffsetLayers { layers, dx: 1, .. }) if layers == &vec![LayerRef::Id(OTHER)]));
	}

	#[test]
	fn invisible_pixels_are_not_picked() {
		let mut f = fixture(false);
		// 0 % opacity: not shown, not picked.
		f.doc.layer_mut(TEXTURE).unwrap().opacity = 0.0;
		assert_eq!(layer_at(&f.doc, &f.store, 100.0, 100.0, false), Some(PHOTO));
		// Clipped to "other" (bottom-right only): shown there, not over the photo.
		let texture = f.doc.layer_mut(TEXTURE).unwrap();
		texture.opacity = 1.0;
		texture.clipped = true;
		assert_eq!(layer_at(&f.doc, &f.store, 100.0, 100.0, false), Some(PHOTO));
		assert_eq!(layer_at(&f.doc, &f.store, 400.0, 400.0, false), Some(TEXTURE));
	}
}
