//! Derived tiles on demand (code review 2026-09-27 R01, R06, R07).
//!
//! Mips, generated-layer caches (shape, text, fill, Smart Object), vector
//! masks and layer-style effects are derived tiles: the store may drop them
//! under memory pressure, and a dropped one comes back only by being computed
//! again from its source. Three things follow:
//!
//! * [`fulfil`] computes exactly the tiles a program asked for — the one
//!   dispatcher for the frame's requests and for every whole-document reader.
//! * [`render_tiles`] renders a batch of tiles for a reader (export, merge,
//!   flatten, sampling), computing only that batch's inputs — never the whole
//!   document's — and holding each tile's input pixels while it renders: a
//!   held `Arc<TileBuffer>` cannot be trimmed.
//! * [`merge`] brings the derived tiles a worker computed on its own copy of
//!   a document into the live one, so the engine thread only clones and
//!   installs, and the computing happens on a worker.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fx_core::{Document, Layer, LayerId, LayerKind};
use fx_render::adjust::LutCache;
use fx_render::program::QuadSlot;
use fx_render::reference::render_tile;
use fx_render::{TileProgram, TileRequest, build_program_checked};
use fx_tiles::{TileBuffer, TileError, TileId, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

/// Rounds of "build, compute what is missing, build again" before a reader
/// gives up: each round computes what the last one found missing (a mip
/// needs its children, an effect its layer's pixels), or what the trim
/// dropped in between.
const MAX_ROUNDS: usize = 8;

/// Compute every tile of `requests` in `doc`: mips (with the tiles below
/// them), generated-layer caches, vector masks and effects. A request from
/// an older snapshot (a layer or a tile the document no longer has) is
/// skipped.
pub fn fulfil(doc: &mut Document, store: &TileStore, requests: &[TileRequest]) {
	let mut shape_tiles: Vec<(LayerId, usize, u32, u32)> = Vec::new();
	let mut mask_tiles: Vec<(LayerId, usize, u32, u32)> = Vec::new();
	let mut effect_tiles: Vec<(LayerId, u8, usize, u32, u32)> = Vec::new();
	for request in requests {
		match request {
			TileRequest::Mip(request) => {
				let Some(layer) = doc.layer_mut(request.layer) else { continue };
				let image = if request.mask {
					match layer.mask.as_mut() {
						Some(mask) => &mut mask.image,
						None => continue,
					}
				} else {
					match &mut layer.kind {
						LayerKind::Pixel { image, .. } => image,
						_ => continue,
					}
				};
				let grid_has = |image: &TiledImage| {
					request.level < image.level_count() && {
						let grid = image.grid(request.level);
						request.x < grid.cols() && request.y < grid.rows()
					}
				};
				if !grid_has(image) {
					continue;
				}
				if let Err(error) = crate::mips::ensure_mip(image, store, request.level, request.x, request.y) {
					tracing::warn!("mip {request:?} failed: {error}");
				}
			}
			// Generated tiles are drawn from their parameters, all levels
			// alike (M6-T06); collect them and draw one batch per layer.
			TileRequest::Vector(request) if request.vector_mask => mask_tiles.push((request.layer, request.level, request.x, request.y)),
			TileRequest::Vector(request) => shape_tiles.push((request.layer, request.level, request.x, request.y)),
			TileRequest::Effect(request) => effect_tiles.push((request.layer, request.effect, request.level, request.x, request.y)),
		}
	}
	if !shape_tiles.is_empty() {
		crate::vector::draw_requests(doc, store, &shape_tiles);
	}
	if !mask_tiles.is_empty() {
		crate::vector::draw_vector_mask_requests(doc, store, &mask_tiles);
	}
	if !effect_tiles.is_empty() {
		crate::effects::draw_effect_requests(doc, store, &effect_tiles);
	}
}

/// One tile of [`render_tiles`].
pub struct Rendered {
	pub tile: (u32, u32),
	/// Premultiplied RGBA in `0..=1`, row-major; `None` when nothing is
	/// drawn there (transparent).
	pub pixels: Option<Vec<[f64; 4]>>,
}

/// Render `tiles` (level `level` of `doc`) with the CPU reference
/// compositor, computing every derived input they need first — only theirs.
/// Each tile holds the pixels it reads while it renders, so the trim cannot
/// drop them mid-way; a tile whose input was dropped before that is built
/// and rendered again. Whatever is computed lands in `doc`: a reader's own
/// copy of the document. Tiles render in parallel; the result keeps the
/// order of `tiles`.
pub fn render_tiles(doc: &mut Document, store: &TileStore, level: usize, tiles: &[(u32, u32)], luts: &mut LutCache) -> Result<Vec<Rendered>, TileError> {
	let mut done: HashMap<(u32, u32), Option<Vec<[f64; 4]>>> = HashMap::with_capacity(tiles.len());
	let mut pending: Vec<(u32, u32)> = tiles.to_vec();
	for _ in 0..MAX_ROUNDS {
		if pending.is_empty() {
			break;
		}
		let programs: Vec<((u32, u32), TileProgram)> = pending.iter().copied().zip(prepare(doc, store, level, &pending, luts)?).collect();
		let results: Vec<((u32, u32), Result<Option<Vec<[f64; 4]>>, TileError>)> =
			programs.into_par_iter().map(|(tile, program)| (tile, render_pinned(&program, store))).collect();
		pending.clear();
		for (tile, result) in results {
			match result {
				Ok(pixels) => {
					done.insert(tile, pixels);
				}
				Err(TileError::Evicted) => pending.push(tile),
				Err(error) => return Err(error),
			}
		}
	}
	if !pending.is_empty() {
		return Err(TileError::Evicted);
	}
	Ok(tiles
		.iter()
		.map(|&tile| Rendered {
			tile,
			pixels: done.remove(&tile).expect("every tile rendered"),
		})
		.collect())
}

/// The programs of `tiles` (level `level` of `doc`, in order) once every
/// derived input they read is computed — into `doc`, a reader's copy. The
/// trim may still drop an input before it is read: [`render_pinned`] then
/// reports `Evicted`, and the caller prepares that tile again.
pub fn prepare(doc: &mut Document, store: &TileStore, level: usize, tiles: &[(u32, u32)], luts: &mut LutCache) -> Result<Vec<TileProgram>, TileError> {
	for _ in 0..MAX_ROUNDS {
		let mut programs = Vec::with_capacity(tiles.len());
		let mut missing: Vec<TileRequest> = Vec::new();
		let mut seen = HashSet::new();
		for &(tx, ty) in tiles {
			match build_program_checked(doc, level, tx, ty, &mut |a| luts.get(a), &|h| store.is_evicted(h)) {
				Ok(program) => programs.push(program),
				Err(requests) => missing.extend(requests.into_iter().filter(|r| seen.insert(*r))),
			}
		}
		if missing.is_empty() {
			return Ok(programs);
		}
		fulfil(doc, store, &missing);
	}
	Err(TileError::Evicted)
}

/// Render one program, holding every tile it reads; `None` for an empty one.
pub fn render_pinned(program: &TileProgram, store: &TileStore) -> Result<Option<Vec<[f64; 4]>>, TileError> {
	if program.is_empty() {
		return Ok(None);
	}
	let mut pins: HashMap<TileId, Arc<TileBuffer>> = HashMap::new();
	for quad in program.ops.iter().flat_map(|op| op.quads()) {
		for slot in &quad.slots {
			if let QuadSlot::Slot(TileSlot::Data(handle)) = slot
				&& !pins.contains_key(&handle.id())
			{
				pins.insert(handle.id(), store.get(handle)?);
			}
		}
	}
	Ok(Some(render_tile(program, &|handle| pins[&handle.id()].clone())))
}

/// Every derived image of a layer, in a fixed order (the mips of its pixels
/// and mask, its generated cache, a Smart Object's composite, its vector mask
/// and its effects). Exhaustive over layer kinds.
fn derived_images(layer: &Layer) -> Vec<&TiledImage> {
	let mut out = Vec::new();
	match &layer.kind {
		LayerKind::Pixel { image, .. } => out.push(image),
		LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } | LayerKind::FillLayer { cache, .. } => out.push(cache),
		LayerKind::Smart { smart, cache } => {
			out.push(cache);
			out.push(&smart.source.composite);
		}
		LayerKind::Group { .. } | LayerKind::Adjustment(_) | LayerKind::SolidFill { .. } => {}
	}
	if let Some(mask) = &layer.mask {
		out.push(&mask.image);
	}
	if let Some(vm) = &layer.vector_mask {
		out.push(&vm.cache);
	}
	out.extend(layer.effects.iter());
	out
}

/// [`derived_images`], mutably (the same order).
fn derived_images_mut(layer: &mut Layer) -> Vec<&mut TiledImage> {
	let mut out = Vec::new();
	match &mut layer.kind {
		LayerKind::Pixel { image, .. } => out.push(image),
		LayerKind::Shape { cache, .. } | LayerKind::Text { cache, .. } | LayerKind::FillLayer { cache, .. } => out.push(cache),
		LayerKind::Smart { smart, cache } => {
			out.push(cache);
			out.push(&mut smart.source.composite);
		}
		LayerKind::Group { .. } | LayerKind::Adjustment(_) | LayerKind::SolidFill { .. } => {}
	}
	if let Some(mask) = &mut layer.mask {
		out.push(&mut mask.image);
	}
	if let Some(vm) = &mut layer.vector_mask {
		out.push(&mut vm.cache);
	}
	out.extend(layer.effects.iter_mut());
	out
}

/// Whether a slot must be computed before it can be read.
fn missing(image: &TiledImage, store: &TileStore, level: usize, tx: u32, ty: u32) -> bool {
	image.is_dirty(level, tx, ty) || matches!(image.slot(level, tx, ty), TileSlot::Data(h) if store.is_evicted(h))
}

/// Install in `live` the derived tiles of `layers` that `computed` — a
/// worker's copy of the same content generation — has ready and `live` still
/// lacks. Tiles `live` computed meanwhile are kept. Returns whether anything
/// was installed.
pub fn merge(live: &mut Document, computed: &Document, layers: &HashSet<LayerId>, store: &TileStore) -> bool {
	let mut installed = false;
	for &id in layers {
		let Some(source) = computed.layer(id) else { continue };
		let Some(target) = live.layer(id) else { continue };
		// Decide on the shared view first: `layer_mut` copies a layer the
		// render snapshot still shares, so only touch one that gains tiles.
		let mut wanted: Vec<(usize, usize, u32, u32)> = Vec::new();
		let (from, to) = (derived_images(source), derived_images(target));
		if from.len() != to.len() {
			continue; // not the same structure: not the same content
		}
		for (i, (a, b)) in from.iter().zip(&to).enumerate() {
			if (a.width(), a.height(), a.level_count()) != (b.width(), b.height(), b.level_count()) {
				continue;
			}
			// Level 0 of an ordinary image is its content, never derived.
			let first = usize::from(!a.is_derived());
			for level in first..a.level_count() {
				let grid = a.grid(level);
				for ty in 0..grid.rows() {
					for tx in 0..grid.cols() {
						if !missing(a, store, level, tx, ty) && missing(b, store, level, tx, ty) {
							wanted.push((i, level, tx, ty));
						}
					}
				}
			}
		}
		if wanted.is_empty() {
			continue;
		}
		let slots: Vec<TileSlot> = wanted.iter().map(|&(i, level, tx, ty)| from[i].slot(level, tx, ty).clone()).collect();
		let Some(target) = live.layer_mut(id) else { continue };
		let mut images = derived_images_mut(target);
		for (&(i, level, tx, ty), slot) in wanted.iter().zip(slots) {
			images[i].set_derived_slot(level, tx, ty, slot);
		}
		installed = true;
	}
	installed
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_core::{BitDepth, ColorProfile, DocumentColor};
	use fx_tiles::{PixelFormat, PixelValue, TileClass, TileStoreConfig};

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join(format!("fx-engine-derived-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	fn doc(w: u32, h: u32) -> Document {
		Document::new(
			w,
			h,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		)
	}

	/// A 512 × 512 pixel layer whose level-1 mip was computed, then dropped by
	/// the trim (R01's reproduction): the program still builds, because the
	/// slot is clean, and a plain read fails. A reader recomputes it instead.
	#[test]
	fn a_reader_recomputes_an_evicted_mip() {
		let dir = std::env::temp_dir().join(format!("fx-engine-derived-evict-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let mut config = TileStoreConfig::for_tests(dir);
		config.hot_budget = 0;
		let store = TileStore::new(config).unwrap();
		let mut d = doc(512, 512);
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba8);
		for ty in 0..2 {
			for tx in 0..2 {
				let mut tile = TileBuffer::zeroed(PixelFormat::Rgba8);
				for px in tile.bytes_mut().chunks_exact_mut(4) {
					px.copy_from_slice(&[255, 0, 0, 255]);
				}
				// Not uniform, so it stays a stored tile: one pixel is blue.
				tile.bytes_mut()[..4].copy_from_slice(&[0, 0, 255, 255]);
				image.set_slot(tx, ty, TileSlot::Data(store.insert(tile, TileClass::Authoritative)));
			}
		}
		let dropped = store.insert(TileBuffer::zeroed(PixelFormat::Rgba8), TileClass::Derived);
		image.set_derived_slot(1, 0, 0, TileSlot::Data(dropped.clone()));
		d.layers
			.push(Arc::new(Layer::new(LayerId(1), "red", LayerKind::Pixel { image, offset: (0, 0) })));
		store.trim();
		assert!(store.is_evicted(&dropped), "the mip was dropped");
		let mut luts = LutCache::default();
		assert!(
			fx_render::build_program(&d, 1, 0, 0, &mut |a| luts.get(a)).is_ok(),
			"the old check sees nothing wrong"
		);
		assert!(matches!(store.get(&dropped), Err(TileError::Evicted)));
		let rendered = render_tiles(&mut d, &store, 1, &[(0, 0)], &mut luts).unwrap();
		let pixel = rendered[0].pixels.as_ref().unwrap()[256 * 100 + 100];
		assert!((pixel[0] - 1.0).abs() < 1e-3 && pixel[2] < 1e-3, "{pixel:?}");
	}

	/// A worker's computed tiles reach the live document; tiles the live
	/// document already has are kept.
	#[test]
	fn merge_installs_only_what_the_live_document_lacks() {
		let store = store();
		let mut live = doc(512, 512);
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba8);
		image.set_slot(0, 0, TileSlot::Solid(PixelValue([65535; 4])));
		live.layers
			.push(Arc::new(Layer::new(LayerId(1), "a", LayerKind::Pixel { image, offset: (0, 0) })));
		let mut computed = live.clone();
		fulfil(
			&mut computed,
			&store,
			&[TileRequest::Mip(fx_render::MipRequest {
				layer: LayerId(1),
				mask: false,
				level: 1,
				x: 0,
				y: 0,
			})],
		);
		let dirty = |d: &Document| match &d.layer(LayerId(1)).unwrap().kind {
			LayerKind::Pixel { image, .. } => image.is_dirty(1, 0, 0),
			_ => unreachable!(),
		};
		assert!(dirty(&live) && !dirty(&computed));
		assert!(merge(&mut live, &computed, &HashSet::from([LayerId(1)]), &store));
		assert!(!dirty(&live), "the mip landed in the live document");
		assert!(!merge(&mut live, &computed, &HashSet::from([LayerId(1)]), &store), "nothing left to install");
	}
}
