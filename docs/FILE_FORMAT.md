# Native file format `.fxd`

Status: **final for format version 1** (M3). This file is normative; the byte
layout below is the same table as task card M3-T01. Goals: open instantly at
any size, save only what changed, never corrupt the previous version on a
crash.

All integers are **little-endian**.

## Layout: append-only log

```
[Header 64 B] [chunk] [chunk] … [chunk] [Footer 64 B]
```

### Header, 64 bytes, at offset 0

| Offset | Type | Field |
| --- | --- | --- |
| 0 | `[u8; 8]` | magic `"FOTOXFXD"` |
| 8 | `u32` | format version = 1 |
| 12 | `u32` | flags = 0 (reserved) |
| 16 | `[u8; 16]` | writer, e.g. `"fotox 0.0.0"`, zero-padded (informational) |
| 32 | `[u8; 32]` | reserved, zero |

### Chunk = 16-byte chunk header + payload

| Offset | Type | Field |
| --- | --- | --- |
| 0 | `u8` | kind: 1 `TILE`, 2 `MANIFEST`, 3 `PREVIEW_TILE` |
| 1 | `u8` | flags = 0 |
| 2 | `u16` | reserved = 0 |
| 4 | `u64` | payload length in bytes |
| 12 | `u32` | checksum of the payload (`crc32fast`, D-023) |

`TILE` / `PREVIEW_TILE` payload:

| Offset | Type | Field |
| --- | --- | --- |
| 0 | `u8` | `PixelFormat` (0 `Rgba8`, 1 `Rgba16`, 2 `Gray8`, 3 `Gray16`) |
| 1 | `u8` | codec (0 raw, 1 zstd, 2 lz4) |
| 2 | `u16` | reserved |
| 4 | `u32` | uncompressed length (= `format.tile_bytes()`, checked) |
| 8 | … | compressed bytes |

`MANIFEST` payload: a zstd frame of UTF-8 JSON (M3-T02, D-025).

### Footer, 64 bytes, always the last 64 bytes written by a save

| Offset | Type | Field |
| --- | --- | --- |
| 0 | `[u8; 8]` | magic `"FXDEND01"` |
| 8 | `u64` | offset of the `MANIFEST` chunk header |
| 16 | `u64` | `MANIFEST` chunk total length (header + payload) |
| 24 | `u64` | end offset of this footer (= its offset + 64) |
| 32 | `u64` | bytes of live data after this save (header + referenced chunks + footer) |
| 40 | `[u8; 20]` | reserved, zero |
| 60 | `u32` | checksum of bytes 0..60 |

## Operations

* **Open**: read the footer → the manifest → build the `Document` with tiles
  in backed residency (D-027). Nothing else is read. Pixel chunks are loaded
  on demand like cold tiles.
* **Save** (incremental): for each tile, if it is already backed by *this*
  file, reuse its chunk offset; otherwise compress (zstd level 1, D-024) and
  append a `TILE`. Append changed preview tiles, then the new `MANIFEST`, then
  the footer. Order: chunks → `sync_data` → footer → `sync_data`. The previous
  footer stays valid until the new one is complete, so a crash at any point
  leaves the previous version readable.
* **Recovery**: if the last 64 bytes are not a valid footer (torn write), scan
  backwards in overlapping 1 MiB blocks for the magic `FXDEND01` and accept
  the first footer whose checksum is valid **and** whose "end offset" equals
  its own position + 64 (see SNIPPETS §13). The next save appends after that
  footer; the torn bytes are overwritten.
* **Compaction**: when live data falls below 50 % of the file length (or on
  Save As), write a fresh file (zstd level 3, D-024) and atomically replace.
* **Per-layer mips** for levels ≥ 3 are stored as `TILE`s too (marked derived
  in the manifest); the flattened composite at levels ≥ 3 is stored as
  `PREVIEW_TILE`s (D-026).

## Errors

* bad magic → `IoError::UnsupportedFormat`;
* newer/unknown version → `IoError::Unsupported("fxd version N")`;
* checksum mismatch → `IoError::Decode("corrupt chunk at …")`;
* header without a valid footer → `IoError::Decode("not a complete .fxd …")`.
