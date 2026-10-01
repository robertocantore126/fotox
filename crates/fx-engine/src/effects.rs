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

use fx_core::styles::{BevelStyle, EffectExtra, EffectKind, EffectParams, GlobalLight, StrokePosition};
use fx_core::{Document, LayerId, LayerKind};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileSlot, TileStore};
use rayon::prelude::*;

/// Mark every effect tile dirty (or rebuild the caches after a canvas size
/// change, or when the style's list of effects changed). FAST: called on
/// every content change, whatever layer changed.
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
		let Some(styles) = &layer.styles else { continue };
		let slots = styles.slots().len();
		let resized = layer.effects.len() != slots || layer.effects.iter().any(|c| c.width() != w || c.height() != h || c.format() != format);
		if resized {
			layer.effects = styles.caches(w, h, format);
		} else {
			for cache in &mut layer.effects {
				cache.mark_all_dirty();
			}
		}
	}
}

/// Compute the requested effect tiles `(layer, effect, level, tx, ty)`;
/// `effect` indexes the layer's [`fx_core::styles::LayerStyles::slots`].
pub fn draw_effect_requests(doc: &mut Document, store: &TileStore, requests: &[(LayerId, u8, usize, u32, u32)]) -> usize {
	let light = GlobalLight {
		angle: doc.global_light,
		altitude: doc.global_altitude,
	};
	let t = TILE_SIZE as i64;
	let mut bounds_of: std::collections::HashMap<LayerId, Option<(f64, f64, f64, f64)>> = std::collections::HashMap::new();
	let resolved: Vec<_> = requests
		.iter()
		.map(|&(id, effect, level, tx, ty)| {
			let styles = doc.layer(id).and_then(|l| l.styles.clone());
			let params = styles.as_ref().and_then(|s| {
				let slot = *s.slots().get(effect as usize)?;
				s.effect_at(slot, light)
			});
			let reach = params
				.as_ref()
				.map_or(0, |p| (fx_core::styles::LayerStyles::reach(p) / f64::from(1u32 << level)).ceil() as i64 + 1);
			(id, effect, level, tx, ty, styles, params, reach)
		})
		.collect();
	// A layer drawn alone (a group, or a mask that shapes the effects) is
	// composited once per tile, at the widest reach its effects there need.
	let mut alone = AloneAlpha::default();
	for (id, _, level, tx, ty, _, params, reach) in &resolved {
		if params.is_some() {
			let widest = alone.reach.entry((*id, *level, *tx, *ty)).or_insert(0);
			*widest = (*widest).max(*reach);
		}
	}
	// Gather the inputs first (drawing any source tile that is missing), then
	// compute in parallel.
	let mut jobs = Vec::new();
	for (id, effect, level, tx, ty, styles, params, reach) in resolved {
		let (Some(styles), Some(params)) = (styles, params) else {
			// Disabled since the frame asked: leave the tile empty.
			if let Some(cache) = doc.layer_mut(id).and_then(|l| l.effects.get_mut(effect as usize)) {
				cache.set_derived_slot(level, tx, ty, TileSlot::Empty);
			}
			continue;
		};
		let scale = f64::from(1u32 << level);
		let side = (t + 2 * reach) as usize;
		let alpha = source_alpha(doc, store, &mut alone, id, &styles, (level, tx, ty), reach);
		// Nothing to cast, glow or outline around here.
		if alpha.iter().all(|&a| a == 0.0) {
			if let Some(cache) = doc.layer_mut(id).and_then(|l| l.effects.get_mut(effect as usize)) {
				cache.set_derived_slot(level, tx, ty, TileSlot::Empty);
			}
			continue;
		}
		// Pattern Overlay's pattern (M12-T04), a pattern-filled stroke's, or
		// Bevel ▸ Texture's, looked up once.
		let pattern = match &params.extra {
			EffectExtra::Pattern { id: pid, .. } => doc.patterns.iter().find(|p| p.id == *pid).cloned(),
			EffectExtra::Bevel { texture: Some(t), .. } => doc.patterns.iter().find(|p| p.id == t.pattern).cloned(),
			_ => None,
		};
		// Align with Layer / Link with Layer: the layer's box, once per layer.
		let bounds = if params.extra.aligned() {
			*bounds_of.entry(id).or_insert_with(|| layer_bounds(doc, store, id))
		} else {
			None
		};
		jobs.push((id, effect, level, tx, ty, params, scale, reach as usize, side, alpha, pattern, bounds));
	}
	let format = doc.color.depth.rgba_format();
	let size = (doc.width, doc.height);
	let tiles: Vec<_> = jobs
		.into_par_iter()
		.map(|(id, effect, level, tx, ty, params, scale, reach, side, alpha, pattern, bounds)| {
			// Document point of the alpha window's first pixel centre.
			let t = TILE_SIZE as f64;
			let origin = ((f64::from(tx) * t - reach as f64 + 0.5) * scale, (f64::from(ty) * t - reach as f64 + 0.5) * scale);
			let texture = Texture { pattern: pattern.as_ref(), origin, bounds };
			let mut coverage = compute(&params, &alpha, side, reach, scale, &texture);
			finish(&mut coverage, &params, &alpha, side, reach);
			let q = params.quality;
			if q.noise > 0.0 {
				add_noise(&mut coverage, q.noise, (tx, ty));
			}
			let buffer = match &params.extra {
				// A glow's gradient: coloured along its falloff (the coverage).
				EffectExtra::Glow { gradient: Some((g, reverse)), .. } => paint_glow(&coverage, format, g, *reverse, q.jitter, (tx, ty)),
				// Per-pixel colour: the gradient / pattern at the document point.
				EffectExtra::Gradient { gradient: g, .. } => {
					let placed = match bounds {
						Some(b) => g.placed_in(b),
						None => g.placed(size),
					};
					paint_with(&coverage, format, |x, y| placed.color_at_point(x, y, x as i64, y as i64), (tx, ty), scale)
				}
				EffectExtra::Pattern { scale: s, angle, phase, .. } => match &pattern {
					Some(p) => {
						let k = (s / 100.0).max(0.01);
						let origin = bounds.map_or((0.0, 0.0), |b| (b.0, b.1));
						let (ox, oy) = (origin.0 + phase.0, origin.1 + phase.1);
						// Counter-clockwise on screen (y down): turn the point back.
						let a = angle.to_radians();
						let (c, sn) = (a.cos(), a.sin());
						paint_with(
							&coverage,
							format,
							|x, y| {
								let (dx, dy) = (x - ox, y - oy);
								p.sample((dx * c - dy * sn) / k, (dx * sn + dy * c) / k)
							},
							(tx, ty),
							scale,
						)
					}
					None => paint(&coverage, [32_768, 32_768, 32_768, 65_535], format),
				},
				_ => paint(&coverage, params.color, format),
			};
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

/// The alpha an effect is drawn from. Photoshop draws a layer's effects
/// from its masked shape unless *Layer / Vector Mask Hides Effects* is on
/// (then the mask cuts the finished effect instead, in the compositor):
/// with a mask that shapes the effects, the layer is composited alone with
/// that mask; a group from its content alone; otherwise its own alpha.
fn source_alpha(doc: &mut Document, store: &TileStore, alone: &mut AloneAlpha, id: LayerId, styles: &fx_core::styles::LayerStyles, (level, tx, ty): (usize, u32, u32), reach: i64) -> Vec<f32> {
	let t = TILE_SIZE as i64;
	let side = (t + 2 * reach) as usize;
	let origin = (i64::from(tx) * t - reach, i64::from(ty) * t - reach);
	let Some(layer) = doc.layer(id) else { return vec![0.0; side * side] };
	let pixel = layer.mask.as_ref().is_some_and(|m| m.enabled) && !styles.layer_mask_hides;
	let vector = layer.vector_mask.as_ref().is_some_and(|m| m.enabled) && !styles.vector_mask_hides;
	if matches!(layer.kind, LayerKind::Group { .. }) || pixel || vector {
		return alone.get(doc, store, (id, pixel, vector), (level, tx, ty), reach);
	}
	read_alpha(doc, store, id, level, origin, side)
}

/// The alpha of layers drawn alone, for one [`draw_effect_requests`] call:
/// the stripped document is built once per layer (and mask choice), and each
/// tile is composited once, at the widest reach any of its effects asked
/// for; narrower effects take the middle of that window.
#[derive(Default)]
struct AloneAlpha {
	/// The widest reach asked for, per `(layer, level, tx, ty)`.
	reach: std::collections::HashMap<(LayerId, usize, u32, u32), i64>,
	docs: std::collections::HashMap<(LayerId, bool, bool), Document>,
	/// The composited windows: `(reach, alpha)`.
	tiles: std::collections::HashMap<(LayerId, bool, bool, usize, u32, u32), (i64, Vec<f32>)>,
}

impl AloneAlpha {
	fn get(&mut self, doc: &Document, store: &TileStore, (id, pixel, vector): (LayerId, bool, bool), (level, tx, ty): (usize, u32, u32), reach: i64) -> Vec<f32> {
		let t = TILE_SIZE as i64;
		let key = (id, pixel, vector, level, tx, ty);
		if !self.tiles.contains_key(&key) {
			let wide = self.reach.get(&(id, level, tx, ty)).copied().unwrap_or(reach).max(reach);
			let sub = self.docs.entry((id, pixel, vector)).or_insert_with(|| alone_doc(doc, id, pixel, vector));
			let side = (t + 2 * wide) as usize;
			let origin = (i64::from(tx) * t - wide, i64::from(ty) * t - wide);
			self.tiles.insert(key, (wide, alone_alpha(sub, store, level, origin, side)));
		}
		let (wide, alpha) = &self.tiles[&key];
		let (wide_side, side) = ((t + 2 * wide) as usize, (t + 2 * reach) as usize);
		if wide_side == side {
			return alpha.clone();
		}
		let skip = (wide - reach) as usize;
		let mut out = Vec::with_capacity(side * side);
		for row in alpha.chunks_exact(wide_side).skip(skip).take(side) {
			out.extend_from_slice(&row[skip..skip + side]);
		}
		out
	}
}

/// The finish of a shadow's, glow's or satin's falloff: the glow's Range,
/// the Contour (anti-aliased or not), and Layer Knocks Out Drop Shadow.
/// `coverage` is the output tile; `alpha` the layer's alpha over the tile
/// and its apron (`reach` wide).
fn finish(coverage: &mut [f32], params: &EffectParams, alpha: &[f32], side: usize, reach: usize) {
	let q = params.quality;
	let glow = matches!(params.kind, EffectKind::OuterGlow | EffectKind::InnerGlow);
	// Range: the part of the falloff the contour spans. Scaled so that the
	// default 50 % is the falloff as computed. VERIFY against Photoshop.
	let k = if glow { 0.5 / q.range.max(0.01) } else { 1.0 };
	if k != 1.0 || q.contour != fx_core::styles::Contour::Linear {
		for c in coverage.iter_mut() {
			if *c > 0.0 {
				let t = (f64::from(*c) * k).min(1.0);
				*c = q.contour.apply_smooth(t, q.anti_aliased) as f32;
			}
		}
	}
	if params.kind == EffectKind::DropShadow && q.knocks_out {
		let t = TILE_SIZE as usize;
		for y in 0..t {
			for x in 0..t {
				let a = alpha[(y + reach) * side + x + reach];
				coverage[y * t + x] *= 1.0 - a;
			}
		}
	}
}

/// A glow painted with its gradient: each pixel's colour is the gradient at
/// its place in the falloff (the left stop at the edge, where the glow is
/// strongest), scattered by Jitter; the alpha is the gradient's, faded in
/// over the glow's last reach. VERIFY the fade against Photoshop.
fn paint_glow(coverage: &[f32], format: PixelFormat, gradient: &fx_core::gradient::Gradient, reverse: bool, jitter: f64, (tx, ty): (u32, u32)) -> TileBuffer {
	let t = TILE_SIZE as usize;
	let mut buffer = TileBuffer::zeroed(format);
	for (i, &c) in coverage.iter().enumerate() {
		if c <= 0.0 {
			continue;
		}
		let mut pos = 1.0 - f64::from(c.min(1.0));
		if jitter > 0.0 {
			let (x, y) = (tx as usize * t + i % t, ty as usize * t + i / t);
			let mut h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
			h ^= h >> 15;
			h = h.wrapping_mul(0x2c1b_3c6d);
			h ^= h >> 12;
			pos += (f64::from(h & 0xffff) / 65_535.0 - 0.5) * jitter;
		}
		let pos = pos.clamp(0.0, 1.0);
		let rgba = gradient.eval(if reverse { 1.0 - pos } else { pos });
		let fade = (f64::from(c) / 0.05).min(1.0);
		let a = rgba[3] * fade;
		match format {
			PixelFormat::Rgba16 => {
				let px = buffer.as_u16_mut();
				for k in 0..3 {
					px[i * 4 + k] = (rgba[k].clamp(0.0, 1.0) * 65535.0).round() as u16;
				}
				px[i * 4 + 3] = (a.clamp(0.0, 1.0) * 65535.0).round() as u16;
			}
			_ => {
				let px = buffer.bytes_mut();
				for k in 0..3 {
					px[i * 4 + k] = (rgba[k].clamp(0.0, 1.0) * 255.0).round() as u8;
				}
				px[i * 4 + 3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
			}
		}
	}
	buffer
}

/// A layer's content box at level 0, `(x, y, w, h)` in document pixels —
/// what Align with Layer places a gradient or pattern over. Shapes and Smart
/// Objects from their geometry; pixel layers from their tiles; text from its
/// drawn pixels (FAST: a whole-layer render per invalidation).
fn layer_bounds(doc: &Document, store: &TileStore, id: LayerId) -> Option<(f64, f64, f64, f64)> {
	use fx_core::pixels::{Content, Placed, content_bounds};
	let layer = doc.layer(id)?;
	let rect = |b: (i32, i32, i32, i32)| (f64::from(b.0), f64::from(b.1), f64::from(b.2 - b.0), f64::from(b.3 - b.1));
	match &layer.kind {
		LayerKind::Pixel { image, offset } => content_bounds(Placed { image, offset: *offset }, Content::Opaque, store).ok().flatten().map(rect),
		LayerKind::Shape { shape, stroke, transform, .. } => {
			let (w, h) = shape.bounds();
			let [a, b, c, d, e, f] = *transform;
			let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| (a * x + c * y + e, b * x + d * y + f));
			let grow = stroke.as_ref().map_or(0.0, |s| s.width / 2.0);
			let (x0, x1) = corners.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.0), m.1.max(p.0)));
			let (y0, y1) = corners.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.1), m.1.max(p.1)));
			Some((x0 - grow, y0 - grow, x1 - x0 + 2.0 * grow, y1 - y0 + 2.0 * grow))
		}
		LayerKind::Smart { smart, .. } => smart.bounds().map(|((x, y), (w, h))| (f64::from(x), f64::from(y), f64::from(w), f64::from(h))),
		LayerKind::Text { .. } => {
			let drawn = crate::derived::layer_content(doc, store, id).ok()?;
			content_bounds(Placed { image: &drawn, offset: (0, 0) }, Content::Opaque, store).ok().flatten().map(rect)
		}
		_ => None,
	}
}

/// Layer `id` alone: drawn Normal at full opacity and fill, without its own
/// styles (as Photoshop computes a group's effects), keeping its pixel mask
/// with `pixel_mask` and its vector mask with `vector_mask`.
fn alone_doc(doc: &Document, id: LayerId, pixel_mask: bool, vector_mask: bool) -> Document {
	let wanted: std::collections::HashSet<LayerId> = std::iter::once(id).collect();
	let mut sub = doc.clone();
	sub.layers = crate::export::keep_layers(&doc.layers, &wanted);
	// The group itself: no styles (no recursion into its own effects), Normal,
	// opaque, no mask; its enclosing groups likewise.
	fn plain(layers: &mut [std::sync::Arc<fx_core::Layer>], target: LayerId, keep: (bool, bool)) {
		for layer in layers {
			let layer = std::sync::Arc::make_mut(layer);
			let own = layer.id == target;
			layer.styles = None;
			layer.effects = Vec::new();
			layer.opacity = 1.0;
			layer.fill = 1.0;
			layer.visible = true;
			layer.clipped = false;
			if !(own && keep.0) {
				layer.mask = None;
			}
			if !(own && keep.1) {
				layer.vector_mask = None;
			}
			layer.blend = fx_core::BlendMode::Normal;
			// Down the path only: the group's own children are its content.
			if !own && let LayerKind::Group { children, .. } = &mut layer.kind {
				plain(children, target, keep);
			}
		}
	}
	plain(&mut sub.layers, id, (pixel_mask, vector_mask));
	sub
}

/// The alpha of `sub` (an [`alone_doc`]) over the region.
fn alone_alpha(sub: &mut Document, store: &TileStore, level: usize, origin: (i64, i64), side: usize) -> Vec<f32> {
	let rect = (origin.0, origin.1, origin.0 + side as i64, origin.1 + side as i64);
	match crate::ai::composite_rect(sub, store, level, rect) {
		Ok(pixels) => pixels.iter().map(|p| p[3]).collect(),
		Err(error) => {
			tracing::warn!("a group's style source failed: {error}");
			vec![0.0; side * side]
		}
	}
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
		LayerKind::Shape { .. } | LayerKind::Text { .. } | LayerKind::Smart { .. } | LayerKind::FillLayer { .. } => (0, 0),
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
		LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } | LayerKind::Smart { cache, .. } | LayerKind::FillLayer { cache, .. } => cache,
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
	// Bring missing tiles up to date: dirty ones, and derived ones the trim
	// dropped (code review 2026-09-27 R01; read as transparent, they would
	// bake a hole into the effect).
	let dirty: Vec<(u32, u32)> = tiles
		.iter()
		.copied()
		.filter(|&(gx, gy)| image.is_dirty(level, gx, gy) || matches!(image.slot(level, gx, gy), TileSlot::Data(h) if store.is_evicted(h)))
		.collect();
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
		LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } | LayerKind::Smart { cache, .. } | LayerKind::FillLayer { cache, .. } => cache,
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
/// What Bevel ▸ Texture needs to sample its pattern: the pattern, the
/// document point of the alpha window's first pixel, the layer's box (Link
/// with Layer).
struct Texture<'a> {
	pattern: Option<&'a fx_core::pattern::Pattern>,
	origin: (f64, f64),
	bounds: Option<(f64, f64, f64, f64)>,
}

fn compute(params: &EffectParams, alpha: &[f32], side: usize, reach: usize, scale: f64, texture: &Texture<'_>) -> Vec<f32> {
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
		// Precise: the exact distance to the edge, so corners stay sharp
		// (Softer blurs the matte).
		EffectKind::OuterGlow if params.quality.precise => {
			let feature: Vec<bool> = alpha.iter().map(|&a| a >= 0.5).collect();
			let d = fx_ops::morph::edt_2d(&feature, side, side);
			alpha.iter().zip(d).map(|(&a, d)| a.max(precise_falloff(d, morph, blur))).collect()
		}
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
		EffectKind::ColorOverlay | EffectKind::GradientOverlay | EffectKind::PatternOverlay => alpha.to_vec(),
		// M12-T04: Inner Glow (edge source) is an inner shadow without offset;
		// from the centre it is the rest of the inside.
		EffectKind::InnerGlow => {
			let center = matches!(params.extra, EffectExtra::Glow { center: true, .. });
			let edge: Vec<f32> = if params.quality.precise {
				let feature: Vec<bool> = alpha.iter().map(|&a| a < 0.5).collect();
				fx_ops::morph::edt_2d(&feature, side, side).into_iter().map(|d| precise_falloff(d, morph, blur)).collect()
			} else {
				let mut src: Vec<f32> = alpha.iter().map(|a| 1.0 - a).collect();
				if morph > 0.0 {
					src = dilate(&src, side, morph);
				}
				box_blur3(&src, side, blur / 2.0)
			};
			edge.iter().zip(alpha).map(|(s, a)| if center { (1.0 - s).max(0.0) * a } else { s * a }).collect()
		}
		EffectKind::Satin => {
			let (o, invert) = match &params.extra {
				EffectExtra::Satin { offset, invert } => (((offset.0 / scale).round() as i64, (offset.1 / scale).round() as i64), *invert),
				_ => ((0, 0), false),
			};
			let shifted = |dx: i64, dy: i64| -> Vec<f32> { (0..side * side).map(|i| at(alpha, (i % side) as i64 - dx, (i / side) as i64 - dy)).collect() };
			let a = box_blur3(&shifted(o.0, o.1), side, blur / 2.0);
			let b = box_blur3(&shifted(-o.0, -o.1), side, blur / 2.0);
			(0..side * side)
				.map(|i| {
					let d = (a[i] - b[i]).abs().min(1.0);
					(if invert { 1.0 - d } else { d }) * alpha[i]
				})
				.collect()
		}
		EffectKind::BevelShadow | EffectKind::BevelHighlight => match &params.extra {
			EffectExtra::Bevel {
				style,
				depth,
				up,
				size,
				soften,
				light,
				highlight,
				technique,
				contour,
				contour_range,
				contour_anti_aliased,
				gloss,
				anti_aliased,
				texture: tex,
				stroke,
			} => {
				// The texture's relief per alpha pixel (0 = flat).
				let relief = match (tex, texture.pattern) {
					(Some(t), Some(p)) if t.depth.abs() > 0.0 => {
						let k = (t.scale / 100.0).max(0.01);
						let o = if t.align { texture.bounds.map_or((0.0, 0.0), |b| (b.0, b.1)) } else { (0.0, 0.0) };
						let o = (o.0 + t.phase.0, o.1 + t.phase.1);
						let amount = (t.depth / 100.0).clamp(-10.0, 10.0) as f32 * if t.invert { -1.0 } else { 1.0 };
						let r: Vec<f32> = (0..side * side)
							.map(|i| {
								let (x, y) = (texture.origin.0 + (i % side) as f64 * scale, texture.origin.1 + (i / side) as f64 * scale);
								let c = p.sample((x - o.0) / k, (y - o.1) / k);
								let luma = (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]) as f32;
								(luma - 0.5) * amount
							})
							.collect();
						Some(r)
					}
					_ => None,
				};
				let shape = BevelShape {
					style: *style,
					depth: *depth,
					up: *up,
					size: size / scale,
					soften: soften / scale,
					light: *light,
					highlight: *highlight,
					technique: *technique,
					contour: *contour,
					contour_range: *contour_range,
					contour_anti_aliased: *contour_anti_aliased,
					gloss: *gloss,
					anti_aliased: *anti_aliased,
				};
				// Stroke Emboss: the bevel follows the stroke's ring, raised
				// like an inner bevel of it.
				match stroke {
					Some((size, position)) => bevel(&stroke_ring(alpha, side, size / scale, *position), side, &shape, relief.as_deref()),
					None => bevel(alpha, side, &shape, relief.as_deref()),
				}
			}
			_ => vec![0.0; side * side],
		},
		EffectKind::Stroke => {
			let (size, position) = params.stroke.unwrap_or((0.0, StrokePosition::Outside));
			stroke_ring(alpha, side, size / scale, position)
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

/// A stroke's coverage: a ring `size` level pixels wide outside, inside or
/// centred on the edge of `alpha`.
fn stroke_ring(alpha: &[f32], side: usize, size: f64, position: StrokePosition) -> Vec<f32> {
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

/// A Precise glow's falloff at distance `d` from the edge: full over the
/// spread (`morph`), then linear to nothing over the rest of the size.
fn precise_falloff(d: f64, morph: f64, blur: f64) -> f32 {
	(1.0 - (d - morph).max(0.0) / blur.max(1.0)).clamp(0.0, 1.0) as f32
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
/// Photoshop's Noise: each pixel keeps a random share of its coverage,
/// fixed per document pixel of the level (so tiles agree at their seams).
fn add_noise(coverage: &mut [f32], noise: f32, (tx, ty): (u32, u32)) {
	let t = TILE_SIZE as usize;
	for (i, c) in coverage.iter_mut().enumerate() {
		if *c <= 0.0 {
			continue;
		}
		let (x, y) = (tx as usize * t + i % t, ty as usize * t + i / t);
		let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
		h ^= h >> 13;
		h = h.wrapping_mul(0x5bd1_e995);
		h ^= h >> 15;
		let r = (h & 0xffff) as f32 / 65_535.0;
		*c *= 1.0 - noise * r;
	}
}

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

/// Bevel & Emboss's parameters for one pass, in level pixels.
struct BevelShape {
	style: BevelStyle,
	depth: f64,
	up: bool,
	size: f64,
	soften: f64,
	light: (f64, f64),
	highlight: bool,
	technique: fx_core::styles::BevelTechnique,
	contour: fx_core::styles::Contour,
	contour_range: f64,
	contour_anti_aliased: bool,
	gloss: fx_core::styles::Contour,
	anti_aliased: bool,
}

/// One pass of Bevel & Emboss (M12-T04): a height field from the distance
/// to the edge, shaped by the Technique, the Contour (over its Range) and the
/// Texture, shaded by the light; the highlight or the shadow part. VERIFY
/// the shading against Photoshop.
fn bevel(alpha: &[f32], side: usize, b: &BevelShape, relief: Option<&[f32]>) -> Vec<f32> {
	use fx_core::styles::{BevelTechnique, Contour};
	let style = b.style;
	let size = b.size.max(0.5);
	let inside: Vec<bool> = alpha.iter().map(|&a| a < 0.5).collect();
	let outside: Vec<bool> = alpha.iter().map(|&a| a >= 0.5).collect();
	let d_in = fx_ops::morph::edt_2d(&inside, side, side);
	let d_out = fx_ops::morph::edt_2d(&outside, side, side);
	// Height 0 at the edge, 1 on the plateau (inside) or the far ground.
	let (reach_in, reach_out) = match style {
		BevelStyle::InnerBevel | BevelStyle::StrokeEmboss => (size, 0.0),
		BevelStyle::OuterBevel => (0.0, size),
		BevelStyle::Emboss | BevelStyle::PillowEmboss => (size / 2.0, size / 2.0),
	};
	// Structure ▸ Contour shapes the profile (on its magnitude).
	// Contour ▸ Range: the contour spans that share of the bevel, flat past it.
	let profile = |v: f64| -> f64 {
		if b.contour == Contour::Linear && b.contour_range >= 1.0 {
			v
		} else {
			v.signum() * b.contour.apply_smooth((v.abs() / b.contour_range).min(1.0), b.contour_anti_aliased)
		}
	};
	let mut h: Vec<f32> = (0..side * side)
		.map(|i| {
			let v = if alpha[i] >= 0.5 {
				if reach_in > 0.0 { (d_in[i] / reach_in).min(1.0) } else { 1.0 }
			} else if reach_out > 0.0 {
				let r = 1.0 - (d_out[i] / reach_out).min(1.0);
				// Outer bevel / emboss: the ground rises towards the shape.
				if style == BevelStyle::PillowEmboss { -r } else { r - 1.0 }
			} else {
				0.0
			};
			profile(v) as f32
		})
		.collect();
	// Technique: Smooth rounds the profile (a blur of about a third of the
	// size); Chisel Hard keeps the exact distance; Chisel Soft softens it a
	// little. VERIFY against Photoshop.
	let technique_blur = match b.technique {
		BevelTechnique::Smooth => size / 3.0,
		BevelTechnique::ChiselHard => 0.0,
		BevelTechnique::ChiselSoft => 1.0,
	};
	let blur = (b.soften + technique_blur).max(0.0);
	if blur > 0.3 {
		h = box_blur3(&h, side, blur / 2.0);
	}
	// Texture: the pattern's relief pressed into the surface (inside).
	if let Some(relief) = relief {
		for i in 0..h.len() {
			h[i] += relief[i] * alpha[i] * 0.25;
		}
	}
	let k = (b.depth / 100.0 * size) as f32 * if b.up { 1.0 } else { -1.0 };
	let (az, alt) = b.light;
	let l = [(alt.cos() * az.cos()) as f32, (-alt.cos() * az.sin()) as f32, alt.sin() as f32];
	let flat = l[2];
	let region = |i: usize| -> f32 {
		let inside = alpha[i];
		match style {
			BevelStyle::InnerBevel | BevelStyle::StrokeEmboss => inside,
			BevelStyle::OuterBevel => (1.0 - inside) * if d_out[i] <= reach_out + 1.0 { 1.0 } else { 0.0 },
			_ => {
				if alpha[i] >= 0.5 || d_out[i] <= reach_out + 1.0 {
					1.0
				} else {
					0.0
				}
			}
		}
	};
	let (highlight, gloss) = (b.highlight, b.gloss);
	let mut out: Vec<f32> = (0..side * side)
		.map(|i| {
			let (x, y) = (i % side, i / side);
			let get = |xx: usize, yy: usize| h[yy.min(side - 1) * side + xx.min(side - 1)];
			let gx = (get(x + 1, y) - get(x.saturating_sub(1), y)) / 2.0;
			let gy = (get(x, y + 1) - get(x, y.saturating_sub(1))) / 2.0;
			let n = [-k * gx, -k * gy, 1.0];
			let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
			let shade = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]) / len;
			// Gloss Contour: the lighting (-1..1) through the curve.
			let shade = if gloss == Contour::Linear { shade } else { (gloss.apply(f64::from((shade + 1.0) / 2.0)) * 2.0 - 1.0) as f32 };
			let v = if highlight { (shade - flat) * 2.0 } else { (flat - shade) * 2.0 };
			v.clamp(0.0, 1.0) * region(i)
		})
		.collect();
	if b.anti_aliased {
		out = box_blur3(&out, side, 0.5);
	}
	out
}

/// A straight-alpha tile whose colour comes from `color(x, y)` (document
/// point of each pixel centre) and whose alpha is coverage × that alpha.
fn paint_with(coverage: &[f32], format: PixelFormat, color: impl Fn(f64, f64) -> [f64; 4], (tx, ty): (u32, u32), scale: f64) -> TileBuffer {
	let t = TILE_SIZE as usize;
	let mut buffer = TileBuffer::zeroed(format);
	for (i, &c) in coverage.iter().enumerate() {
		if c <= 0.0 {
			continue;
		}
		let (x, y) = (
			(f64::from(tx) * t as f64 + (i % t) as f64 + 0.5) * scale,
			(f64::from(ty) * t as f64 + (i / t) as f64 + 0.5) * scale,
		);
		let rgba = color(x, y);
		let a = c.clamp(0.0, 1.0) as f64 * rgba[3];
		match format {
			PixelFormat::Rgba16 => {
				let px = buffer.as_u16_mut();
				px[i * 4..i * 4 + 4].copy_from_slice(
					&[0, 1, 2]
						.map(|k| (rgba[k] * 65535.0).round() as u16)
						.into_iter()
						.chain([(a * 65535.0).round() as u16])
						.collect::<Vec<_>>(),
				);
			}
			_ => {
				let px = buffer.bytes_mut();
				px[i * 4..i * 4 + 4].copy_from_slice(
					&[0, 1, 2]
						.map(|k| (rgba[k] * 255.0).round() as u8)
						.into_iter()
						.chain([(a * 255.0).round() as u8])
						.collect::<Vec<_>>(),
				);
			}
		}
	}
	buffer
}
