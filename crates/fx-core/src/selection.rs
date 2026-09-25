//! The pixel selection (M5-T03, D-040).
//!
//! A selection is a grey coverage image — `Gray8`/`Gray16` like a layer mask —
//! with an integer offset like a pixel layer: 0 = not selected, maximum =
//! selected. Moving the outline therefore never rewrites tiles.
//!
//! The algorithms that build and reshape a selection (rasterising shapes,
//! feather, expand/contract) need `fx-ops`, so they are reached through
//! [`PixelOps`](crate::ops::PixelOps) (recipe R1a). What lives here is the
//! model and the pure per-tile arithmetic (combining two selections).

use std::collections::HashMap;
use std::sync::Arc;

use fx_tiles::{PixelFormat, PixelValue, TILE_SIZE, TileBuffer, TileError, TileSlot, TileStore, TiledImage};
use serde::{Deserialize, Serialize};

use crate::color::BitDepth;

/// A pixel selection: a grey coverage image with an integer offset.
#[derive(Clone, Debug)]
pub struct Selection {
	pub image: TiledImage,
	/// Document pixel the image's (0, 0) sits at.
	pub offset: (i32, i32),
}

impl Selection {
	/// An empty selection the size of `size`, at offset (0, 0).
	pub fn empty(size: (u32, u32), depth: BitDepth) -> Self {
		Self {
			image: TiledImage::new(size.0, size.1, depth.gray_format()),
			offset: (0, 0),
		}
	}

	/// A selection covering the whole canvas (Select All), built from solid
	/// tiles: it costs no pixel memory however large the document is.
	pub fn full(size: (u32, u32), depth: BitDepth) -> Self {
		let mut selection = Self::empty(size, depth);
		for ty in 0..size.1.div_ceil(TILE_SIZE) {
			for tx in 0..size.0.div_ceil(TILE_SIZE) {
				selection.image.set_slot(tx, ty, TileSlot::Solid(PixelValue::gray16(u16::MAX)));
			}
		}
		selection
	}

	/// Whether nothing is selected (every tile empty).
	pub fn is_empty(&self) -> bool {
		self.image.grid(0).non_empty().next().is_none()
	}

	/// The non-empty tiles of the coverage image, in row-major order.
	pub fn tiles(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
		self.image.grid(0).non_empty().map(|(tx, ty, _)| (tx, ty))
	}
}

/// A shape to rasterise into a selection (M5-T03). Coordinates are document
/// pixels and may be fractional (fractional edges are anti-aliased).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum SelectionShape {
	/// `x, y` = top-left corner, `w, h` = size (may be negative: normalised).
	Rect { x: f64, y: f64, w: f64, h: f64 },
	/// The ellipse inscribed in the rectangle.
	Ellipse { x: f64, y: f64, w: f64, h: f64 },
	/// A closed polygon; the non-zero winding rule decides inside.
	Polygon { points: Vec<(f64, f64)> },
	/// One full row of the canvas.
	RowPixel { y: f64 },
	/// One full column of the canvas.
	ColumnPixel { x: f64 },
}

impl SelectionShape {
	/// The shape's bounding box as `(x0, y0, x1, y1)`, normalised so
	/// `x0 <= x1`. `None` for a degenerate shape (an empty polygon).
	pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
		let norm = |x: f64, y: f64, w: f64, h: f64| Some((x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)));
		match self {
			SelectionShape::Rect { x, y, w, h } | SelectionShape::Ellipse { x, y, w, h } => norm(*x, *y, *w, *h),
			SelectionShape::Polygon { points } => {
				let first = points.first()?;
				let mut b = (*first, *first);
				for p in points {
					b.0 = (b.0.0.min(p.0), b.0.1.min(p.1));
					b.1 = (b.1.0.max(p.0), b.1.1.max(p.1));
				}
				Some((b.0.0, b.0.1, b.1.0, b.1.1))
			}
			SelectionShape::RowPixel { y } => Some((0.0, *y, f64::MAX, y + 1.0)),
			SelectionShape::ColumnPixel { x } => Some((*x, 0.0, x + 1.0, f64::MAX)),
		}
	}
}

/// How a new shape combines with the current selection (M5-T03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
	Replace,
	Add,
	Subtract,
	Intersect,
}

/// A reshape operation on the current selection (M5-T03). Distances are in
/// pixels.
///
/// Adjacently tagged (`{"kind": "expand", "px": 2.0}`): a distance is a bare
/// number, which an internally tagged enum cannot hold. The tag is `kind`,
/// not `op`, because `Command` is already tagged `op`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "px", rename_all = "snake_case")]
pub enum SelectModify {
	Expand(f64),
	Contract(f64),
	Border(f64),
	Smooth(f64),
	Feather(f64),
}

/// The grey format of a selection at `depth`.
pub fn gray_format(depth: BitDepth) -> PixelFormat {
	depth.gray_format()
}

/// One grey sample as `0..=1`.
pub fn gray_at(buffer: &TileBuffer, format: PixelFormat, x: u32, y: u32) -> f32 {
	match format {
		PixelFormat::Gray16 => buffer.as_u16()[(y * TILE_SIZE + x) as usize] as f32 / 65535.0,
		_ => buffer.bytes()[(y * TILE_SIZE + x) as usize] as f32 / 255.0,
	}
}

/// Write one grey sample (`0..=1`) into a tile.
pub fn set_gray(buffer: &mut TileBuffer, format: PixelFormat, x: u32, y: u32, value: f32) {
	let value = value.clamp(0.0, 1.0);
	match format {
		PixelFormat::Gray16 => buffer.as_u16_mut()[(y * TILE_SIZE + x) as usize] = (value * 65535.0).round() as u16,
		_ => buffer.bytes_mut()[(y * TILE_SIZE + x) as usize] = (value * 255.0).round() as u8,
	}
}

/// Combine `new` (a freshly rasterised shape) with `old` under `mode`
/// (M5-T03). Only the tiles the shape touches are built; the rest follow from
/// the mode without reading pixels (Replace/Intersect → empty, Add/Subtract →
/// the old selection).
///
/// Returns `None` when the result selects nothing.
pub fn combine(size: (u32, u32), old: Option<&Selection>, new: &Selection, mode: SelectMode, store: &TileStore) -> Result<Option<Selection>, TileError> {
	let format = new.image.format();
	let mut result = Selection::empty(size, if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 });

	let mut tiles: Vec<(u32, u32)> = new.tiles().collect();
	if matches!(mode, SelectMode::Add | SelectMode::Subtract)
		&& let Some(old) = old
	{
		for tile in old.tiles() {
			if !tiles.contains(&tile) {
				tiles.push(tile);
			}
		}
	}

	let mut old_reader = old.map(|old| Coverage::new(old, store, old.image.format()));
	let mut new_reader = Coverage::new(new, store, format);
	let mut any = false;
	for (tx, ty) in tiles {
		let mut buffer = TileBuffer::zeroed(format);
		let base_x = i64::from(tx) * i64::from(TILE_SIZE);
		let base_y = i64::from(ty) * i64::from(TILE_SIZE);
		for py in 0..TILE_SIZE {
			for px in 0..TILE_SIZE {
				let (doc_x, doc_y) = (base_x + i64::from(px), base_y + i64::from(py));
				if doc_x >= i64::from(size.0) || doc_y >= i64::from(size.1) {
					continue;
				}
				let b = new_reader.at(doc_x, doc_y);
				let a = old_reader.as_mut().map_or(0.0, |reader| reader.at(doc_x, doc_y));
				let value = match mode {
					SelectMode::Replace => b,
					// Not `a + b`: a soft edge would count twice and clamp.
					SelectMode::Add => a.max(b),
					// Not `a - b`: it goes negative outside the shape and wraps.
					SelectMode::Subtract => a.min(1.0 - b),
					// Not `a * b`: it darkens a soft edge on both sides.
					SelectMode::Intersect => a.min(b),
				};
				if value > 0.0 {
					any = true;
				}
				set_gray(&mut buffer, format, px, py, value);
			}
		}
		result.image.put_buffer(store, tx, ty, buffer);
	}
	Ok(any.then_some(result))
}

/// The inverse of `source` over `size` (Select ▸ Inverse, M5-T03): coverage
/// `1 - source`, as a canvas-sized selection at offset (0, 0).
///
/// Whole tiles that line up with a source tile are inverted without reading
/// pixels (Empty → solid maximum, Solid(v) → solid maximum − v), so a
/// document-wide inverse costs one operation per tile, not per pixel. Tiles a
/// shifted selection cuts through, and tiles holding real data, go pixel by
/// pixel.
pub fn invert(size: (u32, u32), source: &Selection, store: &TileStore) -> Result<Selection, TileError> {
	let format = source.image.format();
	let depth = if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 };
	let mut result = Selection::empty(size, depth);
	let (ox, oy) = (i64::from(source.offset.0), i64::from(source.offset.1));
	let tile = i64::from(TILE_SIZE);
	// Only a tile-aligned selection maps whole tiles onto whole tiles.
	let aligned = source.offset.0.rem_euclid(TILE_SIZE as i32) == 0 && source.offset.1.rem_euclid(TILE_SIZE as i32) == 0;
	let mut reader = Coverage::new(source, store, format);
	for ty in 0..size.1.div_ceil(TILE_SIZE) {
		for tx in 0..size.0.div_ceil(TILE_SIZE) {
			let (bx, by) = (i64::from(tx) * tile, i64::from(ty) * tile);
			if aligned && (bx - ox) % tile == 0 && (by - oy) % tile == 0 {
				let inside = bx >= ox && by >= oy && bx + tile <= ox + i64::from(source.image.width()) && by + tile <= oy + i64::from(source.image.height());
				if inside {
					let slot = source.image.slot(0, ((bx - ox) / tile) as u32, ((by - oy) / tile) as u32);
					match slot {
						TileSlot::Empty => {
							result.image.set_slot(tx, ty, TileSlot::Solid(PixelValue::gray16(u16::MAX)));
							continue;
						}
						TileSlot::Solid(value) => {
							let inverted = u16::MAX - value.0[0];
							let slot = if inverted == 0 {
								TileSlot::Empty
							} else {
								TileSlot::Solid(PixelValue::gray16(inverted))
							};
							result.image.set_slot(tx, ty, slot);
							continue;
						}
						TileSlot::Data(_) => {}
					}
				}
			}
			let mut buffer = TileBuffer::zeroed(format);
			for py in 0..TILE_SIZE {
				for px in 0..TILE_SIZE {
					let (doc_x, doc_y) = (bx + i64::from(px), by + i64::from(py));
					if doc_x >= i64::from(size.0) || doc_y >= i64::from(size.1) {
						// Outside the canvas: nothing is selected there.
						continue;
					}
					set_gray(&mut buffer, format, px, py, 1.0 - reader.at(doc_x, doc_y));
				}
			}
			result.image.put_buffer(store, tx, ty, buffer);
		}
	}
	Ok(result)
}

/// Reads a selection's coverage with the tiles it touches cached.
struct Coverage<'a> {
	selection: &'a Selection,
	store: &'a TileStore,
	format: PixelFormat,
	cache: HashMap<(u32, u32), Option<Arc<TileBuffer>>>,
}

impl<'a> Coverage<'a> {
	fn new(selection: &'a Selection, store: &'a TileStore, format: PixelFormat) -> Self {
		Self {
			selection,
			store,
			format,
			cache: HashMap::new(),
		}
	}

	fn at(&mut self, doc_x: i64, doc_y: i64) -> f32 {
		let (ix, iy) = (doc_x - i64::from(self.selection.offset.0), doc_y - i64::from(self.selection.offset.1));
		if ix < 0 || iy < 0 || ix >= i64::from(self.selection.image.width()) || iy >= i64::from(self.selection.image.height()) {
			return 0.0;
		}
		let (tx, ty) = (ix as u32 / TILE_SIZE, iy as u32 / TILE_SIZE);
		if !self.cache.contains_key(&(tx, ty)) {
			let tile = match self.selection.image.slot(0, tx, ty) {
				TileSlot::Empty => None,
				TileSlot::Solid(value) => Some(Arc::new(TileBuffer::filled(self.format, *value))),
				TileSlot::Data(handle) => self.store.get(handle).ok(),
			};
			self.cache.insert((tx, ty), tile);
		}
		match &self.cache[&(tx, ty)] {
			Some(buffer) => gray_at(buffer, self.format, ix as u32 % TILE_SIZE, iy as u32 % TILE_SIZE),
			None => 0.0,
		}
	}
}

#[cfg(test)]
mod tests {
	use fx_tiles::{PixelValue, TileStoreConfig};

	use super::*;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-core-selection-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// A selection that covers `rect` (document pixels), at offset (0, 0).
	fn rect_selection(store: &TileStore, size: (u32, u32), rect: (u32, u32, u32, u32)) -> Selection {
		let mut selection = Selection::empty(size, BitDepth::U8);
		let (x0, y0, x1, y1) = rect;
		for ty in y0 / TILE_SIZE..=(y1 - 1) / TILE_SIZE {
			for tx in x0 / TILE_SIZE..=(x1 - 1) / TILE_SIZE {
				let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
				for py in 0..TILE_SIZE {
					for px in 0..TILE_SIZE {
						let x = tx * TILE_SIZE + px;
						let y = ty * TILE_SIZE + py;
						if x >= x0 && x < x1 && y >= y0 && y < y1 {
							set_gray(&mut buffer, PixelFormat::Gray8, px, py, 1.0);
						}
					}
				}
				selection.image.put_buffer(store, tx, ty, buffer);
			}
		}
		selection
	}

	fn at(selection: &Selection, store: &TileStore, x: i64, y: i64) -> f32 {
		let mut reader = Coverage::new(selection, store, selection.image.format());
		reader.at(x, y)
	}

	#[test]
	fn modes_combine_two_overlapping_selections() {
		let store = store();
		let size = (600, 300);
		let old = rect_selection(&store, size, (0, 0, 200, 200));
		let new = rect_selection(&store, size, (100, 0, 300, 200));
		let cases = [
			// Replace drops the old selection entirely: the far sample is in
			// the new shape only.
			(SelectMode::Replace, [(50, 50, 0.0), (150, 50, 1.0), (250, 50, 1.0)]),
			(SelectMode::Add, [(50, 50, 1.0), (150, 50, 1.0), (250, 50, 1.0)]),
			(SelectMode::Subtract, [(50, 50, 1.0), (150, 50, 0.0), (250, 50, 0.0)]),
			(SelectMode::Intersect, [(50, 50, 0.0), (150, 50, 1.0), (250, 50, 0.0)]),
		];
		for (mode, samples) in cases {
			let combined = combine(size, Some(&old), &new, mode, &store).unwrap().expect("something is selected");
			for (x, y, expected) in samples {
				assert!((at(&combined, &store, x, y) - expected).abs() < 0.01, "{mode:?} at ({x}, {y})");
			}
		}
	}

	#[test]
	fn subtracting_everything_leaves_no_selection() {
		let store = store();
		let old = rect_selection(&store, (300, 300), (0, 0, 300, 300));
		let new = rect_selection(&store, (300, 300), (0, 0, 300, 300));
		assert!(combine((300, 300), Some(&old), &new, SelectMode::Subtract, &store).unwrap().is_none());
	}

	#[test]
	fn an_offset_selection_is_read_at_the_right_place() {
		let store = store();
		let mut selection = Selection::empty((600, 300), BitDepth::U8);
		selection.offset = (100, 50);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
		set_gray(&mut buffer, PixelFormat::Gray8, 0, 0, 1.0);
		selection.image.put_buffer(&store, 0, 0, buffer);
		// Image pixel (0, 0) is document pixel (100, 50).
		assert_eq!(at(&selection, &store, 100, 50), 1.0);
		assert_eq!(at(&selection, &store, 0, 0), 0.0);
	}

	#[test]
	fn a_solid_grey_tile_round_trips_through_put_buffer() {
		let store = store();
		let mut deep = Selection::empty((300, 300), BitDepth::U16);
		deep.image
			.put_buffer(&store, 0, 0, TileBuffer::filled(PixelFormat::Gray16, PixelValue::gray16(32768)));
		assert!((at(&deep, &store, 10, 10) - 32768.0 / 65535.0).abs() < 1e-6);
		// An 8-bit selection quantises: 32768 lands on 127.
		let mut shallow = Selection::empty((300, 300), BitDepth::U8);
		shallow
			.image
			.put_buffer(&store, 0, 0, TileBuffer::filled(PixelFormat::Gray8, PixelValue::gray16(32768)));
		assert_eq!(at(&shallow, &store, 10, 10), 127.0 / 255.0);
	}
}
