# Layer scaling: 4K × 6,000 layers (2026-10-03)

Rob asked for a stress test of a 4096 × 4096 document with 6,000 layers.
`audit_scale::target_benchmark` builds it layer by layer: log-uniform
256–2048 px gradient rectangles with noise (incompressible), a mask every 7th
layer, a drop shadow every 20th, an Invert adjustment every 25th, a Smart
Object every 50th, a group every 10 layers. 18.2 GB of raw pixels painted;
budgets hot 1 GB, warm 512 MB (`memory_budget_mb`, `warm_budget_mb`), scratch
on E:. A watchdog killed the test if commit passed 95 % or free RAM stayed
under 250 MB (other programs held ~32 of 39 GB committed).

## The slowdown

Before the fix each layer cost more than the last: 0.28 s per layer up to
500 layers, 0.70 s at 1,000, 1.00 s at 1,500, and 42 GB read back from
scratch for a 1,500-layer document; 6,000 would have taken about 3 hours.

A read profiler (`FOTOX_READ_STATS=1`, `fx_tiles::readstats`: counts reads
that leave the hot tier and samples the caller) put **96 % of tile reads in
`mips::compute_tile`**: every layer's mips were recomputed from level 0 over
and over. Two causes (commit 5ff9e44):

1. **Stale batches thrown away** (`PERF(stale)`). Mips are computed on a copy
   of the document; when the document was edited meanwhile the whole batch
   was discarded, and requests from an older snapshot were dropped. A
   document edited faster than its mips compute never got them. Now a stale
   batch still installs the mips of ordinary images whose level-0 tiles are
   the same (by identity); effects and caches still need the same
   generation.
2. **Mips dropped first** (`PERF(mips)`). The trim dropped derived tiles
   before writing pixels to scratch. A mip costs one read to bring back,
   up to 4^level level-0 reads to recompute. Mips now go to scratch like
   pixels (dropped only when scratch is full), and level-0 pixels leave warm
   first.

| 4K × 1,500 layers | before | fix 1 only | both |
| --- | --- | --- | --- |
| build | 993 s | 572 s | **291 s** |
| per 500 layers | 140 / 351 / 502 s | 170 / 165 / 236 s | **76 / 103 / 111 s** |
| read from scratch | 42 GB | 43 GB | **0.12 GB** |
| pan at fit, p50 | — | 14 ms | **1.9 ms** |

Fix 2 alone changed little (514 → 467 s at 1,000 layers): the mips it kept
were still thrown away by cause 1.

## 4K × 6,000 layers, with the fix

| | |
| --- | --- |
| built | **1,679 s** (28 min), ~110 s per 500 layers at first, ~170 s at the end |
| add one layer (fill + noise + extras), p50 first / last 10 % | 108 / 224 ms (p95 0.5 / 1.2 s) |
| process | working set 2.2 GB, private 4.4 GB; scratch 13 GB on E:; max system commit 91 % |
| read from scratch during the build | 1.3 GB |
| pan at fit, p50 / p95 | 51 / 92 ms |
| brush, pointer → frame, top / middle layer | 1.6 / 2.2 ms |
| pointer up → History | 16–23 ms |
| group hide / show | 15 / 11 ms |
| undo / redo | 14 / 13 ms |
| add an empty layer | 16 ms |
| Save As (6.4 GB .fxd) | 95 s |
| incremental save after a stroke | 202 ms |
| reopen → first view | 32 s |

## Reads queued on one handle (D-099)

Reopen and Save As were slow for one reason: on Windows every read through
one synchronous file handle waits for the one before it, and both the
scratch file and an open `.fxd` had one handle shared by all threads.

- **Reopen** checked each tile's chunk header (the D5 rollback check) with
  two file-length queries and a 16-byte read, one tile at a time. A cold
  open at 1,500 layers spent 9.4 s there (0.29 s when the file was in the
  OS cache); sorting the reads by offset alone changed nothing (9.1 s).
  Now the checks run in one batch, in file order, from several threads.
- **Save As** decompresses tiles read back from scratch on 12 threads, which
  queued on the scratch handle.

`fx_tiles::ReadPool` gives each thread its own handle (`ReOpenFile`, same
file object target). Probes: `scratch::probe_parallel_reads` (12 threads,
4 GB of 96 KiB reads on E:), `open::probe_open_time` (`FOTOX_FXD_PROBE`),
`save::probe_save_as_from_scratch`.

| | before | after |
| --- | --- | --- |
| scratch reads, 12 threads | 262 MiB/s | **853 MiB/s** (= a handle opened per thread) |
| cold `.fxd` open, 1,500 layers (build the document) | 9.4 s | **0.85 s** |
| same, file in the OS cache | 0.29 s | **0.12 s** |
| engine reopen → first view, 4K × 1,500 layers | 6.8 s | **0.45 s** |
| Save As, 4K × 1,500 layers (1.6 GB file) | 19.4 s | **11.3 s** |

**Save As reads without promoting.** A save read every tile through
`TileStore::get`, which installs a hot copy: a Save As of a big document
pushed all of it through RAM, evicting the tiles being worked on and
keeping the trim busy. `TileStore::get_streaming` decodes a tile that is
not hot for the caller only. `save::probe_save_as_from_scratch` (16,384
tiles, A/B alternated three times): 5.0–5.4 s → 4.2–4.4 s.

Brush, undo and incremental save unchanged within noise. `audit_save`: the
corruption and truncation classifications are identical before and after;
its two failures (`cancel_at_every_batch_boundary`,
`compaction_bounds_growth`) fail the same way on the old code (exFAT cannot
replace an open file).

## Open

- **6,000 layers not re-measured** with D-099 (reopen was 32 s, Save As
  95 s).
- Full benchmark runs are noisy on this PC: with other programs holding
  most of the RAM, free memory dipped to 250–500 MB during one 1,500-layer
  run and Save As took 17.7 s, reopen 0.9 s, pan at fit p50 13.5 ms. Compare
  changes A/B in one sitting (the probes), not across runs.
- **Tiles left after close**: ~1 GB hot (and with this fix ~0.5 GB warm and
  2.5 GB scratch) stay after the document closes in `target_benchmark`, in
  both the old and new code; `close_releases_tiles` (plain pixel layers)
  leaves nothing, so the holder is one of the extras (Smart Objects, styles,
  adjustments, masks, groups).
- **"View settled after the build": no frame** at 6,000 layers (no frame within
  3 s of asking); pan at fit did produce frames.
- GPU atlas: stayed 1.5 GB here, grew to 4.6 GB (within its 6 GB cap) in the
  1,500-layer run and the 300-layer close probe; growth is demand-driven.
- Interactive at 1,500 layers is fast (pan 1.9 ms); at 6,000, pan at fit is
  51 ms — every layer's mips are composited for the whole view.
