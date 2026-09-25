//! A tiled image with a mip pyramid.
//!
//! Level 0 is the full-resolution, authoritative content. Level `n` is the
//! image downscaled by `2^n`; its tiles are *derived*: computed on demand from
//! the level below (see [`crate::downsample_2x2`]) and marked dirty when the
//! content under them changes. The last level fits in a single tile.
//!
//! For a 30 000 × 30 000 layer: 118 × 118 tiles at level 0, 8 levels in total.
//!
//! Cloning a `TiledImage` clones tile *handles*, not pixels: it is the
//! snapshot mechanism used by undo and by background rendering.

use crate::format::{PixelFormat, PixelValue, TILE_SIZE};
use crate::store::{TileBuffer, TileClass, TileHandle, TileStore};

/// Content of one tile position.
#[derive(Clone, Debug, Default)]
pub enum TileSlot {
	/// Fully transparent (RGBA) or all zero (gray). Costs no memory.
	#[default]
	Empty,
	/// Every pixel has this value. Costs no tile memory.
	Solid(PixelValue),
	/// Real pixel data.
	Data(TileHandle),
}

impl TileSlot {
	pub fn is_empty(&self) -> bool {
		matches!(self, TileSlot::Empty)
	}

	/// Same content without looking at pixels: both empty, same solid value,
	/// or the very same stored tile. `false` does not prove the contents differ.
	pub fn same_as(&self, other: &TileSlot) -> bool {
		match (self, other) {
			(TileSlot::Empty, TileSlot::Empty) => true,
			(TileSlot::Solid(a), TileSlot::Solid(b)) => a == b,
			(TileSlot::Data(a), TileSlot::Data(b)) => a.same_tile(b),
			_ => false,
		}
	}
}

/// The tiles of one mip level, row-major.
#[derive(Clone, Debug)]
pub struct TileGrid {
	cols: u32,
	rows: u32,
	slots: Vec<TileSlot>,
	/// Only used for derived levels (>= 1): tile must be recomputed.
	dirty: Vec<bool>,
}

impl TileGrid {
	fn new(cols: u32, rows: u32, dirty: bool) -> Self {
		let n = (cols * rows) as usize;
		Self {
			cols,
			rows,
			slots: vec![TileSlot::Empty; n],
			dirty: vec![dirty; n],
		}
	}

	pub fn cols(&self) -> u32 {
		self.cols
	}

	pub fn rows(&self) -> u32 {
		self.rows
	}

	fn index(&self, tx: u32, ty: u32) -> usize {
		assert!(tx < self.cols && ty < self.rows, "tile ({tx},{ty}) outside {}x{} grid", self.cols, self.rows);
		(ty * self.cols + tx) as usize
	}

	pub fn slot(&self, tx: u32, ty: u32) -> &TileSlot {
		&self.slots[self.index(tx, ty)]
	}

	/// Iterate `(tx, ty, slot)` over non-empty slots.
	pub fn non_empty(&self) -> impl Iterator<Item = (u32, u32, &TileSlot)> {
		self.slots
			.iter()
			.enumerate()
			.filter(|(_, s)| !s.is_empty())
			.map(|(i, s)| (i as u32 % self.cols, i as u32 / self.cols, s))
	}
}

#[derive(Clone, Debug)]
pub struct TiledImage {
	width: u32,
	height: u32,
	format: PixelFormat,
	levels: Vec<TileGrid>,
	/// True for an image whose level 0 is derived too — a shape layer's tile
	/// cache (M6-T06), rasterised from its geometry. Every tile starts dirty,
	/// [`set_derived_slot`](Self::set_derived_slot) accepts level 0 and
	/// [`is_dirty`](Self::is_dirty) reports level 0 like any other level.
	derived: bool,
}

/// Number of tiles needed to cover `pixels`.
fn tiles_for(pixels: u32) -> u32 {
	pixels.div_ceil(TILE_SIZE).max(1)
}

impl TiledImage {
	/// A fully empty image. Allocates no tile memory, whatever the size.
	pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
		Self::with_dirty(width, height, format, false)
	}

	/// An image with no authoritative level 0: every tile of every level is
	/// drawn from something outside the document (a shape's geometry, M6-T06)
	/// and starts dirty. Mips are never derived from such an image — each level
	/// is rendered by itself, so every zoom is sharp.
	pub fn derived(width: u32, height: u32, format: PixelFormat) -> Self {
		Self::with_dirty(width, height, format, true)
	}

	fn with_dirty(width: u32, height: u32, format: PixelFormat, dirty: bool) -> Self {
		assert!(width > 0 && height > 0, "empty image");
		let mut levels = Vec::new();
		let (mut w, mut h) = (width, height);
		loop {
			// Level 0 starts clean normally (an all-empty level 0 means all-empty
			// mips, so they are clean too); a derived image has nothing clean.
			levels.push(TileGrid::new(tiles_for(w), tiles_for(h), dirty));
			if w <= TILE_SIZE && h <= TILE_SIZE {
				break;
			}
			w = w.div_ceil(2);
			h = h.div_ceil(2);
		}
		Self {
			width,
			height,
			format,
			levels,
			derived: dirty,
		}
	}

	/// Whether level 0 is derived geometry rather than authoritative pixels.
	pub fn is_derived(&self) -> bool {
		self.derived
	}

	/// Mark every tile of every level dirty: the geometry behind a derived
	/// image changed, so all of it must be drawn again (M6-T06).
	pub fn mark_all_dirty(&mut self) {
		for grid in &mut self.levels {
			grid.dirty.fill(true);
		}
	}

	/// Mark the tiles of every level that touch the document rectangle
	/// `[x0, y0, x1, y1]` dirty, whatever is outside it staying as it is: an
	/// edited shape is redrawn where it is, not over a document that can be
	/// 30 000² (M6-T06). The rectangle is clamped to the image.
	pub fn mark_rect_dirty(&mut self, rect: [f64; 4]) {
		let (w, h) = (f64::from(self.width), f64::from(self.height));
		let (x0, y0) = (rect[0].clamp(0.0, w), rect[1].clamp(0.0, h));
		let (x1, y1) = (rect[2].clamp(0.0, w), rect[3].clamp(0.0, h));
		if !(x0 < x1 && y0 < y1) {
			return;
		}
		// Last pixel the rectangle covers, as whole pixels.
		let last = |v: f64, limit: u32| (v.ceil() as u32 - 1).min(limit - 1);
		for level in 0..self.levels.len() {
			let span = TILE_SIZE << level;
			let grid = &mut self.levels[level];
			let (tx0, ty0) = (x0 as u32 / span, y0 as u32 / span);
			let (tx1, ty1) = (last(x1, self.width) / span, last(y1, self.height) / span);
			for ty in ty0..=ty1.min(grid.rows - 1) {
				for tx in tx0..=tx1.min(grid.cols - 1) {
					grid.dirty[(ty * grid.cols + tx) as usize] = true;
				}
			}
		}
	}

	pub fn width(&self) -> u32 {
		self.width
	}

	pub fn height(&self) -> u32 {
		self.height
	}

	pub fn format(&self) -> PixelFormat {
		self.format
	}

	pub fn level_count(&self) -> usize {
		self.levels.len()
	}

	/// Pixel size of a mip level (rounded up).
	pub fn level_size(&self, level: usize) -> (u32, u32) {
		let div = 1u32 << level;
		(self.width.div_ceil(div), self.height.div_ceil(div))
	}

	pub fn grid(&self, level: usize) -> &TileGrid {
		&self.levels[level]
	}

	pub fn slot(&self, level: usize, tx: u32, ty: u32) -> &TileSlot {
		self.levels[level].slot(tx, ty)
	}

	/// Replace a level-0 tile and mark every mip tile above it dirty.
	pub fn set_slot(&mut self, tx: u32, ty: u32, slot: TileSlot) {
		let grid = &mut self.levels[0];
		let i = grid.index(tx, ty);
		if grid.slots[i].same_as(&slot) {
			return;
		}
		grid.slots[i] = slot;
		self.mark_ancestors_dirty(tx, ty);
	}

	/// Convenience used by every writer: collapse uniform buffers to
	/// `Empty`/`Solid`, otherwise insert into the store as authoritative.
	pub fn put_buffer(&mut self, store: &TileStore, tx: u32, ty: u32, buffer: TileBuffer) {
		assert_eq!(buffer.format(), self.format, "tile format does not match image format");
		let slot = match buffer.uniform_value() {
			Some(v) if v.is_transparent(self.format) || (!self.format.has_alpha() && v.0[0] == 0) => TileSlot::Empty,
			Some(v) => TileSlot::Solid(v),
			None => TileSlot::Data(store.insert(buffer, TileClass::Authoritative)),
		};
		self.set_slot(tx, ty, slot);
	}

	/// Store a recomputed derived tile and clear its dirty flag. Level 0 is
	/// authoritative for an ordinary image (use [`set_slot`](Self::set_slot));
	/// for a [`derived`](Self::derived) image it is just another derived level.
	pub fn set_derived_slot(&mut self, level: usize, tx: u32, ty: u32, slot: TileSlot) {
		assert!(level >= 1 || self.derived, "level 0 is authoritative, use set_slot");
		let grid = &mut self.levels[level];
		let i = grid.index(tx, ty);
		grid.slots[i] = slot;
		grid.dirty[i] = false;
	}

	/// True if a tile must be recomputed before use. Never true at level 0 of
	/// an authoritative image (level 0 *is* the truth there).
	pub fn is_dirty(&self, level: usize, tx: u32, ty: u32) -> bool {
		let grid = &self.levels[level];
		grid.dirty[grid.index(tx, ty)]
	}

	/// Coordinates of dirty tiles at `level`.
	pub fn dirty_tiles(&self, level: usize) -> impl Iterator<Item = (u32, u32)> + '_ {
		let grid = &self.levels[level];
		grid.dirty
			.iter()
			.enumerate()
			.filter(|(_, d)| **d)
			.map(|(i, _)| (i as u32 % grid.cols, i as u32 / grid.cols))
	}

	fn mark_ancestors_dirty(&mut self, mut tx: u32, mut ty: u32) {
		for level in 1..self.levels.len() {
			tx /= 2;
			ty /= 2;
			let grid = &mut self.levels[level];
			let i = grid.index(tx, ty);
			if grid.dirty[i] {
				// Everything above is already dirty.
				break;
			}
			grid.dirty[i] = true;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pyramid_shape_for_30k() {
		let img = TiledImage::new(30_000, 30_000, PixelFormat::Rgba16);
		assert_eq!(img.grid(0).cols(), 118);
		assert_eq!(img.level_count(), 8);
		assert_eq!(img.grid(7).cols(), 1);
		assert_eq!(img.level_size(7), (235, 235));
	}

	#[test]
	fn small_image_has_one_level() {
		let img = TiledImage::new(100, 40, PixelFormat::Rgba8);
		assert_eq!(img.level_count(), 1);
	}

	#[test]
	fn writing_marks_only_ancestors_dirty() {
		let mut img = TiledImage::new(2048, 2048, PixelFormat::Rgba8); // 8x8 tiles, 4 levels
		img.set_slot(5, 2, TileSlot::Solid(PixelValue::rgba8(1, 2, 3, 255)));
		assert!(img.is_dirty(1, 2, 1));
		assert!(img.is_dirty(2, 1, 0));
		assert!(img.is_dirty(3, 0, 0));
		assert_eq!(img.dirty_tiles(1).count(), 1);
		assert!(!img.is_dirty(1, 0, 0));
	}

	#[test]
	fn clone_is_a_snapshot() {
		let mut img = TiledImage::new(512, 512, PixelFormat::Rgba8);
		let red = TileSlot::Solid(PixelValue::rgba8(255, 0, 0, 255));
		img.set_slot(0, 0, red);
		let snapshot = img.clone();
		img.set_slot(0, 0, TileSlot::Empty);
		assert!(matches!(snapshot.slot(0, 0, 0), TileSlot::Solid(_)));
		assert!(img.slot(0, 0, 0).is_empty());
	}

	/// A shape's cache is derived at every level, so every tile starts dirty and
	/// a drawn tile clears only itself (M6-T06).
	#[test]
	fn a_derived_image_starts_dirty_at_every_level() {
		let mut img = TiledImage::derived(512, 512, PixelFormat::Rgba8); // 2 × 2 tiles, 2 levels
		assert!(img.is_derived());
		assert_eq!(img.level_count(), 2);
		assert_eq!(img.grid(1).cols(), 1);
		for level in 0..img.level_count() {
			assert_eq!(img.dirty_tiles(level).count(), (img.grid(level).cols() * img.grid(level).rows()) as usize);
		}
		img.set_derived_slot(0, 0, 0, TileSlot::Solid(PixelValue::rgba8(1, 2, 3, 255)));
		assert_eq!(img.dirty_tiles(0).count(), 3);
		assert!(img.is_dirty(1, 0, 0), "a level above stays dirty: it is drawn by itself");
	}

	#[test]
	fn marking_a_rectangle_dirty_touches_only_its_tiles() {
		// 1024 × 768: 4 × 3 tiles at level 0, 2 × 2 at level 1, 1 × 1 above.
		let mut img = TiledImage::derived(1024, 768, PixelFormat::Rgba8);
		for level in 0..img.level_count() {
			for ty in 0..img.grid(level).rows() {
				for tx in 0..img.grid(level).cols() {
					img.set_derived_slot(level, tx, ty, TileSlot::Empty);
				}
			}
		}
		assert_eq!(img.dirty_tiles(0).count(), 0);
		// A box inside tile (1, 1) only.
		img.mark_rect_dirty([300.0, 300.0, 400.0, 400.0]);
		let mut dirty: Vec<(u32, u32)> = img.dirty_tiles(0).collect();
		dirty.sort_unstable();
		assert_eq!(dirty, vec![(1, 1)]);
		assert!(img.is_dirty(1, 0, 0), "level 1's tile (0,0) covers 300..512");
		assert!(!img.is_dirty(1, 1, 1), "and its tile (1,1) does not");
	}

	#[test]
	fn a_rectangle_spanning_tiles_marks_them_all() {
		let mut img = TiledImage::derived(1024, 768, PixelFormat::Rgba8);
		// A tall box one tile wide: x 300..400 is column 1, y 300..700 is rows
		// 1 and 2 (tile boundaries are at 256, 512 and 768).
		img.mark_rect_dirty([300.0, 300.0, 400.0, 700.0]);
		let mut dirty: Vec<(u32, u32)> = img.dirty_tiles(0).collect();
		dirty.sort_unstable();
		assert_eq!(dirty, vec![(1, 1), (1, 2)]);
	}

	#[test]
	fn a_rectangle_outside_the_image_marks_nothing() {
		let mut img = TiledImage::derived(512, 512, PixelFormat::Rgba8);
		for level in 0..img.level_count() {
			for ty in 0..img.grid(level).rows() {
				for tx in 0..img.grid(level).cols() {
					img.set_derived_slot(level, tx, ty, TileSlot::Empty);
				}
			}
		}
		img.mark_rect_dirty([-500.0, -500.0, -10.0, -10.0]);
		img.mark_rect_dirty([900.0, 900.0, 1200.0, 1200.0]);
		img.mark_rect_dirty([100.0, 100.0, 100.0, 400.0]);
		assert_eq!(img.dirty_tiles(0).count(), 0);
	}
}
