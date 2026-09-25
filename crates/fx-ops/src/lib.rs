//! # fx-ops — pixel operations (milestones M5+)
//!
//! Every operation here works **tile by tile** and only on the tiles it
//! touches: a brush dab touches at most 4–9 tiles; a filter on a selection
//! only the tiles under the selection. Output tiles are new immutable tiles
//! (copy-on-write), committed through a `Command` so undo is automatic.
//!
//! Filters with a radius (blur, sharpen, …) read a 1-tile apron around each
//! output tile. Preview runs at the viewed mip level with the radius scaled by
//! `2^-level`; the final result is computed at level 0 in the background and
//! replaces the preview when done (docs/ARCHITECTURE.md §4.5 — preview/final
//! consistency rules).
//!
//! Modules: `brush` (dab engine, pressure, spacing, 16-bit accumulation),
//! `filter` (Gaussian blur, unsharp mask), `flood`, `gaussian`, `morph`,
//! `neighbourhood`, `raster`, and `resample` (M6-T01: Free Transform, Warp,
//! Image Size — the one place a transform's geometry meets pixels).

pub mod brush;
pub mod filter;
#[cfg(test)]
mod filter_tests;
pub mod flood;
pub mod gaussian;
pub mod morph;
pub mod neighbourhood;
pub mod raster;
pub mod resample;
