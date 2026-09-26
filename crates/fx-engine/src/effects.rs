//! Layer-style effect tiles (M6-T08).
//!
//! Each effect of a styled layer has a derived cache (`Layer::effects`). A
//! tile the render thread asks for is computed here from the layer's alpha at
//! that level, over the tile plus an apron as wide as the effect reaches:
//! shift (shadows), dilate/erode through the exact distance transform, a
//! Gaussian approximated by three box blurs, and the stroke rings. The colour
//! is baked in; the compositor blends it with the effect's mode.
//!
//! Every content change of the document marks every effect cache dirty
//! ([`invalidate`]); only the visible tiles are recomputed.

use fx_core::styles::{EffectKind, EffectParams, StrokePosition};
use fx_core::{Document, LayerId, LayerKind};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

/// Mark every effect tile dirty (or rebuild the caches after a canvas size
/// change). FAST: called on every content change, whatever layer changed.
pub fn invalidate(doc: &mut Document) {
	let mut ids = Vec::new();
	doc.walk(|layer, _| {
		if layer.styles.is_some() {
			ids.push(layer.id);
		}
	});
	let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
	for id in ids {
		let Some(layer) = doc.layer_mut(id) else { continue };
		let resized = layer.effects.len() != EffectKind::ALL.len() || layer.effects.iter().any(|c| c.width() != w || c.height() != h || c.format() != format);
		if resized {
			layer.effects = EffectKind::ALL.iter().map(|_| TiledImage::derived(w, h, format)).collect();
		} else {
			for cache in &mut layer.effects {
				cache.mark_all_dirty();
			}
		}
	}
}

/// Compute the requested effect tiles `(layer, effect, level, tx, ty)`.
pub fn draw_effect_requests(doc: &mut Document, store: &TileStore, requests: &[(LayerId, u8, usize, u32, u32)]) -> usize {
	let global_light = doc.global_light;
	let t = TILE_SIZE as i64;
	// Gather the inputs first (drawing any source tile that is missing), then
	// compute in parallel.
	let mut jobs = Vec::new();
	for &(id, effect, level, tx, ty) in requests {
		let Some(kind) = EffectKind::from_index(effect as usize) else { continue };
		let Some(params) = doc.layer(id).and_then(|l| l.styles.as_ref()).and_then(|s| s.effect(kind, global_light)) else {
			// Disabled since the frame asked: leave the tile empty.
			if let Some(cache) = doc.layer_mut(id).and_then(|l| l.effects.get_mut(effect as usize)) {
				cache.set_derived_slot(level, tx, ty, TileSlot::Empty);
			}
			continue;
		};
		let scale = f64::from(1u32 << level);
		let reach = (fx_core::styles::LayerStyles::reach(&params) / scale).ceil() as i64 + 1;
		let (x0, y0) = (i64::from(tx) * t - reach, i64::from(ty) * t - reach);
		let side = (t + 2 * reach) as usize;
		let alpha = read_alpha(doc, store, id, level, (x0, y0), side);
		// Nothing to cast, glow or outline around here.
		if alpha.iter().all(|&a| a == 0.0) {
			if let Some(cache) = doc.layer_mut(id).and_then(|l| l.effects.get_mut(effect as usize)) {
				cache.set_derived_slot(level, tx, ty, TileSlot::Empty);
			}
			continue;
		}
		jobs.push((id, effect, level, tx, ty, params, scale, reach as usize, side, alpha));
	}
	let format = doc.color.depth.rgba_format();
	let tiles: Vec<_> = jobs
		.into_par_iter()
		.map(|(id, effect, level, tx, ty, params, scale, reach, side, alpha)| {
			let coverage = compute(&params, &alpha, side, reach, scale);
			let buffer = paint(&coverage, params.color, format);
			(id, effect, level, tx, ty, buffer)
		})
		.collect();
	let count = tiles.len();
	for (id, effect, level, tx, ty, buffer) in tiles {
		let slot = crate::vector::slot_for(buffer, format, store);
		if let Some(cache) = doc.layer_mut(id).and_then(|l| l.effects.get_mut(effect as usize)) {
			cache.set_derived_slot(level, tx, ty, slot);
		}
	}
	count
}

/// The layer's alpha at `level` over the square `side × side` region whose
/// top-left corner is `origin` (level pixels), 0..1, row-major.
fn read_alpha(doc: &mut Document, store: &TileStore, id: LayerId, level: usize, origin: (i64, i64), side: usize) -> Vec<f32> {
	let mut out = vec![0.0f32; side * side];
	let t = TILE_SIZE as i64;
	let scale = 1i64 << level;
	let Some(layer) = doc.layer(id) else { return out };
	let offset = match &layer.kind {
		LayerKind::Pixel { offset, .. } => {
			let off = |o: i32| (o as i64 * 2 + scale).div_euclid(scale * 2);
			(off(offset.0), off(offset.1))
		}
		LayerKind::Shape { .. } | LayerKind::Text { .. } => (0, 0),
		LayerKind::SolidFill { .. } => {
			// FAST: a fill layer is opaque over the canvas.
			let (w, h) = ((i64::from(doc.width) + scale - 1) / scale, (i64::from(doc.height) + scale - 1) / scale);
			for y in 0..side as i64 {
				for x in 0..side as i64 {
					let (dx, dy) = (origin.0 + x, origin.1 + y);
					if dx >= 0 && dy >= 0 && dx < w && dy < h {
						out[(y * side as i64 + x) as usize] = 1.0;
					}
				}
			}
			return out;
		}
		_ => return out,
	};
	let derived = layer.kind.is_derived();
	// The image tiles the region touches.
	let (ix0, iy0) = (origin.0 - offset.0, origin.1 - offset.1);
	let (ix1, iy1) = (ix0 + side as i64 - 1, iy0 + side as i64 - 1);
	let image = match &layer.kind {
		LayerKind::Pixel { image, .. } => image,
		LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } => cache,
		_ => return out,
	};
	let level = level.min(image.level_count() - 1);
	let grid = image.grid(level);
	let (cols, rows) = (grid.cols() as i64, grid.rows() as i64);
	let mut tiles = Vec::new();
	for gy in iy0.div_euclid(t).max(0)..=iy1.div_euclid(t).min(rows - 1) {
		for gx in ix0.div_euclid(t).max(0)..=ix1.div_euclid(t).min(cols - 1) {
			tiles.push((gx as u32, gy as u32));
		}
	}
	// Bring missing tiles up to date.
	let dirty: Vec<(u32, u32)> = tiles.iter().copied().filter(|&(gx, gy)| image.is_dirty(level, gx, gy)).collect();
	if !dirty.is_empty() {
		if derived {
			let requests: Vec<_> = dirty.iter().map(|&(gx, gy)| (id, level, gx, gy)).collect();
			crate::vector::draw_requests(doc, store, &requests);
		} else if let Some(layer) = doc.layer_mut(id)
			&& let LayerKind::Pixel { image, .. } = &mut layer.kind
		{
			for &(gx, gy) in &dirty {
				if let Err(error) = crate::mips::ensure_mip(image, store, level, gx, gy) {
					tracing::warn!("effect source mip failed: {error}");
				}
			}
		}
	}
	let Some(layer) = doc.layer(id) else { return out };
	let image = match &layer.kind {
		LayerKind::Pixel { image, .. } => image,
		LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } => cache,
		_ => return out,
	};
	for (gx, gy) in tiles {
		let (tx0, ty0) = (i64::from(gx) * t, i64::from(gy) * t);
		let read: Box<dyn Fn(usize) -> f32> = match image.slot(level, gx, gy) {
			TileSlot::Empty => continue,
			TileSlot::Solid(v) => {
				let a = f32::from(v.0[3]) / 65535.0;
				Box::new(move |_| a)
			}
			TileSlot::Data(handle) => match store.get(handle) {
				Ok(buffer) => match buffer.format() {
					PixelFormat::Rgba8 => Box::new(move |i| f32::from(buffer.bytes()[i * 4 + 3]) / 255.0),
					PixelFormat::Rgba16 => Box::new(move |i| f32::from(buffer.as_u16()[i * 4 + 3]) / 65535.0),
					_ => continue,
				},
				Err(_) => continue, // FAST: an unreadable tile is transparent
			},
		};
		// Overlap of the tile and the region, in image pixels.
		let (x_start, x_end) = (tx0.max(ix0), (tx0 + t - 1).min(ix1));
		let (y_start, y_end) = (ty0.max(iy0), (ty0 + t - 1).min(iy1));
		for y in y_start..=y_end {
			for x in x_start..=x_end {
				let src = ((y - ty0) * t + (x - tx0)) as usize;
				let dst = ((y - iy0) * side as i64 + (x - ix0)) as usize;
				out[dst] = read(src);
			}
		}
	}
	out
}

/// The effect's coverage over the output tile (`TILE_SIZE²`), from the alpha
/// of the region around it.
fn compute(params: &EffectParams, alpha: &[f32], side: usize, reach: usize, scale: f64) -> Vec<f32> {
	let at = |buf: &[f32], x: i64, y: i64| -> f32 {
		if x < 0 || y < 0 || x >= side as i64 || y >= side as i64 {
			0.0
		} else {
			buf[y as usize * side + x as usize]
		}
	};
	let off = ((params.offset.0 / scale).round() as i64, (params.offset.1 / scale).round() as i64);
	let morph = params.morph / scale;
	let blur = params.blur / scale;
	let full: Vec<f32> = match params.kind {
		EffectKind::DropShadow | EffectKind::OuterGlow => {
			let mut src: Vec<f32> = (0..side * side)
				.map(|i| at(alpha, (i % side) as i64 - off.0, (i / side) as i64 - off.1))
				.collect();
			if morph > 0.0 {
				src = dilate(&src, side, morph);
			}
			box_blur3(&src, side, blur / 2.0)
		}
		EffectKind::InnerShadow => {
			let mut src: Vec<f32> = (0..side * side)
				.map(|i| 1.0 - at(alpha, (i % side) as i64 - off.0, (i / side) as i64 - off.1))
				.collect();
			if morph > 0.0 {
				src = dilate(&src, side, morph);
			}
			let blurred = box_blur3(&src, side, blur / 2.0);
			blurred.iter().zip(alpha).map(|(s, a)| s * a).collect()
		}
		EffectKind::ColorOverlay => alpha.to_vec(),
		EffectKind::Stroke => {
			let (size, position) = params.stroke.unwrap_or((0.0, StrokePosition::Outside));
			let size = size / scale;
			let (outer, inner) = match position {
				StrokePosition::Outside => (size, 0.0),
				StrokePosition::Inside => (0.0, size),
				StrokePosition::Center => (size / 2.0, size / 2.0),
			};
			let mut cov = vec![0.0f32; side * side];
			if outer > 0.0 {
				let ring = dilate(alpha, side, outer);
				for i in 0..cov.len() {
					cov[i] += ring[i] * (1.0 - alpha[i]);
				}
			}
			if inner > 0.0 {
				let feature: Vec<bool> = alpha.iter().map(|&a| a < 0.5).collect();
				let d = fx_ops::morph::edt_2d(&feature, side, side);
				for i in 0..cov.len() {
					let band = (inner + 1.0 - d[i]).clamp(0.0, 1.0) as f32;
					cov[i] += band * alpha[i];
				}
			}
			cov.iter().map(|c| c.min(1.0)).collect()
		}
	};
	let t = TILE_SIZE as usize;
	let mut out = vec![0.0f32; t * t];
	for y in 0..t {
		let row = (y + reach) * side + reach;
		out[y * t..(y + 1) * t].copy_from_slice(&full[row..row + t]);
	}
	out
}

/// Grow the coverage by `radius` pixels (round), anti-aliased over 1 px.
fn dilate(src: &[f32], side: usize, radius: f64) -> Vec<f32> {
	let feature: Vec<bool> = src.iter().map(|&a| a >= 0.5).collect();
	let d = fx_ops::morph::edt_2d(&feature, side, side);
	src.iter().zip(d).map(|(&a, d)| a.max((radius + 1.0 - d).clamp(0.0, 1.0) as f32)).collect()
}

/// A Gaussian of `sigma` approximated by three box blurs (running sums, so
/// the cost does not grow with the radius).
fn box_blur3(src: &[f32], side: usize, sigma: f64) -> Vec<f32> {
	if sigma < 0.3 {
		return src.to_vec();
	}
	// Box width for three passes (Kovesi): w = sqrt(12σ²/3 + 1).
	let r = ((((12.0 * sigma * sigma / 3.0) + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
	let mut a = src.to_vec();
	let mut b = vec![0.0f32; src.len()];
	for _ in 0..3 {
		box_pass(&a, &mut b, side, r, true);
		box_pass(&b, &mut a, side, r, false);
	}
	a
}

fn box_pass(src: &[f32], dst: &mut [f32], side: usize, r: usize, horizontal: bool) {
	let norm = 1.0 / (2 * r + 1) as f32;
	for line in 0..side {
		let idx = |i: usize| if horizontal { line * side + i } else { i * side + line };
		let get = |i: i64| if i < 0 || i >= side as i64 { 0.0 } else { src[idx(i as usize)] };
		let mut acc: f32 = (-(r as i64)..=r as i64).map(get).sum();
		for i in 0..side {
			dst[idx(i)] = acc * norm;
			acc += get(i as i64 + r as i64 + 1) - get(i as i64 - r as i64);
		}
	}
}

/// A straight-alpha tile of `color` with `coverage` as its alpha.
fn paint(coverage: &[f32], color: [u16; 4], format: PixelFormat) -> TileBuffer {
	let mut buffer = TileBuffer::zeroed(format);
	let alpha = f32::from(color[3]) / 65535.0;
	match format {
		PixelFormat::Rgba16 => {
			let px = buffer.as_u16_mut();
			for (i, &c) in coverage.iter().enumerate() {
				let a = (c.clamp(0.0, 1.0) * alpha * 65535.0).round() as u16;
				if a == 0 {
					continue;
				}
				px[i * 4..i * 4 + 4].copy_from_slice(&[color[0], color[1], color[2], a]);
			}
		}
		_ => {
			let px = buffer.bytes_mut();
			let c8 = color.map(|v| (v / 257) as u8);
			for (i, &c) in coverage.iter().enumerate() {
				let a = (c.clamp(0.0, 1.0) * alpha * 255.0).round() as u8;
				if a == 0 {
					continue;
				}
				px[i * 4..i * 4 + 4].copy_from_slice(&[c8[0], c8[1], c8[2], a]);
			}
		}
	}
	buffer
}
