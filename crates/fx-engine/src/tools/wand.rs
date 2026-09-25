//! The Magic Wand tool (`W`, M5-T04).
//!
//! A click selects the pixels whose colour is within the tolerance of the
//! clicked one (`fx_ops::flood`), combined with the current selection by the
//! mode the modifiers or the option bar choose. The click is one
//! [`Command::MagicWand`], run as a job: on a large document the flood can take
//! a moment, and the UI stays live meanwhile.

use fx_core::{Command, SelectMode, WandParams};

use crate::tools::{DocPointer, OutlineDrag, Tool, ToolContext, ToolResult, mode_at_press, nudge_outline, selection_mode};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// The option-bar id of the tool.
const ID: &str = "magic-wand";

/// The Magic Wand.
#[derive(Default)]
pub struct MagicWand {
	/// Dragging the selection outline (a press inside the selection, New mode).
	moving: Option<OutlineDrag>,
}

impl MagicWand {
	/// The wand's options from the option bar: Tolerance (0–255, default 32),
	/// Anti-alias and Contiguous (on), Sample All Layers (off).
	fn params(ctx: &ToolContext<'_>, x: f64, y: f64) -> WandParams {
		let settings = ctx.settings;
		WandParams {
			x,
			y,
			tolerance: settings.number(ID, "Tolerance").unwrap_or(32.0).clamp(0.0, 255.0),
			contiguous: settings.bool(ID, "Contiguous").unwrap_or(true),
			anti_alias: settings.bool(ID, "Anti-alias").unwrap_or(true),
			sample_all_layers: settings.bool(ID, "Sample All Layers").unwrap_or(false),
		}
	}
}

impl Tool for MagicWand {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let mode = mode_at_press(event.modifiers, selection_mode(ctx, ID));
				if mode == SelectMode::Replace
					&& let Some(moving) = OutlineDrag::begin(ctx, event)
				{
					self.moving = Some(moving);
					return ToolResult {
						cursor: Some(CursorShape::Move),
						..Default::default()
					};
				}
				if event.x < 0.0 || event.y < 0.0 || event.x >= f64::from(ctx.doc.width) || event.y >= f64::from(ctx.doc.height) {
					// Photoshop: a click outside the canvas deselects (New mode).
					return ToolResult {
						command: (mode == SelectMode::Replace && ctx.doc.selection.is_some()).then_some(Command::Deselect),
						..Default::default()
					};
				}
				ToolResult {
					command: Some(Command::MagicWand {
						params: Self::params(ctx, event.x, event.y),
						mode,
					}),
					..Default::default()
				}
			}
			PointerKind::Move | PointerKind::Down if self.moving.is_some() => {
				if let Some(moving) = &mut self.moving {
					moving.track(event, ctx.view.zoom);
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if self.moving.is_some() => {
				let moving = self.moving.take().expect("checked by the guard");
				// A click inside the selection (no drag) runs the wand there.
				let command = if moving.dragged() {
					moving.command()
				} else {
					Some(Command::MagicWand {
						params: Self::params(ctx, moving.press.x, moving.press.y),
						mode: SelectMode::Replace,
					})
				};
				ToolResult {
					command,
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if self.moving.is_some() {
			if key == "Escape" {
				self.moving = None;
				return ToolResult {
					redraw: true,
					..Default::default()
				};
			}
			return ToolResult::default();
		}
		ToolResult {
			command: nudge_outline(ctx, key),
			..Default::default()
		}
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}

	fn selection_nudge(&self) -> Option<(i32, i32)> {
		self.moving.map(|m| m.delta())
	}
}

#[cfg(test)]
mod tests {
	use fx_core::SelectionShape;

	use super::*;
	use crate::tools::testing::Fixture;

	#[test]
	fn a_click_is_one_wand_command_with_the_option_bar_values() {
		let mut f = Fixture::new("wand", (400, 300), 1.0);
		f.options(
			ID,
			serde_json::json!({ "Tolerance": 10, "Contiguous": false, "Anti-alias": false, "Sample All Layers": true }),
		);
		let mut tool = MagicWand::default();
		let result = f.pointer(&mut tool, PointerKind::Down, 12.5, 30.25, Modifiers::default());
		let Some(Command::MagicWand { params, mode }) = result.command else {
			panic!("{:?}", result.command);
		};
		assert_eq!(mode, SelectMode::Replace);
		assert_eq!((params.x, params.y, params.tolerance), (12.5, 30.25, 10.0));
		assert!(!params.contiguous && !params.anti_alias && params.sample_all_layers);
		// Shift adds, Alt subtracts.
		let shift = Modifiers {
			shift: true,
			..Default::default()
		};
		let result = f.pointer(&mut tool, PointerKind::Down, 1.0, 1.0, shift);
		assert!(matches!(result.command, Some(Command::MagicWand { mode: SelectMode::Add, .. })));
	}

	#[test]
	fn dragging_inside_the_selection_moves_the_outline() {
		let mut f = Fixture::new("wand", (400, 300), 2.0);
		f.select(SelectionShape::Rect {
			x: 10.0,
			y: 10.0,
			w: 100.0,
			h: 100.0,
		});
		let mut tool = MagicWand::default();
		f.pointer(&mut tool, PointerKind::Down, 50.0, 50.0, Modifiers::default());
		f.pointer(&mut tool, PointerKind::Move, 60.4, 45.0, Modifiers::default());
		assert_eq!(tool.selection_nudge(), Some((10, -5)), "the ants follow");
		let result = f.pointer(&mut tool, PointerKind::Up, 60.4, 45.0, Modifiers::default());
		assert!(
			matches!(result.command, Some(Command::OffsetSelection { dx: 10, dy: -5 })),
			"{:?}",
			result.command
		);
		assert_eq!(tool.selection_nudge(), None);
		// A click inside the selection, without a drag, runs the wand.
		f.pointer(&mut tool, PointerKind::Down, 50.0, 50.0, Modifiers::default());
		let result = f.pointer(&mut tool, PointerKind::Up, 50.5, 50.0, Modifiers::default());
		assert!(matches!(result.command, Some(Command::MagicWand { .. })), "{:?}", result.command);
		// Arrow keys nudge by 1 px, Shift+arrow by 10 px.
		assert!(matches!(
			f.key(&mut tool, "ArrowLeft").command,
			Some(Command::OffsetSelection { dx: -1, dy: 0 })
		));
		assert!(matches!(
			f.key(&mut tool, "Shift+ArrowDown").command,
			Some(Command::OffsetSelection { dx: 0, dy: 10 })
		));
	}
}
