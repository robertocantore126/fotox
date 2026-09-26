//! The Paint Bucket and the Magic Eraser (M8-T02): click tools on the Magic
//! Wand's flood. Each click is one command, run as a job like the wand.

use fx_core::fill::FillSource;
use fx_core::{Command, LayerRef, WandParams};

use super::ToolContext;
use super::kinds::{ClickTool, blend_mode};
use crate::Modifiers;

fn inside(ctx: &ToolContext<'_>, p: (f64, f64)) -> bool {
	p.0 >= 0.0 && p.1 >= 0.0 && p.0 < f64::from(ctx.doc.width) && p.1 < f64::from(ctx.doc.height)
}

fn wand(ctx: &ToolContext<'_>, id: &str, p: (f64, f64), all_key: &str) -> WandParams {
	let s = ctx.settings;
	WandParams {
		x: p.0,
		y: p.1,
		tolerance: s.number(id, "Tolerance").unwrap_or(32.0).clamp(0.0, 255.0),
		contiguous: s.bool(id, "Contiguous").unwrap_or(true),
		anti_alias: s.bool(id, "Anti-alias").unwrap_or(true),
		sample_all_layers: s.bool(id, all_key).unwrap_or(false),
	}
}

/// The Paint Bucket (G).
pub struct Bucket;

impl ClickTool for Bucket {
	fn id(&self) -> &'static str {
		"paint-bucket"
	}

	fn click(&mut self, ctx: &mut ToolContext<'_>, p: (f64, f64), _modifiers: Modifiers) -> Result<Option<Command>, String> {
		if !inside(ctx, p) {
			return Ok(None);
		}
		let id = self.id();
		let s = ctx.settings;
		let source = if s.string(id, "Fill").as_deref() == Some("Pattern") {
			match s.number(id, "Pattern") {
				Some(pattern) if pattern > 0.0 => FillSource::Pattern { pattern: pattern as u64 },
				_ => return Err("Pick a pattern in the Patterns panel first".into()),
			}
		} else {
			FillSource::Color { rgba: s.fg }
		};
		Ok(Some(Command::BucketFill {
			layer: LayerRef::Active,
			params: wand(ctx, id, p, "All Layers"),
			source,
			mode: blend_mode(s.string(id, "Mode")),
			opacity: (s.number(id, "Opacity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0),
			preserve_transparency: false,
		}))
	}
}

/// The Magic Eraser (E).
pub struct MagicEraser;

impl ClickTool for MagicEraser {
	fn id(&self) -> &'static str {
		"eraser-magic"
	}

	fn click(&mut self, ctx: &mut ToolContext<'_>, p: (f64, f64), _modifiers: Modifiers) -> Result<Option<Command>, String> {
		if !inside(ctx, p) {
			return Ok(None);
		}
		let id = self.id();
		Ok(Some(Command::MagicErase {
			layer: LayerRef::Active,
			params: wand(ctx, id, p, "Sample All Layers"),
			opacity: (ctx.settings.number(id, "Opacity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0),
		}))
	}
}

/// The Red Eye tool (J, M8-T08).
pub struct RedEye;

impl ClickTool for RedEye {
	fn id(&self) -> &'static str {
		"red-eye"
	}

	fn click(&mut self, ctx: &mut ToolContext<'_>, p: (f64, f64), _modifiers: Modifiers) -> Result<Option<Command>, String> {
		if !inside(ctx, p) {
			return Ok(None);
		}
		let s = ctx.settings;
		Ok(Some(Command::RedEye {
			layer: LayerRef::Active,
			point: p,
			pupil_size: (s.number(self.id(), "Pupil Size").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0),
			darken: (s.number(self.id(), "Darken Amount").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0),
		}))
	}
}
