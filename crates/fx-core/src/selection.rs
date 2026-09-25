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

use fx_tiles::{PixelFormat, PixelValue, TILE_PIXELS, TILE_SIZE, TileBuffer, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;
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

/// The Magic Wand's options (M5-T04): what a `Command::MagicWand` replays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WandParams {
	/// The clicked document pixel.
	pub x: f64,
	pub y: f64,
	/// `0..=255` in 8-bit levels, also for 16-bit documents (Photoshop).
	pub tolerance: f64,
	pub contiguous: bool,
	pub anti_alias: bool,
	/// Compare with the composite of all visible layers instead of the
	/// active layer's own pixels.
	pub sample_all_layers: bool,
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

/// The coverage of one canvas tile of a selection (M5 review): either one
/// value for the whole tile, or `TILE_SIZE²` values in row-major order.
///
/// Every algorithm that reads a selection goes through this, so the offset of
/// a moved selection is honoured in one place and uniform tiles (the inside
/// of a big marquee, Select All) never cost a per-pixel loop.
#[derive(Clone, Debug)]
pub enum TileCoverage {
	Uniform(f32),
	Data(Box<[f32]>),
}

impl TileCoverage {
	/// Coverage of pixel `(x, y)` of the tile.
	pub fn at(&self, x: u32, y: u32) -> f32 {
		match self {
			TileCoverage::Uniform(v) => *v,
			TileCoverage::Data(values) => values[(y * TILE_SIZE + x) as usize],
		}
	}

	/// The values of the whole tile.
	pub fn into_values(self) -> Box<[f32]> {
		match self {
			TileCoverage::Uniform(v) => vec![v; TILE_PIXELS].into_boxed_slice(),
			TileCoverage::Data(values) => values,
		}
	}
}

/// Canvas tile columns and rows of a document of `size`.
pub fn canvas_grid(size: (u32, u32)) -> (u32, u32) {
	(size.0.div_ceil(TILE_SIZE), size.1.div_ceil(TILE_SIZE))
}

/// Pixels of canvas tile `(tx, ty)` that lie on the canvas: `(w, h)`.
pub fn valid_extent(size: (u32, u32), tx: u32, ty: u32) -> (u32, u32) {
	(
		(size.0 - (tx * TILE_SIZE).min(size.0)).min(TILE_SIZE),
		(size.1 - (ty * TILE_SIZE).min(size.1)).min(TILE_SIZE),
	)
}

/// The slot for a tile of uniform coverage `value` in `format` (quantised to
/// the format first, so a uniform tile equals the data tile it stands for).
pub fn uniform_slot(format: PixelFormat, value: f32) -> TileSlot {
	let value = value.clamp(0.0, 1.0);
	let v16 = match format {
		PixelFormat::Gray16 => (value * 65535.0).round() as u16,
		_ => (value * 255.0).round() as u16 * 257,
	};
	if v16 == 0 {
		TileSlot::Empty
	} else {
		TileSlot::Solid(PixelValue::gray16(v16))
	}
}

/// A result tile: uniform coverage becomes a slot without a buffer.
pub enum OutTile {
	Uniform(f32),
	Data(TileBuffer),
}

/// Build a result tile from `values` (row-major, `TILE_SIZE²`). Only the
/// `valid` part (on the canvas) counts: when it is uniform the tile collapses
/// to one value, so an edge tile of a full selection is solid like the
/// others.
pub fn out_tile(format: PixelFormat, values: &[f32], valid: (u32, u32)) -> OutTile {
	let first = values[0];
	let uniform = (0..valid.1).all(|y| values[(y * TILE_SIZE) as usize..(y * TILE_SIZE + valid.0) as usize].iter().all(|v| *v == first));
	if uniform {
		return OutTile::Uniform(first);
	}
	let mut buffer = TileBuffer::zeroed(format);
	for y in 0..valid.1 {
		for x in 0..valid.0 {
			let v = values[(y * TILE_SIZE + x) as usize];
			if v > 0.0 {
				set_gray(&mut buffer, format, x, y, v);
			}
		}
	}
	OutTile::Data(buffer)
}

impl Selection {
	/// The canvas tiles that may hold selected pixels: each non-empty image
	/// tile moved by the offset (up to four canvas tiles when the offset is
	/// not a multiple of the tile size), clipped to the canvas. Sorted.
	pub fn canvas_tiles(&self, size: (u32, u32)) -> Vec<(u32, u32)> {
		let (cols, rows) = canvas_grid(size);
		let tile = i64::from(TILE_SIZE);
		let (ox, oy) = (i64::from(self.offset.0), i64::from(self.offset.1));
		let mut out = Vec::new();
		for (tx, ty) in self.tiles() {
			let x0 = i64::from(tx) * tile + ox;
			let y0 = i64::from(ty) * tile + oy;
			let (cx0, cx1) = (x0.div_euclid(tile), (x0 + tile - 1).div_euclid(tile));
			let (cy0, cy1) = (y0.div_euclid(tile), (y0 + tile - 1).div_euclid(tile));
			for cy in cy0.max(0)..=cy1.min(i64::from(rows) - 1) {
				for cx in cx0.max(0)..=cx1.min(i64::from(cols) - 1) {
					out.push((cx as u32, cy as u32));
				}
			}
		}
		out.sort_unstable_by_key(|&(x, y)| (y, x));
		out.dedup();
		out
	}

	/// The document rectangle `(x0, y0, x1, y1)` (exclusive) the selection may
	/// cover, at tile granularity, clipped to the canvas.
	pub fn canvas_bounds(&self, size: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
		let tiles = self.canvas_tiles(size);
		let first = tiles.first()?;
		let mut b = (first.0, first.1, first.0, first.1);
		for &(x, y) in &tiles {
			b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
		}
		Some((
			b.0 * TILE_SIZE,
			b.1 * TILE_SIZE,
			((b.2 + 1) * TILE_SIZE).min(size.0),
			((b.3 + 1) * TILE_SIZE).min(size.1),
		))
	}

	/// The coverage of canvas tile `(tx, ty)`, with the offset applied. Pixels
	/// outside the selection image read 0; pixels outside the canvas are not
	/// meaningful (callers use [`valid_extent`]).
	pub fn tile_coverage(&self, store: &TileStore, tx: u32, ty: u32) -> Result<TileCoverage, TileError> {
		let format = self.image.format();
		let tile = i64::from(TILE_SIZE);
		let ix = i64::from(tx) * tile - i64::from(self.offset.0);
		let iy = i64::from(ty) * tile - i64::from(self.offset.1);
		let (iw, ih) = (i64::from(self.image.width()), i64::from(self.image.height()));
		let (cols, rows) = (i64::from(self.image.grid(0).cols()), i64::from(self.image.grid(0).rows()));
		type Part = Option<Result<f32, Arc<TileBuffer>>>;
		let read = |sx: i64, sy: i64| -> Result<Part, TileError> {
			if sx < 0 || sy < 0 || sx >= cols || sy >= rows {
				return Ok(None);
			}
			Ok(match self.image.slot(0, sx as u32, sy as u32) {
				TileSlot::Empty => Some(Ok(0.0)),
				TileSlot::Solid(v) => Some(Ok(value_of(format, v.0[0]))),
				TileSlot::Data(handle) => Some(Err(store.get(handle)?)),
			})
		};
		// Aligned: the canvas tile is one image tile.
		if ix.rem_euclid(tile) == 0 && iy.rem_euclid(tile) == 0 {
			return Ok(match read(ix / tile, iy / tile)? {
				None => TileCoverage::Uniform(0.0),
				Some(Ok(v)) => TileCoverage::Uniform(v),
				Some(Err(buffer)) => {
					let mut values = vec![0.0f32; TILE_PIXELS];
					for (i, out) in values.iter_mut().enumerate() {
						*out = gray_at(&buffer, format, i as u32 % TILE_SIZE, i as u32 / TILE_SIZE);
					}
					TileCoverage::Data(values.into_boxed_slice())
				}
			});
		}
		// Unaligned: up to four image tiles.
		let (sx0, sy0) = (ix.div_euclid(tile), iy.div_euclid(tile));
		let mut parts: [[Part; 2]; 2] = [[None, None], [None, None]];
		let mut uniform: Option<f32> = None;
		let mut mixed = false;
		for (j, row) in parts.iter_mut().enumerate() {
			for (i, part) in row.iter_mut().enumerate() {
				let got = read(sx0 + i as i64, sy0 + j as i64)?;
				match &got {
					Some(Ok(v)) => {
						if uniform.is_some_and(|u| u != *v) {
							mixed = true;
						}
						uniform.get_or_insert(*v);
					}
					_ => mixed = true,
				}
				*part = got;
			}
		}
		let inside = ix >= 0 && iy >= 0 && ix + tile <= iw && iy + tile <= ih;
		if !mixed && inside {
			return Ok(TileCoverage::Uniform(uniform.unwrap_or(0.0)));
		}
		let mut values = vec![0.0f32; TILE_PIXELS];
		for y in 0..TILE_SIZE {
			let sy = iy + i64::from(y);
			if sy < 0 || sy >= ih {
				continue;
			}
			let pj = (sy.div_euclid(tile) - sy0) as usize;
			let ly = sy.rem_euclid(tile) as u32;
			for x in 0..TILE_SIZE {
				let sx = ix + i64::from(x);
				if sx < 0 || sx >= iw {
					continue;
				}
				let pi = (sx.div_euclid(tile) - sx0) as usize;
				values[(y * TILE_SIZE + x) as usize] = match &parts[pj][pi] {
					None => 0.0,
					Some(Ok(v)) => *v,
					Some(Err(buffer)) => gray_at(buffer, format, sx.rem_euclid(tile) as u32, ly),
				};
			}
		}
		Ok(TileCoverage::Data(values.into_boxed_slice()))
	}

	/// Build a canvas-aligned selection (offset 0) from result tiles; `None`
	/// when nothing is selected.
	pub fn from_tiles(size: (u32, u32), depth: BitDepth, tiles: Vec<((u32, u32), OutTile)>, store: &TileStore) -> Option<Selection> {
		let mut result = Selection::empty(size, depth);
		let format = result.image.format();
		let mut any = false;
		for ((tx, ty), tile) in tiles {
			match tile {
				OutTile::Uniform(v) => {
					let slot = uniform_slot(format, v);
					any |= !slot.is_empty();
					result.image.set_slot(tx, ty, slot);
				}
				OutTile::Data(buffer) => {
					result.image.put_buffer(store, tx, ty, buffer);
					any |= !result.image.slot(0, tx, ty).is_empty();
				}
			}
		}
		any.then_some(result)
	}
}

/// A grey sample of `format` stored on the 16-bit scale, as `0..=1`.
fn value_of(format: PixelFormat, v16: u16) -> f32 {
	match format {
		PixelFormat::Gray16 => f32::from(v16) / 65535.0,
		_ => f32::from(v16 / 257) / 255.0,
	}
}

/// Reads rectangular patches of a selection's coverage (the apron of a
/// blur or a distance transform), caching the canvas tiles it has read.
/// Outside the canvas the coverage is 0.
pub struct PatchReader<'a> {
	selection: &'a Selection,
	store: &'a TileStore,
	size: (u32, u32),
	cache: HashMap<(u32, u32), Arc<TileCoverage>>,
}

impl<'a> PatchReader<'a> {
	/// A reader over `selection` on a canvas of `size`.
	pub fn new(selection: &'a Selection, store: &'a TileStore, size: (u32, u32)) -> Self {
		Self {
			selection,
			store,
			size,
			cache: HashMap::new(),
		}
	}

	/// The canvas tile's coverage (cached).
	pub fn tile(&mut self, tx: u32, ty: u32) -> Result<Arc<TileCoverage>, TileError> {
		if let Some(tile) = self.cache.get(&(tx, ty)) {
			return Ok(tile.clone());
		}
		let tile = Arc::new(self.selection.tile_coverage(self.store, tx, ty)?);
		self.cache.insert((tx, ty), tile.clone());
		Ok(tile)
	}

	/// Whether every canvas pixel of the rectangle `[x0, x1) × [y0, y1)`
	/// (clipped to the canvas) has coverage `value`, answered from uniform
	/// tiles only (`false` when a data tile is involved).
	pub fn is_uniform(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, value: f32) -> Result<bool, TileError> {
		let tile = i64::from(TILE_SIZE);
		let (cx0, cy0) = (x0.max(0), y0.max(0));
		let (cx1, cy1) = (x1.min(i64::from(self.size.0)), y1.min(i64::from(self.size.1)));
		if cx1 <= cx0 || cy1 <= cy0 {
			return Ok(value == 0.0);
		}
		// Outside the canvas the coverage is 0.
		if value != 0.0 && (x0 < 0 || y0 < 0 || x1 > i64::from(self.size.0) || y1 > i64::from(self.size.1)) {
			return Ok(false);
		}
		for ty in cy0 / tile..=(cy1 - 1) / tile {
			for tx in cx0 / tile..=(cx1 - 1) / tile {
				match self.tile(tx as u32, ty as u32)?.as_ref() {
					TileCoverage::Uniform(v) if *v == value => {}
					_ => return Ok(false),
				}
			}
		}
		Ok(true)
	}

	/// Coverage of the `w × h` rectangle at document `(x0, y0)`, row-major.
	pub fn patch(&mut self, x0: i64, y0: i64, w: usize, h: usize) -> Result<Vec<f32>, TileError> {
		let mut out = vec![0.0f32; w * h];
		let tile = i64::from(TILE_SIZE);
		let cx0 = x0.max(0);
		let cy0 = y0.max(0);
		let cx1 = (x0 + w as i64).min(i64::from(self.size.0));
		let cy1 = (y0 + h as i64).min(i64::from(self.size.1));
		if cx1 <= cx0 || cy1 <= cy0 {
			return Ok(out);
		}
		for ty in cy0 / tile..=(cy1 - 1) / tile {
			for tx in cx0 / tile..=(cx1 - 1) / tile {
				let coverage = self.tile(tx as u32, ty as u32)?;
				let (bx, by) = (tx * tile, ty * tile);
				let (rx0, rx1) = (cx0.max(bx), cx1.min(bx + tile));
				let (ry0, ry1) = (cy0.max(by), cy1.min(by + tile));
				for y in ry0..ry1 {
					let row = &mut out[((y - y0) as usize) * w..((y - y0) as usize + 1) * w];
					let dst = &mut row[(rx0 - x0) as usize..(rx1 - x0) as usize];
					match coverage.as_ref() {
						TileCoverage::Uniform(v) => dst.fill(*v),
						TileCoverage::Data(values) => {
							let src = ((y - by) * tile) as usize;
							dst.copy_from_slice(&values[src + (rx0 - bx) as usize..src + (rx1 - bx) as usize]);
						}
					}
				}
			}
		}
		Ok(out)
	}
}

/// Combine `new` (a freshly rasterised shape) with `old` under `mode`
/// (M5-T03). Only the tiles the shape touches are built; the rest follow from
/// the mode without reading pixels (Replace/Intersect → empty, Add/Subtract →
/// the old selection). Uniform tiles combine as one value, and tiles run on
/// rayon.
///
/// Returns `None` when the result selects nothing.
pub fn combine(size: (u32, u32), old: Option<&Selection>, new: &Selection, mode: SelectMode, store: &TileStore) -> Result<Option<Selection>, TileError> {
	let format = new.image.format();
	let depth = if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 };
	let (old, mode) = match old {
		Some(old) => (old, mode),
		// Nothing selected: Add = Replace; Subtract/Intersect select nothing.
		None if matches!(mode, SelectMode::Replace | SelectMode::Add) => (new, SelectMode::Replace),
		None => return Ok(None),
	};
	let mut tiles = new.canvas_tiles(size);
	if matches!(mode, SelectMode::Add | SelectMode::Subtract) {
		tiles.extend(old.canvas_tiles(size));
		tiles.sort_unstable_by_key(|&(x, y)| (y, x));
		tiles.dedup();
	}
	let op = move |a: f32, b: f32| match mode {
		SelectMode::Replace => b,
		// Not `a + b`: a soft edge would count twice and clamp.
		SelectMode::Add => a.max(b),
		// Not `a - b`: it goes negative outside the shape.
		SelectMode::Subtract => a.min(1.0 - b),
		// Not `a * b`: it darkens a soft edge on both sides.
		SelectMode::Intersect => a.min(b),
	};
	let results: Result<Vec<_>, TileError> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			let b = new.tile_coverage(store, tx, ty)?;
			let a = if mode == SelectMode::Replace {
				TileCoverage::Uniform(0.0)
			} else {
				old.tile_coverage(store, tx, ty)?
			};
			let out = match (&a, &b) {
				(TileCoverage::Uniform(a), TileCoverage::Uniform(b)) => OutTile::Uniform(op(*a, *b)),
				_ => {
					let mut values = vec![0.0f32; TILE_PIXELS];
					for (i, v) in values.iter_mut().enumerate() {
						let (x, y) = (i as u32 % TILE_SIZE, i as u32 / TILE_SIZE);
						*v = op(a.at(x, y), b.at(x, y));
					}
					out_tile(format, &values, valid_extent(size, tx, ty))
				}
			};
			Ok(((tx, ty), out))
		})
		.collect();
	Ok(Selection::from_tiles(size, depth, results?, store))
}

/// The inverse of `source` over `size` (Select ▸ Inverse, M5-T03): coverage
/// `1 - source`, as a canvas-aligned selection (`None` when that selects
/// nothing). Uniform tiles invert without a per-pixel loop, so a
/// document-wide inverse costs one operation per tile.
pub fn invert(size: (u32, u32), source: &Selection, store: &TileStore) -> Result<Option<Selection>, TileError> {
	let format = source.image.format();
	let depth = if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 };
	let (cols, rows) = canvas_grid(size);
	let tiles: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let results: Result<Vec<_>, TileError> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			let out = match source.tile_coverage(store, tx, ty)? {
				TileCoverage::Uniform(v) => OutTile::Uniform(1.0 - v),
				TileCoverage::Data(values) => {
					let inverted: Vec<f32> = values.iter().map(|v| 1.0 - v).collect();
					out_tile(format, &inverted, valid_extent(size, tx, ty))
				}
			};
			Ok(((tx, ty), out))
		})
		.collect();
	Ok(Selection::from_tiles(size, depth, results?, store))
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
		let (tx, ty) = ((x / i64::from(TILE_SIZE)) as u32, (y / i64::from(TILE_SIZE)) as u32);
		selection.tile_coverage(store, tx, ty).unwrap().at(x as u32 % TILE_SIZE, y as u32 % TILE_SIZE)
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
	fn adding_to_a_moved_selection_keeps_the_moved_part() {
		// Review fix: the tiles of an offset selection are canvas tiles moved by
		// the offset, not the image's own tile coordinates.
		let store = store();
		let size = (1000, 300);
		let mut old = rect_selection(&store, size, (0, 0, 100, 100));
		old.offset = (600, 10);
		let new = rect_selection(&store, size, (0, 0, 50, 50));
		let added = combine(size, Some(&old), &new, SelectMode::Add, &store).unwrap().unwrap();
		assert_eq!(added.offset, (0, 0), "the result is canvas-aligned");
		assert_eq!(at(&added, &store, 650, 50), 1.0, "the moved rectangle survives");
		assert_eq!(at(&added, &store, 25, 25), 1.0, "the new one is added");
		assert_eq!(at(&added, &store, 75, 75), 0.0, "the old place is empty");
	}

	#[test]
	fn a_moved_selection_reads_across_four_tiles() {
		let store = store();
		let size = (800, 800);
		let mut selection = rect_selection(&store, size, (0, 0, 300, 300));
		selection.offset = (100, 130);
		let tiles = selection.canvas_tiles(size);
		assert!(tiles.contains(&(1, 1)) && tiles.contains(&(0, 0)) && !tiles.contains(&(3, 3)), "{tiles:?}");
		assert_eq!(at(&selection, &store, 399, 429), 1.0);
		assert_eq!(at(&selection, &store, 400, 429), 0.0);
		assert_eq!(at(&selection, &store, 99, 200), 0.0);
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
