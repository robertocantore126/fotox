//! Frame planning: which composite tiles to draw where, with coarser
//! fallbacks for tiles that are not ready — a frame never waits.
//!
//! Pure logic (no GPU), fully tested. The engine calls [`plan_frame`] every
//! frame with a `ready` lookup into the compositor's results, draws
//! `plan.draws` with [`crate::gpu::viewport::ViewportRenderer`], and asks the
//! compositor for `plan.requests` (already sorted by priority).

use fx_tiles::TILE_SIZE;

use crate::viewport::{ViewTransform, ViewportSize};

/// One textured quad: composite tile `slot`, its sub-rectangle `src`
/// (0..1 UV inside the tile), drawn at `dst` (screen pixels, x0 y0 x1 y1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileDraw {
	pub slot: u32,
	pub src: [f32; 4],
	pub dst: [f32; 4],
	/// Mip level the tile comes from (fallback tiles have a higher one).
	pub level: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
	pub level: usize,
	pub tx: u32,
	pub ty: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FramePlan {
	/// Draw in order (coarse fallbacks first, so sharper tiles cover them).
	pub draws: Vec<TileDraw>,
	/// Tiles to composite, most important first: the target level centre-out,
	/// then the coarse coverage level.
	pub requests: Vec<TileKey>,
	/// True when every visible tile is drawn at the target level.
	pub complete: bool,
	/// Document rectangle on screen (for the transparency checkerboard).
	pub doc_rect: [f32; 4],
}

/// How many levels above the target level are always kept composited as a
/// fallback (1/16 of the tiles for +2).
pub const COVERAGE_LEVELS: usize = 2;

pub fn plan_frame(
	view: &ViewTransform,
	viewport: ViewportSize,
	doc_w: u32,
	doc_h: u32,
	level_count: usize,
	ready: &dyn Fn(TileKey) -> Option<u32>,
) -> FramePlan {
	// Quads are sent in the *unrotated* screen space; the viewport shader
	// turns them about the viewport centre by `ViewTransform::rotation`
	// (M6-T05), so a tile stays a rotated-quad draw.
	let flat = view.unrotated();
	let (dx0, dy0) = flat.doc_to_screen(viewport, 0.0, 0.0);
	let (dx1, dy1) = flat.doc_to_screen(viewport, doc_w as f64, doc_h as f64);
	let mut plan = FramePlan {
		doc_rect: [dx0 as f32, dy0 as f32, dx1 as f32, dy1 as f32],
		complete: true,
		..Default::default()
	};
	let level = view.mip_level(level_count);
	let Some(range) = view.visible_tiles(viewport, doc_w, doc_h, level) else {
		return plan;
	};
	let coverage = (level + COVERAGE_LEVELS).min(level_count - 1);

	// Target tiles, centre-out.
	let centre = view.screen_to_doc(viewport, viewport.width as f64 / 2.0, viewport.height as f64 / 2.0);
	let tile_doc = (TILE_SIZE as f64) * (1u64 << level) as f64;
	let mut targets: Vec<(u32, u32)> = range.iter().collect();
	targets.sort_by(|a, b| {
		let d = |t: &(u32, u32)| {
			let cx = (t.0 as f64 + 0.5) * tile_doc - centre.0;
			let cy = (t.1 as f64 + 0.5) * tile_doc - centre.1;
			cx * cx + cy * cy
		};
		d(a).total_cmp(&d(b))
	});

	let mut fallbacks = Vec::new();
	let mut sharp = Vec::new();
	for &(tx, ty) in &targets {
		let key = TileKey { level, tx, ty };
		let dst = tile_screen_rect(view, viewport, key);
		if let Some(slot) = ready(key) {
			sharp.push(TileDraw {
				slot,
				src: [0.0, 0.0, 1.0, 1.0],
				dst,
				level,
			});
			continue;
		}
		plan.complete = false;
		plan.requests.push(key);
		// Nearest ready ancestor, drawn as the matching sub-rectangle.
		for up in 1..level_count - level {
			let parent = TileKey {
				level: level + up,
				tx: tx >> up,
				ty: ty >> up,
			};
			if let Some(slot) = ready(parent) {
				let n = (1u32 << up) as f32;
				let (fx, fy) = ((tx % (1 << up)) as f32 / n, (ty % (1 << up)) as f32 / n);
				fallbacks.push(TileDraw {
					slot,
					src: [fx, fy, fx + 1.0 / n, fy + 1.0 / n],
					dst,
					level: parent.level,
				});
				break;
			}
		}
	}
	// Coverage level: cheap, keeps something on screen during fast pans/zooms.
	if coverage > level {
		let mut seen = std::collections::HashSet::new();
		for &(tx, ty) in &targets {
			let up = coverage - level;
			let key = TileKey {
				level: coverage,
				tx: tx >> up,
				ty: ty >> up,
			};
			if seen.insert(key) && ready(key).is_none() {
				plan.requests.push(key);
			}
		}
	}
	// Coarsest first so sharper draws cover them.
	fallbacks.sort_by_key(|d| std::cmp::Reverse(d.level));
	plan.draws = fallbacks;
	plan.draws.extend(sharp);
	plan
}

fn tile_screen_rect(view: &ViewTransform, viewport: ViewportSize, key: TileKey) -> [f32; 4] {
	let size = (TILE_SIZE as f64) * (1u64 << key.level) as f64;
	// Unrotated: the shader turns the quad (see `plan_frame`).
	let flat = view.unrotated();
	let (x0, y0) = flat.doc_to_screen(viewport, key.tx as f64 * size, key.ty as f64 * size);
	let (x1, y1) = flat.doc_to_screen(viewport, (key.tx + 1) as f64 * size, (key.ty + 1) as f64 * size);
	[x0 as f32, y0 as f32, x1 as f32, y1 as f32]
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use super::*;

	const VP: ViewportSize = ViewportSize { width: 1600, height: 900 };

	#[test]
	fn all_ready_draws_every_visible_tile_once() {
		let view = ViewTransform {
			zoom: 1.0,
			center_x: 5000.0,
			center_y: 5000.0,
			rotation: 0.0,
		};
		let plan = plan_frame(&view, VP, 30_000, 30_000, 8, &|_| Some(0));
		let range = view.visible_tiles(VP, 30_000, 30_000, 0).unwrap();
		assert!(plan.complete);
		assert_eq!(plan.draws.len(), range.count());
		assert!(plan.requests.is_empty());
	}

	#[test]
	fn missing_tiles_fall_back_to_ready_ancestors() {
		let view = ViewTransform {
			zoom: 1.0,
			center_x: 1100.0,
			center_y: 1100.0,
			rotation: 0.0,
		};
		// Only level-2 tiles are ready.
		let plan = plan_frame(&view, VP, 30_000, 30_000, 8, &|k| (k.level == 2).then_some(100 + k.tx + 10 * k.ty));
		assert!(!plan.complete);
		assert!(plan.draws.iter().all(|d| d.level == 2));
		// Tile (5, 5) at level 0 → parent (1, 1) at level 2, quarter (1, 1) of it.
		let rect = tile_screen_rect(&view, VP, TileKey { level: 0, tx: 5, ty: 5 });
		let draw = plan.draws.iter().find(|d| d.dst == rect).expect("tile (5,5) is visible");
		assert_eq!(draw.slot, 111);
		assert_eq!(draw.src, [0.25, 0.25, 0.5, 0.5]);
		// Requests: target level first, centre tile first.
		assert_eq!(plan.requests[0].level, 0);
		let first = plan.requests[0];
		assert_eq!((first.tx, first.ty), (4, 4), "tile under the viewport centre");
	}

	#[test]
	fn coverage_level_is_requested() {
		let view = ViewTransform::fit(VP, 30_000, 30_000); // level 5
		let plan = plan_frame(&view, VP, 30_000, 30_000, 8, &|_| None);
		assert!(plan.requests.iter().any(|k| k.level == 7));
		assert!(plan.draws.is_empty());
	}

	#[test]
	fn sharp_tiles_are_drawn_after_fallbacks() {
		let view = ViewTransform {
			zoom: 1.0,
			center_x: 1024.0,
			center_y: 1024.0,
			rotation: 0.0,
		};
		let ready: HashMap<TileKey, u32> = [(TileKey { level: 0, tx: 4, ty: 4 }, 1), (TileKey { level: 3, tx: 0, ty: 0 }, 2)].into();
		let plan = plan_frame(&view, VP, 30_000, 30_000, 8, &|k| ready.get(&k).copied());
		let last = plan.draws.last().unwrap();
		assert_eq!(last.level, 0);
		assert!(plan.draws[..plan.draws.len() - 1].iter().all(|d| d.level == 3));
	}
}
