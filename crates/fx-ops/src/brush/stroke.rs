//! A stroke being painted (M5-T06/T07/T08): the stroke buffer and the layer
//! result.
//!
//! Photoshop's stroke model (D-042): within one stroke every dab adds to a
//! coverage buffer `S` with the flow — `S = 1 − (1 − S)(1 − flow·d)`, where
//! `d` is the dab's tip coverage × the selection — and the layer shows
//! `composite(mode, before, colour, opacity · S)`, always recomputed from the
//! layer **as it was when the stroke started**. So opacity is a true ceiling,
//! and a live stroke and its replay (`Command::Stroke`) produce the same
//! pixels: dabs are applied to each pixel in the same order either way.
//!
//! Work is per layer tile on rayon: a batch of dabs updates the `S` tiles it
//! touches and recomputes only the pixels under those dabs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fx_core::pixels::{block_coverage, decode, grow_to_canvas};
use fx_core::selection::TileCoverage;
use fx_core::stroke::{BrushParams, StrokeSample, StrokeTool};
use fx_core::{CommandError, Selection};
use fx_tiles::{PixelFormat, TILE_PIXELS, TILE_SIZE, TileBuffer, TileError, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

use super::heal;
use super::path::{Dab, DabPath};
use super::tip::Tip;

/// A layer tile a batch of dabs changed, and its new pixels.
pub type ChangedTile = ((u32, u32), TileBuffer);

/// A source tile: premultiplied RGBA, `TILE_SIZE²` pixels.
pub type SourceTile = Arc<Vec<[f32; 4]>>;

/// A dab (by index) and the layer pixel rectangle it may touch.
type DabRect = (usize, [i64; 4]);

/// Premultiplied RGBA (`0..=1`) of the clone source, one canvas tile at a
/// time: the layer or the composite as it was when the stroke started.
pub trait SourceTiles: Send + Sync {
	/// Canvas tile `(tx, ty)`, `TILE_SIZE²` pixels, row-major.
	fn tile(&self, tx: u32, ty: u32) -> Result<SourceTile, TileError>;
}

/// A pixel layer as a clone source.
pub struct LayerSource {
	pub image: TiledImage,
	pub offset: (i32, i32),
	pub store: TileStore,
}

impl SourceTiles for LayerSource {
	fn tile(&self, tx: u32, ty: u32) -> Result<SourceTile, TileError> {
		let tile = i64::from(TILE_SIZE);
		let (ix0, iy0) = (i64::from(tx) * tile - i64::from(self.offset.0), i64::from(ty) * tile - i64::from(self.offset.1));
		let (iw, ih) = (i64::from(self.image.width()), i64::from(self.image.height()));
		let format = self.image.format();
		let mut out = vec![[0.0f32; 4]; TILE_PIXELS];
		let mut cache: HashMap<(i64, i64), Option<Vec<[f32; 4]>>> = HashMap::new();
		for y in 0..tile {
			let iy = iy0 + y;
			if iy < 0 || iy >= ih {
				continue;
			}
			for x in 0..tile {
				let ix = ix0 + x;
				if ix < 0 || ix >= iw {
					continue;
				}
				let key = (ix.div_euclid(tile), iy.div_euclid(tile));
				if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
					entry.insert(match self.image.slot(0, key.0 as u32, key.1 as u32) {
						TileSlot::Empty => None,
						TileSlot::Solid(v) => Some(vec![v.0.map(|c| f32::from(c) / 65535.0); TILE_PIXELS]),
						TileSlot::Data(handle) => Some(decode(self.store.get(handle)?.as_ref(), format)),
					});
				}
				if let Some(pixels) = &cache[&key] {
					let p = pixels[(iy.rem_euclid(tile) * tile + ix.rem_euclid(tile)) as usize];
					out[(y * tile + x) as usize] = [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]];
				}
			}
		}
		Ok(Arc::new(out))
	}
}

/// Everything a stroke needs to start.
pub struct StrokeSetup<'a> {
	/// The layer's pixels (RGBA) or its mask (grey) at stroke start.
	pub image: &'a TiledImage,
	/// Canvas pixel of the image's (0, 0).
	pub offset: (i32, i32),
	pub canvas: (u32, u32),
	pub selection: Option<&'a Selection>,
	pub tool: StrokeTool,
	pub brush: BrushParams,
	/// Straight 16-bit RGBA. For a mask, the grey value is `color[0]`.
	pub color: [u16; 4],
	/// The layer's "Lock transparent pixels" (D-049): alpha is kept.
	pub lock_alpha: bool,
	/// The clone/heal source (required by those tools).
	pub source: Option<Arc<dyn SourceTiles>>,
}

/// One tile of the stroke: its coverage buffer and current result.
struct TileState {
	coverage: Box<[f32]>,
	selection: TileCoverage,
	working: TileBuffer,
}

/// A stroke in progress.
pub struct Stroke {
	before: TiledImage,
	offset: (i32, i32),
	canvas: (u32, u32),
	format: PixelFormat,
	selection: Option<Selection>,
	store: TileStore,
	tool: StrokeTool,
	brush: BrushParams,
	color: [f64; 3],
	color_alpha: f64,
	lock_alpha: bool,
	path: DabPath,
	tiles: HashMap<(u32, u32), TileState>,
	source: Option<Arc<dyn SourceTiles>>,
	/// Source tiles read so far (the clone reads the same ones repeatedly).
	source_cache: Mutex<HashMap<(u32, u32), SourceTile>>,
	/// The samples so far (the recorded command).
	samples: Vec<StrokeSample>,
}

impl Stroke {
	/// Start a stroke. A pixel layer is grown to the canvas first (a stroke may
	/// paint anywhere on the canvas).
	pub fn begin(setup: StrokeSetup<'_>, store: &TileStore) -> Result<Self, CommandError> {
		let format = setup.image.format();
		let gray = matches!(format, PixelFormat::Gray8 | PixelFormat::Gray16);
		if gray && matches!(setup.tool, StrokeTool::Clone { .. } | StrokeTool::Heal { .. } | StrokeTool::SpotHeal) {
			return Err(CommandError::NotAllowed("clone and heal work on pixels, not on a mask".into()));
		}
		if matches!(setup.tool, StrokeTool::Clone { .. } | StrokeTool::Heal { .. } | StrokeTool::SpotHeal) && setup.source.is_none() {
			return Err(CommandError::NotAllowed("the clone source is missing".into()));
		}
		let (before, offset) = if gray {
			(setup.image.clone(), setup.offset)
		} else {
			grow_to_canvas(setup.image, setup.offset, setup.canvas, store)?
		};
		let brush = setup.brush.clamped();
		Ok(Self {
			before,
			offset,
			canvas: setup.canvas,
			format,
			selection: setup.selection.cloned(),
			store: store.clone(),
			tool: setup.tool,
			brush,
			color: [0, 1, 2].map(|i| f64::from(setup.color[i]) / 65535.0),
			color_alpha: f64::from(setup.color[3]) / 65535.0,
			lock_alpha: setup.lock_alpha,
			path: DabPath::new(brush),
			tiles: HashMap::new(),
			source: setup.source,
			source_cache: Mutex::new(HashMap::new()),
			samples: Vec::new(),
		})
	}

	/// The image the stroke paints on (grown to the canvas), and its offset:
	/// what the layer holds while the stroke runs.
	pub fn start(&self) -> (&TiledImage, (i32, i32)) {
		(&self.before, self.offset)
	}

	/// The samples so far.
	pub fn samples(&self) -> &[StrokeSample] {
		&self.samples
	}

	/// Add samples; returns the layer tiles they changed.
	pub fn add(&mut self, samples: &[StrokeSample]) -> Result<Vec<ChangedTile>, TileError> {
		self.samples.extend_from_slice(samples);
		let dabs = self.path.push(samples);
		self.paint(&dabs)
	}

	/// Stamp `dabs` (in order) and return the changed tiles.
	fn paint(&mut self, dabs: &[Dab]) -> Result<Vec<ChangedTile>, TileError> {
		if dabs.is_empty() {
			return Ok(Vec::new());
		}
		let tile = i64::from(TILE_SIZE);
		let (cols, rows) = (i64::from(self.before.grid(0).cols()), i64::from(self.before.grid(0).rows()));
		let pencil = matches!(self.tool, StrokeTool::Pencil);
		// Dabs per touched tile, in stroke order, with their pixel rectangles.
		let mut per_tile: HashMap<(u32, u32), Vec<DabRect>> = HashMap::new();
		let sampled = super::tip::sampled(self.brush.tip);
		let tips: Vec<Tip> = dabs
			.iter()
			.map(|d| match &sampled {
				Some(tip) => Tip::sampled(tip.clone(), d.diameter, d.roundness, d.angle, pencil),
				None => Tip::new(d.diameter, self.brush.hardness, d.roundness, d.angle, pencil),
			})
			.collect();
		for (i, (dab, tip)) in dabs.iter().zip(&tips).enumerate() {
			let reach = f64::from(tip.reach());
			// Layer pixel rectangle the dab may touch.
			let x0 = (dab.x - reach - f64::from(self.offset.0)).floor() as i64;
			let y0 = (dab.y - reach - f64::from(self.offset.1)).floor() as i64;
			let x1 = (dab.x + reach - f64::from(self.offset.0)).ceil() as i64;
			let y1 = (dab.y + reach - f64::from(self.offset.1)).ceil() as i64;
			for ty in y0.div_euclid(tile).max(0)..=y1.div_euclid(tile).min(rows - 1) {
				for tx in x0.div_euclid(tile).max(0)..=x1.div_euclid(tile).min(cols - 1) {
					per_tile.entry((tx as u32, ty as u32)).or_default().push((i, [x0, y0, x1, y1]));
				}
			}
		}
		// Take the touched tiles' state out, work on them in parallel, put back.
		let mut work: Vec<((u32, u32), TileState, Vec<DabRect>)> = Vec::with_capacity(per_tile.len());
		for (key, list) in per_tile {
			let state = match self.tiles.remove(&key) {
				Some(state) => state,
				None => self.new_tile(key)?,
			};
			work.push((key, state, list));
		}
		let results: Result<Vec<ChangedTile>, TileError> = work
			.par_iter_mut()
			.map(|((tx, ty), state, list)| {
				let origin = (i64::from(*tx) * tile, i64::from(*ty) * tile);
				let mut dirty = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
				for &(i, [x0, y0, x1, y1]) in list.iter() {
					let dab = &dabs[i];
					let tip = &tips[i];
					let flow = self.brush.flow * dab.strength;
					let lx0 = (x0 - origin.0).max(0);
					let ly0 = (y0 - origin.1).max(0);
					let lx1 = (x1 - origin.0).min(tile - 1);
					let ly1 = (y1 - origin.1).min(tile - 1);
					if lx0 > lx1 || ly0 > ly1 {
						continue;
					}
					dirty = [dirty[0].min(lx0), dirty[1].min(ly0), dirty[2].max(lx1), dirty[3].max(ly1)];
					for ly in ly0..=ly1 {
						let cy = (origin.1 + ly + i64::from(self.offset.1)) as f64 + 0.5;
						for lx in lx0..=lx1 {
							let cx = (origin.0 + lx + i64::from(self.offset.0)) as f64 + 0.5;
							let d = tip.coverage((cx - dab.x) as f32, (cy - dab.y) as f32) * state.selection.at(lx as u32, ly as u32);
							if d > 0.0 {
								let at = (ly * tile + lx) as usize;
								let s = state.coverage[at];
								state.coverage[at] = 1.0 - (1.0 - s) * (1.0 - flow * d);
							}
						}
					}
				}
				if dirty[0] > dirty[2] {
					return Ok(((*tx, *ty), state.working.clone()));
				}
				self.recompute(*tx, *ty, state, dirty)?;
				Ok(((*tx, *ty), state.working.clone()))
			})
			.collect();
		let results = results?;
		for (key, state, _) in work {
			self.tiles.insert(key, state);
		}
		Ok(results)
	}

	/// A tile's state at its first dab: `S = 0`, the selection's coverage, and
	/// the result so far = the pixels before the stroke.
	fn new_tile(&self, (tx, ty): (u32, u32)) -> Result<TileState, TileError> {
		let tile = i64::from(TILE_SIZE);
		let x0 = i64::from(self.offset.0) + i64::from(tx) * tile;
		let y0 = i64::from(self.offset.1) + i64::from(ty) * tile;
		Ok(TileState {
			coverage: vec![0.0f32; TILE_PIXELS].into_boxed_slice(),
			selection: block_coverage(self.selection.as_ref(), &self.store, self.canvas, x0, y0)?,
			working: self.before_buffer(tx, ty)?,
		})
	}

	/// The layer tile as it was when the stroke started.
	fn before_buffer(&self, tx: u32, ty: u32) -> Result<TileBuffer, TileError> {
		Ok(match self.before.slot(0, tx, ty) {
			TileSlot::Empty => TileBuffer::zeroed(self.format),
			TileSlot::Solid(v) => TileBuffer::filled(self.format, *v),
			TileSlot::Data(handle) => (*self.store.get(handle)?).clone(),
		})
	}

	/// Recompute the result pixels of `dirty` (tile pixels, inclusive) from
	/// the pixels before the stroke and the coverage.
	fn recompute(&self, tx: u32, ty: u32, state: &mut TileState, dirty: [i64; 4]) -> Result<(), TileError> {
		let before = self.before_buffer(tx, ty)?;
		let tile = i64::from(TILE_SIZE);
		let opacity = f64::from(self.brush.opacity);
		let gray = matches!(self.format, PixelFormat::Gray8 | PixelFormat::Gray16);
		// The per-pixel op (M7-T08) and the source under this tile when it
		// asks for one (canvas tiles, prefetched).
		let op = super::op::op_for(&self.tool);
		let ctx = super::op::DabContext {
			mode: self.brush.mode,
			color: self.color,
			color_alpha: self.color_alpha,
			lock_alpha: self.lock_alpha,
		};
		let source = match op.needs().source {
			Some(offset) => Some(self.source_window(tx, ty, dirty, offset)?),
			None => None,
		};
		let width = (dirty[2] - dirty[0] + 1) as usize;
		for ly in dirty[1]..=dirty[3] {
			for lx in dirty[0]..=dirty[2] {
				let at = (ly * tile + lx) as usize;
				let k = opacity * f64::from(state.coverage[at]);
				if gray {
					let v = gray_at(&before, self.format, at);
					let out = op.gray(v, k, &ctx);
					set_gray(&mut state.working, self.format, at, out);
					continue;
				}
				let p = pixel_at(&before, self.format, at);
				let a = f64::from(p[3]);
				let backdrop = [f64::from(p[0]) * a, f64::from(p[1]) * a, f64::from(p[2]) * a, a];
				let s = source
					.as_ref()
					.map(|window| window.at((lx - dirty[0]) as usize + (ly - dirty[1]) as usize * width));
				let out = op.pixel(backdrop, s, k, &ctx);
				set_pixel(&mut state.working, self.format, at, out);
			}
		}
		Ok(())
	}

	/// The source pixels under `dirty` of layer tile `(tx, ty)`, shifted by
	/// the clone offset, sampled bilinearly (exactly for whole-pixel offsets).
	fn source_window(&self, tx: u32, ty: u32, dirty: [i64; 4], (dx, dy): (f64, f64)) -> Result<Window, TileError> {
		let tile = i64::from(TILE_SIZE);
		let width = (dirty[2] - dirty[0] + 1) as usize;
		let height = (dirty[3] - dirty[1] + 1) as usize;
		// Canvas position of the window's first pixel centre, minus the offset.
		let sx0 = (i64::from(self.offset.0) + i64::from(tx) * tile + dirty[0]) as f64 - dx;
		let sy0 = (i64::from(self.offset.1) + i64::from(ty) * tile + dirty[1]) as f64 - dy;
		let whole = dx.fract() == 0.0 && dy.fract() == 0.0;
		let mut pixels = vec![[0.0f32; 4]; width * height];
		let mut local: HashMap<(i64, i64), SourceTile> = HashMap::new();
		let canvas = (i64::from(self.canvas.0), i64::from(self.canvas.1));
		let mut fetch = |x: i64, y: i64| -> Result<[f32; 4], TileError> {
			if x < 0 || y < 0 || x >= canvas.0 || y >= canvas.1 {
				return Ok([0.0; 4]);
			}
			let key = (x.div_euclid(tile), y.div_euclid(tile));
			if let std::collections::hash_map::Entry::Vacant(entry) = local.entry(key) {
				entry.insert(self.source_tile(key.0 as u32, key.1 as u32)?);
			}
			Ok(local[&key][(y.rem_euclid(tile) * tile + x.rem_euclid(tile)) as usize])
		};
		for j in 0..height {
			for i in 0..width {
				let (sx, sy) = (sx0 + i as f64, sy0 + j as f64);
				pixels[j * width + i] = if whole {
					fetch(sx as i64, sy as i64)?
				} else {
					let (fx, fy) = (sx.floor(), sy.floor());
					let (wx, wy) = ((sx - fx) as f32, (sy - fy) as f32);
					let (x, y) = (fx as i64, fy as i64);
					let (a, b, c, d) = (fetch(x, y)?, fetch(x + 1, y)?, fetch(x, y + 1)?, fetch(x + 1, y + 1)?);
					let mut p = [0.0f32; 4];
					for k in 0..4 {
						p[k] = (a[k] * (1.0 - wx) + b[k] * wx) * (1.0 - wy) + (c[k] * (1.0 - wx) + d[k] * wx) * wy;
					}
					p
				};
			}
		}
		Ok(Window { pixels })
	}

	/// A source tile, cached for the stroke.
	fn source_tile(&self, tx: u32, ty: u32) -> Result<SourceTile, TileError> {
		if let Some(t) = self.source_cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(tx, ty)) {
			return Ok(t.clone());
		}
		let source = self.source.as_ref().ok_or(TileError::Evicted)?;
		let t = source.tile(tx, ty)?;
		self.source_cache
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.insert((tx, ty), t.clone());
		Ok(t)
	}

	/// End the stroke: the healing tools solve their blend now. Returns the
	/// layer's final image and offset.
	pub fn finish(mut self) -> Result<(TiledImage, (i32, i32)), TileError> {
		if matches!(self.tool, StrokeTool::Heal { .. } | StrokeTool::SpotHeal) {
			self.heal()?;
		}
		let mut image = self.before.clone();
		for ((tx, ty), state) in self.tiles {
			image.put_buffer(&self.store, tx, ty, state.working);
		}
		Ok((image, self.offset))
	}

	/// The healing blend over the stroke's area (M5-T08, D-045).
	fn heal(&mut self) -> Result<(), TileError> {
		let tile = i64::from(TILE_SIZE);
		// The area the stroke covered, in layer pixels, plus a 2 px border.
		let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
		for ((tx, ty), state) in &self.tiles {
			for (i, s) in state.coverage.iter().enumerate() {
				if *s > 0.0 {
					let (x, y) = (i64::from(*tx) * tile + (i as i64 % tile), i64::from(*ty) * tile + (i as i64 / tile));
					b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
				}
			}
		}
		if b[0] > b[2] {
			return Ok(());
		}
		let (iw, ih) = (i64::from(self.before.width()), i64::from(self.before.height()));
		let x0 = (b[0] - 2).max(0);
		let y0 = (b[1] - 2).max(0);
		let x1 = (b[2] + 2).min(iw - 1);
		let y1 = (b[3] + 2).min(ih - 1);
		let (w, h) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
		// Gather: coverage, the pixels before (premultiplied).
		let mut coverage = vec![0.0f32; w * h];
		let mut before = vec![[0.0f32; 4]; w * h];
		let mut buffers: HashMap<(u32, u32), TileBuffer> = HashMap::new();
		for y in 0..h {
			for x in 0..w {
				let (lx, ly) = (x0 + x as i64, y0 + y as i64);
				let key = ((lx / tile) as u32, (ly / tile) as u32);
				let at = ((ly % tile) * tile + lx % tile) as usize;
				if let Some(state) = self.tiles.get(&key) {
					coverage[y * w + x] = state.coverage[at];
				}
				if let std::collections::hash_map::Entry::Vacant(entry) = buffers.entry(key) {
					entry.insert(self.before_buffer(key.0, key.1)?);
				}
				let p = pixel_at(&buffers[&key], self.format, at);
				before[y * w + x] = [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]];
			}
		}
		// The source, shifted by the clone offset or the best spot offset.
		let canvas_origin = (i64::from(self.offset.0) + x0, i64::from(self.offset.1) + y0);
		let offset = match self.tool {
			StrokeTool::Heal { dx, dy, .. } => (dx.round() as i64, dy.round() as i64),
			_ => {
				let diameter = f64::from(self.brush.diameter);
				let candidates = heal::spot_candidates(diameter);
				let mut best = (candidates[0], f64::INFINITY);
				for &(cx, cy) in &candidates {
					let source = self.gather_source(canvas_origin, (w, h), (cx, cy))?;
					let score = heal::ring_ssd(&coverage, &before, &source, w, h);
					if score < best.1 {
						best = ((cx, cy), score);
					}
				}
				best.0
			}
		};
		let source = self.gather_source(canvas_origin, (w, h), offset)?;
		let healed = heal::poisson(&coverage, &before, &source, w, h);
		// Write back: before + (healed − before) · opacity · S, premultiplied.
		let opacity = self.brush.opacity;
		for y in 0..h {
			for x in 0..w {
				let i = y * w + x;
				let k = opacity * coverage[i];
				if k <= 0.0 {
					continue;
				}
				let (lx, ly) = (x0 + x as i64, y0 + y as i64);
				let key = ((lx / tile) as u32, (ly / tile) as u32);
				let at = ((ly % tile) * tile + lx % tile) as usize;
				let mut p = [0.0f64; 4];
				for c in 0..4 {
					p[c] = f64::from(before[i][c] + (healed[i][c] - before[i][c]) * k);
				}
				if let Some(state) = self.tiles.get_mut(&key) {
					set_pixel(&mut state.working, self.format, at, p);
				}
			}
		}
		Ok(())
	}

	/// The source's premultiplied pixels over a canvas rectangle, shifted so
	/// pixel `p` reads the source at `p − offset`.
	fn gather_source(&self, origin: (i64, i64), (w, h): (usize, usize), (dx, dy): (i64, i64)) -> Result<Vec<[f32; 4]>, TileError> {
		let tile = i64::from(TILE_SIZE);
		let canvas = (i64::from(self.canvas.0), i64::from(self.canvas.1));
		let mut out = vec![[0.0f32; 4]; w * h];
		for y in 0..h {
			for x in 0..w {
				let (sx, sy) = (origin.0 + x as i64 - dx, origin.1 + y as i64 - dy);
				if sx < 0 || sy < 0 || sx >= canvas.0 || sy >= canvas.1 {
					continue;
				}
				let t = self.source_tile((sx / tile) as u32, (sy / tile) as u32)?;
				out[y * w + x] = t[((sy % tile) * tile + sx % tile) as usize];
			}
		}
		Ok(out)
	}
}

/// Source pixels under a dirty rectangle.
struct Window {
	pixels: Vec<[f32; 4]>,
}

impl Window {
	fn at(&self, i: usize) -> [f32; 4] {
		self.pixels[i]
	}
}

/// Straight RGBA `0..=1` of pixel `at` of an RGBA tile.
fn pixel_at(buffer: &TileBuffer, format: PixelFormat, at: usize) -> [f32; 4] {
	match format {
		PixelFormat::Rgba16 => {
			let s = &buffer.as_u16()[at * 4..at * 4 + 4];
			[s[0], s[1], s[2], s[3]].map(|c| f32::from(c) / 65535.0)
		}
		_ => {
			let s = &buffer.bytes()[at * 4..at * 4 + 4];
			[s[0], s[1], s[2], s[3]].map(|c| f32::from(c) / 255.0)
		}
	}
}

/// Write premultiplied `p` (f64) as straight RGBA into pixel `at`, rounding once.
fn set_pixel(buffer: &mut TileBuffer, format: PixelFormat, at: usize, p: [f64; 4]) {
	let a = p[3].clamp(0.0, 1.0);
	let straight = if a > 0.0 { [p[0] / a, p[1] / a, p[2] / a, a] } else { [0.0; 4] };
	match format {
		PixelFormat::Rgba16 => {
			for (o, c) in buffer.as_u16_mut()[at * 4..at * 4 + 4].iter_mut().zip(straight) {
				*o = (c.clamp(0.0, 1.0) * 65535.0).round() as u16;
			}
		}
		_ => {
			for (o, c) in buffer.bytes_mut()[at * 4..at * 4 + 4].iter_mut().zip(straight) {
				*o = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
			}
		}
	}
}

fn gray_at(buffer: &TileBuffer, format: PixelFormat, at: usize) -> f64 {
	match format {
		PixelFormat::Gray16 => f64::from(buffer.as_u16()[at]) / 65535.0,
		_ => f64::from(buffer.bytes()[at]) / 255.0,
	}
}

fn set_gray(buffer: &mut TileBuffer, format: PixelFormat, at: usize, v: f64) {
	let v = v.clamp(0.0, 1.0);
	match format {
		PixelFormat::Gray16 => buffer.as_u16_mut()[at] = (v * 65535.0).round() as u16,
		_ => buffer.bytes_mut()[at] = (v * 255.0).round() as u8,
	}
}

/// Paint a whole stroke at once (the replay of `Command::Stroke`): the same
/// code as the live stroke, so the pixels are identical.
pub fn replay(setup: StrokeSetup<'_>, samples: &[StrokeSample], store: &TileStore) -> Result<(TiledImage, (i32, i32)), CommandError> {
	let mut stroke = Stroke::begin(setup, store)?;
	stroke.add(samples).map_err(tile_error)?;
	stroke.finish().map_err(tile_error)
}

fn tile_error(error: TileError) -> CommandError {
	CommandError::NotAllowed(format!("the stroke could not read its tiles: {error}"))
}
