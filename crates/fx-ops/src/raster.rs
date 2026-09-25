//! Selection shape rasterisers (M5-T03, rewritten in the M5 review).
//!
//! Only the tiles the shape's bounding box touches are visited, on rayon, and
//! a tile the shape covers entirely becomes a solid tile without a buffer: a
//! marquee across a 30 000² document costs its outline, not its area.
//!
//! Coverage:
//! * rectangle — the exact area of each pixel covered (`docs/tasks/SNIPPETS.md` §6);
//! * polygon — **non-zero winding**, scan converted with [`SUBSAMPLES`]
//!   sub-scanlines per pixel row and exact horizontal coverage of each span
//!   (the accumulation approach of font rasterisers, sparse: one row buffer per
//!   band of tiles, never an image-sized buffer);
//! * ellipse — a polygon with enough segments that the chord error stays
//!   under 0.05 px, whatever the radius;
//! * one-pixel row/column — a whole row/column at coverage 1.
//!
//! Anti-alias off thresholds every coverage at 0.5, like Photoshop.

use fx_core::selection::{OutTile, Selection, SelectionShape, canvas_grid, out_tile, valid_extent};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};
use rayon::prelude::*;

/// Sub-scanlines per pixel row for polygons (vertical anti-aliasing steps).
pub const SUBSAMPLES: u32 = 16;

/// Rasterise `shape` (document pixels) into a fresh canvas-aligned selection.
pub fn rasterise(shape: &SelectionShape, size: (u32, u32), depth: BitDepth, anti_alias: bool, store: &TileStore) -> Result<Selection, CommandError> {
	if size.0 == 0 || size.1 == 0 {
		return Err(CommandError::NotAllowed("the document is empty".into()));
	}
	let format = depth.gray_format();
	let Some((x0, y0, x1, y1)) = shape.bounds() else {
		return Ok(Selection::empty(size, depth));
	};
	// Clip to the canvas; an empty intersection selects nothing.
	let (x0, y0) = (x0.max(0.0), y0.max(0.0));
	let (x1, y1) = (x1.min(f64::from(size.0)), y1.min(f64::from(size.1)));
	if !(x1 > x0 && y1 > y0) {
		return Ok(Selection::empty(size, depth));
	}
	let (cols, rows) = canvas_grid(size);
	let tx0 = (x0.floor() as u32 / TILE_SIZE).min(cols - 1);
	let ty0 = (y0.floor() as u32 / TILE_SIZE).min(rows - 1);
	let tx1 = ((x1.ceil() as u32).saturating_sub(1) / TILE_SIZE).min(cols - 1);
	let ty1 = ((y1.ceil() as u32).saturating_sub(1) / TILE_SIZE).min(rows - 1);
	let finish = |values: &mut [f32], tx: u32, ty: u32| -> OutTile {
		if !anti_alias {
			for v in values.iter_mut() {
				*v = if *v >= 0.5 { 1.0 } else { 0.0 };
			}
		}
		out_tile(format, values, valid_extent(size, tx, ty))
	};

	let tiles: Vec<((u32, u32), OutTile)> = match shape {
		SelectionShape::Polygon { points } => polygon_tiles(points, size, (tx0, ty0, tx1, ty1), &finish),
		SelectionShape::Ellipse { x, y, w, h } => {
			let points = ellipse_polygon(x + w / 2.0, y + h / 2.0, w.abs() / 2.0, h.abs() / 2.0);
			polygon_tiles(&points, size, (tx0, ty0, tx1, ty1), &finish)
		}
		_ => {
			let list: Vec<(u32, u32)> = (ty0..=ty1).flat_map(|ty| (tx0..=tx1).map(move |tx| (tx, ty))).collect();
			list.par_iter()
				.map(|&(tx, ty)| {
					let mut values = vec![0.0f32; TILE_PIXELS];
					let (bx, by) = (i64::from(tx * TILE_SIZE), i64::from(ty * TILE_SIZE));
					for (i, v) in values.iter_mut().enumerate() {
						let (px, py) = (bx + (i as u32 % TILE_SIZE) as i64, by + (i as u32 / TILE_SIZE) as i64);
						*v = simple_coverage(shape, px, py);
					}
					((tx, ty), finish(&mut values, tx, ty))
				})
				.collect()
		}
	};
	Ok(Selection::from_tiles(size, depth, tiles, store).unwrap_or_else(|| Selection::empty(size, depth)))
}

/// Coverage of pixel `(px, py)` by a rectangle, a row or a column.
fn simple_coverage(shape: &SelectionShape, px: i64, py: i64) -> f32 {
	match shape {
		SelectionShape::Rect { x, y, w, h } => {
			let (x0, x1) = (x.min(x + w), x.max(x + w));
			let (y0, y1) = (y.min(y + h), y.max(y + h));
			rect_coverage(px, py, (x0, y0, x1, y1))
		}
		SelectionShape::RowPixel { y } => f32::from(py == y.floor() as i64),
		SelectionShape::ColumnPixel { x } => f32::from(px == x.floor() as i64),
		SelectionShape::Ellipse { .. } | SelectionShape::Polygon { .. } => 0.0,
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

/// An ellipse as a polygon whose chords stay within 0.05 px of the curve:
/// the sagitta of a chord is `r(1 − cos(π/n)) ≈ rπ²/2n²`, so
/// `n ≥ π·√(r / 0.1)`.
pub fn ellipse_polygon(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<(f64, f64)> {
	let r = rx.max(ry);
	let n = ((std::f64::consts::PI * (r / 0.1).sqrt()).ceil() as usize).clamp(64, 1 << 16);
	(0..n)
		.map(|i| {
			let a = std::f64::consts::TAU * i as f64 / n as f64;
			(cx + rx * a.cos(), cy + ry * a.sin())
		})
		.collect()
}

/// One polygon edge, oriented as drawn (`dir` = +1 going down).
#[derive(Clone, Copy)]
struct Edge {
	x0: f64,
	y0: f64,
	x1: f64,
	y1: f64,
	/// `min(y0, y1)` and `max(y0, y1)`.
	top: f64,
	bottom: f64,
	dir: i32,
}

impl Edge {
	fn x_at(&self, y: f64) -> f64 {
		self.x0 + (y - self.y0) * (self.x1 - self.x0) / (self.y1 - self.y0)
	}
}

/// Scan convert a polygon (non-zero winding) over the tile range, one band of
/// tile rows per rayon task.
fn polygon_tiles(
	points: &[(f64, f64)],
	size: (u32, u32),
	(tx0, ty0, tx1, ty1): (u32, u32, u32, u32),
	finish: &(dyn Fn(&mut [f32], u32, u32) -> OutTile + Sync),
) -> Vec<((u32, u32), OutTile)> {
	if points.len() < 3 {
		return Vec::new();
	}
	let edges: Vec<Edge> = (0..points.len())
		.filter_map(|i| {
			let (a, b) = (points[i], points[(i + 1) % points.len()]);
			// Horizontal edges never cross a scanline.
			(a.1 != b.1 && a.0.is_finite() && a.1.is_finite() && b.0.is_finite() && b.1.is_finite()).then(|| Edge {
				x0: a.0,
				y0: a.1,
				x1: b.0,
				y1: b.1,
				top: a.1.min(b.1),
				bottom: a.1.max(b.1),
				dir: if b.1 > a.1 { 1 } else { -1 },
			})
		})
		.collect();
	let bands: Vec<u32> = (ty0..=ty1).collect();
	bands
		.par_iter()
		.flat_map_iter(|&ty| polygon_band(&edges, size, ty, (tx0, tx1), finish))
		.collect()
}

/// The tiles of one band (tile row `ty`, columns `tx0..=tx1`).
fn polygon_band(
	edges: &[Edge],
	size: (u32, u32),
	ty: u32,
	(tx0, tx1): (u32, u32),
	finish: &(dyn Fn(&mut [f32], u32, u32) -> OutTile + Sync),
) -> Vec<((u32, u32), OutTile)> {
	let band_y0 = f64::from(ty * TILE_SIZE);
	let band_rows = valid_extent(size, tx0, ty).1;
	let band_y1 = band_y0 + f64::from(band_rows);
	let band_edges: Vec<Edge> = edges.iter().filter(|e| e.bottom > band_y0 && e.top < band_y1).copied().collect();
	if band_edges.is_empty() {
		return Vec::new();
	}
	let x_start = tx0 * TILE_SIZE;
	let x_end = ((tx1 + 1) * TILE_SIZE).min(size.0);
	let width = (x_end - x_start) as usize;
	let n_tiles = (tx1 - tx0 + 1) as usize;
	// Per tile of the band: `None` = still uniform with value `uniform[i]`.
	let mut data: Vec<Option<Vec<f32>>> = vec![None; n_tiles];
	let mut uniform: Vec<Option<f32>> = vec![None; n_tiles];
	let mut acc = vec![0.0f32; width + 1];
	let mut diff = vec![0.0f32; width + 1];
	let mut crossings: Vec<(f64, i32)> = Vec::new();
	let weight = 1.0 / SUBSAMPLES as f32;
	for row in 0..band_rows {
		let y = band_y0 + f64::from(row);
		acc.fill(0.0);
		diff.fill(0.0);
		let active: Vec<&Edge> = band_edges.iter().filter(|e| e.bottom > y && e.top < y + 1.0).collect();
		for k in 0..SUBSAMPLES {
			let ys = y + (f64::from(k) + 0.5) / f64::from(SUBSAMPLES);
			crossings.clear();
			for e in &active {
				// Half-open in y so a vertex shared by two edges counts once.
				if e.top <= ys && ys < e.bottom {
					crossings.push((e.x_at(ys), e.dir));
				}
			}
			if crossings.len() < 2 {
				continue;
			}
			crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
			let mut winding = 0;
			let mut span_start = 0.0;
			for &(x, dir) in &crossings {
				let before = winding;
				winding += dir;
				if before == 0 && winding != 0 {
					span_start = x;
				} else if before != 0 && winding == 0 {
					add_span(&mut acc, &mut diff, span_start - f64::from(x_start), x - f64::from(x_start), width, weight);
				}
			}
		}
		// Full pixels were added to `diff`; fold them in.
		let mut run = 0.0f32;
		for (a, d) in acc.iter_mut().zip(diff.iter()) {
			run += *d;
			*a = (*a + run).min(1.0);
		}
		// Scatter the row into the band's tiles, keeping uniform ones buffer-free.
		for (i, tile) in data.iter_mut().enumerate() {
			let x0 = i * TILE_SIZE as usize;
			let x1 = (x0 + TILE_SIZE as usize).min(width);
			let segment = &acc[x0..x1];
			let first = segment[0];
			if tile.is_none() {
				let flat = segment.iter().all(|v| *v == first);
				match uniform[i] {
					None if flat => {
						uniform[i] = Some(first);
						continue;
					}
					Some(u) if flat && u == first => continue,
					_ => {
						// Materialise the rows seen so far.
						let mut values = vec![0.0f32; TILE_PIXELS];
						if let Some(u) = uniform[i] {
							values[..(row * TILE_SIZE) as usize].fill(u);
						}
						*tile = Some(values);
					}
				}
			}
			if let Some(values) = tile {
				let start = (row * TILE_SIZE) as usize;
				values[start..start + segment.len()].copy_from_slice(segment);
			}
		}
	}
	let mut out = Vec::new();
	for (i, tile) in data.into_iter().enumerate() {
		let tx = tx0 + i as u32;
		let mut values = match tile {
			Some(values) => values,
			None => match uniform[i] {
				Some(u) if u > 0.0 => vec![u; TILE_PIXELS],
				_ => continue,
			},
		};
		out.push(((tx, ty), finish(&mut values, tx, ty)));
	}
	out
}

/// Add the span `[a, b)` (row-buffer coordinates) with `weight`: exact
/// partial coverage at both ends, whole pixels through the difference buffer.
fn add_span(acc: &mut [f32], diff: &mut [f32], a: f64, b: f64, width: usize, weight: f32) {
	let (a, b) = (a.max(0.0), b.min(width as f64));
	if b <= a {
		return;
	}
	let (ia, ib) = (a.floor() as usize, b.floor() as usize);
	if ia == ib {
		acc[ia] += (b - a) as f32 * weight;
		return;
	}
	acc[ia] += (ia as f64 + 1.0 - a) as f32 * weight;
	diff[ia + 1] += weight;
	diff[ib] -= weight;
	if ib < width {
		acc[ib] += (b - ib as f64) as f32 * weight;
	}
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
	fn a_big_marquee_stores_solid_tiles_inside() {
		// Review fix: the inside of a large shape costs no buffers.
		let store = store();
		let size = (3000, 3000);
		for shape in [
			SelectionShape::Rect {
				x: 10.5,
				y: 10.5,
				w: 2900.0,
				h: 2900.0,
			},
			SelectionShape::Ellipse {
				x: 0.0,
				y: 0.0,
				w: 3000.0,
				h: 3000.0,
			},
		] {
			let selection = rasterise(&shape, size, BitDepth::U16, true, &store).unwrap();
			assert!(
				matches!(selection.image.slot(0, 5, 5), TileSlot::Solid(_)),
				"{shape:?}: the middle tile is solid"
			);
			let data = selection.image.grid(0).non_empty().filter(|(_, _, s)| matches!(s, TileSlot::Data(_))).count();
			assert!(data < 60, "{shape:?}: only edge tiles hold data ({data})");
		}
	}

	#[test]
	fn a_long_freehand_lasso_is_scan_converted_quickly() {
		// A 20 000-point lasso around a 2 000 px disc: point-in-polygon per
		// pixel would take minutes, the scanline converter well under a second.
		let store = store();
		let points: Vec<(f64, f64)> = (0..20_000)
			.map(|i| {
				let a = std::f64::consts::TAU * f64::from(i) / 20_000.0;
				(1100.0 + 1000.0 * a.cos(), 1100.0 + 1000.0 * a.sin())
			})
			.collect();
		let start = std::time::Instant::now();
		let selection = rasterise(&SelectionShape::Polygon { points }, (2200, 2200), BitDepth::U8, true, &store).unwrap();
		assert!(start.elapsed().as_secs_f64() < 5.0, "{:?}", start.elapsed());
		let exact = PI * 1000.0 * 1000.0;
		let got = area(&selection, &store, (2200, 2200));
		assert!((got - exact).abs() / exact < 0.001, "{got} vs {exact}");
	}

	#[test]
	fn anti_aliased_polygon_edges_are_fractional() {
		let store = store();
		// A triangle with a 45° edge: pixels on the diagonal are half covered.
		let points = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)];
		let selection = rasterise(&SelectionShape::Polygon { points }, (128, 128), BitDepth::U16, true, &store).unwrap();
		let v = value(&selection, &store, 49, 50);
		assert!((v - 0.5).abs() < 0.07, "diagonal pixel {v}");
		assert!((area(&selection, &store, (128, 128)) - 5000.0).abs() < 5.0);
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
