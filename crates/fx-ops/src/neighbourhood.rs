//! Reading a rectangle of pixels around a tile, as premultiplied f32
//! (docs/tasks/HOWTO.md R8, SNIPPETS §2 and §4).

use std::sync::Arc;

use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileError};

/// One tile as the filters see it.
#[derive(Clone)]
pub enum TileRef {
	/// Every pixel has this 16-bit straight value.
	Solid([u16; 4]),
	Data(Arc<TileBuffer>),
}

/// The pixels of one image, level by level, in the image's own (layer-local)
/// tile grid. Implemented by the engine over a `TiledImage` + the tile store.
pub trait LevelSource: Sync {
	/// Format of the image (RGBA only for filters).
	fn format(&self) -> PixelFormat;
	/// Tile `(tx, ty)` of `level`; `None` = transparent (an empty tile, or
	/// outside the image). The caller has made the mips it reads valid.
	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError>;
}

/// A half-open rectangle of pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
	pub x0: i64,
	pub y0: i64,
	pub x1: i64,
	pub y1: i64,
}

impl Rect {
	pub fn width(&self) -> usize {
		(self.x1 - self.x0).max(0) as usize
	}

	pub fn height(&self) -> usize {
		(self.y1 - self.y0).max(0) as usize
	}

	pub fn is_empty(&self) -> bool {
		self.width() == 0 || self.height() == 0
	}
}

/// Premultiplied RGBA, 0..=1.
pub type Px = [f32; 4];

/// Straight 16-bit → premultiplied f32.
#[inline]
pub fn premul16([r, g, b, a]: [u16; 4]) -> Px {
	let a = f32::from(a) / 65535.0;
	[f32::from(r) / 65535.0 * a, f32::from(g) / 65535.0 * a, f32::from(b) / 65535.0 * a, a]
}

/// Premultiplied → straight, 0..=1 (transparent pixels have no colour).
#[inline]
pub fn unpremul([r, g, b, a]: Px) -> Px {
	if a <= 1.0 / 65535.0 {
		[0.0; 4]
	} else {
		[(r / a).clamp(0.0, 1.0), (g / a).clamp(0.0, 1.0), (b / a).clamp(0.0, 1.0), a.clamp(0.0, 1.0)]
	}
}

/// Read `area` (layer-local pixels of `level`) as premultiplied f32, row-major.
///
/// Positions are first clamped to `canvas` — the canvas in the same layer-local
/// coordinates — so outside the document a filter sees the nearest canvas
/// pixel (edge replicate, D-036); inside the canvas, where the layer has no
/// pixels, it sees transparency. Each tile is fetched once per call.
pub fn gather(src: &dyn LevelSource, level: usize, area: Rect, canvas: Rect) -> Result<Vec<Px>, TileError> {
	let t = i64::from(TILE_SIZE);
	let clamp = |v: i64, lo: i64, hi: i64| if hi > lo { v.clamp(lo, hi - 1) } else { v };
	// Per column / per row: the source tile index and the pixel inside it.
	let cols: Vec<(i64, usize)> = (area.x0..area.x1)
		.map(|x| {
			let x = clamp(x, canvas.x0, canvas.x1);
			(x.div_euclid(t), x.rem_euclid(t) as usize)
		})
		.collect();
	let rows: Vec<(i64, usize)> = (area.y0..area.y1)
		.map(|y| {
			let y = clamp(y, canvas.y0, canvas.y1);
			(y.div_euclid(t), y.rem_euclid(t) as usize)
		})
		.collect();
	let format = src.format();
	let mut out = Vec::with_capacity(area.width() * area.height());
	let mut cache: Vec<((i64, i64), Option<TileRef>)> = Vec::new();
	for &(ty, py) in &rows {
		for &(tx, px) in &cols {
			let tile = match cache.iter().find(|(k, _)| *k == (tx, ty)) {
				Some((_, tile)) => tile.clone(),
				None => {
					let tile = src.tile(level, tx, ty)?;
					cache.push(((tx, ty), tile.clone()));
					tile
				}
			};
			out.push(match tile {
				None => [0.0; 4],
				Some(TileRef::Solid(v)) => premul16(v),
				Some(TileRef::Data(buffer)) => premul16(pixel(&buffer, format, px, py)),
			});
		}
	}
	Ok(out)
}

/// One pixel of an RGBA tile as straight 16-bit values.
#[inline]
pub fn pixel(buffer: &TileBuffer, format: PixelFormat, x: usize, y: usize) -> [u16; 4] {
	let i = (y * TILE_SIZE as usize + x) * 4;
	match format {
		PixelFormat::Rgba16 => {
			let s = buffer.as_u16();
			[s[i], s[i + 1], s[i + 2], s[i + 3]]
		}
		PixelFormat::Rgba8 => {
			let b = buffer.bytes();
			[b[i], b[i + 1], b[i + 2], b[i + 3]].map(|v| u16::from(v) * 257)
		}
		// Filters run on RGBA layers only (masks in M5): read as grey.
		PixelFormat::Gray16 => {
			let v = buffer.as_u16()[y * TILE_SIZE as usize + x];
			[v, v, v, u16::MAX]
		}
		PixelFormat::Gray8 => {
			let v = u16::from(buffer.bytes()[y * TILE_SIZE as usize + x]) * 257;
			[v, v, v, u16::MAX]
		}
	}
}

/// Write straight 0..=1 pixels (256², row-major) into a new tile of `format`,
/// rounded (SNIPPETS §1).
pub fn to_tile(pixels: &[Px], format: PixelFormat) -> TileBuffer {
	let mut tile = TileBuffer::zeroed(format);
	match format {
		PixelFormat::Rgba16 => {
			let out = tile.as_u16_mut();
			for (i, p) in pixels.iter().enumerate() {
				for c in 0..4 {
					out[i * 4 + c] = (p[c].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
				}
			}
		}
		PixelFormat::Rgba8 => {
			let out = tile.bytes_mut();
			for (i, p) in pixels.iter().enumerate() {
				for c in 0..4 {
					out[i * 4 + c] = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
				}
			}
		}
		PixelFormat::Gray16 | PixelFormat::Gray8 => unreachable!("filters write RGBA layers only"),
	}
	tile
}
