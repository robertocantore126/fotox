//! Guides, grid and snapping (M7-T06).
//!
//! The View flags (Extras, Show Guides, Grid, Snap, Snap To ▸ …) live in the
//! UI and arrive as the `_view` tool options; the grid spacing is the
//! `_prefs` options (defaults: a line every 100 px, 4 subdivisions).

use fx_core::Document;
use fx_render::{OverlayItem, OverlayStyle, ViewTransform, ViewportSize};

use crate::tools::ToolSettings;

/// Snap distance in screen pixels (Photoshop's is about 8).
pub const RADIUS_PX: f64 = 8.0;

fn flag(settings: &ToolSettings, key: &str, default: bool) -> bool {
	settings.bool("_view", key).unwrap_or(default)
}

/// The grid's major spacing and subdivisions, document pixels.
pub fn grid(settings: &ToolSettings) -> (f64, u32) {
	let every = settings.number("_prefs", "Gridline Every").filter(|v| *v > 0.0).unwrap_or(100.0);
	let subdivisions = settings.number("_prefs", "Subdivisions").filter(|v| *v >= 1.0).unwrap_or(4.0) as u32;
	(every, subdivisions)
}

/// Whether snapping is on at all.
pub fn enabled(settings: &ToolSettings) -> bool {
	flag(settings, "snap", true)
}

/// Snap a document point to the nearest guide, grid line or canvas edge /
/// centre within [`RADIUS_PX`] screen pixels, each axis on its own.
pub fn point(doc: &Document, settings: &ToolSettings, zoom: f64, p: (f64, f64)) -> (f64, f64) {
	if !enabled(settings) {
		return p;
	}
	let radius = RADIUS_PX / zoom.max(1e-6);
	let mut xs: Vec<f64> = Vec::new();
	let mut ys: Vec<f64> = Vec::new();
	if flag(settings, "snap-guides", true) && flag(settings, "guides", true) {
		for g in &doc.guides {
			if g.vertical {
				xs.push(g.position);
			} else {
				ys.push(g.position);
			}
		}
	}
	if flag(settings, "snap-grid", false) {
		let (every, subdivisions) = grid(settings);
		let step = every / f64::from(subdivisions);
		xs.push((p.0 / step).round() * step);
		ys.push((p.1 / step).round() * step);
	}
	if flag(settings, "snap-bounds", false) || flag(settings, "snap-layers", true) {
		// FAST: "Layers" snaps to the canvas, not to each layer's bounds.
		let (w, h) = (f64::from(doc.width), f64::from(doc.height));
		xs.extend([0.0, w / 2.0, w]);
		ys.extend([0.0, h / 2.0, h]);
	}
	let nearest = |v: f64, candidates: &[f64]| {
		candidates
			.iter()
			.copied()
			.filter(|c| (c - v).abs() <= radius)
			.min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs()))
			.unwrap_or(v)
	};
	(nearest(p.0, &xs), nearest(p.1, &ys))
}

/// The guide and grid lines to draw over the view (bounded by the screen).
pub fn overlay(doc: &Document, settings: &ToolSettings, view: &ViewTransform, viewport: ViewportSize) -> Vec<OverlayItem> {
	let mut items = Vec::new();
	if !flag(settings, "extras", true) {
		return items;
	}
	// The visible document rectangle (a rotated view: the corners' box).
	let corners = [
		view.screen_to_doc(viewport, 0.0, 0.0),
		view.screen_to_doc(viewport, f64::from(viewport.width), 0.0),
		view.screen_to_doc(viewport, 0.0, f64::from(viewport.height)),
		view.screen_to_doc(viewport, f64::from(viewport.width), f64::from(viewport.height)),
	];
	let (w, h) = (f64::from(doc.width), f64::from(doc.height));
	let x0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).max(0.0);
	let x1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).min(w);
	let y0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).max(0.0);
	let y1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).min(h);
	if x0 >= x1 || y0 >= y1 {
		return items;
	}
	let line = |a: (f64, f64), b: (f64, f64), color: [f32; 4]| OverlayItem::Polyline {
		points: vec![a, b],
		closed: false,
		style: OverlayStyle::Solid(color),
	};
	if flag(settings, "grid", false) {
		let (every, subdivisions) = grid(settings);
		let zoom = view.zoom.max(1e-6);
		let mut step = every / f64::from(subdivisions);
		// Never closer than 6 screen pixels: coarser steps at low zoom.
		while step * zoom < 6.0 {
			step *= 2.0;
		}
		let major = every;
		let mut x = (x0 / step).ceil() * step;
		while x <= x1 {
			let strong = (x / major).fract().abs() < 1e-9;
			items.push(line((x, y0), (x, y1), if strong { [0.5, 0.5, 0.5, 0.8] } else { [0.5, 0.5, 0.5, 0.35] }));
			x += step;
		}
		let mut y = (y0 / step).ceil() * step;
		while y <= y1 {
			let strong = (y / major).fract().abs() < 1e-9;
			items.push(line((x0, y), (x1, y), if strong { [0.5, 0.5, 0.5, 0.8] } else { [0.5, 0.5, 0.5, 0.35] }));
			y += step;
		}
	}
	if flag(settings, "guides", true) {
		let cyan = [0.0, 1.0, 1.0, 1.0];
		for g in &doc.guides {
			if g.vertical && g.position >= x0 && g.position <= x1 {
				items.push(line((g.position, y0), (g.position, y1), cyan));
			} else if !g.vertical && g.position >= y0 && g.position <= y1 {
				items.push(line((x0, g.position), (x1, g.position), cyan));
			}
		}
	}
	items
}
