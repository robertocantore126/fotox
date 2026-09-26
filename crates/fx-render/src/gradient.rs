//! Gradient and pattern fill layer tiles (M8-T03/T06, D-065): one tile of a
//! fill layer at a level, drawn from its parameters. The Gradient tool's
//! command evaluates the same `GradientFill`, so level 0 matches it.

use fx_core::fill::FillLayer;
use fx_core::gradient::GradientFill;
use fx_core::pattern::Pattern;
use fx_tiles::{PixelFormat, TILE_PIXELS, TILE_SIZE, TileBuffer};

/// Tile `(tx, ty)` of level `level` (each level-`l` pixel covers `2^l`
/// document pixels; it is sampled at its centre).
pub fn render_fill_tile(
	content: &FillLayer,
	placed: Option<&GradientFill>,
	pattern: Option<&Pattern>,
	level: usize,
	(tx, ty): (u32, u32),
	format: PixelFormat,
) -> TileBuffer {
	let scale = f64::from(1u32 << level.min(20));
	let mut pixels = vec![[0.0f32; 4]; TILE_PIXELS];
	for (i, p) in pixels.iter_mut().enumerate() {
		let (px, py) = (i as u32 % TILE_SIZE, i as u32 / TILE_SIZE);
		let (lx, ly) = (i64::from(tx * TILE_SIZE + px), i64::from(ty * TILE_SIZE + py));
		let (x, y) = ((lx as f64 + 0.5) * scale, (ly as f64 + 0.5) * scale);
		// FAST: a coarser level samples one point per pixel (no averaging), so
		// a fine pattern aliases when zoomed out.
		let c = content.color_at(placed, pattern, x, y, lx, ly);
		*p = [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32];
	}
	fx_core::pixels::encode(&pixels, format)
}
