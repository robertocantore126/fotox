//! The Quick Selection tool (M9-T06). Filled in by the card.

use fx_core::selection::Selection;
use fx_core::{BitDepth, CommandError};
use fx_tiles::TileStore;

use crate::flood::WandSource;

pub fn quick_select(
	_source: &dyn WandSource,
	_size: (u32, u32),
	_dabs: &[(f64, f64, f64)],
	_enhance_edge: bool,
	_depth: BitDepth,
	_store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	Err(CommandError::NotAllowed("Quick Selection is not written yet".into()))
}
