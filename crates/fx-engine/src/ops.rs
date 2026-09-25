//! The engine's implementation of [`fx_core::PixelOps`] (M4-T05, recipe R1a):
//! filters with `fx-ops` over the tile store, compositing with the CPU
//! reference compositor (M4-T08).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fx_core::{BitDepth, CommandError, Conversion, Document, FilterParams, LayerId, LayerKind, PixelOps, SelectModify, Selection, SelectionShape, WandParams};
use fx_ops::filter::{self, Geometry};
use fx_ops::neighbourhood::{LevelSource, TileRef};
use fx_tiles::{PixelFormat, TILE_SIZE, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

use crate::mips;

/// Reports a job's progress, 0..=1.
pub type ProgressFn = dyn Fn(f32) + Send + Sync;

/// The engine's pixel operations. `progress` (optional) hears about long
/// operations, for the status bar.
#[derive(Default)]
pub struct EngineOps {
	pub progress: Option<Arc<ProgressFn>>,
	/// What Edit ▸ Copy put aside (M5-T05); jobs share it.
	pub clipboard: fx_core::pixels::SharedClipboard,
}

impl PixelOps for EngineOps {
	fn filter(&self, image: &TiledImage, offset: (i32, i32), canvas: (u32, u32), filter: &FilterParams, store: &TileStore) -> Result<TiledImage, CommandError> {
		let geometry = Geometry {
			offset,
			canvas,
			image: (image.width(), image.height()),
		};
		let tiles = filter::output_tiles(image, &geometry, filter, 0);
		let mut source_image = image.clone();
		prepare_levels(&mut source_image, store, filter, 0, None)?;
		let done = AtomicUsize::new(0);
		let total = tiles.len().max(1);
		let source = ImageSource { image: &source_image, store };
		// Each filtered tile goes into the store at once; only slots are
		// collected (review S1-01).
		let results: Vec<Result<fx_tiles::PlacedSlot, TileError>> = tiles
			.par_iter()
			.map(|&(tx, ty)| {
				let tile = fx_tiles::slot_for(store, filter::filter_tile(&source, &geometry, filter, 0, tx, ty)?);
				let n = done.fetch_add(1, Ordering::Relaxed) + 1;
				if let Some(progress) = &self.progress
					&& (n * 100 / total) != ((n - 1) * 100 / total)
				{
					progress(n as f32 / total as f32);
				}
				Ok(((tx, ty), tile))
			})
			.collect();
		let mut out = image.clone();
		for result in results {
			let ((tx, ty), slot) = result?;
			out.set_slot(tx, ty, slot);
		}
		Ok(out)
	}

	fn composite(&self, doc: &Document, layers: &[LayerId], background: Option<[u16; 4]>, store: &TileStore) -> Result<TiledImage, CommandError> {
		crate::export::composite_layers(doc, layers, background, store, self.progress.as_deref())
	}

	fn convert(&self, image: &TiledImage, conversion: &Conversion<'_>, store: &TileStore) -> Result<TiledImage, CommandError> {
		let transform = rgb_transform(conversion)?;
		let format = image.format();
		let tiles: Vec<(u32, u32, TileSlot)> = image.grid(0).non_empty().map(|(tx, ty, slot)| (tx, ty, slot.clone())).collect();
		let done = AtomicUsize::new(0);
		let total = tiles.len().max(1);
		let converted: Vec<Result<(u32, u32, TileSlot), TileError>> = tiles
			.par_iter()
			.map(|(tx, ty, slot)| {
				let out = match slot {
					TileSlot::Empty => TileSlot::Empty,
					TileSlot::Solid(value) => {
						let mut px = [value.0];
						transform.apply(&mut px);
						TileSlot::Solid(fx_tiles::PixelValue(px[0]))
					}
					TileSlot::Data(handle) => {
						let buffer = store.get(handle)?;
						let mut pixels = to_rgba16(&buffer, format);
						transform.apply(&mut pixels);
						TileSlot::Data(store.insert(from_rgba16(&pixels, format), fx_tiles::TileClass::Authoritative))
					}
				};
				let n = done.fetch_add(1, Ordering::Relaxed) + 1;
				if let Some(progress) = &self.progress
					&& n * 100 / total != (n - 1) * 100 / total
				{
					progress(n as f32 / total as f32);
				}
				Ok((*tx, *ty, out))
			})
			.collect();
		let mut out = image.clone();
		for result in converted {
			let (tx, ty, slot) = result?;
			out.set_slot(tx, ty, slot);
		}
		Ok(out)
	}

	fn convert_color(&self, rgba: [u16; 4], conversion: &Conversion<'_>) -> Result<[u16; 4], CommandError> {
		let mut px = [rgba];
		rgb_transform(conversion)?.apply(&mut px);
		Ok(px[0])
	}

	fn rasterise(&self, shape: &SelectionShape, size: (u32, u32), depth: BitDepth, anti_alias: bool, store: &TileStore) -> Result<Selection, CommandError> {
		fx_ops::raster::rasterise(shape, size, depth, anti_alias, store)
	}

	fn modify_selection(
		&self,
		selection: &Selection,
		op: &SelectModify,
		size: (u32, u32),
		depth: BitDepth,
		store: &TileStore,
	) -> Result<Option<Selection>, CommandError> {
		fx_ops::morph::modify(selection, op, size, depth, store)
	}

	fn stroke(
		&self,
		doc: &Document,
		layer: LayerId,
		target: fx_core::stroke::StrokeTarget,
		tool: &fx_core::stroke::StrokeTool,
		brush: &fx_core::stroke::BrushParams,
		color: [u16; 4],
		samples: &[fx_core::stroke::StrokeSample],
		store: &TileStore,
	) -> Result<(TiledImage, (i32, i32)), CommandError> {
		let prepared = crate::stroke::prepare(doc, layer, target, tool, store)?;
		fx_ops::brush::replay(crate::stroke::setup(&prepared, doc, *tool, *brush, color), samples, store)
	}

	fn clipboard(&self) -> Option<fx_core::pixels::ClipboardImage> {
		self.clipboard.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
	}

	fn magic_wand(&self, doc: &Document, params: &WandParams, store: &TileStore) -> Result<Option<Selection>, CommandError> {
		if !params.x.is_finite() || !params.y.is_finite() || params.x < 0.0 || params.y < 0.0 {
			return Ok(None);
		}
		let wand = fx_ops::flood::Wand {
			seed: (params.x.floor() as u32, params.y.floor() as u32),
			tolerance: (params.tolerance / 255.0) as f32,
			contiguous: params.contiguous,
			anti_alias: params.anti_alias,
		};
		let size = (doc.width, doc.height);
		let depth = doc.color.depth;
		if params.sample_all_layers {
			return fx_ops::flood::magic_wand(&CompositeSource::new(doc.clone(), store), size, &wand, depth, store);
		}
		let Some(id) = doc.active_layer() else {
			return Err(CommandError::NotAllowed("no active layer to sample".into()));
		};
		let layer = doc.layer(id).ok_or(CommandError::NotAllowed("no active layer to sample".into()))?;
		match &layer.kind {
			// A pixel layer's own pixels, whatever its opacity or blend mode.
			LayerKind::Pixel { image, offset } => fx_ops::flood::magic_wand(&LayerSource { image, offset: *offset, store }, size, &wand, depth, store),
			// Anything else: that layer alone, composited.
			_ => {
				let mut sub = doc.clone();
				sub.layers = crate::export::keep_layers(&doc.layers, &std::iter::once(id).collect());
				fx_ops::flood::magic_wand(&CompositeSource::new(sub, store), size, &wand, depth, store)
			}
		}
	}
}

/// The Magic Wand reads the composite of a document through the CPU
/// reference compositor, one canvas tile at a time (M5-T04).
struct CompositeSource<'a> {
	doc: Document,
	store: &'a TileStore,
	luts: std::sync::Mutex<fx_render::adjust::LutCache>,
}

impl<'a> CompositeSource<'a> {
	fn new(doc: Document, store: &'a TileStore) -> Self {
		Self {
			doc,
			store,
			luts: std::sync::Mutex::new(Default::default()),
		}
	}
}

impl fx_ops::flood::WandSource for CompositeSource<'_> {
	fn tile(&self, tx: u32, ty: u32) -> Result<fx_ops::flood::WandTile, CommandError> {
		let program = {
			let mut luts = self.luts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
			fx_render::build_program(&self.doc, 0, tx, ty, &mut |a| luts.get(a))
				.map_err(|_| CommandError::NotAllowed("full-resolution tiles are missing".into()))?
		};
		if program.is_empty() {
			return Ok(fx_ops::flood::WandTile::Uniform([0.0; 4]));
		}
		let fetch = |h: &fx_tiles::TileHandle| self.store.get(h);
		let pixels = fx_render::reference::try_render_tile(&program, &fetch).map_err(|e| CommandError::NotAllowed(format!("a tile could not be read: {e}")))?;
		Ok(fx_ops::flood::WandTile::Data(pixels.iter().map(|p| p.map(|v| v as f32)).collect()))
	}
}

/// The Magic Wand reads one pixel layer's own pixels (M5-T04): canvas tile
/// `(tx, ty)` assembled from the layer's tiles at its offset.
struct LayerSource<'a> {
	image: &'a TiledImage,
	offset: (i32, i32),
	store: &'a TileStore,
}

impl fx_ops::flood::WandSource for LayerSource<'_> {
	fn tile(&self, tx: u32, ty: u32) -> Result<fx_ops::flood::WandTile, CommandError> {
		let tile = i64::from(TILE_SIZE);
		let ix = i64::from(tx) * tile - i64::from(self.offset.0);
		let iy = i64::from(ty) * tile - i64::from(self.offset.1);
		let format = self.image.format();
		let (cols, rows) = (i64::from(self.image.grid(0).cols()), i64::from(self.image.grid(0).rows()));
		let (iw, ih) = (i64::from(self.image.width()), i64::from(self.image.height()));
		// Aligned and inside: one layer tile, and a solid one needs no pixels.
		if ix.rem_euclid(tile) == 0 && iy.rem_euclid(tile) == 0 {
			let (sx, sy) = (ix / tile, iy / tile);
			if sx < 0 || sy < 0 || sx >= cols || sy >= rows {
				return Ok(fx_ops::flood::WandTile::Uniform([0.0; 4]));
			}
			match self.image.slot(0, sx as u32, sy as u32) {
				TileSlot::Empty => return Ok(fx_ops::flood::WandTile::Uniform([0.0; 4])),
				TileSlot::Solid(v) if ix + tile <= iw && iy + tile <= ih => return Ok(fx_ops::flood::WandTile::Uniform(premul(v.0))),
				_ => {}
			}
		}
		let mut pixels = vec![[0.0f32; 4]; fx_tiles::TILE_PIXELS];
		let mut cache: std::collections::HashMap<(i64, i64), Option<Arc<fx_tiles::TileBuffer>>> = std::collections::HashMap::new();
		for y in 0..tile {
			let sy = iy + y;
			if sy < 0 || sy >= ih {
				continue;
			}
			for x in 0..tile {
				let sx = ix + x;
				if sx < 0 || sx >= iw {
					continue;
				}
				let key = (sx.div_euclid(tile), sy.div_euclid(tile));
				if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
					entry.insert(match self.image.slot(0, key.0 as u32, key.1 as u32) {
						TileSlot::Empty => None,
						TileSlot::Solid(v) => Some(Arc::new(fx_tiles::TileBuffer::filled(format, *v))),
						TileSlot::Data(handle) => Some(self.store.get(handle).map_err(|e| CommandError::NotAllowed(e.to_string()))?),
					});
				}
				let Some(buffer) = &cache[&key] else { continue };
				let i = (sy.rem_euclid(tile) * tile + sx.rem_euclid(tile)) as usize;
				let px = match format {
					PixelFormat::Rgba16 => {
						let s = &buffer.as_u16()[i * 4..i * 4 + 4];
						[s[0], s[1], s[2], s[3]]
					}
					_ => {
						let s = &buffer.bytes()[i * 4..i * 4 + 4];
						[u16::from(s[0]) * 257, u16::from(s[1]) * 257, u16::from(s[2]) * 257, u16::from(s[3]) * 257]
					}
				};
				pixels[(y * tile + x) as usize] = premul(px);
			}
		}
		Ok(fx_ops::flood::WandTile::Data(pixels))
	}
}

/// Straight 16-bit RGBA as premultiplied `0..=1`.
fn premul(px: [u16; 4]) -> [f32; 4] {
	let a = f32::from(px[3]) / 65535.0;
	[
		f32::from(px[0]) / 65535.0 * a,
		f32::from(px[1]) / 65535.0 * a,
		f32::from(px[2]) / 65535.0 * a,
		a,
	]
}

fn rgb_transform(conversion: &Conversion<'_>) -> Result<fx_color::RgbTransform, CommandError> {
	fx_color::RgbTransform::new(conversion.from, conversion.to, conversion.intent, conversion.bpc)
		.map_err(|error| CommandError::NotAllowed(format!("cannot convert: {error}")))
}

/// The pixels of an RGBA tile as straight RGBA16.
fn to_rgba16(buffer: &fx_tiles::TileBuffer, format: PixelFormat) -> Vec<[u16; 4]> {
	match format {
		PixelFormat::Rgba16 => buffer.as_u16().chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect(),
		_ => buffer
			.bytes()
			.chunks_exact(4)
			.map(|p| [p[0], p[1], p[2], p[3]].map(|v| u16::from(v) * 257))
			.collect(),
	}
}

/// Straight RGBA16 pixels back into a tile of `format` (8-bit rounded).
fn from_rgba16(pixels: &[[u16; 4]], format: PixelFormat) -> fx_tiles::TileBuffer {
	let mut tile = fx_tiles::TileBuffer::zeroed(format);
	match format {
		PixelFormat::Rgba16 => {
			for (dst, p) in tile.as_u16_mut().chunks_exact_mut(4).zip(pixels) {
				dst.copy_from_slice(p);
			}
		}
		_ => {
			for (dst, p) in tile.bytes_mut().chunks_exact_mut(4).zip(pixels) {
				for c in 0..4 {
					dst[c] = ((u32::from(p[c]) * 255 + 32767) / 65535) as u8;
				}
			}
		}
	}
	tile
}

/// Make the mip levels a filter at `level` reads valid in `image` (a working
/// copy): `level` itself and the coarser level a large blur uses. `tiles`
/// limits the work to a region (a preview's visible tiles, in tiles of
/// `level`); `None` = the whole image.
pub(crate) fn prepare_levels(
	image: &mut TiledImage,
	store: &TileStore,
	filter: &FilterParams,
	level: usize,
	tiles: Option<(u32, u32, u32, u32)>,
) -> Result<(), TileError> {
	let blur_level = level + filter::extra_levels(filter, level);
	for lvl in [level, blur_level] {
		if lvl == 0 || lvl >= image.level_count() {
			continue;
		}
		match tiles {
			None => {
				let grid = image.grid(lvl).clone();
				for ty in 0..grid.rows() {
					for tx in 0..grid.cols() {
						// Recomputes only what is dirty or was evicted.
						mips::ensure_mip(image, store, lvl, tx, ty)?;
					}
				}
			}
			Some((x0, y0, x1, y1)) => {
				// The region at `lvl`, grown by the blur's apron (a few tiles at most).
				let shift = lvl - level;
				let apron = 1 + (3.0 * 32.0 / TILE_SIZE as f32).ceil() as u32;
				let grid = image.grid(lvl).clone();
				let (gx0, gy0) = ((x0 >> shift).saturating_sub(apron), (y0 >> shift).saturating_sub(apron));
				let (gx1, gy1) = (
					((x1 >> shift) + apron).min(grid.cols().saturating_sub(1)),
					((y1 >> shift) + apron).min(grid.rows().saturating_sub(1)),
				);
				for ty in gy0..=gy1 {
					for tx in gx0..=gx1 {
						// Recomputes only what is dirty or was evicted.
						mips::ensure_mip(image, store, lvl, tx, ty)?;
					}
				}
			}
		}
	}
	Ok(())
}

/// A `TiledImage` as the filters' [`LevelSource`].
pub(crate) struct ImageSource<'a> {
	pub image: &'a TiledImage,
	pub store: &'a TileStore,
}

impl LevelSource for ImageSource<'_> {
	fn format(&self) -> PixelFormat {
		self.image.format()
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		if level >= self.image.level_count() {
			return Ok(None);
		}
		let grid = self.image.grid(level);
		if tx < 0 || ty < 0 || tx >= i64::from(grid.cols()) || ty >= i64::from(grid.rows()) {
			return Ok(None);
		}
		Ok(match self.image.slot(level, tx as u32, ty as u32) {
			TileSlot::Empty => None,
			TileSlot::Solid(value) => Some(TileRef::Solid(value.0)),
			TileSlot::Data(handle) => Some(TileRef::Data(self.store.get(handle)?)),
		})
	}
}
