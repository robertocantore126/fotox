//! The Gradient tool (G, M8-T03): a drag → one `Command::FillGradient`.
//!
//! The option bar's gradient (the picker's `Gradient` value) is JSON of
//! `fx_core::gradient::Gradient`, where a stop colour may be `"fg"` or `"bg"`:
//! the swatches at the drag's release (Photoshop's "Foreground to
//! Background" follows the colours).
//!
//! FAST: no live preview while dragging (the line overlay only).

use fx_core::gradient::{Gradient, GradientFill, GradientKind, Method};
use fx_core::{Command, LayerRef};

use super::ToolContext;
use super::kinds::{DragTool, blend_mode};
use crate::Modifiers;

/// The Gradient tool.
pub struct GradientTool;

/// `"fg"` / `"bg"` stop colours replaced by the swatches.
pub fn resolve_gradient(value: Option<&serde_json::Value>, fg: [u16; 4], bg: [u16; 4]) -> Gradient {
	let rgb = |c: [u16; 4]| serde_json::json!([f32::from(c[0]) / 65535.0, f32::from(c[1]) / 65535.0, f32::from(c[2]) / 65535.0]);
	let Some(mut v) = value.cloned() else {
		return Gradient::two([0, 1, 2].map(|i| f32::from(fg[i]) / 65535.0), [0, 1, 2].map(|i| f32::from(bg[i]) / 65535.0));
	};
	if let Some(stops) = v.get_mut("colors").and_then(|c| c.as_array_mut()) {
		for stop in stops {
			match stop.get("color").and_then(|c| c.as_str()) {
				Some("fg") => stop["color"] = rgb(fg),
				Some("bg") => stop["color"] = rgb(bg),
				_ => {}
			}
		}
	}
	serde_json::from_value(v).unwrap_or_else(|_| Gradient::two([0.0; 3], [1.0; 3]))
}

/// The option bar's Type.
pub fn gradient_kind(name: Option<&str>) -> GradientKind {
	match name {
		Some("Radial") => GradientKind::Radial,
		Some("Angle") => GradientKind::Angle,
		Some("Reflected") => GradientKind::Reflected,
		Some("Diamond") => GradientKind::Diamond,
		_ => GradientKind::Linear,
	}
}

impl DragTool for GradientTool {
	fn id(&self) -> &'static str {
		"gradient"
	}

	fn release(&mut self, ctx: &mut ToolContext<'_>, a: (f64, f64), b: (f64, f64), _modifiers: Modifiers) -> Result<Option<Command>, String> {
		if (a.0 - b.0).hypot(a.1 - b.1) < 1.0 {
			return Ok(None);
		}
		let id = self.id();
		let s = ctx.settings;
		let mut gradient = resolve_gradient(s.options.get(id).and_then(|o| o.get("Gradient")), s.fg, s.bg);
		if let Some(method) = s.string(id, "Method") {
			gradient.method = match method.as_str() {
				"Linear" => Method::Linear,
				"Classic" => Method::Classic,
				_ => Method::Perceptual,
			};
		}
		let fill = GradientFill {
			gradient,
			kind: gradient_kind(s.string(id, "Type").as_deref()),
			start: a,
			end: b,
			reverse: s.bool(id, "Reverse").unwrap_or(false),
			dither: s.bool(id, "Dither").unwrap_or(true),
			transparency: s.bool(id, "Transparency").unwrap_or(true),
		};
		Ok(Some(Command::FillGradient {
			layer: LayerRef::Active,
			fill,
			mode: blend_mode(s.string(id, "Mode")),
			opacity: (s.number(id, "Opacity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0),
		}))
	}
}
