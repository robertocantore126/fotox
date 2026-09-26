//! Alpha channels (M9-T01, D-068): stored selections, document data saved in
//! the `.fxd`. The colour channels (R, G, B) are views of the composite and
//! are not stored.

use fx_tiles::{TileError, TileStore, TiledImage};

use crate::color::BitDepth;
use crate::selection::{OutTile, Selection, TileCoverage, canvas_grid, out_tile, uniform_slot, valid_extent};

/// One alpha channel: grey coverage, canvas-aligned (offset 0), document
/// size.
#[derive(Clone, Debug)]
pub struct Channel {
	pub name: String,
	pub image: TiledImage,
	/// The overlay colour when shown (straight 16-bit RGBA).
	pub color: [u16; 4],
	/// `0..=1`.
	pub opacity: f32,
}

impl Channel {
	pub fn new(name: impl Into<String>, image: TiledImage) -> Self {
		Self {
			name: name.into(),
			image,
			color: [65535, 0, 0, 65535],
			opacity: 0.5,
		}
	}

	/// The channel as a selection (offset 0).
	pub fn as_selection(&self) -> Selection {
		Selection {
			image: self.image.clone(),
			offset: (0, 0),
		}
	}
}

/// A selection rewritten canvas-aligned (offset 0), tile by tile; an empty
/// image when nothing is selected. Only the canvas tiles the selection
/// reaches are read.
pub fn canvas_aligned(selection: &Selection, size: (u32, u32), depth: BitDepth, store: &TileStore) -> Result<TiledImage, TileError> {
	let (cols, rows) = canvas_grid(size);
	let format = depth.gray_format();
	// Tile by tile into the image: never all the tiles in memory at once.
	let mut image = TiledImage::new(size.0, size.1, format);
	for ty in 0..rows {
		for tx in 0..cols {
			match selection.tile_coverage(store, tx, ty)? {
				TileCoverage::Uniform(v) => image.set_slot(tx, ty, uniform_slot(format, v)),
				coverage => match out_tile(format, &coverage.into_values(), valid_extent(size, tx, ty)) {
					OutTile::Uniform(v) => image.set_slot(tx, ty, uniform_slot(format, v)),
					OutTile::Data(buffer) => image.put_buffer(store, tx, ty, buffer),
				},
			}
		}
	}
	Ok(image)
}
