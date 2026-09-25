//! Viewport math: document ↔ screen transforms, mip level choice, visible
//! tiles. Pure math, no GPU. Fully implemented; extend, don't rewrite.
//!
//! Conventions
//! * Document space: pixels of the full-resolution document, origin top-left.
//! * Screen space: physical pixels of the viewport rectangle, origin top-left.
//! * `zoom` = screen pixels per document pixel (1.0 = 100 %).
//! * `rotation` (M6-T05) turns the view about the viewport centre, in radians.
//!   It is a rigid turn of the whole view, so a document point's screen image
//!   is `R(rotation)·(p − centre)·zoom + viewport/2` (screen y points down, so
//!   a positive angle turns the document clockwise on screen).

use fx_tiles::TILE_SIZE;

pub const MIN_ZOOM: f64 = 0.01; // 1 %
pub const MAX_ZOOM: f64 = 32.0; // 3200 %

/// Photoshop-like zoom steps for Ctrl+/Ctrl- (as fractions).
pub const ZOOM_STEPS: &[f64] = &[
	0.01, 0.015, 0.02, 0.03, 0.04, 0.05, 0.0625, 0.0833, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 12.0, 16.0, 32.0,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportSize {
	pub width: u32,
	pub height: u32,
}

/// Inclusive-exclusive tile range at one mip level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRange {
	pub level: usize,
	pub x0: u32,
	pub y0: u32,
	pub x1: u32,
	pub y1: u32,
}

impl TileRange {
	pub fn count(&self) -> usize {
		((self.x1 - self.x0) * (self.y1 - self.y0)) as usize
	}

	pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
		(self.y0..self.y1).flat_map(move |y| (self.x0..self.x1).map(move |x| (x, y)))
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
	pub zoom: f64,
	/// Document point shown at the centre of the viewport.
	pub center_x: f64,
	pub center_y: f64,
	/// View rotation about the viewport centre, in radians (M6-T05).
	pub rotation: f64,
}

/// Angle of one Shift-constrained Rotate View step (Photoshop: 15°).
pub const ROTATE_VIEW_STEP_DEG: f64 = 15.0;

impl ViewTransform {
	/// Largest zoom (never above 100 %) that shows the whole document.
	pub fn fit(viewport: ViewportSize, doc_w: u32, doc_h: u32) -> Self {
		let zx = viewport.width as f64 / doc_w as f64;
		let zy = viewport.height as f64 / doc_h as f64;
		Self {
			zoom: zx.min(zy).min(1.0).clamp(MIN_ZOOM, MAX_ZOOM),
			center_x: doc_w as f64 / 2.0,
			center_y: doc_h as f64 / 2.0,
			rotation: 0.0,
		}
	}

	/// A copy with the rotation cleared, for callers that work in the
	/// unrotated screen space (the frame plan sends quads the shader turns).
	pub fn unrotated(&self) -> Self {
		Self { rotation: 0.0, ..*self }
	}

	/// `sin(rotation)` and `cos(rotation)`.
	fn turn(&self) -> (f64, f64) {
		self.rotation.sin_cos()
	}

	pub fn doc_to_screen(&self, viewport: ViewportSize, x: f64, y: f64) -> (f64, f64) {
		let (s, c) = self.turn();
		let dx = (x - self.center_x) * self.zoom;
		let dy = (y - self.center_y) * self.zoom;
		(dx * c - dy * s + viewport.width as f64 / 2.0, dx * s + dy * c + viewport.height as f64 / 2.0)
	}

	pub fn screen_to_doc(&self, viewport: ViewportSize, sx: f64, sy: f64) -> (f64, f64) {
		let (s, c) = self.turn();
		let ox = sx - viewport.width as f64 / 2.0;
		let oy = sy - viewport.height as f64 / 2.0;
		// The inverse turn of `doc_to_screen`.
		let ix = ox * c + oy * s;
		let iy = -ox * s + oy * c;
		(ix / self.zoom + self.center_x, iy / self.zoom + self.center_y)
	}

	/// Pan by a screen-space delta (hand tool drag). The content follows the
	/// pointer, so the delta is turned back into view space before it moves
	/// the centre.
	pub fn pan_screen(&mut self, dx: f64, dy: f64) {
		let (s, c) = self.turn();
		self.center_x -= (dx * c + dy * s) / self.zoom;
		self.center_y -= (-dx * s + dy * c) / self.zoom;
	}

	/// Change zoom keeping the document point under `(sx, sy)` fixed
	/// (Ctrl+wheel, zoom tool click).
	pub fn zoom_at(&mut self, viewport: ViewportSize, sx: f64, sy: f64, new_zoom: f64) {
		let (dx, dy) = self.screen_to_doc(viewport, sx, sy);
		self.zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
		// solve doc_to_screen(dx, dy) == (sx, sy) for the centre
		let (s, c) = self.turn();
		let ox = sx - viewport.width as f64 / 2.0;
		let oy = sy - viewport.height as f64 / 2.0;
		self.center_x = dx - (ox * c + oy * s) / self.zoom;
		self.center_y = dy - (-ox * s + oy * c) / self.zoom;
	}

	/// Next/previous entry of [`ZOOM_STEPS`] relative to the current zoom.
	pub fn step_zoom(&self, direction: i32) -> f64 {
		const EPS: f64 = 1e-6;
		if direction > 0 {
			ZOOM_STEPS.iter().copied().find(|z| *z > self.zoom + EPS).unwrap_or(MAX_ZOOM)
		} else {
			ZOOM_STEPS.iter().rev().copied().find(|z| *z < self.zoom - EPS).unwrap_or(MIN_ZOOM)
		}
	}

	/// Mip level to composite from: the smallest image that still has at
	/// least one source pixel per screen pixel. `level_count` comes from
	/// `TiledImage::level_count` for the document size.
	pub fn mip_level(&self, level_count: usize) -> usize {
		if self.zoom >= 1.0 {
			return 0;
		}
		// 2^level <= 1/zoom  (+epsilon so exact powers of two pick that level)
		let level = (1.0 / self.zoom + 1e-9).log2().floor() as usize;
		level.min(level_count.saturating_sub(1))
	}

	/// Visible part of the document in document pixels, clipped to the
	/// document: `(x0, y0, x1, y1)`, or `None` if nothing is visible.
	pub fn visible_doc_rect(&self, viewport: ViewportSize, doc_w: u32, doc_h: u32) -> Option<(f64, f64, f64, f64)> {
		// The rotated screen rectangle is a quad in document space: the
		// visible rectangle is the bounding box of its four corners.
		let (w, h) = (viewport.width as f64, viewport.height as f64);
		let mut x0 = f64::INFINITY;
		let mut y0 = f64::INFINITY;
		let mut x1 = f64::NEG_INFINITY;
		let mut y1 = f64::NEG_INFINITY;
		for (sx, sy) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
			let (x, y) = self.screen_to_doc(viewport, sx, sy);
			x0 = x0.min(x);
			y0 = y0.min(y);
			x1 = x1.max(x);
			y1 = y1.max(y);
		}
		let x0 = x0.max(0.0);
		let y0 = y0.max(0.0);
		let x1 = x1.min(doc_w as f64);
		let y1 = y1.min(doc_h as f64);
		(x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
	}

	/// Tiles of `level` that intersect the visible rectangle.
	pub fn visible_tiles(&self, viewport: ViewportSize, doc_w: u32, doc_h: u32, level: usize) -> Option<TileRange> {
		let (x0, y0, x1, y1) = self.visible_doc_rect(viewport, doc_w, doc_h)?;
		let tile_doc = (TILE_SIZE as f64) * (1u64 << level) as f64; // document pixels per tile at this level
		let cols = (doc_w as f64 / tile_doc).ceil() as u32;
		let rows = (doc_h as f64 / tile_doc).ceil() as u32;
		Some(TileRange {
			level,
			x0: (x0 / tile_doc).floor() as u32,
			y0: (y0 / tile_doc).floor() as u32,
			x1: ((x1 / tile_doc).ceil() as u32).min(cols),
			y1: ((y1 / tile_doc).ceil() as u32).min(rows),
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const VP: ViewportSize = ViewportSize { width: 1600, height: 900 };

	#[test]
	fn fit_30k_is_about_3_percent() {
		let t = ViewTransform::fit(VP, 30_000, 30_000);
		assert!((t.zoom - 0.03).abs() < 1e-9);
		assert_eq!(t.mip_level(8), 5); // 1/0.03 = 33.3 → 2^5 = 32
	}

	#[test]
	fn screen_doc_roundtrip() {
		let t = ViewTransform {
			zoom: 0.37,
			center_x: 1234.5,
			center_y: 777.0,
			rotation: 0.0,
		};
		let (sx, sy) = t.doc_to_screen(VP, 100.0, 200.0);
		let (x, y) = t.screen_to_doc(VP, sx, sy);
		assert!((x - 100.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9);
	}

	#[test]
	fn zoom_at_keeps_cursor_point() {
		let mut t = ViewTransform::fit(VP, 30_000, 20_000);
		let before = t.screen_to_doc(VP, 300.0, 700.0);
		t.zoom_at(VP, 300.0, 700.0, 2.0);
		let after = t.screen_to_doc(VP, 300.0, 700.0);
		assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6);
	}

	#[test]
	fn mip_levels() {
		let at = |zoom| {
			ViewTransform {
				zoom,
				center_x: 0.0,
				center_y: 0.0,
				rotation: 0.0,
			}
			.mip_level(8)
		};
		assert_eq!(at(2.0), 0);
		assert_eq!(at(1.0), 0);
		assert_eq!(at(0.75), 0);
		assert_eq!(at(0.5), 1);
		assert_eq!(at(0.3), 1);
		assert_eq!(at(0.25), 2);
		assert_eq!(at(0.01), 6);
		assert_eq!(
			ViewTransform {
				zoom: 0.001,
				center_x: 0.0,
				center_y: 0.0,
				rotation: 0.0
			}
			.mip_level(8),
			7,
			"clamped to the last level"
		);
	}

	#[test]
	fn visible_tiles_scale_with_screen_not_document() {
		// At any zoom, the number of visible tiles is bounded by the screen size.
		for zoom in [0.03, 0.1, 0.33, 0.5, 1.0, 4.0] {
			let mut t = ViewTransform::fit(VP, 30_000, 30_000);
			t.zoom = zoom;
			let level = t.mip_level(8);
			let range = t.visible_tiles(VP, 30_000, 30_000, level).unwrap();
			// 1600x900 screen → at most ~ (1600/128+2) * (900/128+2) tiles
			assert!(range.count() <= 15 * 10, "zoom {zoom}: {} tiles", range.count());
		}
	}

	#[test]
	fn zoom_steps() {
		let t = ViewTransform {
			zoom: 1.0,
			center_x: 0.0,
			center_y: 0.0,
			rotation: 0.0,
		};
		assert_eq!(t.step_zoom(1), 2.0);
		assert_eq!(t.step_zoom(-1), 0.6667);
	}

	#[test]
	fn rotated_screen_doc_roundtrip_and_centre() {
		for rotation in [0.0, 30f64.to_radians(), 45f64.to_radians(), 90f64.to_radians(), 180f64.to_radians()] {
			let t = ViewTransform {
				zoom: 0.37,
				center_x: 1234.5,
				center_y: 777.0,
				rotation,
			};
			// The viewport centre always shows the view centre.
			let (cx, cy) = t.doc_to_screen(VP, 1234.5, 777.0);
			assert!((cx - VP.width as f64 / 2.0).abs() < 1e-9 && (cy - VP.height as f64 / 2.0).abs() < 1e-9);
			// And a round trip returns the point.
			let (sx, sy) = t.doc_to_screen(VP, 100.0, 200.0);
			let (x, y) = t.screen_to_doc(VP, sx, sy);
			assert!((x - 100.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9, "rotation {rotation}: ({x}, {y})");
		}
	}

	#[test]
	fn rotated_visible_tiles_stay_bounded_by_the_screen() {
		for rotation in [0.0, 30f64.to_radians(), 45f64.to_radians(), 90f64.to_radians()] {
			for zoom in [0.03, 0.1, 0.33, 1.0, 4.0] {
				let mut t = ViewTransform::fit(VP, 30_000, 30_000);
				t.zoom = zoom;
				t.rotation = rotation;
				let level = t.mip_level(8);
				let range = t.visible_tiles(VP, 30_000, 30_000, level).unwrap();
				// A 45° turn needs the diagonal: (1600+900)/128 ≈ 20 cols/rows.
				assert!(range.count() <= 22 * 22, "rotation {rotation}, zoom {zoom}: {} tiles", range.count());
			}
		}
	}

	#[test]
	fn zoom_at_with_rotation_keeps_the_cursor_point() {
		let mut t = ViewTransform {
			zoom: 0.5,
			center_x: 1000.0,
			center_y: 800.0,
			rotation: 40f64.to_radians(),
		};
		let before = t.screen_to_doc(VP, 300.0, 700.0);
		t.zoom_at(VP, 300.0, 700.0, 2.0);
		let after = t.screen_to_doc(VP, 300.0, 700.0);
		assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6);
	}
}
