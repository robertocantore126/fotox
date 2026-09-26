//! Blur and Sharpen (M8-T05): the dab lays down the layer (or the
//! composite) filtered, through the source window of the stroke engine. The
//! filtered source is computed per canvas tile from the tile and its
//! neighbours (an apron of the kernel's radius), so the work is bounded by the
//! dabs' tiles.
//!
//! FAST: one filter pass per stroke — dabs of the same stroke do not blur
//! the blur again (Photoshop's tool accumulates while you scrub); a new stroke
//! does. VERIFY: the kernel (Gaussian σ = 2 px) and the sharpen amount.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileError};

use crate::brush::stroke::{SourceTile, SourceTiles};

/// What the filtered source does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
	Blur,
	/// Unsharp mask; `protect_detail` halves the amount and clamps overshoot.
	Sharpen {
		protect_detail: bool,
	},
}

const SIGMA: f32 = 2.0;
const RADIUS: i64 = 6;

/// A source that filters another one.
pub struct FilteredTiles {
	inner: Arc<dyn SourceTiles>,
	focus: Focus,
	/// The canvas grid, to leave the neighbours past the edge out.
	grid: (u32, u32),
	cache: Mutex<HashMap<(u32, u32), SourceTile>>,
}

impl FilteredTiles {
	pub fn new(inner: Arc<dyn SourceTiles>, focus: Focus, canvas: (u32, u32)) -> Self {
		Self {
			inner,
			focus,
			grid: (canvas.0.div_ceil(TILE_SIZE), canvas.1.div_ceil(TILE_SIZE)),
			cache: Mutex::new(HashMap::new()),
		}
	}

	fn inner_tile(&self, tx: i64, ty: i64) -> Result<Option<SourceTile>, TileError> {
		if tx < 0 || ty < 0 || tx >= i64::from(self.grid.0) || ty >= i64::from(self.grid.1) {
			return Ok(None);
		}
		if let Some(t) = self
			.cache
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.get(&(tx as u32, ty as u32))
		{
			return Ok(Some(t.clone()));
		}
		let t = self.inner.tile(tx as u32, ty as u32)?;
		self.cache
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.insert((tx as u32, ty as u32), t.clone());
		Ok(Some(t))
	}
}

fn kernel() -> Vec<f32> {
	let k: Vec<f32> = (-RADIUS..=RADIUS).map(|i| (-(i * i) as f32 / (2.0 * SIGMA * SIGMA)).exp()).collect();
	let sum: f32 = k.iter().sum();
	k.into_iter().map(|v| v / sum).collect()
}

impl SourceTiles for FilteredTiles {
	fn tile(&self, tx: u32, ty: u32) -> Result<SourceTile, TileError> {
		let tile = i64::from(TILE_SIZE);
		let side = (tile + 2 * RADIUS) as usize;
		// The tile with its apron; past the canvas edge the nearest pixel
		// (D-036's edge replicate).
		let mut apron = vec![[0.0f32; 4]; side * side];
		let mut parts: HashMap<(i64, i64), Option<SourceTile>> = HashMap::new();
		let (cw, ch) = (i64::from(self.grid.0) * tile, i64::from(self.grid.1) * tile);
		for y in 0..side as i64 {
			let cy = (i64::from(ty) * tile + y - RADIUS).clamp(0, ch - 1);
			for x in 0..side as i64 {
				let cx = (i64::from(tx) * tile + x - RADIUS).clamp(0, cw - 1);
				let key = (cx / tile, cy / tile);
				if let std::collections::hash_map::Entry::Vacant(e) = parts.entry(key) {
					let t = self.inner_tile(key.0, key.1)?;
					e.insert(t);
				}
				if let Some(t) = &parts[&key] {
					apron[y as usize * side + x as usize] = t[((cy % tile) * tile + cx % tile) as usize];
				}
			}
		}
		let k = kernel();
		// Horizontal pass over every apron row, then vertical into the tile.
		let mut horizontal = vec![[0.0f32; 4]; side * TILE_SIZE as usize];
		for y in 0..side {
			for x in 0..TILE_SIZE as usize {
				let mut acc = [0.0f32; 4];
				for (i, w) in k.iter().enumerate() {
					let p = apron[y * side + x + i];
					for c in 0..4 {
						acc[c] += p[c] * w;
					}
				}
				horizontal[y * TILE_SIZE as usize + x] = acc;
			}
		}
		let mut out = vec![[0.0f32; 4]; TILE_PIXELS];
		for y in 0..TILE_SIZE as usize {
			for x in 0..TILE_SIZE as usize {
				let mut acc = [0.0f32; 4];
				for (i, w) in k.iter().enumerate() {
					let p = horizontal[(y + i) * TILE_SIZE as usize + x];
					for c in 0..4 {
						acc[c] += p[c] * w;
					}
				}
				out[y * TILE_SIZE as usize + x] = match self.focus {
					Focus::Blur => acc,
					Focus::Sharpen { protect_detail } => {
						let o = apron[(y + RADIUS as usize) * side + x + RADIUS as usize];
						let amount = if protect_detail { 0.6 } else { 1.2 };
						let mut p = [0.0f32; 4];
						for c in 0..4 {
							p[c] = o[c] + (o[c] - acc[c]) * amount;
						}
						p[3] = o[3];
						for c in 0..3 {
							p[c] = p[c].clamp(0.0, p[3]);
						}
						p
					}
				};
			}
		}
		Ok(Arc::new(out))
	}
}
