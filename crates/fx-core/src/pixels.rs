//! Pixel edits limited by the selection (M5-T05): Fill, Clear, the pixels a
//! copy or a "Layer via Copy" takes, a mask made from the selection, and the
//! blend of a filter's result through the selection.
//!
//! Everything works on a pixel layer's tiles in parallel, reading the
//! selection's coverage for the canvas block under each tile (the layer's
//! offset need not be tile-aligned). A tile whose pixels and coverage are
//! both uniform is answered with one computed pixel and stays a solid tile:
//! filling a 30 000² layer without a selection costs one pixel per tile.
//!
//! Pixels outside the canvas are never changed (Photoshop fills and clears
//! the canvas only).

use std::sync::Arc;

use fx_tiles::{PixelFormat, PixelValue, TILE_PIXELS, TILE_SIZE, TileBuffer, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

use crate::blend::{BlendMode, composite, unpremultiply};
use crate::selection::{Selection, TileCoverage, out_tile, uniform_slot};

/// Where a pixel layer sits: its image and the canvas pixel of its (0, 0).
#[derive(Clone, Copy)]
pub struct Placed<'a> {
	pub image: &'a TiledImage,
	pub offset: (i32, i32),
}

/// What Edit ▸ Fill paints (M5-T05).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillSpec {
	/// Straight 16-bit RGBA; the alpha scales the opacity.
	pub color: [u16; 4],
	pub mode: BlendMode,
	/// `0..=1`.
	pub opacity: f64,
	/// Keep each pixel's alpha (Photoshop's Preserve Transparency and the
	/// layer's transparency lock): the fill is composited source-atop.
	pub preserve_transparency: bool,
}

/// The selection coverage of the 256² canvas block whose top-left pixel is
/// `(x0, y0)`, 0 outside the canvas. Without a selection, 1 on the canvas.
pub fn block_coverage(selection: Option<&Selection>, store: &TileStore, canvas: (u32, u32), x0: i64, y0: i64) -> Result<TileCoverage, TileError> {
	let tile = i64::from(TILE_SIZE);
	let (cw, ch) = (i64::from(canvas.0), i64::from(canvas.1));
	let inside = x0 >= 0 && y0 >= 0 && x0 + tile <= cw && y0 + tile <= ch;
	if x0 + tile <= 0 || y0 + tile <= 0 || x0 >= cw || y0 >= ch {
		return Ok(TileCoverage::Uniform(0.0));
	}
	let aligned = x0.rem_euclid(tile) == 0 && y0.rem_euclid(tile) == 0;
	let coverage = match selection {
		None => TileCoverage::Uniform(1.0),
		Some(selection) if aligned => selection.tile_coverage(store, (x0 / tile) as u32, (y0 / tile) as u32)?,
		Some(selection) => {
			// Up to four canvas tiles under the block.
			let mut values = vec![0.0f32; TILE_PIXELS];
			let mut parts: std::collections::HashMap<(i64, i64), TileCoverage> = std::collections::HashMap::new();
			for y in 0..tile {
				let cy = y0 + y;
				if cy < 0 || cy >= ch {
					continue;
				}
				for x in 0..tile {
					let cx = x0 + x;
					if cx < 0 || cx >= cw {
						continue;
					}
					let key = (cx.div_euclid(tile), cy.div_euclid(tile));
					if let std::collections::hash_map::Entry::Vacant(entry) = parts.entry(key) {
						entry.insert(selection.tile_coverage(store, key.0 as u32, key.1 as u32)?);
					}
					values[(y * tile + x) as usize] = parts[&key].at(cx.rem_euclid(tile) as u32, cy.rem_euclid(tile) as u32);
				}
			}
			return Ok(TileCoverage::Data(values.into_boxed_slice()));
		}
	};
	if inside {
		return Ok(coverage);
	}
	// A block across the canvas edge: nothing outside the canvas.
	let mut values = coverage.into_values();
	for y in 0..tile {
		for x in 0..tile {
			let (cx, cy) = (x0 + x, y0 + y);
			if cx < 0 || cy < 0 || cx >= cw || cy >= ch {
				values[(y * tile + x) as usize] = 0.0;
			}
		}
	}
	Ok(TileCoverage::Data(values))
}

/// Straight RGBA `0..=1` of every pixel of a tile slot.
fn read_tile(slot: &TileSlot, format: PixelFormat, store: &TileStore) -> Result<Tile, TileError> {
	Ok(match slot {
		TileSlot::Empty => Tile::Uniform([0.0; 4]),
		TileSlot::Solid(v) => Tile::Uniform(straight(v.0)),
		TileSlot::Data(handle) => {
			let buffer = store.get(handle)?;
			Tile::Data(decode(&buffer, format))
		}
	})
}

/// A tile's pixels: one value or `TILE_SIZE²` straight RGBA.
enum Tile {
	Uniform([f32; 4]),
	Data(Vec<[f32; 4]>),
}

impl Tile {
	fn at(&self, i: usize) -> [f32; 4] {
		match self {
			Tile::Uniform(p) => *p,
			Tile::Data(pixels) => pixels[i],
		}
	}
}

fn straight(v: [u16; 4]) -> [f32; 4] {
	v.map(|c| f32::from(c) / 65535.0)
}

/// Every pixel of an RGBA tile as straight `0..=1`.
pub fn decode(buffer: &TileBuffer, format: PixelFormat) -> Vec<[f32; 4]> {
	match format {
		PixelFormat::Rgba16 => buffer
			.as_u16()
			.chunks_exact(4)
			.map(|p| [p[0], p[1], p[2], p[3]].map(|c| f32::from(c) / 65535.0))
			.collect(),
		_ => buffer
			.bytes()
			.chunks_exact(4)
			.map(|p| [p[0], p[1], p[2], p[3]].map(|c| f32::from(c) / 255.0))
			.collect(),
	}
}

/// Straight `0..=1` pixels back into a tile of `format`, rounded once.
pub fn encode(pixels: &[[f32; 4]], format: PixelFormat) -> TileBuffer {
	let mut buffer = TileBuffer::zeroed(format);
	match format {
		PixelFormat::Rgba16 => {
			for (out, p) in buffer.as_u16_mut().chunks_exact_mut(4).zip(pixels) {
				for (o, c) in out.iter_mut().zip(p) {
					*o = (c.clamp(0.0, 1.0) * 65535.0).round() as u16;
				}
			}
		}
		_ => {
			for (out, p) in buffer.bytes_mut().chunks_exact_mut(4).zip(pixels) {
				for (o, c) in out.iter_mut().zip(p) {
					*o = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
				}
			}
		}
	}
	buffer
}

/// The slot of a uniform straight pixel, quantised to `format`.
fn pixel_slot(format: PixelFormat, p: [f32; 4]) -> TileSlot {
	let scale = |c: f32| -> u16 {
		match format {
			PixelFormat::Rgba16 => (c.clamp(0.0, 1.0) * 65535.0).round() as u16,
			_ => (c.clamp(0.0, 1.0) * 255.0).round() as u16 * 257,
		}
	};
	let v = PixelValue([scale(p[0]), scale(p[1]), scale(p[2]), scale(p[3])]);
	if v.0[3] == 0 { TileSlot::Empty } else { TileSlot::Solid(v) }
}

/// A computed layer tile.
enum Out {
	Slot(TileSlot),
	Buffer(TileBuffer),
}

/// A computed tile and where it goes.
type Placement = ((u32, u32), Out);

/// Run `op(pixel, coverage)` over the layer's tiles in `tiles`, on rayon;
/// tiles `op` leaves alone keep their handles. Returns the new image.
fn map_layer(
	layer: Placed<'_>,
	tiles: &[(u32, u32)],
	selection: Option<&Selection>,
	canvas: (u32, u32),
	store: &TileStore,
	op: &(dyn Fn([f32; 4], f32) -> [f32; 4] + Sync),
) -> Result<TiledImage, TileError> {
	let format = layer.image.format();
	let tile = i64::from(TILE_SIZE);
	let results: Result<Vec<Option<Placement>>, TileError> = tiles
		.par_iter()
		.map(|&(lx, ly)| {
			let x0 = i64::from(layer.offset.0) + i64::from(lx) * tile;
			let y0 = i64::from(layer.offset.1) + i64::from(ly) * tile;
			let coverage = block_coverage(selection, store, canvas, x0, y0)?;
			// Unselected tiles keep their pixels (`extract` drops them after).
			if matches!(coverage, TileCoverage::Uniform(v) if v <= 0.0) {
				return Ok(None);
			}
			let pixels = read_tile(layer.image.slot(0, lx, ly), format, store)?;
			if let (Tile::Uniform(p), TileCoverage::Uniform(s)) = (&pixels, &coverage) {
				return Ok(Some(((lx, ly), Out::Slot(pixel_slot(format, op(*p, *s))))));
			}
			let out: Vec<[f32; 4]> = (0..TILE_PIXELS)
				.map(|i| op(pixels.at(i), coverage.at(i as u32 % TILE_SIZE, i as u32 / TILE_SIZE)))
				.collect();
			Ok(Some(((lx, ly), Out::Buffer(encode(&out, format)))))
		})
		.collect();
	let mut image = layer.image.clone();
	for ((lx, ly), out) in results?.into_iter().flatten() {
		match out {
			Out::Slot(slot) => image.set_slot(lx, ly, slot),
			Out::Buffer(buffer) => image.put_buffer(store, lx, ly, buffer),
		}
	}
	Ok(image)
}

/// The layer tiles that lie on the canvas, or only those under the
/// selection's bounds when there is one.
fn layer_tiles(layer: Placed<'_>, selection: Option<&Selection>, canvas: (u32, u32)) -> Vec<(u32, u32)> {
	let (x0, y0, x1, y1) = match selection {
		Some(selection) => match selection.canvas_bounds(canvas) {
			Some((x0, y0, x1, y1)) => (i64::from(x0), i64::from(y0), i64::from(x1), i64::from(y1)),
			None => return Vec::new(),
		},
		None => (0, 0, i64::from(canvas.0), i64::from(canvas.1)),
	};
	let tile = i64::from(TILE_SIZE);
	let (ox, oy) = (i64::from(layer.offset.0), i64::from(layer.offset.1));
	let cols = i64::from(layer.image.grid(0).cols());
	let rows = i64::from(layer.image.grid(0).rows());
	let lx0 = ((x0 - ox).div_euclid(tile)).max(0);
	let ly0 = ((y0 - oy).div_euclid(tile)).max(0);
	let lx1 = ((x1 - 1 - ox).div_euclid(tile)).min(cols - 1);
	let ly1 = ((y1 - 1 - oy).div_euclid(tile)).min(rows - 1);
	if lx1 < lx0 || ly1 < ly0 {
		return Vec::new();
	}
	(ly0..=ly1).flat_map(|ly| (lx0..=lx1).map(move |lx| (lx as u32, ly as u32))).collect()
}

/// A pixel layer grown in whole tiles so it covers the canvas: a fill or a
/// brush may paint anywhere on the canvas, also where a moved or pasted layer
/// has no pixels. Existing tiles keep their slots (they move in the grid, no
/// pixel is rewritten) except a partial last column or row that ends up
/// inside the grown image: its pixels past the old edge are cleared. Returns
/// the image and its new offset.
pub fn grow_to_canvas(image: &TiledImage, offset: (i32, i32), canvas: (u32, u32), store: &TileStore) -> Result<(TiledImage, (i32, i32)), TileError> {
	let tile = i64::from(TILE_SIZE);
	let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
	// Whole tiles to add on the left/top, so the tiles keep their grid.
	let add_x = if ox > 0 { (ox + tile - 1) / tile } else { 0 };
	let add_y = if oy > 0 { (oy + tile - 1) / tile } else { 0 };
	let (new_ox, new_oy) = (ox - add_x * tile, oy - add_y * tile);
	let (old_w, old_h) = (i64::from(image.width()), i64::from(image.height()));
	let width = (add_x * tile + old_w).max(i64::from(canvas.0) - new_ox);
	let height = (add_y * tile + old_h).max(i64::from(canvas.1) - new_oy);
	if add_x == 0 && add_y == 0 && width == old_w && height == old_h {
		return Ok((image.clone(), offset));
	}
	let format = image.format();
	let mut grown = TiledImage::new(width as u32, height as u32, format);
	for (tx, ty, slot) in image.grid(0).non_empty() {
		grown.set_slot(tx + add_x as u32, ty + add_y as u32, slot.clone());
	}
	// A partial last column/row now inside the image: clear past the old edge.
	let (cols, rows) = (image.grid(0).cols(), image.grid(0).rows());
	let cut_x = (width > add_x * tile + old_w && old_w % tile != 0).then_some((old_w % tile) as u32);
	let cut_y = (height > add_y * tile + old_h && old_h % tile != 0).then_some((old_h % tile) as u32);
	let bpp = format.bytes_per_pixel();
	for (tx, ty, slot) in image.grid(0).non_empty() {
		let x_cut = cut_x.filter(|_| tx == cols - 1);
		let y_cut = cut_y.filter(|_| ty == rows - 1);
		if x_cut.is_none() && y_cut.is_none() {
			continue;
		}
		let mut buffer = match slot {
			TileSlot::Solid(v) => TileBuffer::filled(format, *v),
			TileSlot::Data(handle) => (*store.get(handle)?).clone(),
			TileSlot::Empty => continue,
		};
		let row_bytes = TILE_SIZE as usize * bpp;
		let bytes = buffer.bytes_mut();
		for y in 0..TILE_SIZE as usize {
			let row = &mut bytes[y * row_bytes..(y + 1) * row_bytes];
			if y_cut.is_some_and(|h| y >= h as usize) {
				row.fill(0);
			} else if let Some(w) = x_cut {
				row[w as usize * bpp..].fill(0);
			}
		}
		grown.put_buffer(store, tx + add_x as u32, ty + add_y as u32, buffer);
	}
	Ok((grown, (new_ox as i32, new_oy as i32)))
}

/// Edit ▸ Fill (M5-T05) of a pixel layer, through the selection (the whole
/// canvas without one). The layer is grown to the canvas first, so the fill
/// reaches every selected canvas pixel.
pub fn fill(
	layer: Placed<'_>,
	selection: Option<&Selection>,
	canvas: (u32, u32),
	spec: &FillSpec,
	store: &TileStore,
) -> Result<(TiledImage, (i32, i32)), TileError> {
	let (grown, offset) = grow_to_canvas(layer.image, layer.offset, canvas, store)?;
	let placed = Placed { image: &grown, offset };
	let tiles = layer_tiles(placed, selection, canvas);
	let colour = [
		f64::from(spec.color[0]) / 65535.0,
		f64::from(spec.color[1]) / 65535.0,
		f64::from(spec.color[2]) / 65535.0,
	];
	let strength = spec.opacity.clamp(0.0, 1.0) * f64::from(spec.color[3]) / 65535.0;
	let op = |p: [f32; 4], s: f32| -> [f32; 4] {
		let a = f64::from(p[3]);
		let backdrop = [f64::from(p[0]) * a, f64::from(p[1]) * a, f64::from(p[2]) * a, a];
		let out = composite(spec.mode, backdrop, colour, strength * f64::from(s), spec.preserve_transparency);
		let rgb = unpremultiply(out);
		[rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, out[3] as f32]
	};
	Ok((map_layer(placed, &tiles, selection, canvas, store, &op)?, offset))
}

/// Edit ▸ Clear (M5-T05): each pixel's alpha × (1 − coverage).
pub fn clear(layer: Placed<'_>, selection: &Selection, canvas: (u32, u32), store: &TileStore) -> Result<TiledImage, TileError> {
	let tiles = layer_tiles(layer, Some(selection), canvas);
	map_layer(layer, &tiles, Some(selection), canvas, store, &|p, s| [p[0], p[1], p[2], p[3] * (1.0 - s)])
}

/// The selected pixels of a layer (Copy, Layer via Copy, M5-T05): alpha ×
/// coverage, everything else transparent, same size and offset as the layer.
pub fn extract(layer: Placed<'_>, selection: Option<&Selection>, canvas: (u32, u32), store: &TileStore) -> Result<TiledImage, TileError> {
	let tiles = layer_tiles(layer, selection, canvas);
	let taken = map_layer(layer, &tiles, selection, canvas, store, &|p, s| [p[0], p[1], p[2], p[3] * s])?;
	// Keep only what was under the selection: the untouched tiles go.
	let mut out = TiledImage::new(layer.image.width(), layer.image.height(), layer.image.format());
	let wanted: std::collections::HashSet<(u32, u32)> = tiles.into_iter().collect();
	let tile = i64::from(TILE_SIZE);
	for (lx, ly, slot) in taken.grid(0).non_empty() {
		if !wanted.contains(&(lx, ly)) {
			continue;
		}
		// Tiles whose coverage was zero were left as they were: drop them.
		let x0 = i64::from(layer.offset.0) + i64::from(lx) * tile;
		let y0 = i64::from(layer.offset.1) + i64::from(ly) * tile;
		if matches!(block_coverage(selection, store, canvas, x0, y0)?, TileCoverage::Uniform(v) if v <= 0.0) {
			continue;
		}
		out.set_slot(lx, ly, slot.clone());
	}
	Ok(out)
}

/// A layer mask from the selection (Layer ▸ Layer Mask ▸ Reveal/Hide
/// Selection, M5-T05): mask pixel `(x, y)` sits at canvas pixel
/// `origin + (x, y)`; `hide` inverts. Grey at `format`.
pub fn mask_from_selection(
	selection: &Selection,
	size: (u32, u32),
	origin: (i32, i32),
	canvas: (u32, u32),
	hide: bool,
	format: PixelFormat,
	store: &TileStore,
) -> Result<TiledImage, TileError> {
	let (cols, rows) = (size.0.div_ceil(TILE_SIZE), size.1.div_ceil(TILE_SIZE));
	let tile = i64::from(TILE_SIZE);
	let list: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let results: Result<Vec<_>, TileError> = list
		.par_iter()
		.map(|&(tx, ty)| {
			let coverage = block_coverage(
				Some(selection),
				store,
				canvas,
				i64::from(origin.0) + i64::from(tx) * tile,
				i64::from(origin.1) + i64::from(ty) * tile,
			)?;
			let valid = ((size.0 - tx * TILE_SIZE).min(TILE_SIZE), (size.1 - ty * TILE_SIZE).min(TILE_SIZE));
			Ok((
				(tx, ty),
				match coverage {
					TileCoverage::Uniform(v) => Err(if hide { 1.0 - v } else { v }),
					TileCoverage::Data(values) => {
						let values: Vec<f32> = if hide { values.iter().map(|v| 1.0 - v).collect() } else { values.into_vec() };
						match out_tile(format, &values, valid) {
							crate::selection::OutTile::Uniform(v) => Err(v),
							crate::selection::OutTile::Data(buffer) => Ok(buffer),
						}
					}
				},
			))
		})
		.collect();
	let mut image = TiledImage::new(size.0, size.1, format);
	for ((tx, ty), tile) in results? {
		match tile {
			Err(v) => image.set_slot(tx, ty, uniform_slot(format, v)),
			Ok(buffer) => image.put_buffer(store, tx, ty, buffer),
		}
	}
	Ok(image)
}

/// A filter's result limited by the selection (M5-T05): `before` where
/// nothing is selected, `after` where everything is, blended (premultiplied)
/// in between. Both images have the same geometry (a filter keeps it).
pub fn blend_through(before: Placed<'_>, after: &TiledImage, selection: &Selection, canvas: (u32, u32), store: &TileStore) -> Result<TiledImage, TileError> {
	let format = after.format();
	let tile = i64::from(TILE_SIZE);
	let cols = after.grid(0).cols();
	let rows = after.grid(0).rows();
	let list: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let empty = TileSlot::Empty;
	let results: Result<Vec<Option<Placement>>, TileError> = list
		.par_iter()
		.map(|&(tx, ty)| {
			let old = if tx < before.image.grid(0).cols() && ty < before.image.grid(0).rows() {
				before.image.slot(0, tx, ty)
			} else {
				&empty
			};
			let new = after.slot(0, tx, ty);
			if old.same_as(new) {
				return Ok(None);
			}
			let x0 = i64::from(before.offset.0) + i64::from(tx) * tile;
			let y0 = i64::from(before.offset.1) + i64::from(ty) * tile;
			let coverage = block_coverage(Some(selection), store, canvas, x0, y0)?;
			match coverage {
				TileCoverage::Uniform(v) if v >= 1.0 => return Ok(None),
				TileCoverage::Uniform(v) if v <= 0.0 => return Ok(Some(((tx, ty), Out::Slot(old.clone())))),
				_ => {}
			}
			let a = read_tile(old, format, store)?;
			let b = read_tile(new, format, store)?;
			let out: Vec<[f32; 4]> = (0..TILE_PIXELS)
				.map(|i| {
					let s = coverage.at(i as u32 % TILE_SIZE, i as u32 / TILE_SIZE);
					let (p, q) = (a.at(i), b.at(i));
					let alpha = p[3] + (q[3] - p[3]) * s;
					if alpha <= 0.0 {
						return [0.0; 4];
					}
					let mut c = [0.0f32; 4];
					for k in 0..3 {
						c[k] = (p[k] * p[3] + (q[k] * q[3] - p[k] * p[3]) * s) / alpha;
					}
					c[3] = alpha;
					c
				})
				.collect();
			Ok(Some(((tx, ty), Out::Buffer(encode(&out, format)))))
		})
		.collect();
	let mut image = after.clone();
	for ((tx, ty), out) in results?.into_iter().flatten() {
		match out {
			Out::Slot(slot) => image.set_slot(tx, ty, slot),
			Out::Buffer(buffer) => image.put_buffer(store, tx, ty, buffer),
		}
	}
	Ok(image)
}

/// A copy of `image` converted to `format` (Paste between 8- and 16-bit
/// documents, M5-T05): 16 → 8 rounds.
pub fn convert_depth(image: &TiledImage, format: PixelFormat, store: &TileStore) -> Result<TiledImage, TileError> {
	if image.format() == format {
		return Ok(image.clone());
	}
	let from = image.format();
	let tiles: Vec<(u32, u32, TileSlot)> = image.grid(0).non_empty().map(|(x, y, s)| (x, y, s.clone())).collect();
	let results: Result<Vec<_>, TileError> = tiles
		.par_iter()
		.map(|(tx, ty, slot)| {
			Ok((
				*tx,
				*ty,
				match slot {
					TileSlot::Data(handle) => Out::Buffer(encode(&decode(store.get(handle)?.as_ref(), from), format)),
					other => Out::Slot(match other {
						TileSlot::Solid(v) => pixel_slot(format, straight(v.0)),
						_ => TileSlot::Empty,
					}),
				},
			))
		})
		.collect();
	let mut out = TiledImage::new(image.width(), image.height(), format);
	for (tx, ty, tile) in results? {
		match tile {
			Out::Slot(slot) => out.set_slot(tx, ty, slot),
			Out::Buffer(buffer) => out.put_buffer(store, tx, ty, buffer),
		}
	}
	Ok(out)
}

/// What the clipboard holds (M5-T05, D-048): pixels that stay in the tile
/// store, placed at a canvas position of the document they came from.
#[derive(Clone, Debug)]
pub struct ClipboardImage {
	pub image: TiledImage,
	/// Canvas pixel of the image's (0, 0) in the source document.
	pub offset: (i32, i32),
	/// The bounds of the copied pixels, `(x0, y0, x1, y1)` in canvas pixels of
	/// the source document (where "Paste" centres, what "Paste in Place" keeps).
	pub bounds: (i32, i32, i32, i32),
}

/// Shared handle of the clipboard; the engine owns it, commands read it
/// through [`crate::PixelOps::clipboard`].
pub type SharedClipboard = Arc<std::sync::Mutex<Option<ClipboardImage>>>;

#[cfg(test)]
mod tests {
	use fx_tiles::TileStoreConfig;

	use super::*;
	use crate::color::BitDepth;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-core-pixels-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// A selection covering an ellipse, rasterised by supersampling (the real
	/// rasteriser lives in `fx-ops`; this is enough for coverage-weighted edges).
	fn ellipse(store: &TileStore, canvas: (u32, u32), cx: f64, cy: f64, r: f64) -> Selection {
		let mut selection = Selection::empty(canvas, BitDepth::U16);
		for ty in 0..canvas.1.div_ceil(TILE_SIZE) {
			for tx in 0..canvas.0.div_ceil(TILE_SIZE) {
				let mut buffer = TileBuffer::zeroed(PixelFormat::Gray16);
				for py in 0..TILE_SIZE {
					for px in 0..TILE_SIZE {
						let (x, y) = (f64::from(tx * TILE_SIZE + px), f64::from(ty * TILE_SIZE + py));
						let mut hits = 0;
						for j in 0..8 {
							for i in 0..8 {
								let (sx, sy) = (x + (f64::from(i) + 0.5) / 8.0, y + (f64::from(j) + 0.5) / 8.0);
								if (sx - cx).powi(2) + (sy - cy).powi(2) <= r * r {
									hits += 1;
								}
							}
						}
						crate::selection::set_gray(&mut buffer, PixelFormat::Gray16, px, py, hits as f32 / 64.0);
					}
				}
				selection.image.put_buffer(store, tx, ty, buffer);
			}
		}
		selection
	}

	fn pixel(image: &TiledImage, store: &TileStore, x: u32, y: u32) -> [f32; 4] {
		read_tile(image.slot(0, x / TILE_SIZE, y / TILE_SIZE), image.format(), store)
			.unwrap()
			.at(((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize)
	}

	fn red() -> FillSpec {
		FillSpec {
			color: [65535, 0, 0, 65535],
			mode: BlendMode::Normal,
			opacity: 1.0,
			preserve_transparency: false,
		}
	}

	#[test]
	fn a_fill_inside_an_ellipse_is_coverage_weighted_at_the_edge() {
		let store = store();
		let canvas = (300, 300);
		let mut image = TiledImage::new(300, 300, PixelFormat::Rgba16);
		for ty in 0..2 {
			for tx in 0..2 {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue::rgba16(65535, 65535, 65535, 65535)));
			}
		}
		let selection = ellipse(&store, canvas, 150.0, 150.0, 100.0);
		let (filled, offset) = fill(Placed { image: &image, offset: (0, 0) }, Some(&selection), canvas, &red(), &store).unwrap();
		assert_eq!(offset, (0, 0));
		assert_eq!(pixel(&filled, &store, 150, 150), [1.0, 0.0, 0.0, 1.0], "inside = the colour");
		assert_eq!(pixel(&filled, &store, 10, 10), [1.0, 1.0, 1.0, 1.0], "outside untouched");
		// On the edge: white blended with red by the coverage.
		let cov = selection.tile_coverage(&store, 0, 0).unwrap();
		let x = (150..256).find(|&x| (0.05..0.95).contains(&cov.at(x, 79))).expect("an edge pixel on row 79");
		let s = cov.at(x, 79);
		let p = pixel(&filled, &store, x, 79);
		assert!((p[1] - (1.0 - s)).abs() < 2.0 / 255.0, "green {p:?} vs coverage {s}");
	}

	#[test]
	fn a_fill_without_selection_keeps_solid_tiles_and_grows_the_layer() {
		let store = store();
		let canvas = (600, 300);
		// A small layer moved right: the fill must still reach the whole canvas.
		let image = TiledImage::new(100, 100, PixelFormat::Rgba8);
		let (filled, offset) = fill(
			Placed {
				image: &image,
				offset: (300, 40),
			},
			None,
			canvas,
			&red(),
			&store,
		)
		.unwrap();
		assert!(offset.0 <= 0 && offset.1 <= 0, "{offset:?}");
		let at = |x: i32, y: i32| pixel(&filled, &store, (x - offset.0) as u32, (y - offset.1) as u32);
		assert_eq!(at(0, 0), [1.0, 0.0, 0.0, 1.0]);
		assert_eq!(at(599, 299), [1.0, 0.0, 0.0, 1.0]);
		// A tile entirely on the canvas stays a solid tile (tiles across the
		// canvas edge hold data: nothing outside the canvas is filled).
		let inner = ((44 - offset.0) as u32 / TILE_SIZE, (40 - offset.1) as u32 / TILE_SIZE);
		assert!(
			matches!(filled.slot(0, inner.0, inner.1), TileSlot::Solid(_)),
			"{:?}",
			filled.slot(0, inner.0, inner.1)
		);
		assert_eq!(at(-1, 5)[3], 0.0, "outside the canvas stays transparent");
	}

	#[test]
	fn clear_removes_the_selected_part_of_a_partially_selected_tile() {
		let store = store();
		let canvas = (256, 256);
		let mut image = TiledImage::new(256, 256, PixelFormat::Rgba8);
		image.set_slot(0, 0, TileSlot::Solid(PixelValue::rgba8(0, 0, 255, 255)));
		let mut selection = Selection::empty(canvas, BitDepth::U8);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
		for y in 0..256 {
			for x in 0..128 {
				crate::selection::set_gray(&mut buffer, PixelFormat::Gray8, x, y, 1.0);
			}
			crate::selection::set_gray(&mut buffer, PixelFormat::Gray8, 128, y, 0.5);
		}
		selection.image.put_buffer(&store, 0, 0, buffer);
		let cleared = clear(Placed { image: &image, offset: (0, 0) }, &selection, canvas, &store).unwrap();
		assert_eq!(pixel(&cleared, &store, 10, 10)[3], 0.0);
		assert!((pixel(&cleared, &store, 128, 10)[3] - 0.5).abs() < 1.0 / 255.0);
		assert_eq!(pixel(&cleared, &store, 200, 10), [0.0, 0.0, 1.0, 1.0]);
	}

	#[test]
	fn extract_takes_only_the_selected_pixels_at_the_same_place() {
		let store = store();
		let canvas = (600, 300);
		let mut image = TiledImage::new(600, 300, PixelFormat::Rgba8);
		for ty in 0..2 {
			for tx in 0..3 {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue::rgba8(10, 20, 30, 255)));
			}
		}
		let selection = ellipse(&store, canvas, 100.0, 100.0, 50.0);
		let taken = extract(Placed { image: &image, offset: (0, 0) }, Some(&selection), canvas, &store).unwrap();
		assert_eq!(pixel(&taken, &store, 100, 100)[3], 1.0);
		assert_eq!(pixel(&taken, &store, 300, 100)[3], 0.0);
		assert!(matches!(taken.slot(0, 2, 0), TileSlot::Empty), "tiles far from the selection are not copied");
	}

	#[test]
	fn a_mask_from_the_selection_follows_the_layer_offset() {
		let store = store();
		let canvas = (512, 512);
		let selection = ellipse(&store, canvas, 300.0, 300.0, 40.0);
		let mask = mask_from_selection(&selection, (512, 512), (100, 100), canvas, false, PixelFormat::Gray16, &store).unwrap();
		let at = |image: &TiledImage, x: u32, y: u32| {
			let tile = crate::selection::Selection {
				image: image.clone(),
				offset: (0, 0),
			};
			tile.tile_coverage(&store, x / TILE_SIZE, y / TILE_SIZE)
				.unwrap()
				.at(x % TILE_SIZE, y % TILE_SIZE)
		};
		assert_eq!(at(&mask, 200, 200), 1.0, "canvas (300, 300) is mask (200, 200)");
		assert_eq!(at(&mask, 300, 300), 0.0);
		let hidden = mask_from_selection(&selection, (512, 512), (100, 100), canvas, true, PixelFormat::Gray16, &store).unwrap();
		assert_eq!(at(&hidden, 200, 200), 0.0);
		assert_eq!(at(&hidden, 10, 10), 1.0);
	}

	#[test]
	fn a_16_bit_image_pastes_into_8_bits_with_rounding() {
		let store = store();
		let mut image = TiledImage::new(300, 10, PixelFormat::Rgba16);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		// 32896 / 257 = 128.0 exactly; 32768 → 127.5 → rounds to 128.
		buffer.as_u16_mut()[..4].copy_from_slice(&[32896, 32768, 0, 65535]);
		image.put_buffer(&store, 0, 0, buffer);
		let eight = convert_depth(&image, PixelFormat::Rgba8, &store).unwrap();
		let TileSlot::Data(handle) = eight.slot(0, 0, 0) else { panic!() };
		assert_eq!(&store.get(handle).unwrap().bytes()[..4], &[128, 128, 0, 255]);
	}
}
