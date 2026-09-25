//! Resampling core (M6-T01): the geometry of Free Transform, Warp, Crop's
//! straighten and Image Size, applied one destination tile at a time.
//!
//! Design (the card `docs/tasks/M6-T01.md`, `HOWTO.md` R1a/R8/R10,
//! `SNIPPETS.md` §2/§10/§11):
//!
//! * The sampler walks **destination → source** (never splats forwards), so it
//!   cannot leave holes and folds resolve to one source pixel per destination
//!   pixel.
//! * A destination tile fetches only the source tiles its mapped box touches,
//!   once each, into a small per-task cache. Memory is bounded by one tile plus
//!   its apron, never by the document (the One Rule).
//! * When the local scale (Jacobian, SNIPPETS §10) says the image is reduced
//!   by more than 2×, the samples come from the matching mip level — exact 2×2
//!   box averages — which removes aliasing at bounded cost. The caller makes
//!   that level valid first (`mips::ensure_mip`, like a filter's blur level).
//! * Sampling is **premultiplied**, and the accumulated colour is clamped to
//!   `0..=a` before un-premultiplying, so kernels with negative lobes cannot
//!   produce a dark fringe or a colour above 1 (SNIPPETS §2).

use fx_core::{Filter, Mapping};
use fx_tiles::{PixelFormat, TILE_PIXELS, TILE_SIZE, TileBuffer, TileError};
use rayon::prelude::*;

use crate::neighbourhood::{LevelSource, Px, TileRef, pixel, premul16, to_tile};

pub mod kernels;
pub mod mapping;
#[cfg(test)]
mod tests;
pub mod warp;

pub use mapping::Transform;

/// One resampled destination tile: its tile coordinates and its pixels.
pub type ResampledTile = ((u32, u32), TileBuffer);

/// Where the source image is and how deep its mip pyramid is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceInfo {
	/// Level-0 pixel size.
	pub size: (u32, u32),
	/// How many mip levels the **caller has made valid** — levels above this
	/// one are never read, even when the scale would ask for a deeper one. A
	/// caller that only ran `ensure_mip` for one level passes `2` (levels 0 and
	/// 1), so the sampler cannot read a stale derived tile. Never more levels
	/// than the image has.
	pub levels: usize,
}

impl SourceInfo {
	/// The pixel size of a mip level (rounded up, like `TiledImage`).
	pub fn level_size(&self, level: usize) -> (i64, i64) {
		let d = 1i64 << level;
		((i64::from(self.size.0) + d - 1) / d, (i64::from(self.size.1) + d - 1) / d)
	}
}

/// Resample the destination tiles `dst_tiles` (tile coordinates of `level`,
/// `level = 0` for full resolution) with `mapping` and `filter`.
///
/// `mapping` maps source image pixels → destination document pixels. The mip
/// levels the sampler may read must already be valid (the caller runs
/// `mips::ensure_mip` for them and reports how many in [`SourceInfo::levels`],
/// exactly as it prepares a filter's blur level). The returned tiles are in the
/// source's format, RGBA or gray, with straight (non-premultiplied) alpha.
pub fn resample(
	src: &dyn LevelSource,
	source: SourceInfo,
	mapping: Mapping,
	filter: Filter,
	level: usize,
	dst_tiles: &[(u32, u32)],
) -> Result<Vec<ResampledTile>, TileError> {
	let format = src.format();
	let transform = Transform::new(mapping);
	let results: Result<Vec<_>, TileError> = dst_tiles
		.par_iter()
		.map(|&(tx, ty)| resample_tile(src, &source, &transform, filter, level, tx, ty, format).map(|tile| ((tx, ty), tile)))
		.collect();
	results
}

/// The source mip level a destination tile reads from (SNIPPETS §10): the
/// smallest level that keeps at least ~one source pixel per destination pixel.
fn source_level(transform: &Transform, filter: Filter, level: usize, levels: usize, centre: (f64, f64)) -> usize {
	let max = levels.saturating_sub(1);
	if !filter.is_interpolating() {
		// Nearest does not average, so a mip would only cost sharpness.
		return level.min(max);
	}
	let per_destination_pixel = transform.scale_at(centre).max(1e-12);
	let effective = per_destination_pixel * (1u64 << level) as f64;
	let chosen = if effective > 2.0 { effective.log2().floor() as usize } else { level };
	chosen.min(max)
}

#[allow(clippy::too_many_arguments)]
fn resample_tile(
	src: &dyn LevelSource,
	source: &SourceInfo,
	transform: &Transform,
	filter: Filter,
	level: usize,
	tx: u32,
	ty: u32,
	format: PixelFormat,
) -> Result<TileBuffer, TileError> {
	let tile = i64::from(TILE_SIZE);
	let step = 1i64 << level;
	let x0 = i64::from(tx) * tile * step;
	let y0 = i64::from(ty) * tile * step;
	let dst_rect = [x0 as f64, y0 as f64, (x0 + tile * step) as f64, (y0 + tile * step) as f64];
	let centre = ((dst_rect[0] + dst_rect[2]) / 2.0, (dst_rect[1] + dst_rect[3]) / 2.0);
	// The local scale decides both the concrete filter (Bicubic Automatic) and
	// the source mip level.
	// Exact case (the card's shortcut): a whole-pixel translation at full
	// resolution is a pixel copy, whatever the filter.
	if level == 0
		&& let Some(offset) = transform.integer_translation()
	{
		return copy_tile(src, *source, offset, tx, ty, format);
	}
	let filter = filter.resolve(1.0 / transform.scale_at(centre).max(1e-12));
	let src_level = source_level(transform, filter, level, source.levels, centre);
	let Some(bounds) = transform.source_bounds(dst_rect) else {
		return Ok(TileBuffer::zeroed(format));
	};

	// The source box in level-`src_level` pixels, grown by the kernel support.
	let scale = (1i64 << src_level) as f64;
	let grow = filter.support().ceil() as i64 + 1;
	let rect = [
		(bounds[0] / scale).floor() as i64 - grow,
		(bounds[1] / scale).floor() as i64 - grow,
		(bounds[2] / scale).ceil() as i64 + grow + 1,
		(bounds[3] / scale).ceil() as i64 + grow + 1,
	];
	let grid = Grid::load(src, *source, src_level, format, rect)?;
	if grid.tiles.iter().all(Option::is_none) {
		// Nothing under this tile (a small object on a big layer, the corners
		// of a turned image): no pixel to sample.
		return Ok(TileBuffer::zeroed(format));
	}
	let candidates = transform.candidates(dst_rect);

	let mut out = vec![[0.0f32; 4]; TILE_PIXELS];
	for j in 0..tile {
		for i in 0..tile {
			let x = dst_rect[0] + (i as f64 + 0.5) * step as f64;
			let y = dst_rect[1] + (j as f64 + 0.5) * step as f64;
			let Some((u, v)) = transform.inverse_point(candidates.as_deref(), (x, y)) else {
				continue;
			};
			out[(j * tile + i) as usize] = sample(&grid, filter, u / scale, v / scale);
		}
	}
	Ok(to_tile(&out, format))
}

/// Copy one destination tile of a whole-pixel translation, exactly.
fn copy_tile(
	src: &dyn LevelSource,
	source: SourceInfo,
	(offset_x, offset_y): (i32, i32),
	tx: u32,
	ty: u32,
	format: PixelFormat,
) -> Result<TileBuffer, TileError> {
	let tile = i64::from(TILE_SIZE);
	let x0 = i64::from(tx) * tile;
	let y0 = i64::from(ty) * tile;
	let (sx0, sy0) = (x0 - i64::from(offset_x), y0 - i64::from(offset_y));
	let grid = Grid::load(src, source, 0, format, [sx0, sy0, sx0 + tile, sy0 + tile])?;
	let mut buffer = TileBuffer::zeroed(format);
	for j in 0..tile {
		for i in 0..tile {
			let p = grid.raw(sx0 + i, sy0 + j);
			if p == [0; 4] {
				continue;
			}
			let index = ((j * tile + i) * 4) as usize;
			match format {
				PixelFormat::Rgba16 => buffer.as_u16_mut()[index..index + 4].copy_from_slice(&p),
				PixelFormat::Rgba8 => {
					for (c, value) in p.iter().enumerate() {
						buffer.bytes_mut()[index + c] = (*value / 257) as u8;
					}
				}
				PixelFormat::Gray16 => buffer.as_u16_mut()[index / 4] = p[0],
				PixelFormat::Gray8 => buffer.bytes_mut()[index / 4] = (p[0] / 257) as u8,
			}
		}
	}
	Ok(buffer)
}

/// One output pixel: a weighted sum of the source taps, premultiplied.
fn sample(grid: &Grid, filter: Filter, u: f64, v: f64) -> Px {
	let mut wx = [(0i64, 0.0); kernels::MAX_TAPS];
	let mut wy = [(0i64, 0.0); kernels::MAX_TAPS];
	let nx = kernels::taps(filter, u, &mut wx);
	let ny = kernels::taps(filter, v, &mut wy);
	let mut acc = [0.0f64; 4];
	for &(iy, weight_y) in &wy[..ny] {
		for &(ix, weight_x) in &wx[..nx] {
			let weight = weight_x * weight_y;
			if weight == 0.0 {
				continue;
			}
			let p = grid.get(ix, iy);
			for c in 0..4 {
				acc[c] += weight * f64::from(p[c]);
			}
		}
	}
	let alpha = acc[3].clamp(0.0, 1.0) as f32;
	if alpha <= 0.0 {
		return [0.0; 4];
	}
	// Negative lobes can overshoot: clamp the premultiplied colour to 0..=a.
	let mut out = [0.0f32; 4];
	for c in 0..3 {
		out[c] = (acc[c].clamp(0.0, f64::from(alpha)) as f32 / alpha).clamp(0.0, 1.0);
	}
	out[3] = alpha;
	out
}

/// The source tiles a destination tile reads, fetched once each.
pub(crate) struct Grid {
	format: PixelFormat,
	tx0: i64,
	ty0: i64,
	cols: i64,
	rows: i64,
	tiles: Vec<Option<TileRef>>,
	/// Level pixel size of the source image.
	size: (i64, i64),
}

impl Grid {
	pub(crate) fn load(src: &dyn LevelSource, source: SourceInfo, level: usize, format: PixelFormat, rect: [i64; 4]) -> Result<Self, TileError> {
		let tile = i64::from(TILE_SIZE);
		let size = source.level_size(level);
		let max_tx = ((size.0 - 1).div_euclid(tile)).max(0);
		let max_ty = ((size.1 - 1).div_euclid(tile)).max(0);
		let tx0 = rect[0].div_euclid(tile).clamp(0, max_tx);
		let ty0 = rect[1].div_euclid(tile).clamp(0, max_ty);
		let tx1 = (rect[2] - 1).div_euclid(tile).clamp(0, max_tx);
		let ty1 = (rect[3] - 1).div_euclid(tile).clamp(0, max_ty);
		let (cols, rows) = (tx1 - tx0 + 1, ty1 - ty0 + 1);
		let mut tiles = Vec::with_capacity((cols * rows) as usize);
		for ty in ty0..=ty1 {
			for tx in tx0..=tx1 {
				tiles.push(src.tile(level, tx, ty)?);
			}
		}
		Ok(Self {
			format,
			tx0,
			ty0,
			cols,
			rows,
			tiles,
			size,
		})
	}

	/// The straight 16-bit pixel at a level pixel position.
	///
	/// Outside the image an RGBA source is transparent (a layer has no content
	/// there) but a gray one is the nearest edge pixel: gray images (masks,
	/// selections) have no transparency, and D-036's rule outside the canvas is
	/// edge replicate. Without it, resampling a mask would darken its border.
	pub(crate) fn raw(&self, x: i64, y: i64) -> [u16; 4] {
		if x < 0 || y < 0 || x >= self.size.0 || y >= self.size.1 {
			if self.format.has_alpha() {
				return [0; 4];
			}
			return self.raw(x.clamp(0, self.size.0 - 1), y.clamp(0, self.size.1 - 1));
		}
		let tile = i64::from(TILE_SIZE);
		let (tx, ty) = (x.div_euclid(tile), y.div_euclid(tile));
		let (cx, cy) = (tx - self.tx0, ty - self.ty0);
		if cx < 0 || cy < 0 || cx >= self.cols || cy >= self.rows {
			return [0; 4];
		}
		match &self.tiles[(cy * self.cols + cx) as usize] {
			None => [0; 4],
			Some(TileRef::Solid(value)) => *value,
			Some(TileRef::Data(buffer)) => pixel(buffer, self.format, x.rem_euclid(tile) as usize, y.rem_euclid(tile) as usize),
		}
	}

	/// The premultiplied pixel at a level pixel position; transparent outside
	/// (an RGBA source), the edge pixel (a gray one) — see [`Grid::raw`].
	fn get(&self, x: i64, y: i64) -> Px {
		premul16(self.raw(x, y))
	}
}
