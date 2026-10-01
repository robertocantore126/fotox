> **Role of this file (2026-10-01):** this was the original fix prompt. Rob decided the fixes will be written code-only (by DeepSeek, from `FIX-PROMPT-2026-10-01.md`). This file is now **our verification plan**: the acceptance criteria, harness gotchas, machine-safety rules and the Phase 4 benchmark that Claude and Rob will use in the testing and refining phase.

# Fotox: fix the findings of the 2026-10-01 audit

You are fixing bugs in Fotox, a Rust/wgpu image editor with a CEF UI.

- **Repository:** `C:\Users\39389\Documents\XuanZhi9\fotox`.
- **Audit report:** `E:\fotox-audit\docs\reports\AUDIT-2026-10-01-large-docs-save-smart-objects.md`. Read sections 1, 2, 4, 9 and 10 before you change anything. Section 9 gives each finding's location, repro, proposed fix and acceptance test.
- **Harnesses that reproduce the findings:** in `E:\fotox-audit\crates\*/tests/audit_*.rs`. They live on the `audit/2026-10-01` branch and are not committed yet.

Your job is to make users stop losing work, in priority order, and to prove each fix with an experiment that failed before it and passes after.

## Ground rules

1. **These instructions override the "fast mode" guidance in AGENTS.md and the project notes.**
   - Every fix needs an acceptance test that you ran, that failed before the change and that passes after it.
   - Do not defer tests to HARDEN.md.
   - The other AGENTS.md rules still apply: the render thread never blocks, no document-sized buffers, `cargo check --workspace --all-targets` stays green, and working code is not rewritten for style.
2. **The report is a hypothesis, not a spec.** Line numbers are from `main` at `1188748`. Re-read the code before editing. If a finding no longer reproduces, say so and move on. If the proposed fix is wrong, choose a better one and explain why.
3. **Keep each fix small and its own commit.** Don't refactor around a fix. When a finding needs an architectural change (listed under "Do not start without asking" below), stop and write a design note instead of building it.
4. **8-bit only for now (Rob's decision, 2026-10-01).**
   - Performance, memory and large-document work targets 8-bit documents. Run scale tests with `FOTOX_AUDIT_DEPTHS=8`.
   - Don't remove or rewrite 16-bit support, and don't do new work for it.
   - The existing small checks that cover 16-bit, such as `exact_round_trip_8_and_16_bit` and the 16-bit kill-loop fixtures, keep running so that 16-bit doesn't break unnoticed.
   - If a fix would only matter at 16 bits, note it in the log as *deferred (16-bit)* and move on.
5. **Report honestly.** Label every result *fixed + verified*, *fixed, not verified* (and why), *not reproduced*, or *deferred*. Never describe a test you didn't run as passing. Quote real numbers and the exact command.

## Setup

1. Record the starting state: branch, HEAD and `git status` for both the main repo and `E:\fotox-audit`.
   - Do not touch `main`'s working tree, `.freebuff/` or `stash@{0}`.
2. In `E:\fotox-audit`, commit the audit files exactly as they are on `audit/2026-10-01`: the five `audit_*.rs` files, the report and `docs/reports/audit-2026-10-01/`. Make one commit.
   - Use the repo's configured git identity, which is a GitHub noreply address. Never use another email.
3. Create a worktree `E:\fotox-fix` on a new branch `fix/audit-2026-10-01` from that commit. Do all the work there.
   - Don't push and don't merge into `main` until Rob says so.
4. Build and test with these settings:
   - `CARGO_TARGET_DIR=E:/fotox-fix-target` and `CARGO_INCREMENTAL=0`. C: is nearly full, so build output goes on E:.
   - Build only the test targets you need, e.g. `cargo test --release -p fx-engine --test audit_smart --no-run`. A full workspace test build is very slow.
5. Shells: the Bash tool is Git Bash. Any command you give Rob to run must be PowerShell 5.1 syntax, which has no `&&`.

## Machine safety (16 GB RAM, RTX 3060 12 GB, 12 threads)

- **Rob may have Fotox open. Never kill it**, and never touch his real documents or `%APPDATA%\Fotox`.
- **Every engine test commits about 7 GB** because the GPU atlas is reserved up front.
  - Run engine tests one at a time with `--test-threads 1`.
  - Never run two engine test processes at once.
  - Check free memory first, with `Get-Counter '\Memory\Available MBytes'` and the commit headroom. An 18.5 GB commit has frozen this PC before.
- **Start the watchdog** before any heavy run:

  ```bash
  powershell -NoProfile -File E:\fotox-audit-run\watchdog.ps1 -Log E:\fotox-fix-run\watchdog.log
  ```

  - Copy it to `E:\fotox-fix-run\` first, and add any new test binary names to its `$names` list.
  - If it kills a run, report that as a result. Don't raise its limits to make a run fit.
- **Isolate preferences.** Engine tests write `%APPDATA%\Fotox\preferences.json`. Always set `APPDATA` to a folder under `E:\fotox-fix-run\` (copy `E:\fotox-audit-run\appdata`, which sets `memory_budget_mb`).
- **Use both volumes for save tests.**
  - Fixtures and outputs go in `E:\fotox-fix-run` (E: is exFAT).
  - Save tests must also run on NTFS: set `FOTOX_AUDIT_DIR` to `%TEMP%\fotox-fix-c`, keep fixtures small, and only run there when C: has 40 GB or more free.
  - Delete the test data when you're done.
- **`audit_lazy_mips` leaks about 250 MB per hung attempt.** Before the fix, run it with `FOTOX_AUDIT_ATTEMPTS` of 15 or fewer.

## Harness gotchas

These all cost time during the audit:

- **`Harness::wait` / `until` drain every message up to the match.** Wait for several expected replies in one loop, or request state again (`RequestLayers`) instead of waiting twice.
- **Exports embed an ICC creation date.** It's the 12 bytes before `acsp`. Mask it before comparing hashes; `audit_scale.rs` already does this.
- **Frames stream continuously.** Measure latency as "time to the first frame whose pixels changed", as `Probe::changed_frame_after` does, not as time to the next frame.
- **Some audit tests are probes.** They print the defect but their assertion passes anyway; for example, `placed_picture_keeps_its_resolution` prints "covers 334/1000 columns". When you fix a finding, turn its probe into a gate: assert the correct behaviour, run it to see it fail on the unfixed code, then fix.
- **Gates must be plain tests where possible.** Make each new gate a normal, non-`#[ignore]` test that is small, fast and doesn't kill processes. Keep heavy, scale or kill-loop tests `#[ignore]`d and document how to run them.

## Work order

Work in this order (data loss first) and finish each item, verified, before starting the next. IDs are the report's.

### Phase 1: stop losing work (focused fixes)

**1. D1: Smart Object sampler deadlock.** `fx-engine/src/mips.rs` `LazyMips::read` holds a `Mutex` across a rayon `par_iter`, nested in `fx_ops::resample::resample`'s `par_iter`.
- **Fix:** never hold the lock across parallel work. Collect what's needed under the lock, release it, compute, then re-lock and install. Alternatively, compute sequentially inside the sampler.
- **Also guard against nested parallelism.** When mip work is reached from code already running on a rayon worker (`rayon::current_thread_index().is_some()`), compute those mips sequentially. GEGL, GIMP's engine, does the same: `gegl_parallel_distribute` runs any job inline while another one is already running. Measure what the guard costs on a large first draw.
- **Also:** give the single derived job (`engine.rs`: `start_derived` / `derived_done` / `derived_running`) a watchdog. A job that doesn't return in N seconds is abandoned, `derived_running` clears, and the event is logged.
- **Accept when:**
  - `audit_lazy_mips` has 0 hangs in 200 attempts with pools of 4, 12 and 32;
  - `pan_diagnostics` with `FOTOX_AUDIT_EXTRAS=smart` and `FOTOX_AUDIT_DIAG_LAYERS=1000` has 0 stalls in 20 runs;
  - tiles are back to 0 MiB within 5 s of closing;
  - a new document renders afterwards.
- **Check the cost:** report how long the successful calls take before and after. They were 165–172 ms before.

**2. D4 + SO2: Edit Contents.** In `engine/m12.rs` `save_contents`:
- clear `dirty` only after the parent was really updated;
- if the parent or the layer is gone, keep the tab dirty and offer "Save as new document";
- route the Save button in a contents tab's close prompt to `save_contents`, not `ask_save_path`;
- when a parent closes while it has open contents tabs, ask about them first;
- update every Smart Object whose `source.uid` matches, including nested ones, as one History step.

Accept when `edit_contents_when_the_parent_goes_away` and `editing_contents_updates_every_instance` assert the correct behaviour and pass.

**3. D3 + D11: Save As over a file another tab uses.**
- Keep a registry from path to open document. Opening a path that's already open activates that tab.
- Save As onto a path that another tab has open asks the user, or is refused.
- Before an incremental append, compare the handle's file ID with the path's current file ID (`GetFileInformationByHandle`: volume serial + file index). If they differ, do a fresh save and rebind the document to the path.

Don't fix this by switching to full-file rewrites on every save, which is how GIMP avoids the problem. The incremental append saves in 7–37 ms after a small edit; keep it.

Accept when `save_as_over_a_file_another_document_has_open` passes on NTFS and exFAT, and `two_documents_from_one_file_saved_from_two_threads` can't get two tabs on one path.

**4. I1 + P5: opening a .fxd.**
- In `fxd/container.rs` `read_chunk`, and wherever backed tile refs are built, reject any `offset + len` past the file's length before allocating.
- In `fxd/manifest.rs`, size the manifest buffer from the zstd frame's content size with a cap, or stream it through `take(LIMIT)`.

Accept when every case in `crafted_fxd_files` ends in a clean error with no abort, and `plain_small_fxd_open_peak` is under 64 MiB (it was 1,026 MiB).

**5. I2 + I3: hostile or huge images.**
- Estimate the decoded size from the header before decoding.
- JPEG: refuse with a clear message above a cap, or decode in tiles.
- TIFF: set real `Limits`, and read one-strip files in pieces or row by row.

Accept when every `crafted_image_files` case gives a clean error with a peak under 1 GB, and `tiff_strip_layout_peak_memory` shows one-strip within about 1.2× of 64-row strips.

**6. D5: a damaged newest save.** When the newest footer's manifest or structure fails, scan back to the previous valid footer, open that version, and show a banner. Put a save counter and timestamp in the footer's reserved bytes so a rollback is detectable. Check that this stays compatible with existing files.

Accept when, in `single_byte_corruption_by_region`, manifest flips open the previous version with a warning, footer flips report the rollback, and the wrong-pixel count stays 0.

**7. D6 + D7 + D10: atomic writes and leftovers.**
- Exports: `sync_all` before renaming. Use `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)` for fresh saves and exports.
- Preferences, brushes, patterns and shapes: write to a temp file, sync, rename, and keep a `.bak`.
- `FxdFile::open` opens read-only and upgrades to read-write lazily, so read-only `.fxd` files open.
- Remove `.part` on error, and sweep stale `*.fxd.part` at start.
- Translate OS error codes into clear messages; error 183 on exFAT is currently misleading.
- **Check free space before saving.** Nothing in the code checks free disk space today. Before a fresh save, check that the volume has the document's estimated size plus a margin free; before an incremental save, the estimated changed bytes. If not, refuse with "Not enough space on E: (needs X GB, Y GB free)". Disk-full during a save was never tested, so add a test: a fault-injecting writer that fails with "disk full" partway through. Both save kinds must keep the old file intact, leave no `.part` behind and keep the document dirty.

Accept when:
- a preference file truncated mid-way loads the previous version;
- a "disk full" failure in the middle of a fresh or an incremental save keeps the old file and the document dirty, with a clear message;
- read-only `.fxd` files open and Save As works from them;
- `cancel_at_every_batch_boundary` and `replace_refused_by_the_os` leave no `.part` behind.

**8. D8: compaction.** After a successful save, if `needs_compaction` (it exists in `fxd/save.rs` and is never called) and the file is over about 256 MB, run a background fresh save and replace the file, using item 3's file-ID check. On exFAT, close the old handles first or skip compaction.

Accept when 100 saves that each repaint one layer leave the file at no more than 2.5× its live bytes.

**9. T1 + T2: small fixes.**
- Engine tests set `APPDATA` to a temp folder in `common::Harness::start`.
- The Preferences dialog default matches the engine's 5 GiB (`ui/js/native/prefs.js:170`).

**10. Grow the GPU atlas on demand (the focused part of P1).**

`fx-render/src/gpu/atlas.rs` `TileAtlas::new` creates every page up front: 6 GiB as 1 GiB pages of 2,048 tile layers each, from `CompositorConfig::atlas_budget` in `compositor.rs`. That puts about 6.6 GB into the process's private bytes on the first frame, whatever the document's size.

- Start with one page.
- When the free list is empty and the page count is below the maximum, create the next page and add its slots to the free list. Evict with the existing clock hand only once every page exists.
- Set the maximum from the adapter's memory, for example half of VRAM, capped at today's 6 GiB, instead of a fixed 6 GiB.
- If the shader binds all the pages at once, rebuild the bind group when a page is added, and bind a one-layer placeholder texture for pages that don't exist yet. Check how the shader indexes the pages first.

Accept when, in `canvas_size_scaling` at 8 bits:
- private bytes after the first 4,096² frame are under 2 GB (they were 6,975 MiB);
- 16,384² still draws, saves, reopens and validates pixel-identical;
- panning stays at or under the audit's p95 (about 5 ms).

Report the private bytes at each size before and after.

### Phase 2: crash recovery (D2)

Write a short design note in `docs/` first and implement it **only after Rob approves**. The design is in report §9 D2 and §6.2 item 12:
- incremental `fxd::save` of dirty documents on a worker to `%LOCALAPPDATA%\Fotox\recovery\<session>\`, every N minutes and after M edits;
- emergency snapshots when the render thread dies, and on any crash the engine thread survives. GIMP has done this since 2.10: it backs up images with unsaved changes when it crashes and offers them at the next start. These snapshots are only a best effort, because an abort, a freeze or a power cut gives them no chance to run, so the periodic snapshots remain the main protection;
- offer recovery on start; clean up on a clean close or a successful save.

Accept when killing the process mid-edit and mid-save, then restarting, offers the document, and its content equals the state no more than N minutes before the kill. Compare exactly, as in `audit_save.rs`.

### Phase 3: large documents (each needs Rob's OK; P2 and P4 are focused)

- **P2:** move pixel Free Transform, Fill, Convert to Smart Object, Rasterize and the Edit Contents save-back onto the pixel-job path (`engine.rs` around 4729). Accept when a zoom sent during each one answers in under 50 ms at 16K. It waited 8.6 s before.
- **P4:** cancel tokens for open, import, save and export (`UiToEngine::CancelTask`), plus a cancel button. fx-io already honours `false` from progress callbacks.
- **SO1:** the nested document's canvas = the union of the layer bounds (or the placed image's own size). Place of a `.fxd` keeps its layers. Accept when `placed_picture_keeps_its_resolution` covers the canvas width after fit and ×1 equals the whole picture.
- **P1 (the rest):** budgets sized from the machine, a configurable warm tier, and backpressure on worker inserts. The lazily grown atlas is step 10. Accept against report §9 P1, using 8-bit documents: with hot = 1 GiB and warm = 1 GiB, a 16,384² 8-bit blur stays under the atlas actually used + 2.5 GiB + 1 GiB, and the 256 MiB-budget run completes. **This is architectural: design note first.**

### Phase 4: the 6,000-layer target

Rob's goal is to beat Photoshop on big documents: **a 16K or 30K canvas, 8-bit, with about 6,000 layers that each cover part of the canvas.** Full-canvas layers are out of scope, since 6,000 of them would be 6 TB at 16K. This combination has never been measured.

**Order and approvals.**
- Start only after step 1 (D1) is fixed, since the deadlock stalls every many-layer run.
- 4.1 and the patch half of 4.3 are focused and can start after Phase 1.
- 4.2, 4.4 and the region half of 4.3 need a design note that Rob approves.

**4.1 The target benchmark (do this first; re-run it after every Phase 4 change).**

Extend `audit_scale.rs` `layer_count_scaling` (or add a test beside it) to build the target document:
- 16,384², 8-bit, 6,000 layers, in groups;
- each layer a random rectangle of 256–4,096 px;
- content that compresses like real art: gradients plus light noise, **not** pure noise. Pure noise is incompressible and would measure the wrong thing;
- the audit's mix: every 7th layer masked, every 20th with a style, every 25th an adjustment, every 50th a Smart Object.

Then a 30,000² variant.

- Before building, estimate the raw and compressed data size and check it fits RAM + scratch on E: (60 GiB scratch limit). Run with the watchdog.
- Measure:
  - build time;
  - add layer, p50 / p99;
  - brush move → changed frame on a layer in the middle of the stack and on the top layer;
  - toggle a group;
  - undo;
  - incremental save after a brush stroke;
  - Save As;
  - reopen and first view;
  - pan p50 / p95 at fit and at 100 %;
  - private bytes and hot / warm / scratch tiles;
  - `.fxd` size.
- Save the first run as the baseline in `docs/reports/FIX-2026-10-01.md`.

Provisional targets, which Rob may change:
- brush to changed frame < 16 ms;
- add layer < 10 ms;
- pan p95 < 16 ms;
- incremental save < 1 s;
- reopen to first view < 2 s.

**4.2 Sparse slot map (design note first).**

`TiledImage` keeps a dense `Vec<TileSlot>` per mip level, one slot per tile position whether it is empty or not: about 17 B each (`fx-tiles/src/image.rs:46-63`). Every canvas-sized image costs about 0.3 MB at 30,000², empty or not. For 6,000 layers that is about 1.8 GB at 30K and about 0.5 GB at 16K, before masks and caches. This is calculated from the code, not measured.

- First add a counter for slot-grid bytes and measure it on the 4.1 document.
- Then replace the dense grid with a sparse structure, such as a hash map or a two-level table keyed by tile position, that stores only non-empty slots, without changing `TiledImage`'s public behaviour.

Accept when:
- slot-grid bytes on the 30K target are below 10 % of today's;
- `exact_round_trip_8_and_16_bit`, the `audit_save` kill loop and `canvas_size_scaling` at 8 bits still pass;
- brush and pan times on the 4.1 benchmark don't get worse.

**4.3 Per-layer costs (P3).**

- **Patches (focused).** Structural edits (add, delete, move, group) send insert / remove / move patches instead of the whole layer list. Property edits already use `layers_patch`. Accept when, on the 4.1 document, adding a layer at 6,000 layers is within 2× of adding one at 100 layers. Today it is 3.4 ms at the start and 37 ms at 5,000.
- **Effects by dependency (focused).** `effects::invalidate` (`effects.rs:16-40`) marks every styled layer dirty on any content change. Re-derive only layers whose own content, style or mask changed. Accept when a brush stroke on an unstyled layer re-derives 0 effect caches, and one on a styled layer re-derives 1. Add a counter to prove it.
- **Effects by region (design note).** Re-derive only the tiles the edit touched, plus the effect's apron.

**4.4 Composite cache for the stack below and above the active layer (investigate, then design note).**

When painting on layer 3,000 of 6,000, the other 5,999 don't change. If they are recomposited for every stroke, the cost grows with the layer count, and the upload budget of 48 tiles per frame makes it worse.

- **First find out what happens today.** Read the compositor (`fx-render/src/gpu/compositor.rs`, the composite cache of `composite_slots`) and measure: does a brush stroke on layer k recomposite every layer in the touched tiles? Report the answer with evidence before designing anything.
- **If it does, design:**
  - a per-tile cache of the composite of everything **below** the active layer;
  - for the stack **above** it, a cached "over" image only when every layer above is a Normal-mode pixel layer with no adjustment, clipping or pass-through group. Porter-Duff over is associative; other blend modes are not, so then the layers above must be recomposited per tile;
  - invalidation when anything outside the active layer changes.

Accept when:
- on the 4.1 document, a brush stroke on layer 3,000 reaches a changed frame within 2× of a stroke on the top layer of a 100-layer document;
- the flattened export after the strokes equals a full recomposite, exactly or within the existing f16 tolerance of the GPU tests.

**4.5 Memory at the target.** Re-run 4.1 after step 10 (atlas grown on demand) and, if Rob approves it, the rest of P1. Report private bytes and scratch use against the baseline.

### Do not start without asking

- D9 (cross-process locking, content-bound chunk refs, which is a format change);
- multi-disk scratch (§6.2 phase 2);
- recovering from wgpu device loss;
- decoding foreign files in a separate process, the way GIMP runs its file loaders as plug-in processes (it's the strongest long-term fix for I2, after step 5's size checks);
- an 8-bit (`Rgba8Unorm`) atlas for 8-bit documents. It would halve GPU memory, but the atlas is shared by every open document and the conversion to float currently happens before upload, so it needs a design note first;
- linked, Replace, Export or Relink Smart Objects, and filter masks (SO3).

Exception: if the per-filter blend-mode control is visible in the UI, hiding it is a focused fix.

## Each commit

1. **Red:** run the gate on the unfixed code and save the failing output.
2. **Fix:** keep the diff minimal.
3. **Green:** run the gate again. Also re-run the neighbouring audit harnesses the change could affect. For example, after any change under `fx-io/fxd`, run the `audit_save` kill loop (`FOTOX_AUDIT_KILLS=20`) on both volumes and the truncation sweep.
4. **Check:** run `cargo check --workspace --all-targets` and `cargo test -p <crate>` for the touched crates.
5. **Commit** with a message naming the finding ID, ending with:

   ```
   Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
   ```

6. **Log it.** Append to `docs/reports/FIX-2026-10-01.md`: finding, what changed (file:line), the command, the before/after numbers, the status label, and anything left over.

## When you stop

Finish with a summary covering:
- the starting and ending state (branch, commits);
- a table with one row per finding: status, commit, and the evidence that proves it;
- every test that was killed, skipped or flaky;
- regressions you noticed;
- what's left, with what each remaining item needs from Rob.

Do not claim parity with Photoshop or any other editor, or that Fotox is faster than it. Without a side-by-side run on the same PC, the same document and the same operations, report only what was measured in Fotox.
