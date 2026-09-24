//! GPU tile atlas (M1-T06).
//!
//! One `wgpu::Texture` array of 256×256 layers per format class:
//! * `Rgba16Float` pages for colour tiles (8-bit and 16-bit documents alike —
//!   display path only; committed results are computed from the CPU tiles or
//!   in f32 compute shaders, see ARCHITECTURE.md §6.3),
//! * `R16Float` pages for masks.
//!
//! A slot is identified by `(TileId)` for source tiles and by a
//! `CompositeKey` for composite results. Budget: `gpu_budget` bytes
//! (default 6 GiB on the 12 GB reference GPU → 6 GiB / 512 KiB ≈ 12 000 slots).
//! Eviction: LRU by frame number; slots used in the current frame are pinned.
//!
//! Upload: CPU tiles are converted (u8/u16 straight → f16 premultiplied) on a
//! worker thread into a staging buffer, then `queue.write_texture`. At most
//! `upload_budget_per_frame` tiles per frame (default 48 ≈ 24 MB/frame) so a
//! fast pan never drops below 60 fps; the viewport shows coarser levels until
//! uploads catch up.

/// Placeholder so the crate compiles; replaced in M1-T06.
pub struct TileAtlas {
	_private: (),
}
