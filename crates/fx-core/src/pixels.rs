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

/// The box of the content one tile holds, in the coordinates of the image it
/// belongs to: `(x0, y0, x1, y1)`, exclusive. `None` when it holds none.
type TileContent = Option<(i64, i64, i64, i64)>;

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

/// What counts as content when a trim looks for the borders to remove
/// (M6-T03).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
	/// Anything not transparent: alpha ≠ 0 for the RGBA formats, grey ≠ 0 for
	/// the grey ones (a selection's coverage, a mask).
	Opaque,
	/// Anything whose pixels differ from `rgba` (straight 16-bit, the way the
	/// document stores them): Trim's corner colour.
	DifferentFrom([u16; 4]),
}

/// The exact rectangle `(x0, y0, x1, y1)` (exclusive, in canvas pixels) of the
/// content of a placed image, or `None` when it has none (M6-T03: Trim's
/// borders, the Crop menu item's selection bounds). Every non-empty tile
/// decides on its own, in parallel; a uniform tile is answered by its one
/// value, so an empty or solid 30 000² image costs one byte comparison per
/// tile.
pub fn content_bounds(layer: Placed<'_>, content: Content, store: &TileStore) -> Result<Option<(i32, i32, i32, i32)>, TileError> {
	let format = layer.image.format();
	let bpp = format.bytes_per_pixel();
	let channel_bytes = if format.has_alpha() { bpp / 4 } else { bpp };
	// The byte that carries "is there anything here": alpha, or a grey value.
	let alpha_at = if format.has_alpha() { channel_bytes * 3 } else { 0 };
	// `DifferentFrom` compares whole pixels; the wanted pixel is quantised to
	// the image's format the same way its tiles were stored.
	let wanted = match content {
		Content::Opaque => None,
		Content::DifferentFrom(rgba) => Some(TileBuffer::filled(format, PixelValue(rgba)).bytes()[..bpp].to_vec()),
	};
	let is_content = |pixel: &[u8]| match &wanted {
		None => pixel[alpha_at..alpha_at + channel_bytes].iter().any(|b| *b != 0),
		Some(wanted) => pixel != wanted.as_slice(),
	};
	let tile = i64::from(TILE_SIZE);
	let (ox, oy) = (i64::from(layer.offset.0), i64::from(layer.offset.1));
	let (iw, ih) = (i64::from(layer.image.width()), i64::from(layer.image.height()));
	let tiles: Vec<(u32, u32, TileSlot)> = layer.image.grid(0).non_empty().map(|(x, y, slot)| (x, y, slot.clone())).collect();
	let results: Result<Vec<TileContent>, TileError> = tiles
		.par_iter()
		.map(|(tx, ty, slot)| {
			let (x0, y0) = (ox + i64::from(*tx) * tile, oy + i64::from(*ty) * tile);
			// A partial last column or row only reaches the image's edge.
			let (vw, vh) = ((iw - i64::from(*tx) * tile).min(tile), (ih - i64::from(*ty) * tile).min(tile));
			let mut box_: TileContent = None;
			let mut hit = |x: i64, y: i64| {
				box_ = Some(match box_ {
					None => (x, y, x + 1, y + 1),
					Some(b) => (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1)),
				});
			};
			match slot {
				TileSlot::Empty => {}
				// One value for the whole tile: either all of it is content or
				// none of it is.
				TileSlot::Solid(value) => {
					let buffer = TileBuffer::filled(format, *value);
					if is_content(&buffer.bytes()[..bpp]) {
						hit(x0, y0);
						hit(x0 + vw - 1, y0 + vh - 1);
					}
				}
				TileSlot::Data(handle) => {
					let buffer = store.get(handle)?;
					for y in 0..vh {
						for x in 0..vw {
							let i = ((y * tile + x) * bpp as i64) as usize;
							if is_content(&buffer.bytes()[i..i + bpp]) {
								hit(x0 + x, y0 + y);
							}
						}
					}
				}
			}
			Ok(box_)
		})
		.collect();
	let mut bounds: TileContent = None;
	for tile in results?.into_iter().flatten() {
		bounds = Some(match bounds {
			None => tile,
			Some(b) => (b.0.min(tile.0), b.1.min(tile.1), b.2.max(tile.2), b.3.max(tile.3)),
		});
	}
	Ok(bounds.map(|(x0, y0, x1, y1)| (x0 as i32, y0 as i32, x1 as i32, y1 as i32)))
}

/// The same canvas content re-anchored (M6): `image` sits at canvas pixel
/// `from`; the result sits at `to` and holds, on every canvas pixel it covers,
/// what `image` held there. Where `image` had no pixel — the band between `to`
/// and `from` when the new origin lies up or left of the old one — the result
/// is `pad` (a mask's outside value). What lies up or left of `to` is dropped.
///
/// A mask has no offset of its own (a linked one sits at its layer's offset,
/// an unlinked one at the canvas origin), so a geometry command that moves a
/// mask's content to another origin needs this. A whole-tile shift moves tiles
/// without rewriting a pixel; otherwise every new tile is assembled from the
/// (at most four) old tiles under it, and a tile whose sources are all one
/// value stays one value.
pub fn place_at(image: &TiledImage, from: (i32, i32), to: (i32, i32), pad: PixelValue, store: &TileStore) -> Result<TiledImage, TileError> {
	if from == to {
		return Ok(image.clone());
	}
	place_in(image, from, to, None, pad, store)
}

/// [`place_at`] into an image of exactly `size` (`None`: up to where the old
/// image ended). What lies past the new size is dropped, what the old image
/// did not reach down or right of it is transparent (M6-T04: a floated
/// selection is cut out to its bounds before it is transformed).
pub fn place_in(
	image: &TiledImage,
	from: (i32, i32),
	to: (i32, i32),
	size: Option<(u32, u32)>,
	pad: PixelValue,
	store: &TileStore,
) -> Result<TiledImage, TileError> {
	let tile = i64::from(TILE_SIZE);
	let format = image.format();
	let bpp = format.bytes_per_pixel();
	// New pixel p holds old pixel p - shift.
	let shift = (i64::from(from.0) - i64::from(to.0), i64::from(from.1) - i64::from(to.1));
	let (old_w, old_h) = (i64::from(image.width()), i64::from(image.height()));
	let (width, height) = match size {
		Some((w, h)) => (i64::from(w.max(1)), i64::from(h.max(1))),
		None => ((old_w + shift.0).clamp(1, i64::from(u32::MAX)), (old_h + shift.1).clamp(1, i64::from(u32::MAX))),
	};
	let mut out = TiledImage::new(width as u32, height as u32, format);
	let pad_slot = if pad.is_transparent(format) || (!format.has_alpha() && pad.0[0] == 0) {
		TileSlot::Empty
	} else {
		TileSlot::Solid(pad)
	};
	let (old_cols, old_rows) = (i64::from(image.grid(0).cols()), i64::from(image.grid(0).rows()));
	let (cols, rows) = (out.grid(0).cols(), out.grid(0).rows());
	// The old slot at an old tile position: the pad up or left of the old
	// image, nothing down or right of it.
	let old_slot = |tx: i64, ty: i64| -> TileSlot {
		if tx < 0 || ty < 0 {
			pad_slot.clone()
		} else if tx >= old_cols || ty >= old_rows {
			TileSlot::Empty
		} else {
			image.slot(0, tx as u32, ty as u32).clone()
		}
	};
	// A whole-tile shift moves tiles, unless a partial last tile of the old
	// image would end up inside the new one (its pixels past the old edge are
	// not the old image's).
	let (last_x, last_y) = (old_w % tile != 0, old_h % tile != 0);
	let edge_inside = (last_x && width > old_w + shift.0) || (last_y && height > old_h + shift.1);
	if shift.0.rem_euclid(tile) == 0 && shift.1.rem_euclid(tile) == 0 && !edge_inside {
		let (sx, sy) = (shift.0 / tile, shift.1 / tile);
		for ty in 0..rows {
			for tx in 0..cols {
				out.set_slot(tx, ty, old_slot(i64::from(tx) - sx, i64::from(ty) - sy));
			}
		}
		return Ok(out);
	}
	let pad_pixel = TileBuffer::filled(format, pad).bytes()[..bpp].to_vec();
	let tiles: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let results: Result<Vec<Placement>, TileError> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			// The old pixel under this tile's top-left corner, and the (up to)
			// four old tiles the tile reads.
			let (ox, oy) = (i64::from(tx) * tile - shift.0, i64::from(ty) * tile - shift.1);
			let (otx, oty) = (ox.div_euclid(tile), oy.div_euclid(tile));
			let sources = [old_slot(otx, oty), old_slot(otx + 1, oty), old_slot(otx, oty + 1), old_slot(otx + 1, oty + 1)];
			if sources.iter().all(|s| s.same_as(&sources[0])) && !matches!(sources[0], TileSlot::Data(_)) {
				return Ok(((tx, ty), Out::Slot(sources[0].clone())));
			}
			let buffers = sources
				.iter()
				.map(|slot| match slot {
					TileSlot::Data(handle) => store.get(handle).map(|b| Some(b.bytes().to_vec())),
					TileSlot::Solid(value) => Ok(Some(TileBuffer::filled(format, *value).bytes().to_vec())),
					TileSlot::Empty => Ok(None),
				})
				.collect::<Result<Vec<_>, _>>()?;
			let mut buffer = TileBuffer::zeroed(format);
			let bytes = buffer.bytes_mut();
			for y in 0..tile {
				let old_y = oy + y;
				for x in 0..tile {
					let old_x = ox + x;
					let at = ((y * tile + x) as usize) * bpp;
					if old_x < 0 || old_y < 0 {
						bytes[at..at + bpp].copy_from_slice(&pad_pixel);
						continue;
					}
					if old_x >= old_w || old_y >= old_h {
						continue;
					}
					let which = usize::from(old_x.div_euclid(tile) > otx) + 2 * usize::from(old_y.div_euclid(tile) > oty);
					if let Some(source) = &buffers[which] {
						let from = ((old_y.rem_euclid(tile) * tile + old_x.rem_euclid(tile)) as usize) * bpp;
						bytes[at..at + bpp].copy_from_slice(&source[from..from + bpp]);
					}
				}
			}
			Ok(((tx, ty), Out::Buffer(buffer)))
		})
		.collect();
	for ((tx, ty), placed) in results? {
		match placed {
			Out::Slot(slot) => out.set_slot(tx, ty, slot),
			Out::Buffer(buffer) => out.put_buffer(store, tx, ty, buffer),
		}
	}
	Ok(out)
}

/// The straight RGBA pixels of the 256² canvas block at `(x0, y0)` of a placed
/// RGBA image; `None` when the block holds nothing of it.
fn read_block(layer: Placed<'_>, x0: i64, y0: i64, store: &TileStore) -> Result<Option<Tile>, TileError> {
	let tile = i64::from(TILE_SIZE);
	let format = layer.image.format();
	let (lx, ly) = (x0 - i64::from(layer.offset.0), y0 - i64::from(layer.offset.1));
	let (w, h) = (i64::from(layer.image.width()), i64::from(layer.image.height()));
	if lx >= w || ly >= h || lx + tile <= 0 || ly + tile <= 0 {
		return Ok(None);
	}
	let (tx0, ty0) = (lx.div_euclid(tile), ly.div_euclid(tile));
	let mut sources: Vec<Option<Tile>> = Vec::with_capacity(4);
	for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
		let (tx, ty) = (tx0 + dx, ty0 + dy);
		let inside = tx >= 0 && ty >= 0 && tx * tile < w && ty * tile < h;
		sources.push(if inside {
			Some(read_tile(layer.image.slot(0, tx as u32, ty as u32), format, store)?)
		} else {
			None
		});
	}
	let aligned = lx.rem_euclid(tile) == 0 && ly.rem_euclid(tile) == 0;
	if aligned && let Some(Some(Tile::Uniform(p))) = sources.first() {
		// The block is one tile, and a uniform one.
		return Ok(Some(Tile::Uniform(*p)));
	}
	let mut out = vec![[0.0; 4]; TILE_PIXELS];
	for y in 0..tile {
		for x in 0..tile {
			let (px, py) = (lx + x, ly + y);
			if px < 0 || py < 0 || px >= w || py >= h {
				continue;
			}
			let which = usize::from(px.div_euclid(tile) > tx0) + 2 * usize::from(py.div_euclid(tile) > ty0);
			if let Some(source) = &sources[which] {
				out[(y * tile + x) as usize] = source.at((py.rem_euclid(tile) * tile + px.rem_euclid(tile)) as usize);
			}
		}
	}
	Ok(Some(Tile::Data(out)))
}

/// `top` composited over `base` (Normal, full opacity), both placed RGBA
/// images of one format: the image covers both, at the union's origin (M6-T04:
/// a transformed selection is dropped back onto its layer). Blocks where
/// neither has a pixel stay empty.
pub fn over(base: Placed<'_>, top: Placed<'_>, store: &TileStore) -> Result<(TiledImage, (i32, i32)), TileError> {
	let format = base.image.format();
	let right = |p: Placed<'_>| i64::from(p.offset.0) + i64::from(p.image.width());
	let bottom = |p: Placed<'_>| i64::from(p.offset.1) + i64::from(p.image.height());
	let origin = (base.offset.0.min(top.offset.0), base.offset.1.min(top.offset.1));
	let width = (right(base).max(right(top)) - i64::from(origin.0)).clamp(1, i64::from(u32::MAX)) as u32;
	let height = (bottom(base).max(bottom(top)) - i64::from(origin.1)).clamp(1, i64::from(u32::MAX)) as u32;
	let mut out = TiledImage::new(width, height, format);
	let (cols, rows) = (out.grid(0).cols(), out.grid(0).rows());
	let tile = i64::from(TILE_SIZE);
	let tiles: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let results: Result<Vec<Option<Placement>>, TileError> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			let (x0, y0) = (i64::from(origin.0) + i64::from(tx) * tile, i64::from(origin.1) + i64::from(ty) * tile);
			let (under, above) = (read_block(base, x0, y0, store)?, read_block(top, x0, y0, store)?);
			let composite = |b: [f32; 4], t: [f32; 4]| -> [f32; 4] {
				let alpha = t[3] + b[3] * (1.0 - t[3]);
				if alpha <= 0.0 {
					return [0.0; 4];
				}
				let mut out = [0.0; 4];
				for c in 0..3 {
					out[c] = (t[c] * t[3] + b[c] * b[3] * (1.0 - t[3])) / alpha;
				}
				out[3] = alpha;
				out
			};
			Ok(match (under, above) {
				(None, None) => None,
				(Some(Tile::Uniform(b)), None) => Some(((tx, ty), Out::Slot(pixel_slot(format, b)))),
				(None, Some(Tile::Uniform(t))) => Some(((tx, ty), Out::Slot(pixel_slot(format, t)))),
				(Some(Tile::Uniform(b)), Some(Tile::Uniform(t))) => Some(((tx, ty), Out::Slot(pixel_slot(format, composite(b, t))))),
				(under, above) => {
					let pixels: Vec<[f32; 4]> = (0..TILE_PIXELS)
						.map(|i| {
							let b = under.as_ref().map_or([0.0; 4], |t| t.at(i));
							let t = above.as_ref().map_or([0.0; 4], |t| t.at(i));
							composite(b, t)
						})
						.collect();
					Some(((tx, ty), Out::Buffer(encode(&pixels, format))))
				}
			})
		})
		.collect();
	for ((tx, ty), placed) in results?.into_iter().flatten() {
		match placed {
			Out::Slot(slot) => out.set_slot(tx, ty, slot),
			Out::Buffer(buffer) => out.put_buffer(store, tx, ty, buffer),
		}
	}
	Ok((out, origin))
}

/// Keep only the pixels inside the canvas rectangle `rect` (M6-T03's Delete
/// Cropped Pixels): a tile entirely outside it goes, and the part of a tile
/// the rectangle crosses is cleared. The image keeps its size, format and
/// offset — nothing moves, so every surviving pixel is untouched and a tile
/// entirely inside keeps its handle. Rows are zeroed, which is the right
/// "empty" for every format (transparent RGBA, black grey).
pub fn clip_to_rect(layer: Placed<'_>, rect: (i32, i32, u32, u32), store: &TileStore) -> Result<TiledImage, TileError> {
	let (rx, ry) = (i64::from(rect.0), i64::from(rect.1));
	// Exclusive right and bottom, in canvas pixels.
	let (rx1, ry1) = (rx + i64::from(rect.2), ry + i64::from(rect.3));
	let tile = i64::from(TILE_SIZE);
	let format = layer.image.format();
	let bpp = format.bytes_per_pixel();
	let (ox, oy) = (i64::from(layer.offset.0), i64::from(layer.offset.1));
	let tiles: Vec<(u32, u32, TileSlot)> = layer.image.grid(0).non_empty().map(|(x, y, slot)| (x, y, slot.clone())).collect();
	let results: Result<Vec<Option<Placement>>, TileError> = tiles
		.par_iter()
		.map(|(tx, ty, slot)| {
			let (x0, y0) = (ox + i64::from(*tx) * tile, oy + i64::from(*ty) * tile);
			let (x1, y1) = (x0 + tile, y0 + tile);
			// The part of the tile that survives.
			let (kx0, ky0) = (x0.max(rx), y0.max(ry));
			let (kx1, ky1) = (x1.min(rx1), y1.min(ry1));
			if kx0 >= kx1 || ky0 >= ky1 {
				return Ok(None);
			}
			if (kx0, ky0, kx1, ky1) == (x0, y0, x1, y1) {
				return Ok(Some(((*tx, *ty), Out::Slot(slot.clone()))));
			}
			let mut buffer = match slot {
				TileSlot::Solid(value) => TileBuffer::filled(format, *value),
				TileSlot::Data(handle) => (*store.get(handle)?).clone(),
				TileSlot::Empty => return Ok(None),
			};
			let row_bytes = TILE_SIZE as usize * bpp;
			let bytes = buffer.bytes_mut();
			for (y, row) in bytes.chunks_exact_mut(row_bytes).enumerate() {
				let cy = y0 + y as i64;
				if cy < ry || cy >= ry1 {
					row.fill(0);
					continue;
				}
				let left = (rx - x0).clamp(0, tile) as usize * bpp;
				let right = (rx1 - x0).clamp(0, tile) as usize * bpp;
				row[..left].fill(0);
				row[right..].fill(0);
			}
			Ok(Some(((*tx, *ty), Out::Buffer(buffer))))
		})
		.collect();
	let mut out = TiledImage::new(layer.image.width(), layer.image.height(), format);
	for ((tx, ty), placement) in results?.into_iter().flatten() {
		match placement {
			Out::Slot(slot) => out.set_slot(tx, ty, slot),
			Out::Buffer(buffer) => out.put_buffer(store, tx, ty, buffer),
		}
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

	#[test]
	fn clip_to_rect_drops_the_outside_tiles_and_clears_the_crossed_ones() {
		let store = store();
		let mut image = TiledImage::new(1024, 1024, PixelFormat::Rgba8);
		for ty in 0..4 {
			for tx in 0..4 {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue::rgba8(10, 20, 30, 255)));
			}
		}
		// The layer sits at (10, 20), so tile (0, 0) is *crossed* by the
		// rectangle and tile (1, 1) is wholly inside it.
		let known = Placed {
			image: &image,
			offset: (10, 20),
		};
		let clipped = clip_to_rect(known, (256, 256, 512, 512), &store).unwrap();
		// `pixel` reads *image* pixels, and the layer sits at (10, 20).
		let inside = [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 1.0];
		assert_eq!(pixel(&clipped, &store, 250, 240), inside, "canvas (260, 260): just inside");
		assert_eq!(pixel(&clipped, &store, 290, 280), inside, "canvas (300, 300): well inside");
		assert_eq!(pixel(&clipped, &store, 245, 240)[3], 0.0, "canvas (255, 260): left of the rectangle");
		assert_eq!(pixel(&clipped, &store, 290, 235)[3], 0.0, "canvas (300, 255): above it");
		assert_eq!(pixel(&clipped, &store, 890, 880)[3], 0.0, "canvas (900, 900): far outside");
		assert!(matches!(clipped.slot(0, 1, 1), TileSlot::Solid(_)), "a tile inside keeps its handle");
		assert!(matches!(clipped.slot(0, 0, 0), TileSlot::Data(_)), "a crossed tile is rewritten once");
		assert!(matches!(clipped.slot(0, 3, 3), TileSlot::Empty), "a tile outside goes");
		assert_eq!(clipped.width(), 1024, "the image keeps its size");
	}

	#[test]
	fn place_at_keeps_every_canvas_pixel_where_it_was() {
		let store = store();
		let mut image = TiledImage::new(300, 280, PixelFormat::Gray8);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
		buffer.bytes_mut()[(5 * TILE_SIZE + 7) as usize] = 200;
		image.put_buffer(&store, 0, 0, buffer);
		image.set_slot(1, 1, TileSlot::Solid(PixelValue::gray16(40 * 257)));
		let gray = |image: &TiledImage, x: u32, y: u32| -> u8 {
			match image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
				TileSlot::Empty => 0,
				TileSlot::Solid(v) => (v.0[0] / 257) as u8,
				TileSlot::Data(h) => store.get(h).unwrap().bytes()[((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize],
			}
		};
		let pad = PixelValue::gray16(65_535);
		// The image sat at (100, 50); it now starts at (30, 20): canvas pixel
		// (107, 55) — old pixel (7, 5) — is new pixel (77, 35).
		let moved = place_at(&image, (100, 50), (30, 20), pad, &store).unwrap();
		assert_eq!((moved.width(), moved.height()), (370, 310));
		assert_eq!(gray(&moved, 77, 35), 200, "canvas (107, 55)");
		assert_eq!(gray(&moved, 78, 35), 0);
		assert_eq!(gray(&moved, 70 + 256 + 3, 30 + 256 + 3), 40, "inside the solid tile");
		assert_eq!(gray(&moved, 10, 10), 255, "the band the old image did not cover is the pad");
		assert_eq!(gray(&moved, 69, 100), 255, "left of the old image");
		assert_eq!(gray(&moved, 70, 100), 0, "its first column");
		// Back again: the original pixels (the pad band is gone).
		let back = place_at(&moved, (30, 20), (100, 50), pad, &store).unwrap();
		assert_eq!((back.width(), back.height()), (300, 280));
		for (x, y) in [(7, 5), (8, 5), (260, 260), (0, 0), (299, 279)] {
			assert_eq!(gray(&back, x, y), gray(&image, x, y), "({x}, {y})");
		}
		// A whole-tile shift moves the tiles themselves.
		let tiles = place_at(&image, (256, 0), (0, 0), pad, &store).unwrap();
		assert!(tiles.slot(0, 1, 0).same_as(image.slot(0, 0, 0)));
		assert!(matches!(tiles.slot(0, 0, 0), TileSlot::Solid(v) if v.0[0] == 65_535));
	}

	#[test]
	fn over_composites_a_placed_image_onto_another() {
		let store = store();
		let mut base = TiledImage::new(300, 300, PixelFormat::Rgba8);
		base.set_slot(0, 0, TileSlot::Solid(PixelValue::rgba8(200, 0, 0, 255)));
		base.set_slot(1, 1, TileSlot::Solid(PixelValue::rgba8(0, 0, 200, 255)));
		let mut top = TiledImage::new(100, 100, PixelFormat::Rgba8);
		top.set_slot(0, 0, TileSlot::Solid(PixelValue::rgba8(0, 255, 0, 128)));
		let (merged, origin) = over(
			Placed {
				image: &base,
				offset: (10, 10),
			},
			Placed {
				image: &top,
				offset: (-40, 200),
			},
			&store,
		)
		.unwrap();
		assert_eq!(origin, (-40, 10), "the union's corner");
		assert_eq!((merged.width(), merged.height()), (350, 300));
		// Canvas (50, 20): base only (red).
		assert_eq!(pixel(&merged, &store, 90, 10), [200.0 / 255.0, 0.0, 0.0, 1.0]);
		// Canvas (20, 250): half-transparent green over red.
		let p = pixel(&merged, &store, 60, 240);
		assert!((p[0] - 0.39).abs() < 0.01 && (p[1] - 0.5).abs() < 0.01 && p[3] == 1.0, "{p:?}");
		// Canvas (−30, 250): the green alone, left of the base.
		assert_eq!(pixel(&merged, &store, 10, 240)[3], 128.0 / 255.0);
		// Canvas (0, 20): nothing there.
		assert_eq!(pixel(&merged, &store, 40, 10)[3], 0.0);
		// Canvas (300, 300): the base's blue tile.
		assert_eq!(pixel(&merged, &store, 340, 290)[2], 200.0 / 255.0);
	}

	#[test]
	fn content_bounds_find_the_exact_rectangle_of_the_content() {
		let store = store();
		let mut image = TiledImage::new(600, 300, PixelFormat::Rgba8);
		// A tile filled with one colour, but for a single pixel.
		let mut buffer = TileBuffer::filled(PixelFormat::Rgba8, PixelValue::rgba8(1, 2, 3, 255));
		let at = |x: u32, y: u32| ((y * TILE_SIZE + x) * 4) as usize;
		buffer.bytes_mut()[at(200, 150)..at(200, 150) + 4].copy_from_slice(&[4, 5, 6, 200]);
		image.put_buffer(&store, 0, 0, buffer);
		image.set_slot(1, 0, TileSlot::Solid(PixelValue::rgba8(9, 9, 9, 255)));
		let placed = Placed {
			image: &image,
			offset: (100, 50),
		};
		assert_eq!(
			content_bounds(placed, Content::Opaque, &store).unwrap(),
			Some((100, 50, 612, 306)),
			"from the opaque tile's corner to the solid tile's far corner"
		);
		// The pixels that *are* the corner colour are not content (the value is
		// 16-bit, like every `PixelValue`, and quantises to the image's format).
		assert_eq!(
			content_bounds(placed, Content::DifferentFrom(PixelValue::rgba8(1, 2, 3, 255).0), &store).unwrap(),
			Some((300, 50, 612, 306)),
			"the background pixels and the solid tile: the painted one is not"
		);
		// Grey images read their value at byte 0 (a mask, a selection).
		let mut coverage = TiledImage::new(300, 300, PixelFormat::Gray8);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
		buffer.bytes_mut()[(10 * TILE_SIZE + 20) as usize] = 255;
		coverage.put_buffer(&store, 0, 0, buffer);
		assert_eq!(
			content_bounds(
				Placed {
					image: &coverage,
					offset: (0, 0)
				},
				Content::Opaque,
				&store
			)
			.unwrap(),
			Some((20, 10, 21, 11))
		);
	}
}
