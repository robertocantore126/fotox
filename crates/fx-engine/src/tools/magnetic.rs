//! The Magnetic Lasso (M9-T07, D-071): the path snaps to edges.
//!
//! A click starts the path; moving the pointer shows the live-wire from the
//! last anchor to the strongest edge within Width of the pointer
//! (`fx_ops::select::livewire` on the composite's luminance in a window
//! around both). A click adds an anchor, anchors are also added
//! automatically when the live segment grows past a length set by
//! Frequency, Backspace removes the last anchor, Enter or a double-click
//! closes the path into one `Command::Select` (a polygon), Escape cancels.
//!
//! FAST: the closing segment is straight; Alt does not switch to the other
//! lassos; pen pressure does not change Width; the composite is the document
//! as it was when the path started.

use std::collections::HashMap;
use std::sync::Arc;

use fx_core::{Command, SelectMode, SelectionShape};
use fx_ops::brush::SourceTiles;
use fx_render::{Overlay, OverlayItem, OverlayStyle};
use fx_tiles::TILE_SIZE;

use crate::tools::{DocPointer, Tool, ToolContext, ToolResult, mode_at_press, selection_mode, selection_shape_options};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, Modifiers, PointerKind};

const ID: &str = "lasso-magnet";
const DOUBLE_CLICK_US: u64 = 400_000;

#[derive(Default)]
pub struct MagneticLasso {
	/// The fixed path: one segment per anchor after the first.
	segments: Vec<Vec<(f64, f64)>>,
	anchors: Vec<(f64, f64)>,
	/// The live segment from the last anchor to the pointer.
	live: Vec<(f64, f64)>,
	mode: Option<SelectMode>,
	source: Option<Arc<crate::stroke::CompositeTiles>>,
	lum_cache: HashMap<(u32, u32), Arc<Vec<f32>>>,
	last_press: Option<u64>,
}

impl MagneticLasso {
	fn width(ctx: &ToolContext<'_>) -> f64 {
		ctx.settings.number(ID, "Width").unwrap_or(10.0).clamp(1.0, 256.0)
	}

	fn contrast(ctx: &ToolContext<'_>) -> f32 {
		ctx.settings
			.string(ID, "Contrast")
			.and_then(|s| s.trim_end_matches('%').parse::<f32>().ok())
			.unwrap_or(10.0)
			/ 100.0
	}

	/// Anchor spacing from Frequency (0–100; Photoshop's default 57).
	fn spacing(ctx: &ToolContext<'_>) -> f64 {
		let f = ctx.settings.number(ID, "Frequency").unwrap_or(57.0).clamp(0.0, 100.0);
		20.0 + (100.0 - f) * 3.0
	}

	/// Luminance of a canvas tile of the composite (cached).
	fn lum_tile(&mut self, tx: u32, ty: u32) -> Option<Arc<Vec<f32>>> {
		if let Some(t) = self.lum_cache.get(&(tx, ty)) {
			return Some(t.clone());
		}
		let tile = self.source.as_ref()?.tile(tx, ty).ok()?;
		let lum: Vec<f32> = tile
			.iter()
			.map(|p| {
				if p[3] > 0.0 {
					(0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]) / p[3] * p[3].min(1.0)
				} else {
					0.0
				}
			})
			.collect();
		let lum = Arc::new(lum);
		self.lum_cache.insert((tx, ty), lum.clone());
		Some(lum)
	}

	/// The luminance of a canvas window (edge-replicated).
	fn window(&mut self, x0: i64, y0: i64, w: usize, h: usize, size: (u32, u32)) -> Vec<f32> {
		let tile = i64::from(TILE_SIZE);
		let mut out = vec![0.0f32; w * h];
		for y in 0..h as i64 {
			let cy = (y0 + y).clamp(0, i64::from(size.1) - 1);
			for x in 0..w as i64 {
				let cx = (x0 + x).clamp(0, i64::from(size.0) - 1);
				if let Some(t) = self.lum_tile((cx / tile) as u32, (cy / tile) as u32) {
					out[y as usize * w + x as usize] = t[((cy % tile) * tile + cx % tile) as usize];
				}
			}
		}
		out
	}

	/// The live-wire from the last anchor to the edge near `p`.
	fn trace(&mut self, ctx: &ToolContext<'_>, p: (f64, f64)) -> Vec<(f64, f64)> {
		let Some(&a) = self.anchors.last() else { return Vec::new() };
		let width = Self::width(ctx);
		let size = (ctx.doc.width, ctx.doc.height);
		let x0 = (a.0.min(p.0) - width - 4.0).floor() as i64;
		let y0 = (a.1.min(p.1) - width - 4.0).floor() as i64;
		let x1 = (a.0.max(p.0) + width + 4.0).ceil() as i64;
		let y1 = (a.1.max(p.1) + width + 4.0).ceil() as i64;
		let (w, h) = ((x1 - x0).clamp(1, 2048) as usize, (y1 - y0).clamp(1, 2048) as usize);
		let lum = self.window(x0, y0, w, h, size);
		let cost = fx_ops::select::livewire::edge_cost(&lum, w, h, Self::contrast(ctx));
		// Snap the end to the cheapest (strongest-edge) pixel within Width.
		let (px, py) = ((p.0 - x0 as f64) as i64, (p.1 - y0 as f64) as i64);
		let r = width as i64;
		let mut end = (px.clamp(0, w as i64 - 1) as usize, py.clamp(0, h as i64 - 1) as usize);
		let mut best = f32::INFINITY;
		for dy in -r..=r {
			for dx in -r..=r {
				let (x, y) = (px + dx, py + dy);
				if dx * dx + dy * dy > r * r || x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
					continue;
				}
				let c = cost[y as usize * w + x as usize];
				if c < best {
					best = c;
					end = (x as usize, y as usize);
				}
			}
		}
		let start = (
			((a.0 - x0 as f64) as i64).clamp(0, w as i64 - 1) as usize,
			((a.1 - y0 as f64) as i64).clamp(0, h as i64 - 1) as usize,
		);
		fx_ops::select::livewire::live_wire(&cost, w, h, start, end)
			.into_iter()
			.map(|(x, y)| (x as f64 + x0 as f64 + 0.5, y as f64 + y0 as f64 + 0.5))
			.collect()
	}

	fn anchor_live(&mut self) {
		if let Some(&end) = self.live.last() {
			self.segments.push(std::mem::take(&mut self.live));
			self.anchors.push(end);
		}
	}

	fn reset(&mut self) {
		self.segments.clear();
		self.anchors.clear();
		self.live.clear();
		self.mode = None;
		self.source = None;
		self.lum_cache.clear();
	}

	fn close(&mut self, ctx: &ToolContext<'_>) -> ToolResult {
		self.anchor_live();
		let mut points: Vec<(f64, f64)> = self.anchors.first().copied().into_iter().collect();
		for s in &self.segments {
			points.extend(s.iter().skip(1));
		}
		let mode = self.mode.unwrap_or(SelectMode::Replace);
		self.reset();
		if points.len() < 3 {
			return ToolResult {
				redraw: true,
				..Default::default()
			};
		}
		let (feather, anti_alias) = selection_shape_options(ctx, ID);
		ToolResult {
			command: Some(Command::Select {
				shape: SelectionShape::Polygon { points },
				mode,
				feather,
				anti_alias,
			}),
			redraw: true,
			..Default::default()
		}
	}
}

impl Tool for MagneticLasso {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		let p = (event.x, event.y);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let double = self.last_press.is_some_and(|t| event.time_us.saturating_sub(t) <= DOUBLE_CLICK_US);
				self.last_press = Some(event.time_us);
				if self.anchors.is_empty() {
					self.mode = Some(mode_at_press(event.modifiers, selection_mode(ctx, ID)));
					self.source = Some(Arc::new(crate::stroke::CompositeTiles::new(ctx.doc.clone(), ctx.store.clone())));
					self.anchors.push(p);
					return ToolResult {
						redraw: true,
						..Default::default()
					};
				}
				if double {
					return self.close(ctx);
				}
				self.live = self.trace(ctx, p);
				self.anchor_live();
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			PointerKind::Move if !self.anchors.is_empty() => {
				self.live = self.trace(ctx, p);
				// Frequency: an automatic anchor once the segment is long.
				let a = *self.anchors.last().expect("not empty");
				if (p.0 - a.0).hypot(p.1 - a.1) > Self::spacing(ctx) {
					self.anchor_live();
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		match key {
			"Enter" if !self.anchors.is_empty() => self.close(ctx),
			"Escape" if !self.anchors.is_empty() => {
				self.reset();
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			"Backspace" | "Delete" if !self.anchors.is_empty() => {
				self.live.clear();
				if self.segments.pop().is_some() {
					self.anchors.pop();
				} else {
					self.reset();
				}
				ToolResult {
					redraw: true,
					..Default::default()
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn overlay(&self) -> Option<Overlay> {
		let first = *self.anchors.first()?;
		let mut points = vec![first];
		for s in &self.segments {
			points.extend(s.iter().skip(1));
		}
		points.extend(self.live.iter().skip(1));
		let mut items = vec![OverlayItem::Polyline {
			points,
			closed: false,
			style: OverlayStyle::Ants,
		}];
		for a in &self.anchors {
			items.push(OverlayItem::Crosshair { at: *a });
		}
		Some(Overlay { items })
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}

	fn deactivate(&mut self, _ctx: &mut ToolContext<'_>) -> ToolResult {
		self.reset();
		ToolResult::default()
	}
}
