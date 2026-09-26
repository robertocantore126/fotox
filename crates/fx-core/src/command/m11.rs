//! M11's content-aware commands: Content-Aware Fill (T02), the Patch tool's
//! and Content-Aware Move's commits (T03, T04).
//!
//! PatchMatch runs on a **working grid**: the region of interest (the hole
//! plus the area to sample from) read tile by tile and box-averaged down so
//! it stays under [`BUDGET`] pixels. The result is written back into the hole
//! only, bilinearly upsampled when the grid was reduced. Nothing the size of
//! the document is allocated.

use std::collections::HashMap;

use fx_tiles::{TILE_PIXELS, TILE_SIZE};

use super::*;
use crate::selection::PatchReader;

/// Pixels PatchMatch works on at most (D-077's pixel budget).
/// FAST: 768² keeps the single-threaded NNF search at a few seconds; a
/// bigger hole is filled at a coarser scale and upsampled (blurry).
const BUDGET: i64 = 768 * 768;

type Px = [f32; 4];
type Rect = (i64, i64, i64, i64);

/// Where Content-Aware Fill puts its result (the workspace's Output To).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FillOutput {
	#[default]
	Current,
	NewLayer,
	Duplicate,
}

/// The working grid: canvas rectangle `(x0, y0)` + `w × h` cells of
/// `scale × scale` pixels.
#[derive(Clone, Copy, Debug)]
struct Grid {
	x0: i64,
	y0: i64,
	w: usize,
	h: usize,
	scale: i64,
}

impl Grid {
	fn over(rect: Rect) -> Grid {
		let (rw, rh) = (rect.2 - rect.0, rect.3 - rect.1);
		let mut scale = 1i64;
		while (rw / scale) * (rh / scale) > BUDGET {
			scale += 1;
		}
		Grid {
			x0: rect.0,
			y0: rect.1,
			w: (rw + scale - 1).div_euclid(scale).max(1) as usize,
			h: (rh + scale - 1).div_euclid(scale).max(1) as usize,
			scale,
		}
	}
	fn rect(&self) -> Rect {
		(self.x0, self.y0, self.x0 + self.w as i64 * self.scale, self.y0 + self.h as i64 * self.scale)
	}
}

/// The tight canvas bounds of the pixels `selection` covers (exclusive).
pub(super) fn coverage_bounds(selection: &Selection, canvas: (u32, u32), store: &TileStore) -> Result<Option<Rect>, CommandError> {
	let tile = i64::from(TILE_SIZE);
	let mut b: Option<Rect> = None;
	let mut add = |x0: i64, y0: i64, x1: i64, y1: i64| {
		b = Some(match b {
			None => (x0, y0, x1, y1),
			Some(o) => (o.0.min(x0), o.1.min(y0), o.2.max(x1), o.3.max(y1)),
		});
	};
	for (tx, ty) in selection.canvas_tiles(canvas) {
		let (bx, by) = (i64::from(tx) * tile, i64::from(ty) * tile);
		let (vw, vh) = crate::selection::valid_extent(canvas, tx, ty);
		match selection.tile_coverage(store, tx, ty)? {
			crate::selection::TileCoverage::Uniform(v) => {
				if v > 0.0 {
					add(bx, by, bx + i64::from(vw), by + i64::from(vh));
				}
			}
			crate::selection::TileCoverage::Data(values) => {
				let mut t = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
				for y in 0..vh as i64 {
					for x in 0..vw as i64 {
						if values[(y * tile + x) as usize] > 0.0 {
							t = [t[0].min(x), t[1].min(y), t[2].max(x), t[3].max(y)];
						}
					}
				}
				if t[0] <= t[2] {
					add(bx + t[0], by + t[1], bx + t[2] + 1, by + t[3] + 1);
				}
			}
		}
	}
	Ok(b)
}

fn clip(r: Rect, canvas: (u32, u32)) -> Rect {
	(r.0.max(0), r.1.max(0), r.2.min(i64::from(canvas.0)), r.3.min(i64::from(canvas.1)))
}

fn premul(p: Px) -> Px {
	[p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
}

fn unpremul(p: Px) -> Px {
	if p[3] <= 0.0 {
		[0.0; 4]
	} else {
		[(p[0] / p[3]).min(1.0), (p[1] / p[3]).min(1.0), (p[2] / p[3]).min(1.0), p[3].min(1.0)]
	}
}

/// A layer tile's straight pixels (`None` = transparent).
fn layer_tile(image: &TiledImage, tx: i64, ty: i64, store: &TileStore) -> Result<Option<Vec<Px>>, CommandError> {
	if tx < 0 || ty < 0 || tx >= i64::from(image.grid(0).cols()) || ty >= i64::from(image.grid(0).rows()) {
		return Ok(None);
	}
	Ok(match image.slot(0, tx as u32, ty as u32) {
		TileSlot::Empty => None,
		TileSlot::Solid(v) => Some(vec![v.0.map(|c| f32::from(c) / 65535.0); TILE_PIXELS]),
		TileSlot::Data(handle) => Some(crate::pixels::decode(store.get(handle)?.as_ref(), image.format())),
	})
}

/// The layer's premultiplied pixels box-averaged onto `grid`, and the share
/// of each cell's pixels that are on the canvas.
fn read_grid(image: &TiledImage, offset: (i32, i32), grid: &Grid, canvas: (u32, u32), store: &TileStore) -> Result<Vec<Px>, CommandError> {
	let tile = i64::from(TILE_SIZE);
	let r = clip(grid.rect(), canvas);
	let mut acc = vec![[0.0f32; 4]; grid.w * grid.h];
	let mut count = vec![0u32; grid.w * grid.h];
	for y in r.1..r.3 {
		for x in r.0..r.2 {
			count[((y - grid.y0) / grid.scale) as usize * grid.w + ((x - grid.x0) / grid.scale) as usize] += 1;
		}
	}
	let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
	// Layer tiles under the rectangle.
	for ty in (r.1 - oy).div_euclid(tile)..=(r.3 - 1 - oy).div_euclid(tile) {
		for tx in (r.0 - ox).div_euclid(tile)..=(r.2 - 1 - ox).div_euclid(tile) {
			let Some(pixels) = layer_tile(image, tx, ty, store)? else {
				continue;
			};
			for py in 0..tile {
				let cy = oy + ty * tile + py;
				if cy < r.1 || cy >= r.3 {
					continue;
				}
				for px in 0..tile {
					let cx = ox + tx * tile + px;
					if cx < r.0 || cx >= r.2 {
						continue;
					}
					let cell = ((cy - grid.y0) / grid.scale) as usize * grid.w + ((cx - grid.x0) / grid.scale) as usize;
					let p = premul(pixels[(py * tile + px) as usize]);
					for c in 0..4 {
						acc[cell][c] += p[c];
					}
				}
			}
		}
	}
	Ok(acc
		.iter()
		.zip(&count)
		.map(|(a, n)| if *n > 0 { a.map(|v| v / *n as f32) } else { [0.0; 4] })
		.collect())
}

/// The largest coverage of `selection` in each cell of `grid`.
fn grid_coverage(selection: &Selection, grid: &Grid, canvas: (u32, u32), store: &TileStore) -> Result<Vec<f32>, CommandError> {
	let tile = i64::from(TILE_SIZE);
	let r = clip(grid.rect(), canvas);
	let mut out = vec![0.0f32; grid.w * grid.h];
	let mut reader = PatchReader::new(selection, store, canvas);
	let mut y = r.1;
	while y < r.3 {
		let h = (tile - y.rem_euclid(tile)).min(r.3 - y);
		let mut x = r.0;
		while x < r.2 {
			let w = (tile - x.rem_euclid(tile)).min(r.2 - x);
			let patch = reader.patch(x, y, w as usize, h as usize)?;
			for j in 0..h {
				for i in 0..w {
					let cell = ((y + j - grid.y0) / grid.scale) as usize * grid.w + ((x + i - grid.x0) / grid.scale) as usize;
					out[cell] = out[cell].max(patch[(j * w + i) as usize]);
				}
			}
			x += w;
		}
		y += h;
	}
	Ok(out)
}

/// Bilinear sample of the grid's premultiplied pixels at canvas `(x, y)`
/// (pixel centres).
fn sample_grid(grid: &Grid, pixels: &[Px], x: i64, y: i64) -> Px {
	if grid.scale == 1 {
		let (gx, gy) = ((x - grid.x0) as usize, (y - grid.y0) as usize);
		return pixels[gy.min(grid.h - 1) * grid.w + gx.min(grid.w - 1)];
	}
	let s = grid.scale as f32;
	let fx = ((x - grid.x0) as f32 + 0.5) / s - 0.5;
	let fy = ((y - grid.y0) as f32 + 0.5) / s - 0.5;
	let (x0, y0) = (fx.floor(), fy.floor());
	let (ax, ay) = (fx - x0, fy - y0);
	let at = |xx: f32, yy: f32| -> Px {
		let xi = (xx.max(0.0) as usize).min(grid.w - 1);
		let yi = (yy.max(0.0) as usize).min(grid.h - 1);
		pixels[yi * grid.w + xi]
	};
	let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
	[0, 1, 2, 3].map(|k| (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay)
}

/// Write `value(x, y)` (premultiplied) into `image` wherever `selection`
/// covers `hole` (canvas rect), mixed by the coverage. The image covers the
/// canvas (grown by the caller).
fn write_through(
	image: &mut TiledImage,
	offset: (i32, i32),
	selection: &Selection,
	hole: Rect,
	canvas: (u32, u32),
	store: &TileStore,
	value: &dyn Fn(i64, i64) -> Px,
) -> Result<(), CommandError> {
	let tile = i64::from(TILE_SIZE);
	let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
	let format = image.format();
	let mut reader = PatchReader::new(selection, store, canvas);
	for ty in (hole.1 - oy).div_euclid(tile)..=(hole.3 - 1 - oy).div_euclid(tile) {
		for tx in (hole.0 - ox).div_euclid(tile)..=(hole.2 - 1 - ox).div_euclid(tile) {
			if tx < 0 || ty < 0 || tx >= i64::from(image.grid(0).cols()) || ty >= i64::from(image.grid(0).rows()) {
				continue;
			}
			let (bx, by) = (ox + tx * tile, oy + ty * tile);
			let coverage = reader.patch(bx, by, tile as usize, tile as usize)?;
			if coverage.iter().all(|c| *c <= 0.0) {
				continue;
			}
			let mut pixels = layer_tile(image, tx, ty, store)?.unwrap_or_else(|| vec![[0.0; 4]; TILE_PIXELS]);
			for (i, k) in coverage.iter().enumerate() {
				if *k <= 0.0 {
					continue;
				}
				let (cx, cy) = (bx + i as i64 % tile, by + i as i64 / tile);
				let before = premul(pixels[i]);
				let after = value(cx, cy);
				pixels[i] = unpremul([0, 1, 2, 3].map(|c| before[c] + (after[c] - before[c]) * k));
			}
			image.put_buffer(store, tx as u32, ty as u32, crate::pixels::encode(&pixels, format));
		}
	}
	Ok(())
}

/// Fill what `hole` selects on the layer by PatchMatch, sampling from
/// `sample` (a canvas rectangle; `None` = Auto, a band around the hole).
/// Returns the new image and offset (grown to the canvas), or `None` when
/// the selection covers nothing.
#[allow(clippy::too_many_arguments)]
pub(super) fn patch_fill_layer(
	image: &TiledImage,
	offset: (i32, i32),
	canvas: (u32, u32),
	hole: &Selection,
	sample: Option<Rect>,
	onto: Option<(&TiledImage, (i32, i32))>,
	seed: u64,
	ops: &dyn PixelOps,
	store: &TileStore,
) -> Result<Option<(TiledImage, (i32, i32))>, CommandError> {
	let Some(hb) = coverage_bounds(hole, canvas, store)? else {
		return Ok(None);
	};
	let size = (hb.2 - hb.0).max(hb.3 - hb.1);
	let roi = match sample {
		None => {
			// Auto: a band around the hole (VERIFY: Photoshop's auto area).
			let band = (size * 3 / 4).max(48);
			clip((hb.0 - band, hb.1 - band, hb.2 + band, hb.3 + band), canvas)
		}
		Some(s) => clip((hb.0.min(s.0) - 8, hb.1.min(s.1) - 8, hb.2.max(s.2) + 8, hb.3.max(s.3) + 8), canvas),
	};
	let grid = Grid::over(roi);
	let pixels = read_grid(image, offset, &grid, canvas, store)?;
	let coverage = grid_coverage(hole, &grid, canvas, store)?;
	let holes: Vec<bool> = coverage.iter().map(|c| *c > 0.0).collect();
	let on_canvas = clip(grid.rect(), canvas);
	let sampling: Vec<bool> = (0..grid.w * grid.h)
		.map(|i| {
			let (cx, cy) = (grid.x0 + (i % grid.w) as i64 * grid.scale, grid.y0 + (i / grid.w) as i64 * grid.scale);
			let inside = cx >= on_canvas.0 && cy >= on_canvas.1 && cx + grid.scale <= on_canvas.2 && cy + grid.scale <= on_canvas.3;
			let in_sample = sample.is_none_or(|s| cx >= s.0 && cy >= s.1 && cx < s.2 && cy < s.3);
			// FAST: transparent areas are never sampled.
			inside && in_sample && !holes[i] && pixels[i][3] > 0.99
		})
		.collect();
	if !sampling.iter().any(|s| *s) {
		return Err(CommandError::NotAllowed("there is nothing to sample around the selection".into()));
	}
	let filled = ops.patch_fill(&pixels, grid.w, grid.h, &holes, &sampling, seed)?;
	let (mut out, out_offset) = match onto {
		Some((image, offset)) => crate::pixels::grow_to_canvas(image, offset, canvas, store)?,
		None => crate::pixels::grow_to_canvas(image, offset, canvas, store)?,
	};
	write_through(&mut out, out_offset, hole, hb, canvas, store, &|x, y| sample_grid(&grid, &filled, x, y))?;
	Ok(Some((out, out_offset)))
}

/// Edit ▸ Content-Aware Fill / Edit ▸ Fill ▸ Content-Aware (M11-T02).
pub(super) fn content_aware_fill(
	doc: &mut Document,
	layer: &LayerRef,
	output: FillOutput,
	seed: u64,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("Content-Aware Fill needs a selection".into()));
	};
	let ops = pixel_ops(ctx, "Content-Aware Fill")?;
	let canvas = (doc.width, doc.height);
	let id = resolve(doc, layer)?;
	let (image, offset) = match &doc.layer(id).expect("resolved").kind {
		LayerKind::Pixel { image, offset } => (image.clone(), *offset),
		_ => return Err(CommandError::NotAllowed("the layer has no pixels; rasterise it first".into())),
	};
	if matches!(image.format(), PixelFormat::Gray8 | PixelFormat::Gray16) {
		// FAST: grey layers not handled.
		return Err(CommandError::NotAllowed("Content-Aware Fill needs colour pixels".into()));
	}
	let label = "Content-Aware Fill";
	match output {
		FillOutput::Current => {
			if doc.layer(id).expect("resolved").locked_pixels {
				return Err(CommandError::Locked(id));
			}
			let Some((new, new_offset)) = patch_fill_layer(&image, offset, canvas, &selection, None, None, seed, ops, ctx.tiles)? else {
				return Err(CommandError::NotAllowed("the selection is empty".into()));
			};
			set_pixels(doc, id, new, new_offset);
			Ok(CommandEffect {
				label: label.into(),
				pixels_changed: vec![id],
				..Default::default()
			})
		}
		FillOutput::NewLayer => {
			let blank = TiledImage::new(canvas.0, canvas.1, image.format());
			let Some((new, new_offset)) = patch_fill_layer(&image, offset, canvas, &selection, None, Some((&blank, (0, 0))), seed, ops, ctx.tiles)? else {
				return Err(CommandError::NotAllowed("the selection is empty".into()));
			};
			let name = doc.next_default_name(NameKind::Pixel);
			let new_id = doc.allocate_layer_id();
			doc.selected = vec![id];
			insert_above_active(
				doc,
				Arc::new(Layer::new(
					new_id,
					name,
					LayerKind::Pixel {
						image: new,
						offset: new_offset,
					},
				)),
			);
			doc.selected = vec![new_id];
			Ok(CommandEffect {
				label: label.into(),
				structure_changed: true,
				..Default::default()
			})
		}
		FillOutput::Duplicate => {
			let Some((new, new_offset)) = patch_fill_layer(&image, offset, canvas, &selection, None, None, seed, ops, ctx.tiles)? else {
				return Err(CommandError::NotAllowed("the selection is empty".into()));
			};
			duplicate_layers(doc, &[LayerRef::Id(id)])?;
			let copy = *doc.selected.first().expect("the copy is selected");
			set_pixels(doc, copy, new, new_offset);
			Ok(CommandEffect {
				label: label.into(),
				structure_changed: true,
				..Default::default()
			})
		}
	}
}

/// The canvas pixels a selection covers, read straight from a layer: for a
/// patch / move, the content under `selection` shifted by `(dx, dy)`.
fn shifted_reader<'a>(image: &'a TiledImage, offset: (i32, i32), store: &'a TileStore) -> impl FnMut(i64, i64) -> Px + 'a {
	let tile = i64::from(TILE_SIZE);
	let mut cache: HashMap<(i64, i64), Option<Vec<Px>>> = HashMap::new();
	move |x, y| {
		let (lx, ly) = (x - i64::from(offset.0), y - i64::from(offset.1));
		let key = (lx.div_euclid(tile), ly.div_euclid(tile));
		// FAST: unwrap-free but a missing tile reads transparent.
		let t = cache.entry(key).or_insert_with(|| layer_tile(image, key.0, key.1, store).ok().flatten());
		match t {
			Some(p) => premul(p[(ly.rem_euclid(tile) * tile + lx.rem_euclid(tile)) as usize]),
			None => [0.0; 4],
		}
	}
}

/// The Patch tool's commit (M11-T03): the selected area takes the content
/// `(dx, dy)` away (Source mode) — or, in Destination mode, the content under
/// the selection is copied `(dx, dy)` away. `content_aware` fills by
/// PatchMatch sampling only there; otherwise the healing blend (D-045).
pub(super) fn patch(
	doc: &mut Document,
	layer: &LayerRef,
	dx: i64,
	dy: i64,
	destination: bool,
	content_aware: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("the Patch tool needs a selection".into()));
	};
	let ops = pixel_ops(ctx, "Patch")?;
	let canvas = (doc.width, doc.height);
	let (id, image, offset) = pixel_target(doc, layer)?;
	// The hole and where its content comes from: in Destination mode the
	// hole is the selection moved by (dx, dy) and the source the selection.
	let (hole, (sx, sy)) = if destination {
		let mut moved = selection.clone();
		moved.offset = (selection.offset.0 + dx as i32, selection.offset.1 + dy as i32);
		(moved, (-dx, -dy))
	} else {
		(selection.clone(), (dx, dy))
	};
	let Some(hb) = coverage_bounds(&hole, canvas, ctx.tiles)? else {
		return Err(CommandError::NotAllowed("the selection is empty".into()));
	};
	let (new, new_offset) = if content_aware {
		let sample = (hb.0 + sx, hb.1 + sy, hb.2 + sx, hb.3 + sy);
		// FAST: Structure / Color options ignored; the sampled rectangle is the
		// patch's box at the source.
		match patch_fill_layer(&image, offset, canvas, &hole, Some(sample), None, 1, ops, ctx.tiles)? {
			Some(r) => r,
			None => return Err(CommandError::NotAllowed("the selection is empty".into())),
		}
	} else {
		// The healing blend over the hole's box plus a 2 px ring.
		let r = clip((hb.0 - 2, hb.1 - 2, hb.2 + 2, hb.3 + 2), canvas);
		let (w, h) = ((r.2 - r.0) as usize, (r.3 - r.1) as usize);
		if (w * h) as i64 > 4 * BUDGET {
			// FAST: big patches are refused rather than solved on a pyramid window.
			return Err(CommandError::NotAllowed("the patch is too large".into()));
		}
		let mut read = shifted_reader(&image, offset, ctx.tiles);
		let before: Vec<Px> = (0..w * h).map(|i| read(r.0 + (i % w) as i64, r.1 + (i / w) as i64)).collect();
		let source: Vec<Px> = (0..w * h).map(|i| read(r.0 + (i % w) as i64 + sx, r.1 + (i / w) as i64 + sy)).collect();
		let mut reader = PatchReader::new(&hole, ctx.tiles, canvas);
		let coverage = reader.patch(r.0, r.1, w, h)?;
		let healed = ops.heal_blend(&coverage, &before, &source, w, h)?;
		let (mut out, out_offset) = crate::pixels::grow_to_canvas(&image, offset, canvas, ctx.tiles)?;
		write_through(&mut out, out_offset, &hole, hb, canvas, ctx.tiles, &|x, y| {
			healed[((y - r.1) as usize).min(h - 1) * w + ((x - r.0) as usize).min(w - 1)]
		})?;
		(out, out_offset)
	};
	set_pixels(doc, id, new, new_offset);
	Ok(CommandEffect {
		label: "Patch Tool".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

/// Content-Aware Move's commit (M11-T04): the selected content is moved (or
/// copied, `extend`) by `(dx, dy)`; a move fills the old place by PatchMatch.
/// The selection follows the content.
pub(super) fn content_aware_move(
	doc: &mut Document,
	layer: &LayerRef,
	dx: i64,
	dy: i64,
	extend: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("Content-Aware Move needs a selection".into()));
	};
	let ops = pixel_ops(ctx, "Content-Aware Move")?;
	let canvas = (doc.width, doc.height);
	let (id, image, offset) = pixel_target(doc, layer)?;
	let Some(hb) = coverage_bounds(&selection, canvas, ctx.tiles)? else {
		return Err(CommandError::NotAllowed("the selection is empty".into()));
	};
	// 1. The old place: filled (Move) or kept (Extend).
	let (mut out, out_offset) = if extend {
		crate::pixels::grow_to_canvas(&image, offset, canvas, ctx.tiles)?
	} else {
		patch_fill_layer(&image, offset, canvas, &selection, None, None, 7, ops, ctx.tiles)?
			.ok_or_else(|| CommandError::NotAllowed("the selection is empty".into()))?
	};
	// 2. The content at the new place, through the moved selection.
	// FAST: composited by the selection's coverage; no edge blend (Structure /
	// Color), no Transform on Drop scaling.
	let mut moved = selection.clone();
	moved.offset = (selection.offset.0 + dx as i32, selection.offset.1 + dy as i32);
	let target = clip((hb.0 + dx, hb.1 + dy, hb.2 + dx, hb.3 + dy), canvas);
	if target.0 < target.2 && target.1 < target.3 {
		let read = std::sync::Mutex::new(shifted_reader(&image, offset, ctx.tiles));
		write_through(&mut out, out_offset, &moved, target, canvas, ctx.tiles, &|x, y| {
			(read.lock().expect("one thread"))(x - dx, y - dy)
		})?;
	}
	set_pixels(doc, id, out, out_offset);
	// The selection follows the content.
	if let Some(s) = doc.selection.as_mut() {
		s.offset = moved.offset;
	}
	Ok(CommandEffect {
		label: "Content-Aware Move".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

/// Pixels seam carving works on at most (D-080: a mip level for big images).
/// FAST: 640², seams are `scale` pixels wide at full resolution.
const SEAM_BUDGET: i64 = 640 * 640;

/// Edit ▸ Content-Aware Scale (M11-T05): the layer's image to `width ×
/// height`, `amount` (`0..=1`) of the change by seam carving and the rest by
/// a plain scale. `protect` = an alpha channel's index.
#[allow(clippy::too_many_arguments)]
pub(super) fn content_aware_scale(
	doc: &mut Document,
	layer: &LayerRef,
	width: u32,
	height: u32,
	amount: f64,
	protect: Option<usize>,
	protect_skin: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	use rayon::prelude::*;
	let ops = pixel_ops(ctx, "Content-Aware Scale")?;
	let (id, image, offset) = pixel_target(doc, layer)?;
	if width == 0 || height == 0 {
		return Err(CommandError::NotAllowed("the size must be at least one pixel".into()));
	}
	let (w, h) = (i64::from(image.width()), i64::from(image.height()));
	let mut scale = 1i64;
	while (w / scale) * (h / scale) > SEAM_BUDGET {
		scale += 1;
	}
	let grid = Grid {
		x0: 0,
		y0: 0,
		w: (w / scale).max(1) as usize,
		h: (h / scale).max(1) as usize,
		scale,
	};
	let layer_size = (image.width(), image.height());
	let pixels = read_grid(&image, (0, 0), &grid, layer_size, ctx.tiles)?;
	let mut protection = vec![0.0f32; grid.w * grid.h];
	if let Some(index) = protect {
		let channel = doc.channels.get(index).ok_or_else(|| CommandError::NotAllowed("no such channel".into()))?;
		let at = Grid {
			x0: i64::from(offset.0),
			y0: i64::from(offset.1),
			..grid
		};
		protection = grid_coverage(&channel.as_selection(), &at, (doc.width, doc.height), ctx.tiles)?;
	}
	if protect_skin {
		for (p, v) in pixels.iter().zip(protection.iter_mut()) {
			let q = unpremul(*p);
			// FAST: a crude RGB skin rule (VERIFY: Photoshop's skin detector).
			if q[0] > 0.35 && q[0] > q[1] && q[1] > q[2] && q[0] - q[2] > 0.08 && q[0] - q[1] < 0.35 {
				*v = v.max(1.0);
			}
		}
	}
	// Carve to the amount's share of the change, scale the rest.
	let carve_to = |from: i64, to: u32| -> usize {
		((from as f64 + amount.clamp(0.0, 1.0) * (f64::from(to) - from as f64)) / scale as f64)
			.round()
			.max(1.0) as usize
	};
	let (tw, th) = (carve_to(w, width), carve_to(h, height));
	let (aw, ah, map) = ops.seam_carve(&pixels, grid.w, grid.h, &protection, tw, th)?;
	// Output tiles: each pixel → carved full-res pixel → working cell → source.
	let format = image.format();
	let (cw, ch) = ((aw as i64 * scale) as f64, (ah as i64 * scale) as f64);
	let mut out = TiledImage::new(width, height, format);
	let tile = i64::from(TILE_SIZE);
	let keys: Vec<(u32, u32)> = (0..out.grid(0).rows()).flat_map(|ty| (0..out.grid(0).cols()).map(move |tx| (tx, ty))).collect();
	let buffers: Vec<((u32, u32), fx_tiles::TileBuffer)> = keys
		.par_iter()
		.map(|&(tx, ty)| -> Result<_, CommandError> {
			let mut cache: HashMap<(i64, i64), Option<Vec<Px>>> = HashMap::new();
			let mut pixels = vec![[0.0f32; 4]; TILE_PIXELS];
			for py in 0..tile {
				let oy = i64::from(ty) * tile + py;
				if oy >= i64::from(height) {
					break;
				}
				// FAST: nearest neighbour for the plain-scale part.
				let yc = (((oy as f64 + 0.5) * ch / f64::from(height)) as i64).clamp(0, ch as i64 - 1);
				for px in 0..tile {
					let ox = i64::from(tx) * tile + px;
					if ox >= i64::from(width) {
						break;
					}
					let xc = (((ox as f64 + 0.5) * cw / f64::from(width)) as i64).clamp(0, cw as i64 - 1);
					let (sx, sy) = map[(yc / scale) as usize * aw + (xc / scale) as usize];
					let x = (i64::from(sx) * scale + xc % scale).min(w - 1);
					let y = (i64::from(sy) * scale + yc % scale).min(h - 1);
					let key = (x / tile, y / tile);
					if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(key) {
						e.insert(layer_tile(&image, key.0, key.1, ctx.tiles)?);
					}
					if let Some(t) = &cache[&key] {
						pixels[(py * tile + px) as usize] = t[((y % tile) * tile + x % tile) as usize];
					}
				}
			}
			Ok(((tx, ty), crate::pixels::encode(&pixels, format)))
		})
		.collect::<Result<_, _>>()?;
	for ((tx, ty), buffer) in buffers {
		out.put_buffer(ctx.tiles, tx, ty, buffer);
	}
	// FAST: the layer mask is not scaled with the pixels.
	set_pixels(doc, id, out, offset);
	Ok(CommandEffect {
		label: "Content-Aware Scale".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}
