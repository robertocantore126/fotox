//! Select and Mask (M9-T05). Filled in by the card.

use fx_core::select_ops::Refine;
use fx_core::selection::Selection;
use fx_core::{BitDepth, CommandError};
use fx_tiles::TileStore;

use crate::flood::WandSource;

pub fn refine(
	_source: &dyn WandSource,
	_selection: &Selection,
	_size: (u32, u32),
	_params: &Refine,
	_depth: BitDepth,
	_store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	Err(CommandError::NotAllowed("Select and Mask is not written yet".into()))
}
