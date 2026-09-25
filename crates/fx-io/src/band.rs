//! The band every importer assembles (crate docs, "Streaming is mandatory"):
//! rows arrive in order, already converted to RGBA of the document depth,
//! and every 256 rows are cut into tiles and inserted into the image.

use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileStore, TiledImage};
use rayon::prelude::*;

use crate::IoError;

/// Rows per band = tile height.
pub(crate) const BAND: usize = TILE_SIZE as usize;

/// One row of RGBA samples (`width × 4`), 8- or 16-bit.
#[derive(Clone, Copy)]
pub(crate) enum RowRef<'a> {
	U8(&'a [u8]),
	U16(&'a [u16]),
}

/// The 256-row band being assembled, and where it starts in the image.
pub(crate) struct Band {
	format: PixelFormat,
	row_samples: usize,
	u8: Vec<u8>,
	u16: Vec<u16>,
	/// Image row of the band's first row.
	y0: usize,
	/// Rows filled so far.
	filled: usize,
}

impl Band {
	/// A band for an image of `format` whose rows have `row_samples` samples.
	pub(crate) fn new(format: PixelFormat, row_samples: usize) -> Self {
		let sixteen = format == PixelFormat::Rgba16;
		Self {
			format,
			row_samples,
			u8: if sixteen { Vec::new() } else { vec![0; row_samples * BAND] },
			u16: if sixteen { vec![0; row_samples * BAND] } else { Vec::new() },
			y0: 0,
			filled: 0,
		}
	}

	/// Append image row `y` (rows must arrive in order).
	pub(crate) fn push_row(&mut self, row: RowRef<'_>, image: &mut TiledImage, store: &TileStore, y: usize) -> Result<(), IoError> {
		debug_assert_eq!(y, self.y0 + self.filled, "rows arrive in order");
		let at = self.filled * self.row_samples;
		match row {
			RowRef::U8(r) => self.u8[at..at + self.row_samples].copy_from_slice(r),
			RowRef::U16(r) => self.u16[at..at + self.row_samples].copy_from_slice(r),
		}
		self.filled += 1;
		if self.filled == BAND {
			self.flush(image, store)?;
		}
		Ok(())
	}

	/// Cut the filled rows into tiles (rows below them are transparent) and
	/// insert them; start the next band.
	pub(crate) fn flush(&mut self, image: &mut TiledImage, store: &TileStore) -> Result<(), IoError> {
		if self.filled == 0 {
			return Ok(());
		}
		// Rows past the image bottom must be transparent in the edge tiles.
		let tail = self.filled * self.row_samples;
		if let Some(rest) = self.u8.get_mut(tail..) {
			rest.fill(0);
		}
		if let Some(rest) = self.u16.get_mut(tail..) {
			rest.fill(0);
		}

		let ty = (self.y0 / BAND) as u32;
		let cols = image.grid(0).cols();
		let tiles: Vec<(u32, TileBuffer)> = (0..cols).into_par_iter().map(|tx| (tx, self.tile(tx))).collect();
		for (tx, tile) in tiles {
			image.put_buffer(store, tx, ty, tile);
		}
		self.y0 += BAND;
		self.filled = 0;
		Ok(())
	}

	/// Tile column `tx` of the band; pixels right of the image are transparent.
	fn tile(&self, tx: u32) -> TileBuffer {
		let size = TILE_SIZE as usize;
		let x0 = tx as usize * size;
		let width = self.row_samples / 4;
		let copy = size.min(width - x0) * 4;
		let mut tile = TileBuffer::zeroed(self.format);
		if self.format == PixelFormat::Rgba16 {
			let out = tile.as_u16_mut();
			for r in 0..BAND {
				let src = &self.u16[r * self.row_samples + x0 * 4..][..copy];
				out[r * size * 4..][..copy].copy_from_slice(src);
			}
		} else {
			let out = tile.bytes_mut();
			for r in 0..BAND {
				let src = &self.u8[r * self.row_samples + x0 * 4..][..copy];
				out[r * size * 4..][..copy].copy_from_slice(src);
			}
		}
		tile
	}
}
