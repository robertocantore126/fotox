//! Select ▸ Color Range (M9-T03). Filled in by the card.

use fx_core::select_ops::SelectOp;
use fx_core::selection::Selection;
use fx_core::{BitDepth, CommandError};
use fx_tiles::TileStore;

use crate::flood::WandSource;

pub fn color_range(
	_source: &dyn WandSource,
	_size: (u32, u32),
	_op: &SelectOp,
	_depth: BitDepth,
	_store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	Err(CommandError::NotAllowed("Color Range is not written yet".into()))
}
