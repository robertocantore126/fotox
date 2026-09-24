//! Lazy tile compositor (M1-T07 single layer, M2 full stack).
//!
//! Contract
//! * Input: an immutable `Arc<fx_core::Document>` snapshot + a tile
//!   coordinate `(level, tx, ty)`.
//! * Output: one composited Rgba16Float tile in the atlas.
//! * Cost is proportional to the number of *non-empty* layer tiles at that
//!   coordinate, never to the document size.
//!
//! Caches (all keyed so they need no manual invalidation):
//! * **Tile composite cache** — key = hash of, for each contributing layer:
//!   `(layer id, TileId or Solid value of its slot, opacity, blend, visible,
//!   mask slot, adjustment params)`. Because tiles are immutable, the key
//!   changes exactly when the result would. Unchanged tiles after an edit are
//!   cache hits automatically.
//! * **Stack split cache (M2-T05)** — while a layer is being edited
//!   (painting, moving, changing opacity), the stack is split into
//!   `below` (all layers under it, composited once), the edited layer, and
//!   `above` (composited once, only valid for Normal-mode layers above; a
//!   non-Normal layer above forces re-blending from that layer up).
//!   Painting on layer 347 of 800 then costs ~3 blends per tile.
//!
//! Blend order and group semantics follow docs/BLEND_MODES.md exactly
//! (pass-through groups, clipping masks, knockout not supported yet).

/// Placeholder so the crate compiles; replaced in M1-T07.
pub struct Compositor {
	_private: (),
}
