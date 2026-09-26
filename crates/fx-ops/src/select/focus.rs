//! Select ▸ Focus Area (M9-T04). Filled in by the card.

use fx_core::selection::Selection;
use fx_core::{BitDepth, CommandError};
use fx_tiles::TileStore;

use crate::flood::WandSource;

pub fn focus_area(
	_source: &dyn WandSource,
	_size: (u32, u32),
	_in_focus: f32,
	_noise: f32,
	_soften: bool,
	_depth: BitDepth,
	_store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	Err(CommandError::NotAllowed("Focus Area is not written yet".into()))
}
