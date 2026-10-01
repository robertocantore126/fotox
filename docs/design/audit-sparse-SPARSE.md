# SPARSE: two-level COW tile grids (untested)

Today fx-tiles/src/image.rs:46 TileGrid stores dense Vec slots and Vec bool dirty, cloned with each TiledImage snapshot. set_slot:247, set_derived_slot:274, mark_all_dirty:169 and ancestors:302 directly index these arrays. non_empty:84 enumerates row-major; manifests depend on this stable order.

Keep old TileGrid struct/implementation in this same file under feature dense-grid. Add internal grid setters/dirty getters usable by both implementations; TiledImage API/signatures unchanged. New grid has Vec<Option<Arc<Chunk>>> indexed by (ty/16)*chunk_cols+tx/16. Chunks contain256 slots/dirty flags, accessed with local (ty%16)*16+tx%16. Arc::make_mut copies only edited chunks; outer table clone copies a small Vec of pointers, not every slot. 30K level0 table64 pointers rather than13924 slots.

Default dirty flag represents missing chunks in an all-dirty derived level. One clean-empty override bit per chunk is needed: without this, freeing an entirely empty clean chunk would make it dirty again under the grid default. The clean override table is small (one bit-value entry/chunk). Partial dirty/clean overrides retain their chunk. Empty+clean chunk drops; mark_all_dirty clears clean overrides and can drop entirely empty chunks because missing chunks already express dirty. Padded edge positions start clean and empty so complete valid cleaning permits reclaim.

All iteration remains global row-major (scan logical rows/cols, skipping absent content) rather than chunk-major. Dirty iteration honours default and overrides. Reads of missing chunks return stable Empty; writing same slot preserves current early return and ancestor propagation. No runtime switch; build feature dense-grid selects old layout for comparison.

Invariants: cols/rows/assertions and ancestor stopping unchanged; set_derived_slot clears exactly one dirty flag; clone snapshots independent; transparent solid handling unchanged; preserve all 8/16-bit handles and save ordering. No document-sized pixel buffer. Metadata outer table still scales at one pointer per256 positions, intentional.

Risks: chunk COW copies256 handles on an edit, clean-empty overrides at edge grids easy to get wrong. Fully dirty empty levels must not allocate all chunks. Logical non_empty/dirty iteration remains O(canvas positions), so CPU scanning is not eliminated; sparse populated large tables still cost. Compile both layouts without executing tests; Rob/Claude compare save manifests, Undo/COW, masks/mips/effects and many sparse layers.
