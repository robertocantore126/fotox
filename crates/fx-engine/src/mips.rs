//! Mip generation for a [`TiledImage`] (M1-T05).
//!
//! Mip tiles are derived data: computed from level 0 on demand, stored as
//! [`TileClass::Derived`] (the store may drop them under memory pressure,
//! after which `get` reports [`TileError::Evicted`]) and never part of undo.
//!
//! Two entry points:
//! * [`ensure_mip`] — make one mip tile usable, computing exactly the dirty or
//!   evicted tiles below it first (what `fx_render::build_program`'s
//!   `MipRequest`s ask for).
//! * [`ensure_all_mips`] — the whole pyramid, level by level (idle work, the
//!   `mips` benchmark).
//!
//! Tiles of one level are independent, so each level is computed on the rayon
//! pool and then committed in one pass. The engine thread runs these on a
//! *copy* of the document's images and commits the results (M1-T07 wires the
//! scheduling; `TiledImage` is cheap to clone).

use std::collections::BTreeSet;

use fx_tiles::{ChildPixels, downsample_2x2};
use fx_tiles::{TileBuffer, TileClass, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

/// How often [`ensure_mip`] restarts when a tile it just computed from is
/// evicted before it could be read. Each restart recomputes only what was lost.
const MAX_RETRIES: usize = 4;

/// Make mip tile `(level, tx, ty)` of `image` clean and readable, and return
/// its slot. Level 0 is returned as it is.
pub fn ensure_mip(image: &mut TiledImage, store: &TileStore, level: usize, tx: u32, ty: u32) -> Result<TileSlot, TileError> {
	for _ in 0..MAX_RETRIES {
		let mut needed: Vec<BTreeSet<(u32, u32)>> = vec![BTreeSet::new(); level + 1];
		collect_needed(image, store, level, tx, ty, &mut needed);
		match compute_levels(image, store, &needed) {
			Ok(()) => return Ok(image.slot(level, tx, ty).clone()),
			// A child was dropped by the trim thread between being computed
			// and being read: go again, recomputing only what is missing.
			Err(TileError::Evicted) => continue,
			Err(error) => return Err(error),
		}
	}
	Err(TileError::Evicted)
}

/// Compute every dirty tile of every mip level (level 1 first, so each level
/// reads a clean one below it). Evicted tiles are left alone: they are
/// recomputed when something asks for them.
pub fn ensure_all_mips(image: &mut TiledImage, store: &TileStore) -> Result<(), TileError> {
	for level in 1..image.level_count() {
		let tiles: Vec<(u32, u32)> = image.dirty_tiles(level).collect();
		if tiles.is_empty() {
			continue;
		}
		let results = compute_tiles(image, store, level, &tiles)?;
		commit(image, level, results);
	}
	Ok(())
}

/// Whether mip tile `(level, tx, ty)` has to be (re)computed before use.
fn needs_compute(image: &TiledImage, store: &TileStore, level: usize, tx: u32, ty: u32) -> bool {
	if level == 0 {
		return false;
	}
	if image.is_dirty(level, tx, ty) {
		return true;
	}
	// Derived tiles are only ever hot; one that is not hot has been dropped.
	matches!(image.slot(level, tx, ty), TileSlot::Data(handle) if handle.class() == TileClass::Derived && !store.is_hot(handle))
}

/// Walk down from `(level, tx, ty)` and record, per level, every tile that
/// must be computed for it to become usable.
fn collect_needed(image: &TiledImage, store: &TileStore, level: usize, tx: u32, ty: u32, needed: &mut [BTreeSet<(u32, u32)>]) {
	if !needs_compute(image, store, level, tx, ty) || !needed[level].insert((tx, ty)) {
		return;
	}
	let below = image.grid(level - 1);
	for (cx, cy) in children(tx, ty) {
		if cx < below.cols() && cy < below.rows() {
			collect_needed(image, store, level - 1, cx, cy, needed);
		}
	}
}

fn compute_levels(image: &mut TiledImage, store: &TileStore, needed: &[BTreeSet<(u32, u32)>]) -> Result<(), TileError> {
	for (level, tiles) in needed.iter().enumerate().skip(1) {
		if tiles.is_empty() {
			continue;
		}
		let tiles: Vec<(u32, u32)> = tiles.iter().copied().collect();
		let results = compute_tiles(image, store, level, &tiles)?;
		commit(image, level, results);
	}
	Ok(())
}

/// Compute `tiles` of `level` from level `level - 1`, in parallel.
fn compute_tiles(image: &TiledImage, store: &TileStore, level: usize, tiles: &[(u32, u32)]) -> Result<Vec<(u32, u32, TileSlot)>, TileError> {
	tiles
		.par_iter()
		.map(|&(tx, ty)| compute_tile(image, store, level, tx, ty).map(|slot| (tx, ty, slot)))
		.collect()
}

fn commit(image: &mut TiledImage, level: usize, results: Vec<(u32, u32, TileSlot)>) {
	for (tx, ty, slot) in results {
		image.set_derived_slot(level, tx, ty, slot);
	}
}

/// One mip tile from its four children. Uniform results cost no tile memory.
fn compute_tile(image: &TiledImage, store: &TileStore, level: usize, tx: u32, ty: u32) -> Result<TileSlot, TileError> {
	let format = image.format();
	let below = image.grid(level - 1);
	// Keep the children's buffers alive while `ChildPixels` borrows them.
	let mut buffers: [Option<std::sync::Arc<TileBuffer>>; 4] = [None, None, None, None];
	let coords = children(tx, ty);
	for (i, &(cx, cy)) in coords.iter().enumerate() {
		if cx < below.cols()
			&& cy < below.rows()
			&& let TileSlot::Data(handle) = below.slot(cx, cy)
		{
			buffers[i] = Some(store.get(handle)?);
		}
	}
	let pixels: [ChildPixels<'_>; 4] = std::array::from_fn(|i| {
		let (cx, cy) = coords[i];
		if cx >= below.cols() || cy >= below.rows() {
			return ChildPixels::Empty;
		}
		match below.slot(cx, cy) {
			TileSlot::Empty => ChildPixels::Empty,
			TileSlot::Solid(value) => ChildPixels::Solid(*value),
			TileSlot::Data(_) => ChildPixels::Data(buffers[i].as_deref().expect("loaded above for every data slot")),
		}
	});

	// All four empty → empty; all four the same solid → that solid. No pixels.
	match pixels {
		[ChildPixels::Empty, ChildPixels::Empty, ChildPixels::Empty, ChildPixels::Empty] => return Ok(TileSlot::Empty),
		[ChildPixels::Solid(a), ChildPixels::Solid(b), ChildPixels::Solid(c), ChildPixels::Solid(d)] if a == b && b == c && c == d => {
			return Ok(TileSlot::Solid(a));
		}
		_ => {}
	}

	let buffer = downsample_2x2(format, pixels);
	Ok(match buffer.uniform_value() {
		Some(v) if v.is_transparent(format) || (!format.has_alpha() && v.0[0] == 0) => TileSlot::Empty,
		Some(v) => TileSlot::Solid(v),
		None => TileSlot::Data(store.insert(buffer, TileClass::Derived)),
	})
}

/// Child coordinates at the level below: `[top_left, top_right, bottom_left, bottom_right]`.
fn children(tx: u32, ty: u32) -> [(u32, u32); 4] {
	[(2 * tx, 2 * ty), (2 * tx + 1, 2 * ty), (2 * tx, 2 * ty + 1), (2 * tx + 1, 2 * ty + 1)]
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_tiles::{PixelFormat, PixelValue, TileStoreConfig};

	/// A test store large enough that nothing is trimmed behind a test's back.
	fn store() -> TileStore {
		let mut config = TileStoreConfig::for_tests(std::env::temp_dir().join("fx-engine-tests"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).unwrap()
	}

	fn noise(seed: u16) -> TileBuffer {
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		for (i, v) in buffer.as_u16_mut().iter_mut().enumerate() {
			*v = if i % 4 == 3 { 65535 } else { (i as u16).wrapping_mul(31).wrapping_add(seed) };
		}
		buffer
	}

	#[test]
	fn an_empty_image_has_clean_empty_mips() {
		let store = store();
		let mut image = TiledImage::new(1000, 700, PixelFormat::Rgba16);
		let top = image.level_count() - 1;
		assert!(ensure_mip(&mut image, &store, top, 0, 0).unwrap().is_empty());
	}

	#[test]
	fn solid_level_zero_gives_solid_mips_without_pixel_work() {
		let store = store();
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba16);
		let red = PixelValue::rgba16(65535, 0, 0, 65535);
		for ty in 0..2 {
			for tx in 0..2 {
				image.set_slot(tx, ty, TileSlot::Solid(red));
			}
		}
		assert!(matches!(ensure_mip(&mut image, &store, 1, 0, 0).unwrap(), TileSlot::Solid(v) if v == red));
		assert_eq!(store.stats().live_tiles, 0, "no tile memory for a solid pyramid");
	}

	#[test]
	fn ensure_mip_computes_only_what_it_needs_and_cleans_it() {
		let store = store();
		let mut image = TiledImage::new(1024, 1024, PixelFormat::Rgba16);
		for ty in 0..4 {
			for tx in 0..4 {
				image.put_buffer(&store, tx, ty, noise((tx * 4 + ty) as u16));
			}
		}
		// Level 1 tile (0, 0) needs level-0 tiles (0..2, 0..2) only.
		let slot = ensure_mip(&mut image, &store, 1, 0, 0).unwrap();
		assert!(matches!(slot, TileSlot::Data(_)));
		assert!(!image.is_dirty(1, 0, 0));
		assert!(image.is_dirty(1, 1, 1), "an unrelated tile stays dirty");
		// The level-2 tile pulls in the remaining level-1 tiles.
		ensure_mip(&mut image, &store, 2, 0, 0).unwrap();
		assert_eq!(image.dirty_tiles(1).count(), 0);
		assert_eq!(image.dirty_tiles(2).count(), 0);
	}

	#[test]
	fn mip_pixels_match_downsample_of_the_children() {
		let store = store();
		let mut image = TiledImage::new(512, 256, PixelFormat::Rgba16);
		let (a, b) = (noise(1), noise(2));
		image.put_buffer(&store, 0, 0, a.clone());
		image.put_buffer(&store, 1, 0, b.clone());
		let TileSlot::Data(handle) = ensure_mip(&mut image, &store, 1, 0, 0).unwrap() else {
			panic!("expected pixel data");
		};
		let expected = downsample_2x2(
			PixelFormat::Rgba16,
			[ChildPixels::Data(&a), ChildPixels::Data(&b), ChildPixels::Empty, ChildPixels::Empty],
		);
		assert_eq!(store.get(&handle).unwrap().bytes(), expected.bytes());
		assert_eq!(handle.class(), TileClass::Derived);
	}

	#[test]
	fn ensure_all_mips_cleans_the_whole_pyramid() {
		let store = store();
		let mut image = TiledImage::new(3000, 2000, PixelFormat::Rgba8);
		let mut tile = TileBuffer::zeroed(PixelFormat::Rgba8);
		tile.bytes_mut()[..4].copy_from_slice(&[1, 2, 3, 255]);
		image.put_buffer(&store, 5, 3, tile);
		ensure_all_mips(&mut image, &store).unwrap();
		for level in 1..image.level_count() {
			assert_eq!(image.dirty_tiles(level).count(), 0, "level {level} is clean");
		}
		// The one non-empty tile propagates to the top as data.
		let top = image.level_count() - 1;
		assert!(!image.slot(top, 0, 0).is_empty());
	}
}
