//! # fx-render — compositing and everything that touches the GPU
//!
//! Pipeline (docs/ARCHITECTURE.md §4):
//!
//! ```text
//! ViewTransform ──► visible tiles at mip level L          (viewport.rs)
//!        │
//!        ▼
//! build_program(doc snapshot, L, tx, ty)                   (program.rs)
//!        │ Err(dirty mips) → engine computes them, fall back to level L+1
//!        ▼
//! key in composite cache? ──yes──► draw
//!        │ no
//!        ▼
//! GPU compositor: upload missing source tiles to the atlas (budgeted),
//! run the program for all pending tiles in one dispatch    (gpu/)
//! ```
//!
//! * [`viewport`] — view transform, level choice, visible tiles.
//! * [`program`] — per-tile composite programs + cache keys.
//! * [`blend`], [`adjust`] — blend-mode and adjustment math (f64).
//! * [`reference`] — CPU reference compositor (defines correct output).
//! * [`frame`] — per-frame plan: tiles to draw, coarser fallbacks, requests.
//! * [`gpu`] — tile atlas and GPU compositor.
//! * [`vector`] — a shape layer's tiles, rasterised from its geometry (M6-T06).
//! * [`text`] — a text layer's tiles: layout (parley), outlines (skrifa) and
//!   the same rasteriser (M6-T07).
//! * [`test_pattern`] — procedural stand-in for a document (M0).

pub mod adjust;
pub mod blend;
pub mod frame;
pub mod gpu;
pub mod overlay;
pub mod program;
pub mod reference;
pub mod test_pattern;
pub mod text;
pub mod vector;
pub mod viewport;

pub use frame::{FramePlan, TileDraw, TileKey, plan_frame};

pub use overlay::{Overlay, OverlayItem, OverlayStyle, OverlayVertex};
pub use program::{EffectRequest, MipRequest, TileProgram, TileRequest, VectorRequest, build_program};
pub use test_pattern::{TestPatternRenderer, VIEWPORT_FORMAT};
pub use text::{FontEntry, Fonts, TextLayout, TextLine, TextRect, render_text_tile};
pub use vector::render_shape_tile;
pub use viewport::{TileRange, ViewTransform, ViewportSize};

#[cfg(test)]
mod program_tests;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod text_tests;
#[cfg(test)]
mod vector_tests;
