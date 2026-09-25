//! The native `.fxd` format (M3): an append-only, tile-addressable log.
//!
//! * the container — header, chunk framing, footer and crash recovery (M3-T01);
//! * the manifest — the versioned serde model of the document (M3-T02).

mod container;
pub mod manifest;
pub mod open;
pub mod save;
#[cfg(test)]
mod tests;

pub use container::{ChunkKind, ChunkRef, Codec, Footer, FxdFile, FxdWriter, TilePayload, parse_tile_payload};
