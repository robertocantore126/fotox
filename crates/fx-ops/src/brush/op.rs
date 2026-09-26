//! The per-pixel part of a brush stroke (M7-T08): a [`DabOp`].
//!
//! The stroke engine (`stroke.rs`) keeps everything else — spacing, pressure,
//! the coverage buffer (opacity ceiling, flow build-up), the selection, undo
//! and live/replay equality. A new painting tool is a new `DabOp`: it gets the
//! pixel before the stroke (premultiplied), the source pixel when it asked
//! for a source window (Clone-like tools), and `k` = opacity × coverage.

use fx_core::BlendMode;
use fx_core::blend::composite;
use fx_core::stroke::StrokeTool;

/// What every op may read about the stroke.
#[derive(Clone, Copy, Debug)]
pub struct DabContext {
	pub mode: BlendMode,
	/// The paint colour, straight 0..1.
	pub color: [f64; 3],
	pub color_alpha: f64,
	pub lock_alpha: bool,
}

/// What an op needs from the engine besides the pixel under the dab.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Needs {
	/// A source window shifted by this offset (document pixels), sampled
	/// bilinearly (Clone, Heal).
	pub source: Option<(f64, f64)>,
}

/// The per-pixel operation of a painting tool.
pub trait DabOp: Send + Sync {
	fn needs(&self) -> Needs {
		Needs::default()
	}

	/// The new premultiplied pixel from `backdrop` (premultiplied RGBA 0..1),
	/// `source` (the source window's premultiplied pixel, when asked for) and
	/// `k` = opacity × coverage.
	fn pixel(&self, backdrop: [f64; 4], source: Option<[f32; 4]>, k: f64, ctx: &DabContext) -> [f64; 4];

	/// The new value of a grey target (a mask) from `v` (`0..=1`) (M8-T04);
	/// by default the paint colour's grey, `v + (grey − v)·k`.
	fn gray(&self, v: f64, k: f64, ctx: &DabContext) -> f64 {
		v + (ctx.color[0] - v) * k
	}
}

/// A `DabOp` that carries a buffer along the path (Smudge, Mixer Brush in
/// M8). The engine calls `pick_up` at every dab before painting it.
pub trait StatefulDabOp: DabOp {
	fn pick_up(&mut self, under: [f64; 4]);
}

/// Brush and Pencil: the colour through the blend mode.
pub struct Paint;

impl DabOp for Paint {
	fn pixel(&self, backdrop: [f64; 4], _source: Option<[f32; 4]>, k: f64, ctx: &DabContext) -> [f64; 4] {
		composite(ctx.mode, backdrop, ctx.color, k * ctx.color_alpha, ctx.lock_alpha)
	}
}

/// The Eraser: alpha down to transparency; with Lock Transparent Pixels it
/// paints the colour instead (M5).
pub struct Erase;

impl DabOp for Erase {
	fn pixel(&self, backdrop: [f64; 4], source: Option<[f32; 4]>, k: f64, ctx: &DabContext) -> [f64; 4] {
		if ctx.lock_alpha {
			return Paint.pixel(backdrop, source, k, ctx);
		}
		[
			backdrop[0] * (1.0 - k),
			backdrop[1] * (1.0 - k),
			backdrop[2] * (1.0 - k),
			backdrop[3] * (1.0 - k),
		]
	}
}

/// Clone Stamp and Healing Brush (live look): the source pixel through the
/// blend mode.
pub struct CloneSource {
	pub offset: (f64, f64),
}

impl DabOp for CloneSource {
	fn needs(&self) -> Needs {
		Needs { source: Some(self.offset) }
	}

	fn pixel(&self, backdrop: [f64; 4], source: Option<[f32; 4]>, k: f64, ctx: &DabContext) -> [f64; 4] {
		let s = source.unwrap_or([0.0; 4]);
		let sa = f64::from(s[3]);
		let rgb = if sa > 0.0 {
			[f64::from(s[0]) / sa, f64::from(s[1]) / sa, f64::from(s[2]) / sa]
		} else {
			[0.0; 3]
		};
		composite(ctx.mode, backdrop, rgb, k * sa, ctx.lock_alpha)
	}
}

/// The live look of a spot heal: a dark veil where it will heal.
pub struct Veil;

impl DabOp for Veil {
	fn pixel(&self, backdrop: [f64; 4], _source: Option<[f32; 4]>, k: f64, ctx: &DabContext) -> [f64; 4] {
		composite(BlendMode::Normal, backdrop, [0.0; 3], 0.35 * k, ctx.lock_alpha)
	}
}

/// The op of one of M5's stroke tools.
pub fn op_for(tool: &StrokeTool) -> Box<dyn DabOp> {
	match tool {
		StrokeTool::Brush | StrokeTool::Pencil => Box::new(Paint),
		StrokeTool::Eraser => Box::new(Erase),
		StrokeTool::Clone { dx, dy, .. } | StrokeTool::Heal { dx, dy, .. } => Box::new(CloneSource { offset: (*dx, *dy) }),
		StrokeTool::SpotHeal => Box::new(Veil),
		StrokeTool::Dodge { range, protect_tones } | StrokeTool::Burn { range, protect_tones } => Box::new(super::ops::tone::Tone {
			lighten: matches!(tool, StrokeTool::Dodge { .. }),
			range: *range,
			protect_tones: *protect_tones,
		}),
		StrokeTool::Sponge { saturate, vibrance } => Box::new(super::ops::tone::Sponge {
			saturate: *saturate,
			vibrance: *vibrance,
		}),
		StrokeTool::BgEraser { sample, tolerance, protect } => {
			let rgb = |c: [u16; 3]| c.map(|v| f64::from(v) / 65535.0);
			Box::new(super::ops::background_eraser::BackgroundEraser {
				sample: rgb(*sample),
				tolerance: f64::from(*tolerance),
				protect: protect.map(rgb),
			})
		}
	}
}
