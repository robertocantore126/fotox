//! Smart Filters (M12-T03): filled in by that card.

use fx_ops::resample::SourceInfo;
use fx_tiles::{TileBuffer, TileError, TileStore};

use crate::ops::ImageSource;

/// Run `filters` over freshly resampled Smart Object tiles.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(
	_view: &ImageSource<'_>,
	_source: SourceInfo,
	_transform: fx_core::Mapping,
	_filters: &[fx_core::smart::SmartFilter],
	_level: usize,
	_canvas: (u32, u32),
	tiles: Vec<((u32, u32), TileBuffer)>,
	_store: &TileStore,
) -> Result<Vec<((u32, u32), TileBuffer)>, TileError> {
	Ok(tiles)
}
