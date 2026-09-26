//! Filter ▸ Liquify (M11-T06) as a warp session (`tools::transform`): the
//! brush edits a sparse displacement field (4-px cells, D-078); every change
//! registers the field as a `Mapping::Custom`, so the engine previews it on
//! the visible tiles and Enter / ✓ applies it as one resample job.
//!
//! FAST: a warp session on the canvas, not Photoshop's modal workspace; no
//! Freeze / Thaw mask, Show Mesh / Backdrop, Hand / Zoom inside, stylus
//! pressure, mesh load / save; the history step is named "Free Transform".

use fx_core::Mapping;
use fx_core::warp_map::{DispField, WarpData, register};
use fx_ops::liquify::{Brush, dab};
use fx_render::{OverlayItem, OverlayStyle};

use crate::tools::DocPointer;
use crate::tools::transform::{CustomWarp, Update};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, PointerKind};

/// D-078: a node every 4 document pixels.
const CELL: f64 = 4.0;

#[derive(Clone, Debug)]
pub struct Liquify {
	field: DispField,
	id: Option<u64>,
	brush: Brush,
	size: f64,
	pressure: f64,
	rate: f64,
	last: Option<(f64, f64)>,
	hover: Option<(f64, f64)>,
	alt: bool,
}

impl Default for Liquify {
	fn default() -> Self {
		Self {
			field: DispField::new(CELL),
			id: None,
			brush: Brush::ForwardWarp,
			size: 100.0,
			pressure: 1.0,
			rate: 0.8,
			last: None,
			hover: None,
			alt: false,
		}
	}
}

impl Liquify {
	fn publish(&mut self) {
		self.id = (!self.field.is_identity()).then(|| register(WarpData::Field(self.field.clone())));
	}

	fn brush_now(&self) -> Brush {
		match (self.brush, self.alt) {
			(Brush::TwirlClockwise, true) => Brush::TwirlCounter,
			(Brush::Pucker, true) => Brush::Bloat,
			(Brush::Bloat, true) => Brush::Pucker,
			(b, _) => b,
		}
	}
}

impl CustomWarp for Liquify {
	fn clone_box(&self) -> Box<dyn CustomWarp> {
		Box::new(self.clone())
	}

	fn bar(&self) -> &'static str {
		"_warp-liquify"
	}

	fn mapping(&self) -> Option<Mapping> {
		self.id.map(|id| Mapping::Custom {
			id,
			src: [0.0; 2],
			dst: [0.0; 2],
		})
	}

	fn pointer(&mut self, event: &DocPointer, _zoom: f64) -> Update {
		let at = (event.x, event.y);
		self.hover = Some(at);
		self.alt = event.modifiers.alt;
		let radius = self.size / 2.0;
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				self.last = Some(at);
				let brush = self.brush_now();
				if !matches!(brush, Brush::ForwardWarp | Brush::PushLeft) {
					dab(&mut self.field, brush, at, radius, (0.0, 0.0), self.pressure * self.rate);
					self.publish();
					return Update::Changed { dragging: true };
				}
				Update::Redraw
			}
			PointerKind::Move if self.last.is_some() && event.buttons & BUTTON_LEFT != 0 => {
				let last = self.last.expect("checked");
				let brush = self.brush_now();
				let (mx, my) = (at.0 - last.0, at.1 - last.1);
				let moved = (mx * mx + my * my).sqrt();
				if matches!(brush, Brush::ForwardWarp | Brush::PushLeft) {
					// Dabs every quarter radius along the motion.
					let step = (radius / 4.0).max(1.0);
					if moved < step {
						return Update::Redraw;
					}
					let n = (moved / step).ceil();
					for k in 0..n as usize {
						let t = (k as f64 + 0.5) / n;
						let c = (last.0 + mx * t, last.1 + my * t);
						dab(&mut self.field, brush, c, radius, (mx / n, my / n), self.pressure);
					}
				} else {
					dab(&mut self.field, brush, at, radius, (mx, my), self.pressure * self.rate);
				}
				self.last = Some(at);
				self.publish();
				Update::Changed { dragging: true }
			}
			PointerKind::Up => {
				self.last = None;
				Update::Changed { dragging: false }
			}
			_ => Update::Redraw,
		}
	}

	fn set_option(&mut self, key: &str, value: &serde_json::Value) -> Update {
		let num = || value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()));
		match key {
			"Tool" => {
				self.brush = Brush::from_name(value.as_str().unwrap_or_default());
				Update::None
			}
			"Size" => {
				self.size = num().unwrap_or(self.size).clamp(1.0, 15000.0);
				Update::Redraw
			}
			"Pressure" => {
				self.pressure = (num().unwrap_or(100.0) / 100.0).clamp(0.0, 1.0);
				Update::None
			}
			"Rate" => {
				self.rate = (num().unwrap_or(80.0) / 100.0).clamp(0.0, 1.0);
				Update::None
			}
			_ => Update::None,
		}
	}

	fn key(&mut self, key: &str) -> Option<Update> {
		// Photoshop's [ and ] resize the brush.
		match key {
			"[" => self.size = (self.size / 1.2).max(1.0),
			"]" => self.size = (self.size * 1.2).min(15000.0),
			// Restore All.
			"Backspace" | "Delete" => {
				self.field = DispField::new(CELL);
				self.id = None;
				return Some(Update::Changed { dragging: false });
			}
			_ => return None,
		}
		Some(Update::Redraw)
	}

	fn overlay(&self) -> Vec<OverlayItem> {
		self.hover
			.map(|centre| OverlayItem::Circle {
				centre,
				radius: self.size / 2.0,
				style: OverlayStyle::Xor,
			})
			.into_iter()
			.collect()
	}

	fn status(&self) -> String {
		"Liquify: drag to warp · [ ] brush size · Delete restores all · Enter applies".into()
	}

	fn cursor(&self) -> CursorShape {
		CursorShape::Crosshair
	}
}
