//! # fx-render — everything that touches the GPU
//!
//! Pipeline (docs/ARCHITECTURE.md §4):
//!
//! ```text
//! ViewTransform ──► visible tiles at mip level L
//!                        │
//!                        ▼
//!   CompositeCache hit? ──yes──► draw tile quad into viewport texture
//!         │ no
//!         ▼
//!   Compositor: for each visible layer (bottom → top), get its level-L tile
//!   into the TileAtlas (upload if needed), blend into an accumulator tile,
//!   store result in CompositeCache ──► draw
//!         │ tile not ready (upload budget spent / level-L mip dirty)?
//!         ▼
//!   draw the parent tile at level L+1 upscaled (never block a frame)
//! ```
//!
//! * [`viewport`] — pure math, implemented.
//! * [`atlas`] — GPU residency of tiles (M1-T06).
//! * [`compositor`] — lazy tile compositing + caches (M1-T07, M2).
//! * [`reference`] — CPU reference implementations used to test shaders (M2-T02).

pub mod atlas;
pub mod compositor;
pub mod reference;
pub mod viewport;

pub use viewport::{TileRange, ViewTransform, ViewportSize};
