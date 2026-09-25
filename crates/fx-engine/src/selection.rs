//! The marching-ants outline of the pixel selection (M5-T03).
//!
//! Extracts the 50 % contour with marching squares at the **view level L**
//! (the same level the viewport composites from), inside the visible
//! rectangle only, and joins the unit segments across tile edges into closed
//! polylines in document coordinates. The result is an [`Overlay`] the render
//! thread tessellates and draws (M5-T02).
//!
//! Working at level L is what keeps this cheap on a 30 000² document: at fit
//! (level 7) one sample covers a 128 × 128 block, so the outline of the whole
//! canvas is a handful of thousands of samples, not a billion.

use std::collections::HashMap;
use std::sync::Arc;

use fx_core::selection::{Selection, gray_at};
use fx_render::{Overlay, OverlayItem, OverlayStyle};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileSlot, TileStore};

/// A rectangle in document pixels: `(x0, y0, x1, y1)`, exclusive at the far
/// edge.
pub type DocRect = (i64, i64, i64, i64);

/// The selection's outline as an overlay, or an empty overlay when nothing is
/// selected (or nothing of it is visible).
pub fn contour(selection: &Selection, store: &TileStore, level: usize, visible: DocRect) -> Result<Overlay, fx_tiles::TileError> {
	let Some(bbox) = document_bounds(selection) else {
		return Ok(Overlay::default());
	};
	// The selection's bounding box, intersected with the visible rect and the
	// canvas. Grown by one level pixel so a selection edge exactly on the box
	// is still seen as a boundary.
	let width = i64::from(selection.image.width());
	let height = i64::from(selection.image.height());
	let x0 = bbox.0.max(visible.0).max(0);
	let y0 = bbox.1.max(visible.1).max(0);
	let x1 = bbox.2.min(visible.2).min(width);
	let y1 = bbox.3.min(visible.3).min(height);
	if x1 <= x0 || y1 <= y0 {
		return Ok(Overlay::default());
	}
	let step = 1i64 << level;
	let half = if level > 0 { step / 2 } else { 0 };
	// Level-L pixel range, grown by one sample on each side (bounded by the canvas).
	let i0 = (x0 >> level).max(0) - 1;
	let j0 = (y0 >> level).max(0) - 1;
	let i1 = ((x1 + step - 1) >> level).min(div_ceil(width, step)) + 1;
	let j1 = ((y1 + step - 1) >> level).min(div_ceil(height, step)) + 1;
	let (w, h) = ((i1 - i0) as usize, (j1 - j0) as usize);
	if w < 2 || h < 2 {
		return Ok(Overlay::default());
	}

	// Coverage at each level-L sample (the block's centre; nearest sample,
	// which is what the ants need — they are a one-pixel indicator, not a
	// measurable shape).
	let format = selection.image.format();
	let mut reader = Reader::new(selection, store, format);
	let mut inside = vec![false; w * h];
	for j in 0..h {
		let doc_y = ((j0 + j as i64) << level) + half;
		for i in 0..w {
			let doc_x = ((i0 + i as i64) << level) + half;
			inside[j * w + i] = reader.at(doc_x, doc_y) >= 0.5;
		}
	}

	// Unit boundary segments between an inside and an outside sample.
	let at = |i: i64, j: i64| -> bool {
		if i < 0 || j < 0 || i >= w as i64 || j >= h as i64 {
			return false;
		}
		inside[j as usize * w + i as usize]
	};
	let mut segments: Vec<((i64, i64), (i64, i64))> = Vec::new();
	for j in 0..h as i64 {
		for i in 0..w as i64 {
			if !at(i, j) {
				continue;
			}
			if !at(i, j - 1) {
				segments.push(((i, j), (i + 1, j)));
			}
			if !at(i, j + 1) {
				segments.push(((i, j + 1), (i + 1, j + 1)));
			}
			if !at(i - 1, j) {
				segments.push(((i, j), (i, j + 1)));
			}
			if !at(i + 1, j) {
				segments.push(((i + 1, j), (i + 1, j + 1)));
			}
		}
	}
	if segments.is_empty() {
		return Ok(Overlay::default());
	}

	let chains = join(segments);
	let mut items = Vec::new();
	for (points, closed) in chains {
		if points.len() < 2 {
			continue;
		}
		let document: Vec<(f64, f64)> = points.iter().map(|&(i, j)| (((i0 + i) << level) as f64, ((j0 + j) << level) as f64)).collect();
		items.push(OverlayItem::Polyline {
			points: document,
			closed,
			style: OverlayStyle::Ants,
		});
	}
	Ok(Overlay { items })
}

/// The document rectangle the selection covers, from its non-empty tiles
/// (tile granularity, so it never scans pixels).
fn document_bounds(selection: &Selection) -> Option<DocRect> {
	let (ox, oy) = (i64::from(selection.offset.0), i64::from(selection.offset.1));
	let mut bbox: Option<DocRect> = None;
	for (tx, ty) in selection.tiles() {
		let x0 = i64::from(tx) * i64::from(TILE_SIZE) + ox;
		let y0 = i64::from(ty) * i64::from(TILE_SIZE) + oy;
		let rect = (x0, y0, x0 + i64::from(TILE_SIZE), y0 + i64::from(TILE_SIZE));
		bbox = Some(match bbox {
			None => rect,
			Some((ax0, ay0, ax1, ay1)) => (ax0.min(rect.0), ay0.min(rect.1), ax1.max(rect.2), ay1.max(rect.3)),
		});
	}
	bbox
}

/// Join unit segments into polylines. Each lattice point keeps the list of
/// its neighbours; a walk consumes edges until it returns to the start (a
/// closed loop) or runs out (an open chain, at a clipped edge). Consecutive
/// collinear steps are collapsed into one segment.
fn join(segments: Vec<((i64, i64), (i64, i64))>) -> Vec<(Vec<(i64, i64)>, bool)> {
	let mut neighbours: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
	for (a, b) in segments {
		neighbours.entry(a).or_default().push(b);
		neighbours.entry(b).or_default().push(a);
	}
	let mut chains = Vec::new();
	// Prefer open chains' starts so a shape clipped by the viewport does not
	// hide the closed loops.
	// Collected up front: the walk below mutates `neighbours`.
	let mut order: Vec<(i64, i64)> = neighbours.iter().filter(|(_, list)| list.len() % 2 == 1).map(|(point, _)| *point).collect();
	order.extend(neighbours.keys().copied());
	for start in order {
		while neighbours.get(&start).is_some_and(|list| !list.is_empty()) {
			let mut points = vec![start];
			let mut current = start;
			let closed;
			loop {
				let Some(next) = neighbours.get_mut(&current).and_then(|list| list.pop()) else {
					closed = false;
					break;
				};
				if let Some(list) = neighbours.get_mut(&next)
					&& let Some(position) = list.iter().position(|p| *p == current)
				{
					list.swap_remove(position);
				}
				if next == start {
					closed = true;
					break;
				}
				points.push(next);
				current = next;
			}
			let simplified = simplify(points);
			if simplified.len() >= 2 {
				chains.push((simplified, closed));
			}
		}
	}
	chains
}

/// `a / b` rounded up (the operands are non-negative).
fn div_ceil(a: i64, b: i64) -> i64 {
	(a + b - 1) / b
}

/// Drop points that continue the previous direction (a straight run becomes
/// two points).
fn simplify(points: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
	let mut out: Vec<(i64, i64)> = Vec::with_capacity(points.len());
	for point in points {
		if out.len() >= 2 {
			let a = out[out.len() - 2];
			let b = out[out.len() - 1];
			let same_direction = (b.0 - a.0, b.1 - a.1) == (point.0 - b.0, point.1 - b.1);
			if same_direction {
				out.pop();
			}
		}
		out.push(point);
	}
	out
}

/// One tile's coverage, or `None` where the selection has no tile.
type CachedTile = Option<Arc<TileBuffer>>;

/// Reads coverage from the selection, keeping the last tile so a row-major
/// scan does not look a tile up per sample.
struct Reader<'a> {
	selection: &'a Selection,
	store: &'a TileStore,
	format: PixelFormat,
	cached: Option<((u32, u32), CachedTile)>,
}

impl<'a> Reader<'a> {
	fn new(selection: &'a Selection, store: &'a TileStore, format: PixelFormat) -> Self {
		Self {
			selection,
			store,
			format,
			cached: None,
		}
	}

	fn at(&mut self, doc_x: i64, doc_y: i64) -> f32 {
		let (ix, iy) = (doc_x - i64::from(self.selection.offset.0), doc_y - i64::from(self.selection.offset.1));
		if ix < 0 || iy < 0 || ix >= i64::from(self.selection.image.width()) || iy >= i64::from(self.selection.image.height()) {
			return 0.0;
		}
		let key = (ix as u32 / TILE_SIZE, iy as u32 / TILE_SIZE);
		if self.cached.as_ref().is_none_or(|(cached, _)| *cached != key) {
			let tile = match self.selection.image.slot(0, key.0, key.1) {
				TileSlot::Empty => None,
				TileSlot::Solid(value) => Some(Arc::new(TileBuffer::filled(self.format, *value))),
				TileSlot::Data(handle) => self.store.get(handle).ok(),
			};
			self.cached = Some((key, tile));
		}
		match &self.cached.as_ref().expect("just stored").1 {
			Some(buffer) => gray_at(buffer, self.format, ix as u32 % TILE_SIZE, iy as u32 % TILE_SIZE),
			None => 0.0,
		}
	}
}

#[cfg(test)]
mod tests {
	use fx_core::BitDepth;
	use fx_core::selection::set_gray;
	use fx_tiles::{PixelFormat, TileStoreConfig};

	use super::*;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-engine-selection-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// A disc of `radius` around `(cx, cy)`.
	fn disc(store: &TileStore, size: (u32, u32), centre: (f64, f64), radius: f64) -> Selection {
		let mut selection = Selection::empty(size, BitDepth::U8);
		for ty in 0..size.1.div_ceil(TILE_SIZE) {
			for tx in 0..size.0.div_ceil(TILE_SIZE) {
				let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
				let mut any = false;
				for py in 0..TILE_SIZE {
					for px in 0..TILE_SIZE {
						let (x, y) = (tx * TILE_SIZE + px, ty * TILE_SIZE + py);
						let (dx, dy) = (x as f64 + 0.5 - centre.0, y as f64 + 0.5 - centre.1);
						if dx * dx + dy * dy <= radius * radius {
							set_gray(&mut buffer, PixelFormat::Gray8, px, py, 1.0);
							any = true;
						}
					}
				}
				if any {
					selection.image.put_buffer(store, tx, ty, buffer);
				}
			}
		}
		selection
	}

	#[test]
	fn a_disc_gives_one_closed_polyline() {
		let store = store();
		let size = (600, 600);
		let selection = disc(&store, size, (300.0, 300.0), 100.0);
		let overlay = contour(&selection, &store, 0, (0, 0, 600, 600)).unwrap();
		assert_eq!(overlay.items.len(), 1, "{:?}", overlay.items);
		match &overlay.items[0] {
			OverlayItem::Polyline { points, closed, style } => {
				assert!(*closed, "the disc's outline is a closed loop");
				assert_eq!(*style, OverlayStyle::Ants);
				assert!(points.len() >= 8, "a circle is approximated by many points");
				// The outline stays on the disc (within a pixel).
				for (x, y) in points {
					let d = ((x - 300.0).powi(2) + (y - 300.0).powi(2)).sqrt();
					assert!((d - 100.0).abs() <= 2.0, "point ({x}, {y}) at distance {d}");
				}
			}
			other => panic!("expected a polyline, got {other:?}"),
		}
	}

	#[test]
	fn a_full_canvas_selection_has_one_loop_and_an_empty_one_none() {
		let store = store();
		let size = (512, 512);
		let mut full = Selection::empty(size, BitDepth::U8);
		full.image.set_slot(0, 0, TileSlot::Solid(fx_tiles::PixelValue::gray16(u16::MAX)));
		let overlay = contour(&full, &store, 0, (0, 0, 512, 512)).unwrap();
		assert_eq!(overlay.items.len(), 1);
		let empty = Selection::empty(size, BitDepth::U8);
		assert!(contour(&empty, &store, 0, (0, 0, 512, 512)).unwrap().items.is_empty());
	}

	#[test]
	fn a_higher_level_costs_fewer_samples_but_stays_a_loop() {
		let store = store();
		let size = (2048, 2048);
		let selection = disc(&store, size, (1024.0, 1024.0), 500.0);
		let fine = contour(&selection, &store, 0, (0, 0, 2048, 2048)).unwrap();
		let coarse = contour(&selection, &store, 3, (0, 0, 2048, 2048)).unwrap();
		let count = |overlay: &Overlay| match &overlay.items[0] {
			OverlayItem::Polyline { points, .. } => points.len(),
			_ => 0,
		};
		assert_eq!(fine.items.len(), 1);
		assert_eq!(coarse.items.len(), 1);
		assert!(count(&coarse) < count(&fine), "the coarse contour is simpler");
	}
}
