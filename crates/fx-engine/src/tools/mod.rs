//! Viewport tools (M5-T01, recipe R5 in `docs/tasks/HOWTO.md`).
//!
//! A tool consumes the pointer events the *view* did not (pan/zoom keeps
//! priority: Space, the Hand tool and the middle button never reach a tool),
//! works in document coordinates ([`DocPointer`], the inverse of the view
//! transform) and answers with a [`ToolResult`]: a command to execute (R1), a
//! cursor change, a status line, or an engine-side effect (the eyedropper's
//! colour).
//!
//! Overlays (marching ants, the brush outline, selection handles) are drawn by
//! [`Tool::overlay`] in document coordinates and handed to the render thread
//! (M5-T02), which tessellates them (`fx_render::overlay`).

use std::collections::HashMap;

use fx_core::{Command, Document};
use fx_render::Overlay;
use fx_tiles::TileStore;

use crate::ops::EngineOps;
use crate::{CursorShape, Modifiers, PointerKind};

pub mod eyedropper;

pub use eyedropper::sample_pixel;

/// A pointer event in document pixel coordinates (M5-T01): [`PointerInput`]
/// mapped through the inverse of the view transform.
///
/// [`PointerInput`]: crate::PointerInput
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocPointer {
	pub kind: PointerKind,
	/// Document pixels (f64, so sub-pixel positions survive zoom).
	pub x: f64,
	pub y: f64,
	pub pressure: f32,
	pub tilt_x: f32,
	pub tilt_y: f32,
	pub buttons: u8,
	pub modifiers: Modifiers,
	pub time_us: u64,
}

/// Which swatch a picked colour goes to (Alt with the eyedropper = background).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTarget {
	Foreground,
	Background,
}

impl ColorTarget {
	/// The `target` field of `EngineToUi::ColorPicked`.
	pub fn as_str(self) -> &'static str {
		match self {
			ColorTarget::Foreground => "fg",
			ColorTarget::Background => "bg",
		}
	}
}

/// The colours and option-bar values the tools read (M5-T01).
#[derive(Clone, Debug)]
pub struct ToolSettings {
	/// Foreground colour, 16-bit RGBA (straight alpha).
	pub fg: [u16; 4],
	/// Background colour, 16-bit RGBA (straight alpha).
	pub bg: [u16; 4],
	/// The last `UiToEngine::ToolOptions` per tool id, keyed by the option
	/// bar's field text without the colon.
	pub options: HashMap<String, serde_json::Value>,
}

impl Default for ToolSettings {
	fn default() -> Self {
		Self {
			fg: [0, 0, 0, u16::MAX],
			bg: [u16::MAX, u16::MAX, u16::MAX, u16::MAX],
			options: HashMap::new(),
		}
	}
}

impl ToolSettings {
	/// The value of option `key` of `tool` as a number (integers read as f64).
	pub fn number(&self, tool: &str, key: &str) -> Option<f64> {
		self.options.get(tool)?.get(key)?.as_f64()
	}

	/// The value of option `key` of `tool` as a string.
	pub fn string(&self, tool: &str, key: &str) -> Option<String> {
		self.options.get(tool)?.get(key)?.as_str().map(str::to_owned)
	}

	/// The value of option `key` of `tool` as a bool.
	pub fn bool(&self, tool: &str, key: &str) -> Option<bool> {
		self.options.get(tool)?.get(key)?.as_bool()
	}
}

/// What a tool produced for one pointer event.
#[derive(Clone, Debug, Default)]
pub struct ToolResult {
	/// A command the engine should execute through `History` (R1).
	pub command: Option<Command>,
	/// A colour the tool picked (the eyedropper), for the UI swatch.
	pub picked: Option<([u16; 4], ColorTarget)>,
	/// The cursor to show now.
	pub cursor: Option<CursorShape>,
	/// A status line (Info panel / status bar, M5-T10).
	pub info: Option<String>,
}

/// The document and services a tool works with.
pub struct ToolContext<'a> {
	pub doc: &'a mut Document,
	pub store: &'a TileStore,
	pub ops: &'a EngineOps,
	pub settings: &'a ToolSettings,
}

/// A viewport tool (M5-T01).
pub trait Tool {
	/// Handle one pointer event in document coordinates.
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult;

	/// The overlay to draw over the viewport, in document coordinates
	/// (M5-T02): marching ants, the brush outline, handles. `None` = nothing.
	fn overlay(&self) -> Option<Overlay> {
		None
	}

	/// The cursor to show while this tool is active and the pointer is idle.
	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Default
	}
}

/// The tool registry: one boxed tool per id, created on first use, so a tool's
/// state (a drag in progress, the last clone source) lives as long as the
/// engine.
#[derive(Default)]
pub struct Tools {
	active: HashMap<String, Box<dyn Tool>>,
}

impl Tools {
	/// The tool for `id`, created on first use. `None` for a tool id M5 does
	/// not implement yet: the view still pans and zooms, the pointer is just
	/// ignored.
	pub fn get(&mut self, id: &str) -> Option<&mut Box<dyn Tool>> {
		if !self.active.contains_key(id) {
			self.active.insert(id.to_owned(), new_tool(id)?);
		}
		self.active.get_mut(id)
	}
}

/// The tool a UI tool id maps to, or `None` when the milestone does not
/// implement it yet (M5-T04 and T06+ add the rest).
fn new_tool(id: &str) -> Option<Box<dyn Tool>> {
	match id {
		"eyedropper" => Some(Box::new(eyedropper::Eyedropper)),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unknown_tools_have_no_implementation() {
		let mut tools = Tools::default();
		assert!(tools.get("brush").is_none(), "brush arrives with M5-T06");
		assert!(tools.get("eyedropper").is_some());
	}

	#[test]
	fn settings_read_typed_options() {
		let mut settings = ToolSettings::default();
		settings.options.insert(
			"eyedropper".into(),
			serde_json::json!({"Sample Size": "3 by 3 Average", "Sample": "Current Layer", "Show Sampling Ring": true, "Area": 3}),
		);
		assert_eq!(settings.string("eyedropper", "Sample Size").as_deref(), Some("3 by 3 Average"));
		assert_eq!(settings.string("eyedropper", "Sample").as_deref(), Some("Current Layer"));
		assert!(settings.bool("eyedropper", "Show Sampling Ring").unwrap());
		assert_eq!(settings.number("eyedropper", "Area"), Some(3.0));
		assert_eq!(settings.string("eyedropper", "Missing"), None);
		assert_eq!(settings.string("brush", "Size"), None);
	}
}
