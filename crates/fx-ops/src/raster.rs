//! Selection shape rasterisers (M5-T03).
//!
//! One tile at a time, and only the tiles the shape's bounding box touches:
//! a marquee across a 30 000² document costs its own area, not the canvas.
//!
//! Coverage:
//! * rectangle — exact area of the pixel covered by the rectangle
//!   (`docs/tasks/SNIPPETS.md` §6);
//! * ellipse — 1/0 from a normalised distance test, supersampled only in the
//!   one-pixel band around the boundary;
//! * polygon — the non-zero winding rule, supersampled only on the boundary;
//! * one-pixel row/column — a whole row/column at coverage 1.
//!
//! Anti-alias off thresholds every coverage at 0.5, like Photoshop.

use fx_core::selection::{Selection, SelectionShape, set_gray};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_SIZE, TileBuffer, TileStore};

/// Supersampling per axis for the boundary band of an ellipse or polygon.
const SUPERSAMPLE: u32 = 8;

/// Rasterise `shape` (document pixels) into a fresh selection.
pub fn rasterise(shape: &SelectionShape, size: (u32, u32), depth: BitDepth, anti_alias: bool, store: &TileStore) -> Result<Selection, CommandError> {
	if size.0 == 0 || size.1 == 0 {
		return Err(CommandError::NotAllowed("the document is empty".into()));
	}
	let format = depth.gray_format();
	let mut selection = Selection::empty(size, depth);
	let Some((x0, y0, x1, y1)) = shape.bounds() else {
		return Ok(selection);
	};
	// Clip to the canvas; an empty intersection selects nothing.
	let x0 = x0.max(0.0);
	let y0 = y0.max(0.0);
	let x1 = x1.min(f64::from(size.0));
	let y1 = y1.min(f64::from(size.1));
	if !(x1 > x0 && y1 > y0) {
		return Ok(selection);
	}
	let (tx0, ty0) = (x0.floor() as u32 / TILE_SIZE, y0.floor() as u32 / TILE_SIZE);
	let (tx1, ty1) = (
		(x1.ceil() as u32).saturating_sub(1) / TILE_SIZE,
		(y1.ceil() as u32).saturating_sub(1) / TILE_SIZE,
	);
	for ty in ty0..=ty1 {
		for tx in tx0..=tx1 {
			let mut buffer = TileBuffer::zeroed(format);
			let (base_x, base_y) = (i64::from(tx) * i64::from(TILE_SIZE), i64::from(ty) * i64::from(TILE_SIZE));
			for py in 0..TILE_SIZE {
				for px in 0..TILE_SIZE {
					let (doc_x, doc_y) = (base_x + i64::from(px), base_y + i64::from(py));
					let mut coverage = coverage(shape, doc_x, doc_y, size);
					if !anti_alias {
						coverage = if coverage >= 0.5 { 1.0 } else { 0.0 };
					}
					if coverage > 0.0 {
						set_gray(&mut buffer, format, px, py, coverage);
					}
				}
			}
			selection.image.put_buffer(store, tx, ty, buffer);
		}
	}
	Ok(selection)
}

/// Coverage of the pixel whose top-left corner is `(px, py)`, in document
/// pixels (0 outside the canvas).
fn coverage(shape: &SelectionShape, px: i64, py: i64, size: (u32, u32)) -> f32 {
	if px < 0 || py < 0 || px >= i64::from(size.0) || py >= i64::from(size.1) {
		return 0.0;
	}
	let (fx, fy) = (px as f64, py as f64);
	match shape {
		SelectionShape::Rect { x, y, w, h } => {
			let (x0, x1) = (x.min(x + w), x.max(x + w));
			let (y0, y1) = (y.min(y + h), y.max(y + h));
			rect_coverage(px, py, (x0, y0, x1, y1))
		}
		SelectionShape::Ellipse { x, y, w, h } => {
			let cx = x + w / 2.0;
			let cy = y + h / 2.0;
			ellipse_coverage(cx, cy, (w.abs() / 2.0).max(f64::EPSILON), (h.abs() / 2.0).max(f64::EPSILON), fx, fy)
		}
		SelectionShape::Polygon { points } => polygon_coverage(points, fx, fy),
		SelectionShape::RowPixel { y } => f32::from(py == y.floor() as i64),
		SelectionShape::ColumnPixel { x } => f32::from(px == x.floor() as i64),
	}
}

/// Exact area of pixel `(px, py)` — the square `[px, px+1) × [py, py+1)` —
/// covered by the rectangle `[x0, x1) × [y0, y1)` (SNIPPETS §6).
///
/// Wrong: testing only the pixel centre (no anti-aliasing) or the corner
/// (everything shifts by half a pixel).
fn rect_coverage(px: i64, py: i64, (x0, y0, x1, y1): (f64, f64, f64, f64)) -> f32 {
	let ox = ((px + 1) as f64).min(x1) - (px as f64).max(x0);
	let oy = ((py + 1) as f64).min(y1) - (py as f64).max(y0);
	(ox.max(0.0) * oy.max(0.0)) as f32
}

/// Exact coverage of an axis-aligned pixel by an ellipse: a full 1/0 outside
/// the boundary band, supersampled inside it.
fn ellipse_coverage(cx: f64, cy: f64, rx: f64, ry: f64, px: f64, py: f64) -> f32 {
	if rx <= 0.0 || ry <= 0.0 {
		return 0.0;
	}
	let inside = |x: f64, y: f64| {
		let nx = (x - cx) / rx;
		let ny = (y - cy) / ry;
		nx * nx + ny * ny <= 1.0
	};
	// Half-diagonal of one pixel in normalised units: the furthest the
	// boundary can sit from the pixel centre and still touch this pixel.
	// A displacement of at most half a pixel per axis maps to at most this
	// much in the ellipse's normalised (u, v) space.
	let margin = 0.5 * (1.0 / (rx * rx) + 1.0 / (ry * ry)).sqrt();
	let (mx, my) = (px + 0.5, py + 0.5);
	let d = (((mx - cx) / rx).powi(2) + ((my - cy) / ry).powi(2)).sqrt();
	if d <= 1.0 - margin {
		return 1.0;
	}
	if d >= 1.0 + margin {
		return 0.0;
	}
	supersample(px, py, &|x, y| inside(x, y))
}

/// Coverage of a polygon by the non-zero winding rule: a full 1/0 outside the
/// boundary band, supersampled inside it. A self-intersecting shape (a star)
/// fills its doubly-wound middle, like Photoshop.
fn polygon_coverage(points: &[(f64, f64)], px: f64, py: f64) -> f32 {
	if points.len() < 3 {
		return 0.0;
	}
	let inside = |x: f64, y: f64| winding(points, x, y);
	// Corners of the pixel plus its centre: all in, all out, or boundary.
	let corners = [inside(px, py), inside(px + 1.0, py), inside(px + 1.0, py + 1.0), inside(px, py + 1.0)];
	let centre = inside(px + 0.5, py + 0.5);
	if corners.iter().all(|c| *c) && centre {
		return 1.0;
	}
	if corners.iter().all(|c| !*c) && !centre {
		return 0.0;
	}
	supersample(px, py, &inside)
}

/// `SUPERSAMPLE²` samples of `inside` over the pixel.
fn supersample(px: f64, py: f64, inside: &dyn Fn(f64, f64) -> bool) -> f32 {
	let step = 1.0 / f64::from(SUPERSAMPLE);
	let mut hits = 0u32;
	for j in 0..SUPERSAMPLE {
		for i in 0..SUPERSAMPLE {
			if inside(px + (f64::from(i) + 0.5) * step, py + (f64::from(j) + 0.5) * step) {
				hits += 1;
			}
		}
	}
	hits as f32 / (SUPERSAMPLE * SUPERSAMPLE) as f32
}

/// The non-zero winding rule (`true` = inside), including the top-left
/// half-open rule so neighbouring shapes never double-select an edge.
fn winding(points: &[(f64, f64)], x: f64, y: f64) -> bool {
	let mut winding = 0i32;
	for i in 0..points.len() {
		let a = points[i];
		let b = points[(i + 1) % points.len()];
		let side = (b.0 - a.0) * (y - a.1) - (x - a.0) * (b.1 - a.1);
		if a.1 <= y {
			if b.1 > y && side > 0.0 {
				winding += 1;
			}
		} else if b.1 <= y && side < 0.0 {
			winding -= 1;
		}
	}
	winding != 0
}

#[cfg(test)]
mod tests {
	use std::f64::consts::PI;

	use fx_core::selection::gray_at;
	use fx_tiles::{TileSlot, TileStoreConfig};

	use super::*;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-ops-raster-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// The total coverage of a selection in pixels (its area).
	fn area(selection: &Selection, store: &TileStore, size: (u32, u32)) -> f64 {
		let format = selection.image.format();
		let mut total = 0.0;
		for ty in 0..size.1.div_ceil(TILE_SIZE) {
			for tx in 0..size.0.div_ceil(TILE_SIZE) {
				match selection.image.slot(0, tx, ty) {
					TileSlot::Empty => {}
					TileSlot::Solid(value) => {
						// A solid tile stores the value on the 16-bit scale.
						let v = f64::from(value.0[0]) / 65535.0;
						// Only the in-canvas pixels of the tile.
						let w = (size.0 - tx * TILE_SIZE).min(TILE_SIZE);
						let h = (size.1 - ty * TILE_SIZE).min(TILE_SIZE);
						total += v * f64::from(w) * f64::from(h);
					}
					TileSlot::Data(handle) => {
						let buffer = store.get(handle).unwrap();
						let w = (size.0 - tx * TILE_SIZE).min(TILE_SIZE);
						let h = (size.1 - ty * TILE_SIZE).min(TILE_SIZE);
						for y in 0..h {
							for x in 0..w {
								total += f64::from(gray_at(&buffer, format, x, y));
							}
						}
					}
				}
			}
		}
		total
	}

	fn value(selection: &Selection, store: &TileStore, x: u32, y: u32) -> f32 {
		let format = selection.image.format();
		match selection.image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
			TileSlot::Empty => 0.0,
			TileSlot::Solid(v) => f32::from(v.0[0]) / 65535.0,
			TileSlot::Data(handle) => gray_at(&store.get(handle).unwrap(), format, x % TILE_SIZE, y % TILE_SIZE),
		}
	}

	#[test]
	fn a_fractional_rectangle_covers_its_edges_exactly() {
		let store = store();
		let shape = SelectionShape::Rect {
			x: 0.25,
			y: 0.25,
			w: 10.0,
			h: 10.0,
		};
		let selection = rasterise(&shape, (16, 16), BitDepth::U8, true, &store).unwrap();
		// Corner pixel: 0.75 × 0.75 covered.
		assert!((value(&selection, &store, 0, 0) - 0.5625).abs() < 0.01, "{}", value(&selection, &store, 0, 0));
		// Fully inside.
		assert_eq!(value(&selection, &store, 5, 5), 1.0);
		// The right edge at x = 10.25: pixel 10 is 0.25 covered.
		assert!((value(&selection, &store, 10, 5) - 0.25).abs() < 0.01);
		// Area = 100 px exactly.
		assert!((area(&selection, &store, (16, 16)) - 100.0).abs() < 0.05);
	}

	#[test]
	fn anti_alias_off_rounds_coverage_at_half() {
		let store = store();
		let shape = SelectionShape::Rect {
			x: 0.75,
			y: 0.0,
			w: 4.0,
			h: 4.0,
		};
		let soft = rasterise(&shape, (8, 8), BitDepth::U8, true, &store).unwrap();
		let hard = rasterise(&shape, (8, 8), BitDepth::U8, false, &store).unwrap();
		assert!((value(&soft, &store, 0, 1) - 0.25).abs() < 0.01);
		assert_eq!(value(&hard, &store, 0, 1), 0.0, "0.25 < 0.5 rounds down");
		assert_eq!(value(&hard, &store, 1, 1), 1.0);
	}

	#[test]
	fn an_ellipse_covers_pi_ab() {
		let store = store();
		let (w, h) = (400u32, 300u32);
		let shape = SelectionShape::Ellipse {
			x: 0.0,
			y: 0.0,
			w: f64::from(w),
			h: f64::from(h),
		};
		let selection = rasterise(&shape, (w, h), BitDepth::U8, true, &store).unwrap();
		let exact = PI * f64::from(w) / 2.0 * f64::from(h) / 2.0;
		let got = area(&selection, &store, (w, h));
		let error = (got - exact).abs() / exact;
		assert!(error < 0.001, "area {got} vs {exact} ({:.3} %)", error * 100.0);
	}

	#[test]
	fn a_polygon_uses_the_non_zero_winding_rule() {
		let store = store();
		// A five-pointed star drawn with the "pentagram" method: self
		// intersecting, so its middle is wound twice.
		let (cx, cy, r) = (100.0, 100.0, 80.0);
		let points: Vec<(f64, f64)> = (0..5)
			.map(|i| {
				let angle = -PI / 2.0 + f64::from(i) * 4.0 * PI / 5.0;
				(cx + r * angle.cos(), cy + r * angle.sin())
			})
			.collect();
		let shape = SelectionShape::Polygon { points };
		let selection = rasterise(&shape, (200, 200), BitDepth::U8, true, &store).unwrap();
		assert_eq!(value(&selection, &store, 100, 100), 1.0, "the doubly-wound middle is inside");
		assert_eq!(value(&selection, &store, 100, 10), 0.0, "outside the top point");
		assert!(value(&selection, &store, 100, 40) > 0.9, "inside the top point");
	}

	#[test]
	fn a_row_selection_is_one_pixel_tall() {
		let store = store();
		let shape = SelectionShape::RowPixel { y: 3.9 };
		let selection = rasterise(&shape, (10, 10), BitDepth::U8, true, &store).unwrap();
		assert_eq!(value(&selection, &store, 4, 3), 1.0);
		assert_eq!(value(&selection, &store, 4, 4), 0.0);
		assert!((area(&selection, &store, (10, 10)) - 10.0).abs() < 0.01);
	}

	#[test]
	fn a_shape_outside_the_canvas_selects_nothing() {
		let store = store();
		let shape = SelectionShape::Rect {
			x: -50.0,
			y: -50.0,
			w: 10.0,
			h: 10.0,
		};
		let selection = rasterise(&shape, (100, 100), BitDepth::U8, true, &store).unwrap();
		assert!(selection.is_empty());
	}
}
