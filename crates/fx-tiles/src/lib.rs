//! # fx-tiles — the pixel storage layer of Fotox
//!
//! Everything that holds pixels holds them as **256×256 tiles** managed by a
//! [`TileStore`]. This crate knows nothing about layers, blend modes or GPUs.
//!
//! Core rules (see `docs/ARCHITECTURE.md` §3):
//!
//! 1. **Tiles are immutable.** Editing a tile means building a new
//!    [`TileBuffer`] and inserting it, which yields a new [`TileHandle`].
//!    Old handles stay valid while someone (e.g. the undo history) holds them.
//!    This is what makes undo, snapshots and background rendering cheap and safe.
//! 2. **Handles are reference counted (RAII).** Cloning a handle is cheap; the
//!    tile is freed when the last handle drops.
//! 3. **Residency is invisible to callers.** A tile may be hot (RAM), warm
//!    (LZ4-compressed in RAM), cold (on the scratch file) or backed by the
//!    source document file. [`TileStore::get`] always returns the pixels,
//!    possibly after decompressing or reading from disk.
//! 4. **Derived tiles are disposable.** Mip levels and caches are inserted as
//!    [`TileClass::Derived`]. Under memory pressure they are dropped instead of
//!    being written to disk, and `get` reports [`TileError::Evicted`] so the
//!    caller regenerates them. Scratch disk space is precious (see
//!    `docs/PERFORMANCE.md`).
//! 5. **Uniform tiles are not stored.** A fully transparent tile is
//!    [`TileSlot::Empty`], a single-colour tile is [`TileSlot::Solid`]. Only
//!    tiles with real content become [`TileSlot::Data`].

mod format;
mod image;
mod mip;
mod scratch;
mod store;

pub use format::{PixelFormat, PixelValue, TILE_PIXELS, TILE_SIZE};
pub use image::{PlacedSlot, TileGrid, TileSlot, TiledImage, slot_for};
pub use mip::{ChildPixels, downsample_2x2};
pub use store::{Backed, TileBuffer, TileClass, TileError, TileHandle, TileId, TileSource, TileStore, TileStoreConfig, TileStoreStats};
