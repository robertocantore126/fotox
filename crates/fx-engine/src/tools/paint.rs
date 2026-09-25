//! The painting tools (M5-T07/T08/T09): Brush, Pencil, Eraser, Clone Stamp,
//! Healing Brush and Spot Healing Brush.
//!
//! A tool turns pointer events into stroke samples (after smoothing) and hands
//! them to the engine as [`StrokeEvent`]s; the engine paints them live and
//! records one `Command::Stroke` at pen-up. The tool also draws the brush
//! outline (and the clone source's crosshair) as an overlay, and picks a
//! colour on Alt+click like Photoshop's temporary eyedropper.

use fx_core::BlendMode;
use fx_core::stroke::{BrushParams, StrokeSample, StrokeTarget, StrokeTool};
use fx_render::{Overlay, OverlayItem, OverlayStyle};

use crate::tools::{ColorTarget, DocPointer, StrokeEvent, Tool, ToolContext, ToolResult, sample_pixel};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

/// Which painting tool an instance is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	Brush,
	Pencil,
	Eraser,
	Clone,
	Heal,
	SpotHeal,
}

/// Below this many screen pixels the outline is replaced by a crosshair.
const SMALL_OUTLINE_PX: f64 = 6.0;

/// A painting tool: one instance per UI tool id.
pub struct Paint {
	id: &'static str,
	kind: Kind,
	/// The pointer, for the outline.
	hover: Option<(f64, f64)>,
	/// A stroke is being painted.
	stroking: bool,
	/// The smoothed pen position (the end of the "pulled string").
	pen: Option<(f64, f64)>,
	/// The diameter of the current stroke, in document pixels.
	diameter: f64,
	/// The clone source point (Alt+click).
	source: Option<(f64, f64)>,
	/// Destination − source of an aligned clone, kept between strokes.
	aligned: Option<(f64, f64)>,
	/// The offset of the stroke being painted (for the source crosshair).
	stroke_offset: Option<(f64, f64)>,
	/// The view's zoom at the last event (the outline's screen size).
	zoom: f64,
}

impl Paint {
	/// The tool for UI id `id`.
	pub fn new(id: &'static str, kind: Kind) -> Self {
		Self {
			id,
			kind,
			hover: None,
			stroking: false,
			pen: None,
			diameter: 0.0,
			source: None,
			aligned: None,
			stroke_offset: None,
			zoom: 1.0,
		}
	}

	/// The brush the option bar describes.
	fn brush(&self, ctx: &ToolContext<'_>) -> BrushParams {
		let s = ctx.settings;
		let percent = |key: &str, default: f64| (s.number(self.id, key).unwrap_or(default) / 100.0).clamp(0.0, 1.0) as f32;
		let mode = s
			.string(self.id, "Mode")
			.and_then(|m| serde_json::from_value::<BlendMode>(serde_json::Value::String(m.to_lowercase().replace([' ', '-'], "_"))).ok())
			.unwrap_or_default();
		let default_size = if self.kind == Kind::Pencil { 3.0 } else { 40.0 };
		let pencil_like = self.kind == Kind::Pencil || (self.kind == Kind::Eraser && s.string(self.id, "Mode").is_some_and(|m| m != "Brush"));
		BrushParams {
			diameter: s.number(self.id, "Size").unwrap_or(default_size).clamp(1.0, 5000.0) as f32,
			hardness: if pencil_like {
				1.0
			} else {
				percent("Hardness", if self.kind == Kind::Brush { 75.0 } else { 50.0 })
			},
			roundness: 1.0,
			angle: 0.0,
			spacing: percent("Spacing", 25.0).max(0.01),
			opacity: percent("Opacity", 100.0),
			flow: percent("Flow", 100.0),
			mode: if matches!(self.kind, Kind::Eraser | Kind::SpotHeal) {
				BlendMode::Normal
			} else {
				mode
			},
			pressure_size: s.bool(self.id, "Pressure for size").unwrap_or(false),
			pressure_opacity: s.bool(self.id, "Pressure for opacity").unwrap_or(false),
		}
	}

	/// The stroke tool, or why the stroke cannot start.
	fn stroke_tool(&mut self, ctx: &ToolContext<'_>, at: (f64, f64)) -> Result<StrokeTool, String> {
		let s = ctx.settings;
		Ok(match self.kind {
			Kind::Brush => StrokeTool::Brush,
			Kind::Pencil => StrokeTool::Pencil,
			// The eraser's Pencil/Block modes use a hard tip (see `brush`).
			Kind::Eraser => StrokeTool::Eraser,
			Kind::SpotHeal => StrokeTool::SpotHeal,
			Kind::Clone | Kind::Heal => {
				let Some(source) = self.source else {
					return Err("Alt-click to define a source point".into());
				};
				let aligned = s.bool(self.id, "Aligned").unwrap_or(true);
				let offset = match (aligned, self.aligned) {
					(true, Some(offset)) => offset,
					_ => (at.0 - source.0, at.1 - source.1),
				};
				if aligned {
					self.aligned = Some(offset);
				}
				self.stroke_offset = Some(offset);
				let sample_all = match self.kind {
					Kind::Clone => s.string(self.id, "Sample").is_none_or(|v| v != "Current Layer"),
					_ => s.bool(self.id, "Sample All Layers").unwrap_or(true),
				};
				if self.kind == Kind::Clone {
					StrokeTool::Clone {
						dx: offset.0,
						dy: offset.1,
						sample_all,
					}
				} else {
					StrokeTool::Heal {
						dx: offset.0,
						dy: offset.1,
						sample_all,
					}
				}
			}
		})
	}

	/// The paint colour: the foreground; the eraser's is the background (it
	/// shows on a mask, or on a layer with locked transparency). A mask gets
	/// the colour's luminance.
	fn color(&self, ctx: &ToolContext<'_>, mask: bool) -> [u16; 4] {
		let base = if self.kind == Kind::Eraser { ctx.settings.bg } else { ctx.settings.fg };
		if !mask {
			return base;
		}
		let lum = (0.299 * f64::from(base[0]) + 0.587 * f64::from(base[1]) + 0.114 * f64::from(base[2])).round() as u16;
		[lum, lum, lum, u16::MAX]
	}

	/// The sample a pointer event gives, with the pen pulled along a string of
	/// `Smoothing × diameter` (0 = the pointer itself).
	fn follow(&mut self, ctx: &ToolContext<'_>, event: &DocPointer) -> Option<StrokeSample> {
		let smoothing = (ctx.settings.number(self.id, "Smoothing").unwrap_or(0.0) / 100.0).clamp(0.0, 1.0);
		let length = smoothing * self.diameter;
		let target = (event.x, event.y);
		let pen = match self.pen {
			None => target,
			Some(pen) => {
				let (dx, dy) = (target.0 - pen.0, target.1 - pen.1);
				let d = (dx * dx + dy * dy).sqrt();
				if d <= length {
					return None;
				}
				let pull = (d - length) / d;
				(pen.0 + dx * pull, pen.1 + dy * pull)
			}
		};
		self.pen = Some(pen);
		Some(StrokeSample {
			x: pen.0,
			y: pen.1,
			pressure: if event.pressure > 0.0 { event.pressure } else { 1.0 },
			tilt_x: event.tilt_x,
			tilt_y: event.tilt_y,
			time_us: event.time_us,
		})
	}
}

impl Tool for Paint {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		self.hover = Some((event.x, event.y));
		self.zoom = ctx.view.zoom;
		if !self.stroking {
			// The outline follows the option bar's size.
			self.diameter = f64::from(self.brush(ctx).diameter);
		}
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				// Alt: the clone source, or Photoshop's temporary eyedropper.
				if event.modifiers.alt {
					if matches!(self.kind, Kind::Clone | Kind::Heal) {
						self.source = Some((event.x, event.y));
						self.aligned = None;
						return ToolResult {
							info: Some("Clone source set".into()),
							redraw: true,
							..Default::default()
						};
					}
					if matches!(self.kind, Kind::Brush | Kind::Pencil) {
						return match sample_pixel(ctx.doc, event.x, event.y, 1, None, ctx.store) {
							Ok(rgba) => ToolResult {
								picked: Some((rgba, ColorTarget::Foreground)),
								..Default::default()
							},
							Err(error) => ToolResult {
								info: Some(error.to_string()),
								..Default::default()
							},
						};
					}
				}
				let tool = match self.stroke_tool(ctx, (event.x, event.y)) {
					Ok(tool) => tool,
					Err(text) => {
						return ToolResult {
							info: Some(text),
							..Default::default()
						};
					}
				};
				let brush = self.brush(ctx);
				self.diameter = f64::from(brush.diameter);
				self.pen = None;
				self.stroking = true;
				let target = if ctx.mask_target { StrokeTarget::Mask } else { StrokeTarget::Pixels };
				let color = self.color(ctx, ctx.mask_target);
				let first = self.follow(ctx, event).into_iter().collect();
				ToolResult {
					strokes: vec![StrokeEvent::Begin {
						target,
						tool,
						brush,
						color,
						samples: first,
					}],
					cursor: Some(self.cursor(event.modifiers)),
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move if self.stroking => {
				let samples: Vec<StrokeSample> = self.follow(ctx, event).into_iter().collect();
				ToolResult {
					strokes: if samples.is_empty() { Vec::new() } else { vec![StrokeEvent::Add(samples)] },
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Up if self.stroking => {
				self.stroking = false;
				// Catch up with the pointer at the end of the stroke (Photoshop's
				// "catch-up on stroke end").
				let last = self.pen.filter(|pen| *pen != (event.x, event.y)).map(|_| StrokeSample {
					x: event.x,
					y: event.y,
					pressure: if event.pressure > 0.0 { event.pressure } else { 1.0 },
					tilt_x: event.tilt_x,
					tilt_y: event.tilt_y,
					time_us: event.time_us,
				});
				self.pen = None;
				self.stroke_offset = None;
				let mut events = Vec::new();
				if let Some(last) = last {
					events.push(StrokeEvent::Add(vec![last]));
				}
				events.push(StrokeEvent::End);
				ToolResult {
					strokes: events,
					redraw: true,
					..Default::default()
				}
			}
			// A hover moves the outline only.
			PointerKind::Move => ToolResult {
				redraw: true,
				..Default::default()
			},
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let (x, y) = self.hover?;
		let mut items = Vec::new();
		// The brush outline, or a crosshair when it would be too small to see.
		if self.diameter * self.zoom < SMALL_OUTLINE_PX {
			items.push(OverlayItem::Crosshair { at: (x, y) });
		} else {
			items.push(OverlayItem::Circle {
				centre: (x, y),
				radius: self.diameter / 2.0,
				style: OverlayStyle::Xor,
			});
		}
		// The clone source: where the stroke reads, or the point set by Alt.
		match (self.stroke_offset, self.source) {
			(Some((dx, dy)), _) if self.stroking => items.push(OverlayItem::Crosshair { at: (x - dx, y - dy) }),
			(_, Some(source)) if matches!(self.kind, Kind::Clone | Kind::Heal) => items.push(OverlayItem::Crosshair { at: source }),
			_ => {}
		}
		Some(Overlay { items })
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		// The outline (or crosshair) is drawn by the overlay.
		CursorShape::None
	}
}
