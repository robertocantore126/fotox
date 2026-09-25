//! Exact pixel permutations (M6-T02): Image ▸ Rotate 90° / 180° and the two
//! canvas flips.
//!
//! A permutation is not a resample: every destination pixel takes exactly one
//! source pixel, unchanged, so 16-bit data survives bit-exactly and no kernel
//! runs. Routing a quarter turn through the resampler would soften every edge
//! and lose the low bits — and S17 (a 10 000² rotation in ≤ 30 s) is an I/O
//! problem, not a filtering one.
//!
//! Destination tile `(tx, ty)` is built by walking its pixels and asking
//! [`Permutation::source_pixel`] where each one comes from, so a partial last
//! row or column stays exact whatever the image size is (the tile grid is
//! padded to whole tiles). The one source rectangle a destination tile reads is
//! fetched once, through [`Grid`] — the same bounded cache the resampler uses,
//! so memory stays one tile plus its apron, never the image (the One Rule).
//!
//! A destination tile whose source region is uniformly solid (a mask, a
//! selection, a filled layer) becomes `Solid` without touching a pixel, which
//! is what makes rotating a document of solid layers instant.

use fx_core::Permutation;
use fx_tiles::{PixelFormat, PixelValue, TILE_SIZE, TileBuffer, TileClass, TileError, TileSlot, TileStore};
use rayon::prelude::*;

use crate::neighbourhood::{LevelSource, TileRef};
use crate::resample::{Grid, SourceInfo};

/// One destination tile of a permutation: its tile coordinates and its slot.
pub type PermutedTile = ((u32, u32), TileSlot);

/// Permute the destination tiles `dst_tiles` (tile coordinates of the permuted
/// image, of size `op.size(source_size)`) from `src`.
///
/// Tiles that lie entirely outside the permuted image come back [`TileSlot::Empty`];
/// an empty source region is never allocated. The caller puts the slots into the
/// image it is building (`TiledImage::set_slot`), which also marks the mip
/// levels above them dirty.
pub fn permute(
	src: &dyn LevelSource,
	source_size: (u32, u32),
	op: Permutation,
	dst_tiles: &[(u32, u32)],
	store: &TileStore,
) -> Result<Vec<PermutedTile>, TileError> {
	let plan = Plan {
		op,
		source_size,
		dest_size: op.size(source_size),
		format: src.format(),
	};
	let results: Result<Vec<_>, TileError> = dst_tiles
		.par_iter()
		.map(|&(tx, ty)| plan.tile(src, tx, ty, store).map(|slot| ((tx, ty), slot)))
		.collect();
	results
}

/// What every destination tile of a permutation needs to know.
struct Plan {
	op: Permutation,
	/// Size of the image being permuted.
	source_size: (u32, u32),
	/// Size of the permuted image (`op.size(source_size)`).
	dest_size: (u32, u32),
	format: PixelFormat,
}

impl Plan {
	fn tile(&self, src: &dyn LevelSource, tx: u32, ty: u32, store: &TileStore) -> Result<TileSlot, TileError> {
		let tile = i64::from(TILE_SIZE);
		let (x0, y0) = (i64::from(tx) * tile, i64::from(ty) * tile);
		// The part of the tile that is inside the permuted image.
		let (x1, y1) = ((x0 + tile).min(i64::from(self.dest_size.0)), (y0 + tile).min(i64::from(self.dest_size.1)));
		if x0 >= x1 || y0 >= y1 {
			return Ok(TileSlot::Empty);
		}
		// A permutation maps a rectangle to a rectangle, so the four corners of
		// the clipped destination tile bound the source pixels it reads exactly.
		let mut rect = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
		for (x, y) in [(x0, y0), (x1 - 1, y0), (x1 - 1, y1 - 1), (x0, y1 - 1)] {
			let Some(source) = self.op.source_pixel((x as u32, y as u32), self.source_size) else {
				return Ok(TileSlot::Empty);
			};
			rect[0] = rect[0].min(i64::from(source.0));
			rect[1] = rect[1].min(i64::from(source.1));
			rect[2] = rect[2].max(i64::from(source.0) + 1);
			rect[3] = rect[3].max(i64::from(source.1) + 1);
		}
		// Nothing moves when the whole source region is one solid value.
		if let Some(value) = uniform_source(src, self.source_size, rect)? {
			return Ok(uniform_slot(self.format, value));
		}
		let grid = Grid::load(
			src,
			SourceInfo {
				size: self.source_size,
				levels: 1,
			},
			0,
			self.format,
			rect,
		)?;
		let mut buffer = TileBuffer::zeroed(self.format);
		for y in y0..y1 {
			for x in x0..x1 {
				let Some(source) = self.op.source_pixel((x as u32, y as u32), self.source_size) else {
					continue;
				};
				write(
					&mut buffer,
					self.format,
					(x - x0) as usize,
					(y - y0) as usize,
					grid.raw(i64::from(source.0), i64::from(source.1)),
				);
			}
		}
		Ok(match buffer.uniform_value() {
			Some(value) => uniform_slot(self.format, value.0),
			None => TileSlot::Data(store.insert(buffer, TileClass::Authoritative)),
		})
	}
}

/// The one value the source region `rect` holds everywhere, or `None` when it
/// is not uniform or reaches outside the image (`Empty` there, so not solid).
fn uniform_source(src: &dyn LevelSource, size: (u32, u32), rect: [i64; 4]) -> Result<Option<[u16; 4]>, TileError> {
	let tile = i64::from(TILE_SIZE);
	if rect[0] < 0 || rect[1] < 0 || rect[2] > i64::from(size.0) || rect[3] > i64::from(size.1) {
		return Ok(None);
	}
	let (tx0, ty0) = (rect[0].div_euclid(tile), rect[1].div_euclid(tile));
	let (tx1, ty1) = ((rect[2] - 1).div_euclid(tile), (rect[3] - 1).div_euclid(tile));
	let mut value = None;
	for ty in ty0..=ty1 {
		for tx in tx0..=tx1 {
			match src.tile(0, tx, ty)? {
				Some(TileRef::Solid(v)) if value.is_none_or(|previous: [u16; 4]| previous == v) => value = Some(v),
				_ => return Ok(None),
			}
		}
	}
	Ok(value)
}

/// The canonical slot for a value that fills a whole tile: transparent (or
/// black for gray, which has no alpha) collapses to [`TileSlot::Empty`].
fn uniform_slot(format: PixelFormat, value: [u16; 4]) -> TileSlot {
	let value = PixelValue(value);
	if value.is_transparent(format) || (!format.has_alpha() && value.0[0] == 0) {
		TileSlot::Empty
	} else {
		TileSlot::Solid(value)
	}
}

/// One pixel of a destination tile, copied exactly (`pixel()` scales 8-bit to
/// the 16-bit scale, and dividing by 257 is the exact inverse).
fn write(buffer: &mut TileBuffer, format: PixelFormat, x: usize, y: usize, px: [u16; 4]) {
	let index = (y * TILE_SIZE as usize + x) * 4;
	match format {
		PixelFormat::Rgba16 => buffer.as_u16_mut()[index..index + 4].copy_from_slice(&px),
		PixelFormat::Rgba8 => {
			for (c, value) in px.iter().enumerate() {
				buffer.bytes_mut()[index + c] = (*value / 257) as u8;
			}
		}
		PixelFormat::Gray16 => buffer.as_u16_mut()[index / 4] = px[0],
		PixelFormat::Gray8 => buffer.bytes_mut()[index / 4] = (px[0] / 257) as u8,
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use fx_tiles::{PixelFormat, TILE_PIXELS, TileStoreConfig, TiledImage};

	use super::*;

	/// A one-or-two-tile source whose pixel `(x, y)` encodes its position, so a
	/// permutation is checked pixel by pixel.
	struct Source {
		w: u32,
		h: u32,
		format: PixelFormat,
		/// Written into the blue channel; `0` = leave the tile empty.
		fill: u16,
	}

	impl Source {
		fn new(w: u32, h: u32, fill: u16) -> Self {
			Self {
				w,
				h,
				format: PixelFormat::Rgba16,
				fill,
			}
		}

		fn at(&self, x: u32, y: u32) -> [u16; 4] {
			[x as u16 * 100 + 7, y as u16 * 100 + 11, self.fill, 65_535]
		}
	}

	impl LevelSource for Source {
		fn format(&self) -> PixelFormat {
			self.format
		}

		fn tile(&self, _level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
			let tile = i64::from(TILE_SIZE);
			let (x0, y0) = (tx * tile, ty * tile);
			if x0 >= i64::from(self.w) || y0 >= i64::from(self.h) {
				return Ok(None);
			}
			let mut buffer = TileBuffer::zeroed(self.format);
			for y in 0..tile {
				for x in 0..tile {
					let (gx, gy) = (x0 + x, y0 + y);
					if gx >= i64::from(self.w) || gy >= i64::from(self.h) {
						continue;
					}
					if self.fill == 0 {
						continue;
					}
					let p = self.at(gx as u32, gy as u32);
					let i = ((y * tile + x) * 4) as usize;
					buffer.as_u16_mut()[i..i + 4].copy_from_slice(&p);
				}
			}
			Ok(Some(TileRef::Data(Arc::new(buffer))))
		}
	}

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-ops-permute-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// Every tile of a `size` image's level-0 grid.
	fn dst_tiles(size: (u32, u32)) -> Vec<(u32, u32)> {
		let (cols, rows) = (size.0.div_ceil(TILE_SIZE), size.1.div_ceil(TILE_SIZE));
		(0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect()
	}

	/// Permute a whole image and assemble it as straight RGBA16.
	fn permute_image(src: &Source, op: Permutation, store: &TileStore) -> (Vec<[u16; 4]>, (u32, u32)) {
		let size = op.size((src.w, src.h));
		let mut image = TiledImage::new(size.0, size.1, PixelFormat::Rgba16);
		for tiles in dst_tiles(size).chunks(4) {
			for ((tx, ty), slot) in permute(src, (src.w, src.h), op, tiles, store).unwrap() {
				image.set_slot(tx, ty, slot);
			}
		}
		let mut out = vec![[0u16; 4]; (size.0 * size.1) as usize];
		for (tx, ty, slot) in image.grid(0).non_empty() {
			let (x0, y0) = (tx * TILE_SIZE, ty * TILE_SIZE);
			match slot {
				TileSlot::Empty => {}
				TileSlot::Solid(value) => {
					for y in 0..TILE_SIZE {
						for x in 0..TILE_SIZE {
							let (gx, gy) = (x0 + x, y0 + y);
							if gx < size.0 && gy < size.1 {
								out[(gy * size.0 + gx) as usize] = value.0;
							}
						}
					}
				}
				TileSlot::Data(handle) => {
					let buffer = store.get(handle).unwrap();
					for y in 0..TILE_SIZE {
						for x in 0..TILE_SIZE {
							let (gx, gy) = (x0 + x, y0 + y);
							if gx >= size.0 || gy >= size.1 {
								continue;
							}
							let i = ((y * TILE_SIZE + x) * 4) as usize;
							let s = buffer.as_u16();
							out[(gy * size.0 + gx) as usize] = [s[i], s[i + 1], s[i + 2], s[i + 3]];
						}
					}
				}
			}
		}
		(out, size)
	}

	#[test]
	fn every_permutation_moves_pixels_exactly() {
		// 300 × 520 spans 2 × 3 tiles: the partial last column and row are real.
		let (w, h) = (300, 520);
		let src = Source::new(w, h, 40_000);
		let store = store();
		for op in [
			Permutation::Rot90Cw,
			Permutation::Rot90Ccw,
			Permutation::Rot180,
			Permutation::FlipHorizontal,
			Permutation::FlipVertical,
		] {
			let (out, size) = permute_image(&src, op, &store);
			assert_eq!(size, op.size((w, h)));
			for y in 0..size.1 {
				for x in 0..size.0 {
					let source = op.source_pixel((x, y), (w, h)).unwrap();
					assert_eq!(out[(y * size.0 + x) as usize], src.at(source.0, source.1), "{op:?} at ({x},{y})");
				}
			}
		}
	}

	/// A `TiledImage` as a source (the engine's `ImageSource`, minimal).
	struct Tiles<'a> {
		image: &'a TiledImage,
		store: &'a TileStore,
	}

	impl LevelSource for Tiles<'_> {
		fn format(&self) -> PixelFormat {
			self.image.format()
		}

		fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
			if level >= self.image.level_count() || tx < 0 || ty < 0 {
				return Ok(None);
			}
			let grid = self.image.grid(level);
			if tx >= i64::from(grid.cols()) || ty >= i64::from(grid.rows()) {
				return Ok(None);
			}
			Ok(match self.image.slot(level, tx as u32, ty as u32) {
				TileSlot::Empty => None,
				TileSlot::Solid(value) => Some(TileRef::Solid(value.0)),
				TileSlot::Data(handle) => Some(TileRef::Data(self.store.get(handle)?)),
			})
		}
	}

	/// The fixture's tiles as a `TiledImage`.
	fn image_of(src: &Source, store: &TileStore) -> TiledImage {
		let mut image = TiledImage::new(src.w, src.h, PixelFormat::Rgba16);
		for (tx, ty) in dst_tiles((src.w, src.h)) {
			let Some(TileRef::Data(buffer)) = src.tile(0, i64::from(tx), i64::from(ty)).unwrap() else {
				continue;
			};
			image.put_buffer(store, tx, ty, (*buffer).clone());
		}
		image
	}

	#[test]
	fn four_quarter_turns_are_the_original_image() {
		let src = Source::new(300, 520, 40_000);
		let store = store();
		let mut image = image_of(&src, &store);
		for _ in 0..4 {
			let size = (image.width(), image.height());
			let dest = Permutation::Rot90Cw.size(size);
			let mut next = TiledImage::new(dest.0, dest.1, PixelFormat::Rgba16);
			let view = Tiles { image: &image, store: &store };
			for tiles in dst_tiles(dest).chunks(4) {
				for ((tx, ty), slot) in permute(&view, size, Permutation::Rot90Cw, tiles, &store).unwrap() {
					next.set_slot(tx, ty, slot);
				}
			}
			image = next;
		}
		assert_eq!((image.width(), image.height()), (src.w, src.h));
		for (tx, ty, slot) in image.grid(0).non_empty() {
			let TileSlot::Data(handle) = slot else {
				panic!("a rotated real tile stays real");
			};
			let buffer = store.get(handle).unwrap();
			for y in 0..TILE_SIZE {
				for x in 0..TILE_SIZE {
					let (gx, gy) = (tx * TILE_SIZE + x, ty * TILE_SIZE + y);
					if gx >= src.w || gy >= src.h {
						continue;
					}
					let i = ((y * TILE_SIZE + x) * 4) as usize;
					let s = buffer.as_u16();
					assert_eq!([s[i], s[i + 1], s[i + 2], s[i + 3]], src.at(gx, gy), "({gx},{gy})");
				}
			}
		}
	}

	#[test]
	fn a_solid_source_needs_no_pixels() {
		/// Every tile solid, and no pixel data at all.
		struct Solid([u16; 4]);

		impl LevelSource for Solid {
			fn format(&self) -> PixelFormat {
				PixelFormat::Gray16
			}

			fn tile(&self, _level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
				// The grid of a 600 × 400 image: 3 × 2 tiles.
				if tx < 0 || ty < 0 || tx > 2 || ty > 1 {
					return Ok(None);
				}
				Ok(Some(TileRef::Solid(self.0)))
			}
		}

		let store = store();
		let source = Solid([9_000, 0, 0, 0]);
		let size = (600, 400);
		let tiles = dst_tiles(Permutation::Rot90Cw.size(size));
		for ((tx, ty), slot) in permute(&source, size, Permutation::Rot90Cw, &tiles, &store).unwrap() {
			assert!(matches!(slot, TileSlot::Solid(value) if value.0 == source.0), "({tx},{ty}) {slot:?}");
		}
	}

	#[test]
	fn an_empty_source_stays_empty() {
		let src = Source::new(300, 520, 0);
		let store = store();
		let (out, size) = permute_image(&src, Permutation::Rot90Cw, &store);
		assert_eq!(out.len(), (size.0 * size.1) as usize);
		assert!(out.iter().all(|p| *p == [0; 4]));
		assert_eq!(TILE_PIXELS, 256 * 256);
	}
}
