//! The engine side of the local AI (M13-T01): the document's composite read
//! at a bounded working resolution (from the mips, never level 0 of a big
//! document), and the model manager's actions.
//!
//! FAST: the missing tiles a program asks for are served here on the
//! calling thread (mips, shape / vector-mask / effect tiles), up to three
//! rounds; the output mask is upsampled bilinearly by the caller (the M9-T05
//! edge refinement is not applied yet).

use fx_core::Document;
use fx_render::adjust::LutCache;
use fx_render::blend::unpremultiply;
use fx_render::build_program;
use fx_render::program::TileRequest;
use fx_render::reference::render_tile;
use fx_tiles::{TILE_SIZE, TileStore};

/// The composite at the mip level whose long side is at most `max_side`:
/// straight RGBA `0..=1`, its size, and the level (document pixels per
/// working pixel = `2^level`).
pub struct Working {
	pub pixels: Vec<[f32; 4]>,
	pub width: usize,
	pub height: usize,
	pub level: usize,
}

impl Working {
	pub fn scale(&self) -> f64 {
		f64::from(1u32 << self.level)
	}
}

/// Serve the tiles a program is missing (the frame loop's work, inline).
pub fn serve(doc: &mut Document, store: &TileStore, requests: &[TileRequest]) {
	let mut shapes = Vec::new();
	let mut masks = Vec::new();
	let mut effects = Vec::new();
	for request in requests {
		match request {
			TileRequest::Mip(r) => {
				let Some(layer) = doc.layer_mut(r.layer) else { continue };
				let image = if r.mask {
					match layer.mask.as_mut() {
						Some(mask) => &mut mask.image,
						None => continue,
					}
				} else {
					match &mut layer.kind {
						fx_core::LayerKind::Pixel { image, .. } => image,
						_ => continue,
					}
				};
				if let Err(error) = crate::mips::ensure_mip(image, store, r.level, r.x, r.y) {
					tracing::warn!("mip {r:?} failed: {error}");
				}
			}
			TileRequest::Vector(r) if r.vector_mask => masks.push((r.layer, r.level, r.x, r.y)),
			TileRequest::Vector(r) => shapes.push((r.layer, r.level, r.x, r.y)),
			TileRequest::Effect(r) => effects.push((r.layer, r.effect, r.level, r.x, r.y)),
		}
	}
	if !shapes.is_empty() {
		crate::vector::draw_requests(doc, store, &shapes);
	}
	if !masks.is_empty() {
		crate::vector::draw_vector_mask_requests(doc, store, &masks);
	}
	if !effects.is_empty() {
		crate::effects::draw_effect_requests(doc, store, &effects);
	}
}

/// Read the composite at a working resolution (M13-T01): the number of tiles
/// read is bounded by `max_side²`, whatever the document size.
pub fn working_composite(doc: &mut Document, store: &TileStore, max_side: u32) -> Result<Working, String> {
	let mut level = 0usize;
	while (doc.width.max(doc.height) >> level) > max_side && level < 16 {
		level += 1;
	}
	let (w, h) = (doc.width.div_ceil(1 << level).max(1) as usize, doc.height.div_ceil(1 << level).max(1) as usize);
	let t = TILE_SIZE as usize;
	let (cols, rows) = (w.div_ceil(t), h.div_ceil(t));
	let mut luts = LutCache::default();
	let mut pixels = vec![[0.0f32; 4]; w * h];
	for ty in 0..rows {
		for tx in 0..cols {
			let mut program = None;
			for _ in 0..4 {
				match build_program(doc, level, tx as u32, ty as u32, &mut |a| luts.get(a)) {
					Ok(p) => {
						program = Some(p);
						break;
					}
					Err(requests) => serve(doc, store, &requests),
				}
			}
			let program = program.ok_or_else(|| "the document's tiles could not be prepared".to_owned())?;
			let fetch = |handle: &fx_tiles::TileHandle| store.get(handle).expect("tile of a live document");
			let tile = render_tile(&program, &fetch);
			for y in 0..t.min(h - ty * t) {
				for x in 0..t.min(w - tx * t) {
					let p = tile[y * t + x];
					let rgb = unpremultiply(p);
					pixels[(ty * t + y) * w + tx * t + x] = [rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, p[3] as f32];
				}
			}
		}
	}
	Ok(Working {
		pixels,
		width: w,
		height: h,
		level,
	})
}
