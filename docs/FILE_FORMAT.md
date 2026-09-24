# Native file format `.fxd` — draft

Status: **draft**, finalised at the start of M3. Goals: open instantly at any
size, save only what changed, never corrupt the previous version on a crash.

## Layout: append-only log

```
[Header 64 B] [chunk] [chunk] … [chunk] [Footer 64 B]
```

* **Header**: magic `FOTOXFXD`, format version (u32), reserved.
* **Chunks** — each: `kind u8, flags u8, reserved u16, length u64, crc32c u32, payload`.
  * `TILE` — one tile: `PixelFormat` u8, codec u8 (`0` raw, `1` zstd), zstd
    level 3 payload of `format.tile_bytes()` bytes.
  * `MANIFEST` — the whole document structure, zstd-compressed JSON:
    size, colour, ppi, layer tree with all properties, adjustment
    parameters, and for every `TiledImage` (layer pixels, masks, selection)
    its tile table: per slot `Empty` | `Solid(value)` | `Tile(chunk offset)`.
  * `PREVIEW` — flattened composite, levels ≥ 3 only (≈ 1.5 % of level-0
    size), for an instant first frame and thumbnails.
* **Footer**: magic `FXDEND01`, offset of the current `MANIFEST`, its crc,
  file length at write time.

## Operations

* **Open**: read the footer → manifest → build the `Document` with tiles in
  `Backed { file, offset, len }` residency. Nothing else is read. Pixel
  chunks are loaded on demand like cold tiles. (Backed tiles are never
  copied to scratch.)
* **Save** (incremental): for each tile in the document, if it is `Backed`
  by *this* file, reuse its offset; otherwise compress (rayon) and append a
  `TILE`. Append `PREVIEW` chunks that changed, then the new `MANIFEST`, then
  the footer; `fsync`. The previous footer stays valid until the new one is
  complete.
* **Recovery**: if the last footer is torn, scan backwards for the previous
  valid footer.
* **Compaction**: when dead chunks exceed 50 % of the file (or on Save As),
  write a fresh file and atomically replace.
* **Per-layer mips** for levels ≥ 3 are stored as `TILE`s too (marked
  derived in the manifest): without them, the first frame of a 200-layer
  document would need every level-0 tile.
