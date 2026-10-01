# Fotox audit — large documents, save integrity, Smart Objects, scratch storage

Date: 2026-10-01. Auditor: Claude (Opus 5.5), at Rob's request.
Scope as asked: performance, reliability, data safety, large-document
handling, native Smart Objects, scratch storage, input safety. Earlier
reports were treated as hypotheses and rechecked against the code
(section 8).

**This report contains no production code changes.** Everything added is
test harnesses (listed in Appendix A), all `#[ignore]`d, on a separate,
uncommitted worktree. Fixes are recommendations.

## 0. State, method, labels

| | |
| --- | --- |
| Repository | `C:\Users\39389\Documents\XuanZhi9\fotox`, branch `main` at `1188748` (pushed). Working tree: only `.freebuff/` untracked; `stash@{0}` ("wip before locks merge") still kept. Nothing there was modified. |
| Audit worktree | `E:\fotox-audit`, branch `audit/2026-10-01` created from `main` (`1188748`), **uncommitted**: 5 new test files + this report + `docs/reports/audit-2026-10-01/` logs. Target dir `E:\fotox-audit-target`. |
| Machine | Rob's PC: 16 GB RAM (1.8–7 GB available during the runs: other apps open), commit limit 42 GB, RTX 3060 12 GB, 12 threads. C: NTFS (58 GB free), **E: exFAT** (899 GB free). |
| Builds | `cargo test --release` (thin LTO), Rust 1.97.1. |
| Isolation | Every engine test ran with `APPDATA` pointed at `E:\fotox-audit-run\appdata*` (the engine writes the recent list into `%APPDATA%\Fotox\preferences.json` — see finding T1). All fixtures and outputs in `E:\fotox-audit-run` or `%TEMP%\fotox-audit-c`. Your Fotox was not running and was not touched. |
| Safety | A PowerShell watchdog (`E:\fotox-audit-run\watchdog.ps1`) killed any audit process when available RAM fell under 1.2 GB, commit headroom under 3 GB, or C: under 40 GB. It fired 5 times (all reported below as results, not hidden). |

Labels used for every finding:

* **Reproduced** — an experiment in this audit showed it (command + log given).
* **Source-supported risk** — read in the current code, not triggered.
* **Not tested** — in scope but not exercised; says why.
* **Unsupported** — claimed somewhere, contradicted or not found.

What "native GPU" means here: the engine + render threads on the RTX 3060
(DX12/wgpu), driven by the same messages the UI sends
(`crates/fx-engine/tests/common`). **No test drove the CEF window**; "frame"
means the render thread delivered a viewport texture, not that the shell
presented it. Latency "input → changed frame" was measured by reading back
every delivered frame on the GPU until its pixels differed (the readback
adds a few ms: values are upper bounds).

---

## 1. Summary

**Verdict.** The core is sound where it matters most: the `.fxd`
append-only container survived 170 process kills, 1 203 truncation points
(× 2 volumes) and 288 bit flips without ever returning wrong pixels from an intact-looking
read; round trips are bit-exact; Smart Object transforms are truly lossless
(0 difference after 5 × 10 %↔1000 % cycles, while a pixel layer fell to 11.7 dB
PSNR). Documents up to **30 000² (8- and 16-bit, with an 8 192² real-data
region)** and **16 384² 16-bit fully covered with noise** were edited, saved,
closed, reopened and verified pixel-identical.

But users can lose work today in several ways that were **reproduced**:

1. **A deadlock in the Smart Object sampler** (`LazyMips` holds a mutex
   across a nested rayon `par_iter`) froze rendering of the whole session
   in 7 of 11 runs with Smart Objects (5 of 5 at 1 000 layers, 2 of 5 at
   300), in both layer-sweep runs at 1 000 and 5 000 layers, and once froze
   the engine thread itself (all 31 threads idle, Edit Contents never
   answered, shutdown never returned). Isolated repro: ≥ 8 hangs in 15 calls
   on a 12-thread pool; successful calls take 165 ms.
2. **No autosave or crash recovery at all.** Any crash, abort, freeze or
   OOM loses every unsaved change in every open document.
3. **Save As over a file another tab has open** (NTFS): the other tab's
   next Save reports success but writes into the orphaned old file; its
   work is gone when it closes.
4. **Edit Contents tabs drop edits**: after the parent closes or the layer is
   deleted, saving the contents marks the tab clean and it closes without
   asking; "Save" in its close prompt writes an unrelated `.fxd` instead.
5. **Malformed inputs abort the process or commit tens of GB**: a crafted
   `.fxd` footer and a one-strip TIFF abort Fotox; a 600-byte JPEG header
   commits 16.4 GB instantly.
6. **Memory budgets do not bound memory**: the GPU atlas alone puts 6.6 GB
   into the process's private bytes up front; the 3 GiB warm tier is not
   configurable; the hot budget is overshot up to 8× under load. A 16 384²
   16-bit Gaussian Blur reached 13.4 GB private with 627 MB RAM left.

Smart Objects are genuinely non-destructive for transforms, filters,
nesting, save/reopen and rasterize, but **instances do not share edits**,
**placing or converting anything larger than the canvas crops it to the
canvas**, and Replace/Export/Relink Contents, linked objects, filter masks
and per-filter blend modes do not exist.

---

## 2. Architecture and resource limits (traced)

### 2.1 Data flow

| Stage | Where | Authoritative or regenerable | Notes |
| --- | --- | --- | --- |
| Tiles | `fx-tiles` `TileStore`: 256² immutable tiles, `Arc` handles; hot → warm (LZ4 in RAM) → cold (scratch file) → backed (the opened `.fxd`) | level 0 of layers, masks, channels, Smart Object composites and nested documents: **authoritative**; mips, shape/text/fill/Smart Object caches, effects: **derived** (dropped under pressure, `TileError::Evicted`) | `store.rs` |
| Tile metadata | `TiledImage` = dense `Vec<TileSlot>` per mip level | — | `image.rs:46-63`: one slot per tile **position**, empty or not (≈ 17 B each). |
| Undo | `History`: 50 whole-`Document` snapshots (tiles shared by handle) | authoritative (old tiles live as long as their step) | `history.rs:19-33`: bounded by **step count, not bytes**. |
| Compositing | render thread: `TilePipeline` → `GpuCompositor`, atlas of `Rgba16Float` tiles | regenerable | atlas **6 GiB allocated up front** (`compositor.rs:120-127`, `atlas.rs:46-74`); upload budget 48 tiles/frame. Render thread only `try_get_hot`s; misses load on rayon (`render.rs:516-529`). |
| Derived tiles | one background job at a time on a document copy (`engine.rs:4143-4230`), merged back if the generation still matches | regenerable | see D1: one hung job stops all derived rendering. |
| Heavy commands | "pixel jobs" on worker threads (`engine.rs:4729-4767`) | — | **not jobs**: Free Transform of pixels, Fill, Convert to Smart Object, Rasterize, Edit Contents save-back (P3). |
| Save | snapshot (`open.doc.clone()`) → worker → `fx_io::fxd::save` | — | incremental append to the open file or fresh `.part` + rename (section 4). |
| Previews/thumbnails | `thumbs::ThumbQueue` (2 own threads, one waiting job per layer) | regenerable | |
| Smart Objects | `SmartSource { doc: Arc<Document>, composite: TiledImage }` + `transform` + filters; cache derived per layer | nested doc + composite authoritative | section 5. |
| Recovery | **none** (crash dialog says "Changes that were not saved are lost", `fx-app/src/crash.rs:53-60`) | — | D2. |

### 2.2 Resource budgets as they really are

Measured (`fx-engine-canvas-*.log`, "mem" lines): a fresh engine is 274 MiB
private; **the first document drawn makes it ≈ 6 975 MiB private** whatever
its size (GPU atlas + composite cache, `gpu reserved 6656 MiB`), and it stays
there after every document is closed. On top of that:

| Tier | Budget | Configurable | Observed |
| --- | --- | --- | --- |
| hot (RAM) | 5 GiB default | Preferences ▸ Memory Budget (`memory_budget_mb`, at next start) | 1 GiB budget → 1 429–1 534 MiB at 12–16 K; **256 MiB budget → 1 988–2 353 MiB** (8–9×) |
| warm (LZ4 RAM) | 3 GiB | **no** | filled to 2.9–3.0 GiB before any scratch use |
| scratch | 60 GiB file limit | folder only (must exist) | 1.2–1.8 GiB used at 16 K 16-bit |
| GPU atlas | 6 GiB + 0.5 GiB | **no** | allocated on the first frame |

So a 16 GB machine running a 12 K 16-bit blur sits at **≈ 12 GB private**
(measured 11 976 MiB peak, `canvas-12k16`), and a 16 K 16-bit blur crossed
13.4 GB with 627 MB left (watchdog kill). `docs/PERFORMANCE.md` §2's
"Fotox peak ≈ 10 GB whatever the document size" omits the atlas's commit:
**unsupported** as stated.

### 2.3 Document-wide work and blocking (selected)

* Engine-thread operations that scale with the canvas (measured, P3): Free
  Transform of a pixel layer 0.5 s (4 K) → 2.1 s (8 K) → **8.6 s (16 K)**; a zoom
  sent meanwhile waits the full time. Select + Fill 0.19 s → **1.1 s** (30 K).
  Edit Contents save-back 1.33 s for an 8 K source.
* `effects::invalidate` (`effects.rs:16-40`) marks **every** styled layer's
  caches dirty on any content change (source-supported; with 250 styled
  layers in the 5 000-layer test each edit re-derives them).
* Trim (`store.rs:726-772`) snapshots and sorts **all live tiles** each
  pass (250 ms when over budget) and skips tiles anyone holds — why the hot
  budget is overshot.
* Dense slot grids: every canvas-sized image (each pixel layer, mask,
  shape/text/fill/Smart Object cache, each effect cache) costs ≈ 0.3 MB of
  metadata at 30 000², ≈ 31 MB at the 300 000 px limit (source-supported).
* Cancellation: open/import/save/export progress closures always return
  `true` (`engine.rs:1628, 1730, 2859`) — **none of them can be cancelled**
  (P4). Only AI jobs react to Escape.

### 2.4 Failure handling

| Failure | Behaviour | Label |
| --- | --- | --- |
| Allocation failure | Rust aborts the process (`0xC0000409`), no save, no recovery | Reproduced (I1, I2) |
| GPU device loss / validation / OOM | no `on_uncaptured_error`, no device-lost callback (`fx-app/src/gpu.rs:82-88`): wgpu's default handler panics → crash dialog | Source-supported |
| Worker panic | save/export/import/derived/pixel jobs are `catch_unwind`ed and reported | Source-supported (checked in code) |
| Scratch write error / full | logged, tile stays in RAM over budget, `scratch_full` flag not shown in the UI | Source-supported (X-section) |
| Corrupt backed tile | render retries the load every frame (`render.rs:516-529`); effects treat it as transparent (`effects.rs:467`); Save As of the document fails | Source-supported; save failure reproduced (S8) |
| Derived job never returns | **all derived rendering in the session stops** (`derived_running` never clears) | Reproduced (D1) |

---

## 3. Large-document measurements

Harness: `crates/fx-engine/tests/audit_scale.rs` (`canvas_size_scaling`,
`layer_count_scaling`, `smart_objects_at_scale`, `pan_diagnostics`,
`frame_larger_than_the_hot_budget`). Content is **noise** (gradient +
Add Noise 25 %, or 256² random patches): every tile is real, mostly
incompressible data. Validation: export a flattened TIFF before saving and
after close + reopen; compare (the ICC profile's creation date is masked —
it is the only byte difference between two exports of the same pixels; the
first 4 K run proved the pixels identical with numpy).

Hot budget: 2 GiB (4 K, 8 K, layers), 1 GiB (12 K, 16 K, 30 K) — set in the
isolated preferences so the runs fit this PC's free memory.

### 3.1 Canvas size (one full noise layer + duplicate + transform/blur history)

| | 4 096² 8 | 4 096² 16 | 8 192² 8 | 8 192² 16 | 16 384² 8 | 16 384² 16 | 30 000² 8 † | 30 000² 16 † |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| new document | 6 ms | 10 ms | 15 ms | 31 ms | 51 ms | 78–169 ms | 165 ms | 248 ms |
| gradient + noise (jobs) | 0.35 s | 0.35 s | 2.1 s | 1.4 s | 5.5 s | 5.8–6.1 s | 2.9 s | 2.7 s |
| tiles after one layer | 170 MiB hot | 341 hot | 682 hot | 1 365 hot | 995 hot + 1 128 warm | 921 hot + 2 278 warm | 787 hot | 921 + 214 warm |
| zoom fit→100 % settled | 7 ms | 8 ms | 9 ms | 10 ms | 12 ms | 18 ms | 8 ms | 8 ms |
| pan @100 %, wheel → changed frame p50 / p95 / max | 1.1 / 4.3 / 4.9 ms | 0.6 / 4.0 / 4.6 | 0.5 / 2.9 / 4.7 | 0.6 / 3.7 / 4.1 | 1.2 / 4.6 / 6.1 | 0.8 / 3.8 / 4.1 | 1.2 / 4.2 / 6.1 | 0.8 / 3.9 / 4.2 |
| brush move → changed frame p50 / max | 1.6 / 2.5 ms | 2.2 / 2.8 | 1.8 / 4.8 | 2.1 / 3.4 | 2.6 / 4.2 | 2.1 / 3.8 | 1.8 / 3.2 | 1.9 / 3.4 |
| select 50 % + fill (engine thread) | 186 ms | 193 ms | 473 ms | 480 ms | 893 ms | 887–962 ms | 1 003 ms | 1 106 ms |
| **Free Transform of the layer (engine thread blocked)** | 507 ms | 481 ms | 2 126 ms | 1 999 ms | **8 591 ms** | **8 167–8 558 ms** | 5 977 ms | 5 827 ms |
| duplicate / hide (settled) | 1 / 42 ms | 1 / 52 | 0.3 / 53 | 2 / 67 | 3 / 60 | 3 / 67 | 2 / 163 | 1 / 175 |
| Gaussian Blur 4 px (job) | 245 ms | 255 ms | 997 ms | 970 ms | 3 938 ms | **killed ‡** | 2 935 ms | 3 090 ms |
| undo / redo p50 | 1.5 / 1.5 ms | 1.5 / 1.6 | 1.6 / 1.6 | 1.5 / 1.6 | 1.6 / 1.6 | 1.5 / 1.5 | 1.2 / 1.6 | 1.6 / 1.6 |
| Save As .fxd (size) | 0.22 s (63 MiB) | 0.54 s (191) | 0.68 s (250) | 2.3 s (750) | 4.9 s (994) | 4.0 s (1 612) | 0.79 s (222) | 1.9 s (723) |
| export TIFF | 0.5 s | 0.6 s | 1.9 s | 2.2 s | 6.9 s | 8.8 s | 20.1 s (2.6 GB) | 26.1 s (5.1 GB) |
| pan during an incremental save, p95 | 4.9 ms | — | 5.7 | 6.6 | 6.4 | 2.4 | 5.2 | 2.2 |
| reopen / first view | 7 / 70 ms | 6 / 111 | 8 / 100 | 9 / 183 | 15 / 15 | 21 / 23 | 20 / 144 | 19 / 19 |
| **reopened export = pre-save export** | **yes** | **yes** | **yes** | **yes** | **yes** | **yes** (no blur) | **yes** | **yes** |
| incremental save after a property edit | 7 ms | 8 ms | 18 ms | 20 ms | 31 ms | 26 ms | 19 ms | 37 ms |
| tiles after close | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 (but +520 MiB private stayed) |

† 30 000² canvases carry real data in a centred 8 192² region (the rest is the white background and the 15 000² fill). A fully painted 30 000² layer was **not tested**: §2.2 shows a full 16 384² 16-bit layer already exceeds this PC under a blur.
‡ 16 384² 16-bit: the Gaussian Blur job drove private bytes to 13.4 GB (627 MB available) and the watchdog stopped it. Rerun without the blur: everything else passed and validated.

Frame intervals while panning were 22–25 ms p50 (≤ 35 ms max) **with my
readbacks running**; without readback the first 4 K run showed 16.6–17.5 ms
p50 (60 Hz). Panning never stalled on canvas size.

**Largest documents successfully edited, saved, reopened and validated
(pixel-identical):** 16 384² 16-bit fully covered (1.6 GB `.fxd`, 1.8 GB
scratch in use), and 30 000² 16-bit with an 8 192² data region (723 MB `.fxd`,
5.1 GB TIFF exports identical).

### 3.2 Layer count (4 000 × 3 000 8-bit, 256² noise per layer; every 7th masked, 20th drop shadow, 25th + Invert adjustment, 50th → Smart Object, groups of 10)

| | 100 | 1 000 | 5 000 |
| --- | --- | --- | --- |
| build | 0.4 s | 5.6 s | **115 s** |
| add one layer p50, first → last 10 % | 3.2 → 3.1 ms | 3.3 → 6.5 ms | 3.4 → **37.1 ms** (p99 75, max 114) |
| tiles after build | 72 MiB | 1 018 MiB | 1 888 MiB |
| toggle bottom group, settled | 63 ms | 10 ms | 35 ms |
| pan @100 % | p50 0.6 ms | **stalled (D1)** | **stalled (D1)** |
| brush on a new top layer, move → frame p50 / up → history | 1.3 / 2.0 ms | 3.7 / 3.2 ms | 5.0 / 18.3 ms |
| undo p50 | 1.6 ms | 2.3 ms | 12.0 ms |
| Save As | 69 ms (21 MiB) | 708 ms (218 MiB) | 3.4 s (1 090 MiB) |
| reopen / first view | 10 / 222 ms | 32 / 34 ms | 136 / 151 ms |
| tiles still held after close | 0 | **949 MiB** | **538 MiB** |

At 1 000 and 5 000 layers the view stopped updating and closed documents
kept their tiles: that is finding D1, not layer count as such
(`pan_diagnostics` with the same 1 000 plain noise layers pans and frees
memory normally).

### 3.3 Smart Objects at scale (8 192² 8-bit noise source, 20 instances)

| | |
| --- | --- |
| Convert to Smart Object (engine thread) | 677 ms; view 868 ms |
| 20 instances (Duplicate + scale 15–33 %) | 84 ms; view 826 ms; tiles 1 028 → 1 029 MiB (instances add almost nothing at fit) |
| pan over instances @100 %, wheel → changed frame | p50 0.5–0.6 ms, max 3.8 ms |
| 4 Smart Filters (blur, USM, median, find edges) | parameters 6 ms; **in the second run the stack never appeared in 120 s** (D1) |
| Edit Contents saved back | **1 333 ms on the engine thread** (a zoom waited 1 333 ms) |
| Save As | 1.8 s, 603 MiB: the source is stored once (not × 20), but the nested layers *and* the composite are both stored (≈ 2× the source) |
| second run | the engine thread deadlocked at Edit Contents (D1) |

### 3.4 Remaining unknowns

* A fully painted 30 000² 16-bit layer (7.2 GB) and anything past it: not
  tested on this PC (see §2.2); the scratch path was exercised only to 1.8 GB.
* Presentation latency in the CEF window, UI-side layer-panel cost at 5 000
  layers: not tested (no window driven).
* GPU memory actually used (only the reserved 6 656 MiB is reported).
* Whether a frame needing more source tiles than the hot budget livelocks
  (the "hold what the consumer reads next" pattern): not determined — the
  256 MiB-budget run ran out of RAM first (§2.2).

---

## 4. Save integrity and recovery

Harness: `crates/fx-io/tests/audit_save.rs` (real `fx_io::fxd::save/open`),
`audit_smart.rs::edits_during_a_save_and_failed_saves` (real engine). Every
volume-dependent test ran on **C: (NTFS)** and **E: (exFAT)**.

### 4.1 What was tested and what happened

| Test | NTFS (C:) | exFAT (E:) |
| --- | --- | --- |
| Exact round trip, 8/16-bit, 700² and 2 048²: groups, mask, offset layer, solid fill, adjustment, Smart Object with filter, solid tiles, edge tiles; then an incremental save of the reopened doc | identical (fingerprint = manifest JSON with every tile ref replaced by the tile's hash) | — |
| **Process killed** at a random point in a loop of incremental saves (each save repaints one 1 024² or 512² 16-bit noise layer) | 15 + 20 kills: always opens to the last confirmed or the interrupted save, every tile exact | 30 + 20 kills: same |
| Process killed in a loop of fresh saves (Save As) | 15 + 20 kills: same; one `doc.fxd.part` left | 30 + 20 kills: same; `.part` left |
| (harness note) | in 3 of the 170 kills the child died before its *first* save completed: no file, but no save had been confirmed either — counted as "missing file" by the harness, not a defect | |
| Truncation at 1 203 cut points (every 16 KiB + ±1/±64 around each of 6 footers) | 1 203 / 1 203 open the newest complete version, pixels exact | same |
| One flipped bit, 24 samples per region (both volumes, identical results) | header: 3 unopenable (magic/version), 21 fine; older chunks: 7 tile read errors, 17 fine; **newest tile chunks: 24 read errors**; **newest manifest: 24 files that do not open at all**; **newest footer: 24 silent rollbacks**; wrong pixels: **0** | — |
| Newest save's chunks zeroed, footer kept (a write reordering) | the file **does not open** ("unknown chunk kind 0") although the previous version is intact | — |
| Cancel at each of 11 progress points | incremental: old version intact; fresh: old intact, `.part` left | incremental: ok; **fresh: "Access is denied" from the 2nd fresh save on** (the doc's own file is open) |
| Unreadable tile mid-save (its backing file corrupt) | save fails, target intact (`.part` left for fresh) | same |
| Target held open by another program (share read only) | "Access is denied", target intact, `.part` left | same |
| Read-only target | "Access is denied", `.part` left | "Cannot create a file when that file already exists (183)" (misleading), `.part` left |
| Open a read-only `.fxd` | **refused** ("Access is denied") | **refused** |
| **Save As A over X while tab B is open from X; then B saves** | Save As succeeds; **B's save reports success (16 tiles written) but X holds A** — B's work only lives in the unlinked old file | Save As fails "Access is denied"; B saves into X correctly |
| Two tabs from one file, saved from two threads × 8 | file valid, holds the last writer | same |
| **Two processes** saving one file for 4 s | 5 rounds (≈ 16 saves each, 512² doc): file consistent each time | run 1 (≈ 250 saves each): a tile is **wrong pixels without any error** (stamp v65 where v63 expected); run 2: the newest version has an unreadable tile ("unknown chunk kind 0") |
| Engine: edits during a 6 000² 16-bit save | edits answered in 62–67 ms; file = state at save start; document **stays dirty** | — |
| Engine: save to a missing folder / over a held file | error toast "Could not save …", document stays dirty, target intact, `.part` left | — |
| Throughput (4 × 4 096² 16-bit noise) | fresh 0.51–4.03 s (1 001–127 MiB/s raw; the slow run followed a 3 GB kill test on C:) | 1.10–1.63 s (467–314 MiB/s) |
| Growth without compaction | **1.5 GB after 11 s** of saves of a 8 MiB doc | **6.6 GB after 28 s** of saves of a 32 MiB doc |

### 4.2 Reliability, in practice

* "Saved"/clean is reported **only after** `commit` (chunks → `sync_data` →
  footer → `sync_data`) returned (`fxd/container.rs:572-593`,
  `engine.rs:2529-2585`). Edits during a save keep the document dirty
  (generation check). No premature "Saved" was found.
* A process kill never lost a confirmed save (170 kills, two volumes).
* Durability against **power loss** is *not* proven by process kills. What
  it relies on: `FlushFileBuffers` honoured by the disk (write cache with
  power-loss protection or flush passthrough), NTFS journaling of the
  rename. Fresh saves rename without `MOVEFILE_WRITE_THROUGH` and without
  flushing the directory (D6); exports and the preference/brush/pattern
  libraries do not flush at all before rename / truncate-in-place (D6, D7).
  **exFAT (E:) has no journal**: a power loss during a rename or a
  directory update can damage the directory; scratch there is fine,
  documents there carry that risk.
* There is **no autosave and no recovery file** (D2). The "recovery" in the
  code is the container's ability to fall back to the previous footer after
  a torn write — and even that does not fall back when the newest manifest
  is damaged (D5).

---

## 5. Native Smart Objects

Harness: `crates/fx-engine/tests/audit_smart.rs` (real engine; pixels checked
by exporting 16-bit TIFFs and comparing to the expected image).

### 5.1 Capability matrix

| Capability | Status | Evidence |
| --- | --- | --- |
| Source kept at its resolution; transforms only change a matrix | **Implemented and verified** | 5 × (10 % → 1000 %) and 7 × 51.43° rotations: max Δ **0**; pixel layer: PSNR 11.7 dB. Mid-way export differs (the transforms ran). |
| Placed / converted content larger than the canvas | **Broken (reproduced)** | Place 3000×2400 into 1000×800: the object covers 334/1000 columns after "fit"; scaled ×3 it equals the picture's **centre crop exactly** — everything outside the canvas is gone (SO1). |
| Edit Contents → parent updates | **Verified** for the edited layer | instance A equals inverted original exactly. |
| Instances (Duplicate Layer shares `uid`) update together | **Missing (reproduced)** | instance B still equals the unedited original (SO2). |
| New Smart Object via Copy (independent) | Implemented, not verified | code `command/m12.rs:158-183`. |
| Replace Contents / Export Contents / Relink | **Missing** | menu items disabled (`ui/js/data/menus.js:345-347`). |
| Linked Smart Objects | **Missing** | `linked`/`linked_mtime` are stored and round-tripped but never read; Place Linked does nothing more than Place. |
| Nested Smart Objects, propagation | **Verified** | inner edit → outer save → top updated exactly; Undo restores exactly. |
| Cycle protection | Not applicable today (sources are embedded copies, no linking) | — |
| Raster contents | Verified | |
| Layered contents | **Partial** | Convert keeps the layers editable; **Place of a `.fxd` flattens it to one layer** (`engine.rs:1652-1666`, source). Vector/text inside: kept when converted (they are layers of the nested doc), not separately tested. |
| Smart Filters: add, toggle stack, reorder, per-filter opacity, Undo/Redo, save/reopen | **Verified** | all exact (max Δ 0) where they should be equal, different where they should differ. |
| Smart Filter mask | **Missing** | `smart_filters.rs:9` FAST; D-083 promised it. |
| Per-filter blend mode | **Missing (silently)** | stored and saved, **ignored when rendering** (`smart_filters.rs:9`). |
| Re-edit a filter's parameters | Not verified (UI path) | HARDEN-H2 lists double-click re-edit as not done. |
| Object mask, opacity, blend, styles on the Smart Object layer | Implemented (generic layer path), not separately verified | |
| Colour management | Convert to Profile reaches the composite and nested doc (R10); not verified here | |
| Undo/redo of content edits, transforms, filters | Verified (content edit, filters) | |
| Save / reopen | **Verified** (transform, filters, nested doc) | reopened export exact. |
| Autosave / recovery of Smart Objects | **Missing** (no autosave at all) | D2. |
| Explicit Rasterize | **Verified** | max Δ 0 vs the Smart Object. |
| Edit Contents tab safety | **Broken (reproduced)** | D4. |
| Large sources: source shared by instances | **Verified** (save stores it once) | 603 MiB for 20 instances of an 8 K source. |
| Resolution-appropriate previews / mips | Implemented (lazy mips, sampler picks the level) | instances at fit cost ~0 extra tiles. **The lazy mips deadlock (D1).** |
| Bounded caches, regeneration after eviction | Implemented (derived class + `is_evicted`, R01) | not stressed beyond the runs above. |
| Recompute only affected tiles | Partial: the cache is reset whole on any transform/filter change (`*cache = TiledImage::derived(...)`) but only visible tiles are drawn | source |
| Cancellation / stale-result rejection | Generation check on merge (`derived_done`) | source; but a hung job is never abandoned (D1). |
| Cost of deep nesting / long stacks | Edit Contents save-back 1.3 s per 8 K level on the engine thread; filter stacks recompute aprons every draw (`smart_filters.rs:9`) | measured / source |

### 5.2 Workflow consequences

* Mock-up / template workflows (one design placed in many frames, edit once)
  do not work: each instance must be edited separately (SO2).
* Placing a high-resolution photo into a smaller layout **silently throws
  away the parts outside the canvas** — the user sees a "fitted" object
  that is actually a shrunken centre crop (SO1). Scaling it up later can
  never bring the rest back.
* Smart Filters are trustworthy for order/opacity/visibility, but there is
  no way to localise them (no mask) and the blend-mode control (if exposed)
  lies.
* Large documents with a few Smart Objects can freeze the canvas
  intermittently (D1) — the most visible problem a user would hit first.

---

## 6. Scratch storage

### 6.1 What exists (verified in code)

* One scratch **file** per process (`scratch.rs`), `FILE_FLAG_DELETE_ON_CLOSE`
  (a crash cannot leak it on Windows), positional I/O, 4 KiB extents from a
  best-fit free list, **hard limit 60 GiB**, no free-space check.
* Only **authoritative** tiles go there, LZ4 blocks, after hot (RAM) and warm
  (LZ4 RAM) are full. Derived tiles are dropped instead. Tiles of an opened
  `.fxd` are read from the document file, never copied.
* Location: `%LOCALAPPDATA%\Fotox\scratch`, or Preferences ▸ Scratch Folder
  (one folder, must exist, applied at the next start, **silently ignored**
  if it does not exist — `lib.rs:321-323`).
* Status bar shows scratch GB and GPU GB (`ui/js/canvas.js:146`).
  `scratch_full` and write errors are **not** shown.
* No checksum on scratch reads (LZ4 length check only): a disk error can
  return wrong pixels silently.
* Trimming is synchronous within the trim thread (compress + `seek_write`
  per tile, one at a time), woken on over-budget inserts and every 250 ms;
  producers are never slowed down, so RAM overshoots (§2.2).
* Undo history lives in the same tiers with no byte limit (50 steps).

### 6.2 Proposal (phased)

**Phase 1 — make what exists safe (focused changes)**

1. Budgets from the machine: at start read total/available physical
   memory and VRAM; hot = 25 % of RAM, warm = 10 % (configurable), atlas =
   min(6 GiB, 50 % of VRAM) and **grown lazily**; show the effective values
   in Preferences.
2. Backpressure: when hot + warm exceed budget by > 25 %, `TileStore::insert`
   from a **worker** blocks until the trim catches up (never on the render
   thread; the engine thread only waits through jobs). This bounds RAM.
3. Scratch errors are user-visible: protocol `MemoryStats` gains
   `scratch_full`, `scratch_error`, `scratch_free_bytes`; status bar turns
   amber/red with "Scratch disk E: is full — free space or add a disk".
4. Free-space reserve: never let the scratch file grow when the volume has
   less than max(5 GB, 5 %) free; treat that like `scratch_full`.
5. Per-extent CRC32 (stored in the extent table, not the file), checked on
   read: a mismatch becomes `TileError::Corrupt`, logged and shown.
6. The Scratch Folder preference validates on OK (exists, writable,
   free space shown) and is never silently ignored.

**Phase 2 — scratch disks like a tiled editor needs**

7. Ordered list of scratch locations, each with a quota; allocate
   **incrementally** (64 MiB segments, `create_new`, deleted on close) and
   fill in priority order, spilling to the next when a quota or reserve is
   reached; a missing/unwritable drive at start is skipped with a warning.
8. Asynchronous bounded I/O: a writer thread with a queue capped at e.g. 256
   MiB; demotion enqueues, the tile keeps its warm copy until the write
   completes (so readers never block on a queued write).
9. Per-session ownership: segments named `fotox-<pid>-<session>-<n>.scratch`
   in a `fotox-scratch` subfolder with a lock file; at start, delete segments
   whose owning pid is gone (delete-on-close already covers Windows; this
   covers other platforms and copied folders).
10. Settings / status UI (Preferences ▸ Performance): table of locations
    (path, free space, quota, used now, state ok/full/missing/error), memory
    budgets with the machine's totals, a live usage bar, and an action per
    failure ("Pick another folder", "Raise quota", "Free X GB on E:").

**Phase 3 — undo and recovery storage**

11. History by bytes as well as steps: when undo tiles exceed a byte
    budget, drop the oldest steps (tell the user in the History panel).
12. Recovery is **not** scratch: write recovery snapshots as `.fxd` files in
    `%LOCALAPPDATA%\Fotox\recovery\<session>\` (see D2), not in the scratch
    file, so a crash keeps them.

What scratch storage improves: documents whose authoritative tiles plus
undo exceed RAM (big 16-bit layers, long histories, many open documents).
What it does not: the GPU atlas commit (needs lazy sizing), engine-thread
blocking (P3), per-edit CPU work (effects invalidation, filters), memory
spikes of jobs that produce faster than the trim (needs backpressure), and
compositing many layers at 100 % (upload budget, O4).

---

## 7. Input and resource safety

Harness: `crates/fx-io/tests/audit_inputs.rs`; each crafted file is opened
in a **child process** (opened, then every tile read as drawing would), with
a 6 GB safety valve.

| Input | Result | Label |
| --- | --- | --- |
| `.fxd` footer pointing at a 1 TiB manifest chunk (valid CRCs) | **process aborts** (`0xC0000409`) — `read_chunk` allocates `payload_len` before checking it against the file size (`container.rs:436`) | Reproduced (I1) |
| `.fxd` tile reference of 1 TiB | clean error (header/ref length mismatch) | Reproduced (safe) |
| `.fxd` tile reference past EOF | clean error | Reproduced (safe) |
| `.fxd` 300 000² canvas, 200 small layers | opens | Reproduced (safe) |
| `.fxd` 1 000 canvas-sized layers at 30 000² | opens; child peak 1 028 MiB | Reproduced |
| groups nested 50 / 60 deep (written by `fxd::save`) | open (earlier report said 60 failed: **now fixed**) | Reproduced |
| groups nested 200 deep | clean error (serde recursion limit) | Reproduced |
| manifest zstd bomb 32 KiB → 1 GiB | clean error after a 1 GiB allocation | Reproduced |
| **any** `.fxd` open | commits **1 026 MiB** transiently: `zstd::bulk::decompress(payload, 1 GiB)` reserves the full capacity without the crate's `experimental` feature (`manifest.rs:832-836`) | Reproduced (P5) |
| TIFF 300 000² RGBA16, one Deflate strip | **process aborts** (one-strip buffer; `Limits::unlimited()`, `tiff.rs:136-142`) | Reproduced (I2) |
| TIFF 10 000² RGB16, one strip vs 64-row strips (real files) | peak 1 573 MiB vs 830 MiB: one-strip files are **not streamed** | Reproduced (I3) |
| TIFF 12 000² RGBA16 header with a 64 KiB strip (≈ 65 KB file) | one strip: child committed **2 227 MiB** before the clean error; 64-row strips: 86 MiB | Reproduced (I3) |
| **JPEG header 65 535² (≈ 600 bytes)** | child committed **16.4 GB at once** (killed by the valve) | Reproduced (I2) |
| JPEG header 30 000² | 6.7 GB in 11 s (killed) | Reproduced |
| External references | none followed (linked Smart Objects not implemented; ICC/LUT/ABR read only when the user picks them) | Source |

Recommendations: check every length/offset against the file size before
allocating (`read_chunk`, manifest refs at open); size the manifest buffer
from the zstd frame header or stream with a cap; cap decoded pixels per
import by a memory estimate *before* decoding (JPEG: refuse or tile-decode
above e.g. 1 GB of RGBA; TIFF: set the crate's `Limits` to one band-batch's
worth and read one-strip files row by row with `read_chunk`'s byte range);
use fallible allocation (`Vec::try_reserve`) at every format boundary, and
open foreign files in a **separate process** (or at least catch the
abort-prone parts) so a hostile file cannot kill the documents already open.

---

## 8. Earlier findings rechecked

| Earlier | Claim | Now |
| --- | --- | --- |
| STRESS O1 | groups ≥ 60 deep save but do not reopen | **Fixed**: 60 reopens (Reproduced); engine caps new nesting at 10; 200 is refused cleanly. |
| STRESS O2 | every edit resends the layer list | **Partly fixed**: property edits send `layers_patch`; structural edits still send all (adding one layer at 5 000 costs 37 ms p50 vs 3.4 ms at the start). |
| STRESS O3 | budgets ignore the machine; atlas up front | **Still open, worse than described**: 6 975 MiB private on the first frame; warm not configurable; hot budget overshot 8× (Reproduced). |
| STRESS O4 | 100 % zoom on many layers slow (upload 48 tiles/frame) | `upload_budget: 48` unchanged (source); not separately measured (D1 interfered). |
| STRESS F1/F2 | thumbnail stack overflow; render kept last doc | No recurrence; but D1 now keeps closed documents' tiles alive. |
| CODE-REVIEW R01 (evicted derived tiles) | fixed | Fix present (`is_evicted`, `insert_held`, `LazyMips` holding) — and `LazyMips` is where D1 lives. |
| R04 (concurrent exports) | fixed | Fix present (job-unique `.part`, lease). |
| R07 (derived work blocks engine) | fixed by one background job | Present; that single job is the D1 single point of failure. |
| R09 (nested image entries panic) | fixed | Present (`check_image_entry`); but chunk sizes are still unchecked (I1). |
| HARDEN H2 Smart Object items | not done | Confirmed still missing: union canvas, linked, replace/export/relink, filter mask, per-filter modes, re-edit. |
| Memory 2026-10-01 00:51 freeze (event 2004, 18.5 GB commit) | — | Consistent with §2.2: 6.6 GB atlas + 3 GiB warm + hot + AI warm-up. |

---

## 9. Findings (detail)

Severity: **Critical** (silent loss of user work, likely), **High** (loss or
corruption possible / app unusable), **Medium**, **Low**.

### D1 — Smart Object sampler deadlock freezes rendering (and can freeze the engine) — Critical, Reproduced

* **Consequence.** The canvas stops drawing Smart Objects, mips, effects,
  shapes and text — in **every** document — until Fotox restarts; closed
  documents' tiles stay in RAM; in one run the engine thread blocked too
  (Edit Contents never opened, shutdown never returned) — in the app that
  is a frozen UI with unsaved work and no way to save.
* **Where.** `fx-engine/src/mips.rs:95-117` `LazyMips::read` locks
  `self.image` (a `std::sync::Mutex`) and, holding it, calls `ensure_mip` →
  `compute_levels` → `compute_tiles` (`mips.rs:195-200`, rayon `par_iter`).
  Its caller `fx_ops::resample::resample` (`fx-ops/src/resample/mod.rs:67-82`)
  is a rayon `par_iter` over destination tiles, each calling
  `LazyMips::tile`. A worker waiting in the inner `par_iter` steals an outer
  task, which locks the mutex its own thread holds. Path: `smart.rs:32-73`
  (`draw`) and `smart_filters.rs`. The derived job then never returns, so
  `derived_running` stays `true` (`engine.rs:4143-4230`) and nothing derived
  is ever computed again.
* **Repro.** `audit_lazy_mips.rs` (isolated, no engine):
  `FOTOX_AUDIT_POOL=12 … audit_lazy_mips -- --ignored` → ≥ 8 of 15 calls hang
  > 30 s; calls that complete take 165–172 ms; pool of 4: 1 / 15.
  Engine: `pan_diagnostics` with Smart Objects → 7 / 11 runs (5 / 5 at
  1 000 layers, 2 / 5 at 300, 0 / 1 at 100): 100 % view blank (2 colours),
  152–721 MiB held after close; the same stall in both `layer_count_scaling`
  runs at 1 000 and 5 000 layers (949 / 538 MiB held); then a **new**
  document with Clouds shows only UI colours after 8 s (494 colours without
  the stall). `smart_objects_at_scale` run 2: all 31 threads idle (0.03 CPU-s
  in 10 s), stuck waiting for Edit Contents' view.
* **Fix.** Never hold the image lock across parallel work: compute the
  needed mips outside the lock (collect needed tiles under the lock, release,
  compute on a copy, re-lock and install if still dirty), or make
  `compute_tiles` sequential inside `LazyMips`, or use a `parking_lot`
  `RwLock` + per-tile once-cells. Also: give the derived job a watchdog
  (abandon and restart after N s, clear `derived_running`) so a future hang
  degrades instead of freezing.
* **Tradeoffs.** Sequential mip compute inside the sampler is slower for
  big first draws; the copy-outside-lock version costs a clone of the slot
  grid.
* **Acceptance.** `audit_lazy_mips` 0 hangs in 200 calls on 4, 12 and 32
  threads; `pan_diagnostics` (smart, 1 000 layers) 0 stalls in 20 runs; tiles
  0 MiB 5 s after close; a new document renders after any of those runs.
  Same plain-layer control: `FOTOX_AUDIT_EXTRAS=0` never stalled (4 runs),
  styles alone never blanked the view (1 run).

### D2 — No autosave or crash recovery — Critical, Reproduced (absence) + Source

* **Consequence.** Every crash path — the aborts in I1/I2, a wgpu device
  loss, an OOM, D1's freeze, a power cut — loses all unsaved work in all
  documents. The crash dialog says so.
* **Where.** No autosave/recovery code anywhere (`grep -ri autosave` →
  nothing); `fx-app/src/crash.rs:53-60`.
* **Fix.** Periodic background **recovery snapshots**: every N minutes (and
  after M edits) for each dirty document, take `open.doc.clone()` (cheap:
  tiles are shared) and write it with the existing `fxd::save` to
  `%LOCALAPPDATA%\Fotox\recovery\<session>\<doc>.fxd` — incremental append
  to that recovery file, so the cost is the changed tiles. On start, list
  recovery files of dead sessions and offer them. Delete on clean close or
  successful save. On a render-thread panic, the engine thread is alive:
  write recovery snapshots before exiting.
* **Tradeoffs.** Disk writes (bounded by changed tiles); recovery folder on
  the fast disk; must not block (worker thread, same as Save).
* **Acceptance.** Kill the process during editing (and during a save) →
  restart offers the document; reopened content equals the state ≤ N
  minutes before the kill (exact compare as in `audit_save.rs`).

### D3 — Save As over a file another tab is backed by loses that tab's later saves — Critical, Reproduced (NTFS)

* **Consequence.** Tab B opened `X.fxd`. Tab A is Saved As `X.fxd`. Tab B's
  next Save says "Saved" (16 tiles written in the test) but writes into the
  replaced, unlinked file; X holds A; B's work disappears when B closes.
  Opening the same file twice is allowed, so "B" can also be a second tab of
  the same file.
* **Where.** `engine.rs:2808-2819` only checks the document's *own* path;
  `fxd/save.rs:148-158` renames over X while other handles are open (Rust's
  rename uses POSIX semantics on NTFS); `container.rs:509-520` `append_to`
  writes through the old handle. No open-document registry by path.
* **Repro.** `audit_save.rs::save_as_over_a_file_another_document_has_open`
  with `FOTOX_AUDIT_DIR` on C: → assertion fails; on exFAT the Save As fails
  instead (no loss).
* **Fix.** (a) Keep a registry path → open documents; Save As onto a path
  open in another tab asks ("X is open in another tab: replace and close it /
  cancel"); opening a path already open activates that tab. (b) Before an
  incremental append, compare the handle's file id
  (`GetFileInformationByHandle`: volume serial + file index) with the path's
  current file id; if they differ, save fresh to the path and rebind.
* **Acceptance.** The test above: B's save either is refused with a message
  or lands in X; two opens of one path give one tab.

### D4 — Edit Contents tabs lose edits — High, Reproduced

* **Consequence.** (1) Parent closed (no warning about the open contents
  tab) → saving the contents toasts "document … was closed" and marks the
  tab **clean**; closing it then asks nothing. (2) Smart Object layer
  deleted → saving the contents does nothing visible; tab clean; closes
  without asking. (3) "Save" in a dirty contents tab's close prompt opens
  **Save As** for an unrelated `.fxd` instead of updating the object.
* **Where.** `engine/m12.rs:312-352` (`open.dirty = false` at line 327
  before the parent/layer checks; `let Some(layer) … else { return }` at 336);
  `engine.rs:2626-2630` (`CloseAnswer::Save` → `self.save` →
  `ask_save_path`); `smart_children` not consulted on close of the parent.
* **Repro.** `audit_smart.rs::edit_contents_when_the_parent_goes_away`.
* **Fix.** Clear `dirty` only after the parent was updated; when the parent
  or layer is gone, keep the tab dirty and offer "Save as new document";
  route close-prompt Save of a contents tab to `save_contents`; closing a
  parent with open contents tabs asks about them first.
* **Acceptance.** The same test: every close of a contents tab with
  unapplied edits prompts; "Save" updates the parent (History shows "Edit
  Contents").

### D5 — One damaged byte in the newest save makes the file unopenable; footer damage rolls back silently — Medium, Reproduced

* **Consequence.** A flipped bit in the newest manifest (24/24) or zeroed
  newest chunks → "does not open" although every older version is intact; a
  flipped bit in the newest footer → the previous version opens with no
  message (24/24).
* **Where.** `fxd/open.rs:32-53` (one footer tried), `container.rs:324-347`.
* **Fix.** On a manifest/structure error, keep scanning back for the
  previous valid footer and open that with a banner "Recovered the version
  saved at …; the latest save was damaged"; put a save counter and timestamp
  in the footer's reserved bytes so a rollback is detectable and shown.
* **Acceptance.** `single_byte_corruption_by_region`: manifest flips open the
  previous version with a warning; footer flips report the rollback.

### D6 — Rename durability: exports and fresh saves — Medium, Source-supported

* Exports write `<name>.<pid>-<job>.part`, flush the `BufWriter` but never
  `sync_all` before the rename (`export.rs:97-121`, `tiff_write.rs:266-276`):
  after a power loss the final name can hold a zero-length or partial file.
  Fresh `.fxd` saves sync data but rename without `MOVEFILE_WRITE_THROUGH`.
* **Fix.** `sync_all` the part before renaming; on Windows use
  `MoveFileExW(…, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`.
* **Acceptance.** Code review + a fault-injection layer (a `File` wrapper in
  tests) asserting the order write → sync → rename.

### D7 — Preferences, brushes, patterns written in place — Medium, Source-supported

`prefs.rs:36-53`, `brushes.rs:104-117` (and patterns, shapes) use
`std::fs::write` (truncate + write). A crash mid-write leaves a broken JSON;
`Prefs::load` then silently returns defaults (the user's gradients, recent
list, scratch folder… gone). **Fix:** write temp + sync + rename, keep a
`.bak`. **Acceptance:** truncate the file mid-way → previous version loaded.

### D8 — Incremental saves never compact — Medium-High, Reproduced

* **Consequence.** Every save appends changed tiles; dead chunks are never
  reclaimed: 6.6 GB file for a 32 MiB document after ~800 saves (E:), 1.5 GB
  after 11 s (C:). A heavily repainted large document grows by its painted
  area at every save, forever (a Save As compacts, by accident).
* **Where.** `fxd/save.rs:178` `needs_compaction` exists; nothing calls it.
* **Fix.** After a successful save, if `needs_compaction(live, len)` and the
  file is over e.g. 256 MB, schedule a background fresh save to `X.compact`
  then replace X (with D3's id check; on exFAT close the handles first or
  keep appending).
* **Acceptance.** 100 saves of a document repainting one layer each time:
  file ≤ 2.5 × live bytes.

### D9 — Two processes writing one `.fxd` give wrong pixels without an error — Medium, Reproduced (exFAT; NTFS 0 / 5 rounds)

* **Consequence.** Chunk references are not bound to their content: process
  B overwrote a chunk A's manifest points at with a same-size chunk of its
  own (valid CRC) → wrong tile, no error. The app's single-instance lock
  prevents two local Fotox instances; a file on a share opened from two PCs
  is not protected.
* **Where.** `container.rs:46-82` (lease is process-wide only), manifest
  slot entries `[tx, ty, "t", offset, len]`.
* **Fix.** `LockFileEx` an exclusive byte range while a document's file is
  open for writing (read-only open for the second opener); store the chunk's
  CRC32 in the manifest's tile refs and check it on read.
* **Acceptance.** `two_processes_saving_one_file`: second process gets a
  "file is in use" error; no Garbage verdicts in 20 rounds.

### D10 — Read-only files, exFAT replace, leftovers — Medium, Reproduced

* A read-only `.fxd` (or one on a read-only share) **cannot be opened**:
  `FxdFile::open` requires write access (`container.rs:377`). Fix: open
  read-only, upgrade to read+write lazily at save (or Save As).
* On exFAT, any fresh save over a file this process still holds (history
  tiles backed by it) fails "Access is denied"; read-only target → error 183
  (misleading). Fix: close/rebind the old handle first, or detect and save
  incrementally; map error codes to clear messages.
* `.part` files are left after every failed, cancelled or killed fresh save
  (`save.rs:88-93`); the next fresh save to the same target reuses (truncates)
  the same name. Fix: remove on error; unique part names like exports; sweep
  stale `*.fxd.part` older than the process at start.

### D11 — Same file in two tabs: last save silently wins — Medium, Reproduced

`two_documents_from_one_file_saved_from_two_threads`: both "saved"; the file
holds only the last. Fix with D3's registry (activate the existing tab).

### P1 — Memory budgets do not bound memory — High, Reproduced

See §2.2: atlas 6.6 GB private from the first frame (stays until exit), warm
3 GiB fixed, hot 1 GiB → 1.5 GiB and 256 MiB → 2.0–2.4 GiB. Runs killed by
the watchdog at 11.3–13.4 GB private. Fix: §6.2 items 1–2 (machine-sized,
lazy atlas, backpressure). Acceptance: with hot = 1 GiB, warm = 1 GiB, the
16 384² 16-bit blur run stays under (atlas actually used + 2.5 GiB + 1 GiB),
and the 256 MiB run completes.

### P2 — Engine-thread blocking on large layers — Medium, Reproduced

Free Transform of a pixel layer (8.6 s at 16 K; a zoom waits as long),
Select+Fill (1.1 s at 30 K), Convert to Smart Object (0.7 s at 8 K), Edit
Contents save-back (1.3 s at 8 K). Fix: add `Transform` (pixels), `Fill`,
`ConvertToSmartObject`, `Rasterize` and the save-back composite to the
pixel-job path (`engine.rs:4729-4767`). Acceptance: a zoom sent during each
answers in < 50 ms.

### P3 — Layer-count costs — Medium, Reproduced

Adding a layer at 5 000 layers: 37 ms p50 (114 ms max) vs 3.4 ms at the
start; full layer lists on structural edits (O2 remainder); every styled
layer re-derived on any content edit (`effects.rs:16-40`). Fix: insert/remove
patches; invalidate only effects whose source layer or whose region changed.

### P4 — No cancellation of open/import/save/export — Medium, Source-supported

Progress closures return `true` (`engine.rs:1628, 1730, 2859`). A 26 s 30 K
export or a mistaken huge import cannot be stopped. Fix: a cancel token per
task, `UiToEngine::CancelTask { task }`, a cancel button on the progress
bar; fx-io already honours `false` (verified: `cancel_at_every_batch_boundary`).

### P5 — Every `.fxd` open commits 1 GiB — Medium, Reproduced

`manifest.rs:832-836`; peak private 1 026 MiB for a 512² file. Fix:
`zstd_safe::get_frame_content_size` → `Vec::with_capacity(size.min(LIMIT))`
or `zstd::stream::Decoder` + `take(LIMIT)`. Acceptance: open peak < 64 MiB
for a 1 MB manifest.

### I1 — Crafted `.fxd` aborts the process — High, Reproduced

`container.rs:436` allocates the payload length before comparing with the
file length. Fix: reject any chunk whose `offset + len` exceeds the file
length (known at open) — at `read_chunk` and when building backed tiles.

### I2 — Image headers commit tens of GB or abort — High, Reproduced

JPEG 65 535² header → 16.4 GB instantly; TIFF one-strip 300 000² → abort.
Fix in §7. Acceptance: all `crafted_image_files` cases end in a clean error
with peak < 1 GB.

### I3 — One-strip TIFFs are not streamed — Medium, Reproduced

1 573 vs 830 MiB at 10 000² 16-bit; for 30 000² 16-bit RGB the strip alone is
5.4 GB. Fix: read the strip's byte range in pieces (uncompressed) or
row-by-row decode.

### SO1 — Content outside the canvas is cropped when placed or converted — High, Reproduced

* **Where.** `command/m12.rs:74-115`: the nested document is
  `Document::new(doc.width, doc.height …)` ("FAST: the nested document has
  the parent's canvas"); `engine.rs:3977-4031` Place = paste + convert.
* **Repro.** `audit_smart.rs::placed_picture_keeps_its_resolution`
  (assertion passes only because the scaled-back *crop* is exact; the
  "covers 334 of 1000 columns" line is the defect).
* **Fix.** Nested canvas = union of the converted layers' bounds (and the
  placed image's own size); the transform places it. Place `.fxd` keeps its
  layers (embed the opened document, not its flattened composite).
* **Acceptance.** Place 3000×2400 in 1000×800 → after fit the object covers
  the canvas width; scaling ×3 about its centre equals the full picture's
  centre crop, and ×1 at origin equals the whole picture.

### SO2 — Instances are not updated by Edit Contents — High, Reproduced

`engine/m12.rs:341-342` ("FAST … instances sharing the source are not
updated"). Fix: after save-back, update every Smart Object in the document
(and nested ones) whose `source.uid` matches; one History step.
Acceptance: `editing_contents_updates_every_instance` passes.

### SO3 — Missing Smart Object features — Medium, Source-supported

Replace / Export Contents, Relink, linked objects (never read), filter
mask, per-filter blend modes (stored but ignored — a silent lie if exposed),
Place `.fxd` flattened. Priorities in §10.4.

### X1 — Scratch failures are silent; no checksums — Medium, Source-supported

§6.1. Fix: §6.2 items 3–6.

### T1 — Engine tests write the user's real preferences — Low, Reproduced (observed)

`%APPDATA%\Fotox\preferences.json` "recent" holds paths like
`…\Temp\fx-engine-save-flow-729128\photo.fxd`: the repo's engine tests
(`remember_recent`) write the real file. Fix: tests set `APPDATA` (or a
prefs path override) to a temp folder in `common::Harness::start`.

### T2 — Preferences dialog shows a 4 096 MB default — Low, Source

`ui/js/native/prefs.js:170` defaults to 4096 while the engine default is
5 GiB; opening Preferences and pressing OK silently changes the budget.

---

## 10. Conclusions

### 10.1 Five highest-priority changes (data loss first)

1. **Fix the `LazyMips` deadlock and add a watchdog to the derived job**
   (D1). It freezes the canvas, can freeze the engine, and pins memory.
2. **Recovery snapshots** (D2), reusing incremental `fxd::save` on a
   worker, plus emergency snapshots when the render thread dies.
3. **Path registry + file-id check before append** (D3, D11): no more saves
   into orphaned files, one tab per file.
4. **Edit Contents safety and instance propagation** (D4, SO2): never mark
   a contents tab clean unless its edits landed; update every instance.
5. **Bounded memory and hostile-input hardening** (P1, I1, I2, P5):
   machine-sized budgets with backpressure and a lazy atlas; length checks
   before allocation; decoded-size caps.

### 10.2 Measured limits

* Edited, saved, reopened, **pixel-identical**: 16 384² 16-bit fully
  painted (1.6 GB file); 30 000² 8/16-bit with an 8 192² painted region;
  5 000 layers of real data (1.1 GB file, reopen 136 ms).
* Not completed on this 16 GB PC: a full-layer Gaussian Blur at 16 384²
  16-bit (13.4 GB private, killed). Fully painted 30 000² layers: not
  attempted (would exceed the same limit).
* Interactive costs stayed low with canvas size (pan/brush ≤ 6 ms to a
  changed frame) **except** engine-thread operations (Free Transform 8.6 s at
  16 K) and anything touched by D1.
* Unknowns: in-window presentation latency, UI panel cost at 5 000 layers,
  true VRAM use, the hot-budget livelock question.

### 10.3 Save / recovery reliability

* Container: excellent against process death and torn writes (170 kills,
  1 203 cuts, no wrong pixels from a single corruption). Snapshot and dirty
  state are correct; "Saved" is never premature.
* Weak spots: no autosave/recovery; replacement while another tab is backed
  by the file (NTFS); damaged newest manifest is not survivable; unbounded
  file growth; two writers on a share; exFAT replace failures; leftovers.
* Practical limits: power-loss durability depends on the disk honouring
  flushes; exFAT (E:) has no journal — keep documents on NTFS, use E: for
  scratch.

### 10.4 Smart Objects: matrix (§5.1) and priorities

1. D1 deadlock (blocks everything else from being usable at scale).
2. SO1 canvas cropping (silent content loss at Place/Convert).
3. SO2 instance propagation + D4 contents-tab safety.
4. Replace Contents (keeps transform/mask/filters), Export Contents.
5. Filter mask and per-filter blend modes (or hide the mode control).
6. Edit Contents save-back as a job; per-source mip cache shared by
   instances (key by `uid`) instead of per-layer copies.
7. Linked Smart Objects with mtime watch and relink, with cycle detection
   (a path stack while loading) once linking exists.

### 10.5 Scratch disk — phases

Phase 1 (focused): machine-sized budgets, backpressure, visible scratch
errors, free-space reserve, extent CRCs, validated folder preference.
Phase 2 (architectural): ordered multi-location scratch with quotas,
incremental segments, async bounded writer, session ownership and cleanup,
the Performance page. Phase 3: history byte budget; recovery snapshots kept
outside scratch.

### 10.6 Focused fixes vs architectural changes

| Focused (days) | Architectural (weeks) |
| --- | --- |
| D1 lock scope + derived-job watchdog | D2 recovery system (UI, lifecycle, cleanup) |
| D3 file-id check; D11/D3 path registry | P1 budgets with backpressure + lazy atlas sized to VRAM |
| D4 contents-tab dirty/close fixes; SO2 uid propagation | SO1 union-canvas nested documents; Place `.fxd` as layers |
| D5 footer fallback + banner; D10 read-only open, `.part` cleanup | D9 cross-process locking + content-bound chunk refs (format change) |
| D6/D7 sync + atomic replace | Multi-disk scratch (§6.2 phase 2) |
| D8 background compaction (uses existing `needs_compaction`) | Foreign-file decoding in a separate process |
| P2 move Transform/Fill/Convert/Rasterize to jobs | Effects invalidation by region/dependency (P3) |
| P4 cancel tokens; P5 manifest buffer; I1/I2 length + size caps; I3 strip reading | Device-loss recovery (recreate wgpu device, re-upload) |

---

## Appendix A — Harness files (audit worktree, uncommitted)

| File | What |
| --- | --- |
| `crates/fx-io/tests/audit_save.rs` | round trip, kill loops (child processes), concurrency, replacement, cancel, unreadable tile, OS refusals, truncation sweep, bit flips, zeroed chunks, throughput |
| `crates/fx-io/tests/audit_inputs.rs` | crafted `.fxd`/TIFF/JPEG in child processes with a memory valve; strip-layout peak; canvas-sized metadata; open peak |
| `crates/fx-engine/tests/audit_smart.rs` | Smart Object transforms, place, instances, orphaned contents tabs, nesting, smart filters, rasterize, save/reopen; edits during save; failed saves |
| `crates/fx-engine/tests/audit_scale.rs` | canvas sweep, layer sweep, Smart Objects at scale, pan diagnostics, tight budget |
| `crates/fx-engine/tests/audit_lazy_mips.rs` | isolated D1 repro |
| `E:\fotox-audit-run\watchdog.ps1` | resource watchdog |

## Appendix B — Commands

Build (from `E:\fotox-audit`, Git Bash):

```bash
export CARGO_TARGET_DIR=E:/fotox-audit-target CARGO_INCREMENTAL=0
cargo test --release -p fx-io --test audit_save --test audit_inputs --no-run
cargo test --release -p fx-engine --test audit_smart --test audit_scale --test audit_lazy_mips --no-run
```

Run (always with an isolated APPDATA; the `memory_budget_mb` there is the
hot budget used):

```bash
export APPDATA=E:/fotox-audit-run/appdata FOTOX_AUDIT_DIR=E:/fotox-audit-run
# save integrity (repeat with FOTOX_AUDIT_DIR=$TEMP/fotox-audit-c for NTFS)
FOTOX_AUDIT_SIDE=512 FOTOX_AUDIT_KILLS=20 cargo test --release -p fx-io --test audit_save -- --ignored --nocapture --test-threads 1
cargo test --release -p fx-io --test audit_inputs -- --ignored --nocapture --test-threads 1
cargo test --release -p fx-engine --test audit_smart -- --ignored --nocapture --test-threads 1
# large documents
FOTOX_AUDIT_SIDES=4096,8192 FOTOX_AUDIT_DEPTHS=8,16 FOTOX_AUDIT_EXPORT=1 cargo test --release -p fx-engine --test audit_scale canvas_size_scaling -- --ignored --nocapture --test-threads 1
APPDATA=E:/fotox-audit-run/appdata-1g FOTOX_AUDIT_SIDES=16384 FOTOX_AUDIT_DEPTHS=16 FOTOX_AUDIT_EXPORT=1 FOTOX_AUDIT_SKIP_BLUR=1 cargo test … canvas_size_scaling …
APPDATA=E:/fotox-audit-run/appdata-1g FOTOX_AUDIT_SIDES=30000 FOTOX_AUDIT_DEPTHS=8,16 FOTOX_AUDIT_EXPORT=1 FOTOX_AUDIT_PATCH=8192 cargo test … canvas_size_scaling …
FOTOX_AUDIT_LAYERS=100,1000,5000 cargo test … layer_count_scaling …
FOTOX_AUDIT_EXTRAS=smart FOTOX_AUDIT_DIAG_LAYERS=1000 cargo test … pan_diagnostics …
FOTOX_AUDIT_POOL=12 FOTOX_AUDIT_ATTEMPTS=15 cargo test --release -p fx-engine --test audit_lazy_mips -- --ignored --nocapture
```

Warning: `audit_lazy_mips` leaks the hung attempts' memory (≈ 250 MB each)
until the process exits; run it with few attempts.

## Appendix C — Logs

`docs/reports/audit-2026-10-01/`: `fx-io-audit_save.log`,
`fx-io-audit_inputs.log`, `fx-engine-audit_smart.log`,
`fx-engine-canvas-*.log`, `fx-engine-layers.log`, `fx-engine-so-scale.log`,
`fx-engine-tight3.log`, `fx-engine-pan-fresh-*.log`,
`fx-engine-pan-repetitions.log`, `watchdog.log` (1–3 s samples of available
RAM, commit headroom, C: free space and the audit process's private bytes;
`KILL` lines are the five watchdog stops).

Two log files hold a different run than the one quoted: `fx-engine-so-scale.log`
is the **second** Smart-Objects-at-scale run (the one that deadlocked); the
first run's numbers in §3.3 were read from the console. The
`fx-io-audit_save.log` / `fx-engine-audit_smart.log` files are the final
re-runs on both volumes (their numbers match or are quoted as ranges with
the first runs). `fx-engine-tight3.log` is a 256 MiB-budget run stopped by the
watchdog.
