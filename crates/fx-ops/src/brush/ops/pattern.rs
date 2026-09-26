//! The Pattern Stamp's source (M8-T06): a document pattern tiled over the
//! canvas, from `origin` (the canvas origin when Aligned, so the pattern
//! continues between strokes; the stroke's first point otherwise).
//!
//! Impressionist: every source pixel is read from a jittered position
//! (seeded by the pixel, so live = replay). VERIFY: Photoshop's
//! impressionist look is a blotchy paint effect, not this jitter.

use std::sync::Arc;

use fx_core::pattern::Pattern;
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileError};

use crate::brush::stroke::{SourceTile, SourceTiles};

pub struct PatternTiles {
	pub pattern: Pattern,
	pub origin: (i64, i64),
	pub impressionist: bool,
}

impl SourceTiles for PatternTiles {
	fn tile(&self, tx: u32, ty: u32) -> Result<SourceTile, TileError> {
		let tile = i64::from(TILE_SIZE);
		let mut out = vec![[0.0f32; 4]; TILE_PIXELS];
		for (i, p) in out.iter_mut().enumerate() {
			let (x, y) = (i64::from(tx) * tile + (i as i64 % tile), i64::from(ty) * tile + (i as i64 / tile));
			let (mut sx, mut sy) = (x - self.origin.0, y - self.origin.1);
			if self.impressionist {
				let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) as u64;
				let h = h.wrapping_mul(0x9e37_79b9_7f4a_7c15);
				sx += ((h >> 40) % 9) as i64 - 4;
				sy += ((h >> 20) % 9) as i64 - 4;
			}
			let c = self.pattern.at(sx, sy);
			let a = c[3] as f32;
			*p = [c[0] as f32 * a, c[1] as f32 * a, c[2] as f32 * a, a];
		}
		Ok(Arc::new(out))
	}
}
