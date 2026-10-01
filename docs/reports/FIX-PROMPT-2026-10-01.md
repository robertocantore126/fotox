# Fotox: fix the findings of the 2026-10-01 audit (code only)

You are fixing bugs in Fotox, a Rust/wgpu image editor with a CEF UI.

- **Repository:** `C:\Users\39389\Documents\XuanZhi9\fotox`.
- **Audit report:** `E:\fotox-audit\docs\reports\AUDIT-2026-10-01-large-docs-save-smart-objects.md`. Read sections 1, 2, 4, 9 and 10 before you change anything. Section 9 gives each finding's location, cause and proposed fix.

**This is a code-only task.**
- Write the fixes; don't write or run tests.
- Rob and Claude will test and refine afterwards, using the audit's harnesses and a separate verification plan.
- Your job is to make the changes cleanly, in a way that's easy to check, and to say exactly what you did and what you weren't sure about.

## Rules

1. **Code only.**
   - Don't write new tests, benchmarks or harnesses. Don't run test binaries, the app or any long-running process.
   - Don't edit or delete the audit's test files (`crates/*/tests/audit_*.rs`). If an API change breaks how they compile, make the smallest change that compiles them again, and log it.
2. **It must compile.** If you can run commands, `cargo check --workspace --all-targets` must be green after every item.
   - Set `CARGO_TARGET_DIR=E:/fotox-fix-target`; C: is nearly full.
   - Run only `cargo check`, never `cargo test` or a release build.
3. **Read before you change.** The report's line numbers are from `main` at `1188748`. Re-read the code first. If a finding's cause isn't what the report says, fix the real cause and explain it in the log. If you can't find it, skip the item and say so.
4. **Small, findable changes.**
   - One commit per item, and don't refactor around a fix.
   - Mark every change with a comment `// AUDIT-FIX(<ID>): <one line why>`, e.g. `// AUDIT-FIX(D1): don't hold the image lock across par_iter`. This is how we'll find your changes to test them.
   - Keep the existing rules: the render thread never blocks, no document-sized buffers, working code is not rewritten for style.
5. **8-bit only for now.**
   - Performance and memory work targets 8-bit documents.
   - Don't remove or rewrite 16-bit support, and don't add new work for it.
   - Don't break it either: code paths shared with 16-bit must still handle it.
6. **The big changes are in a second prompt** (`fotox-fix-prompt-2-big.md`), which runs after this one. Skip items marked **second prompt**: don't write code or notes for them.
7. **Be honest in the log.** Don't call anything "fixed" or "working"; you haven't tested it. Say "implemented, untested". List every assumption you made and every place you weren't sure.

## Setup

1. Record the starting state: branch, HEAD and `git status` for both the main repo and `E:\fotox-audit`.
   - Don't touch `main`'s working tree, `.freebuff/` or `stash@{0}`.
2. In `E:\fotox-audit`, commit the audit files exactly as they are on `audit/2026-10-01`, as one commit. The files are the five `audit_*.rs` tests, the audit report, `FIX-VERIFY-2026-10-01.md` and `docs/reports/audit-2026-10-01/`.
   - Use the repo's configured git identity, which is a GitHub noreply address. Never use another email.
3. Create a worktree `E:\fotox-fix` on a new branch `fix/audit-2026-10-01` from that commit, and work only there.
   - Don't push and don't merge into `main`.
4. Rob may have Fotox open. Never kill it, and never touch his documents or `%APPDATA%\Fotox`.

## Items, in order (data loss first; IDs are the report's)

### Phase 1: stop losing work

**1. D1: Smart Object sampler deadlock.** `fx-engine/src/mips.rs` `LazyMips::read` (around lines 95–117) locks the image `Mutex` and, holding it, calls `ensure_mip` → `compute_tiles` (a rayon `par_iter`, around line 195). Its caller, `fx_ops::resample::resample` (`fx-ops/src/resample/mod.rs:67-82`), is itself a `par_iter` over destination tiles that each call `LazyMips::tile`. A worker waiting in the inner `par_iter` steals an outer task, which locks the mutex its own thread already holds.

- **Never hold the lock across parallel work.** Collect what's needed under the lock, release it, compute, then re-lock and install, if the level is still dirty.
- **Guard against nested parallelism.** When mip work is reached from code already on a rayon worker (`rayon::current_thread_index().is_some()`), compute those mips sequentially. GEGL, GIMP's engine, does the same: `gegl_parallel_distribute` runs a job inline while another is already running.
- **Watchdog for the derived job.** The single derived job (`engine.rs`, around 4143–4230: `start_derived` / `derived_done` / `derived_running`) gets one. A job that hasn't returned after N seconds (make N a constant, e.g. 60) is abandoned: `derived_running` clears, its result is ignored if it arrives later (generation check), and the event is logged.

**2. D4 + SO2: Edit Contents.** In `engine/m12.rs` `save_contents` (around lines 312–352):
- clear `dirty` only after the parent was really updated. Today it's cleared at line 327, before the checks;
- if the parent or the layer is gone, keep the tab dirty and offer "Save as new document";
- route the Save button in a contents tab's close prompt to `save_contents`, not `ask_save_path` (`engine.rs` around 2626, `close_answer`);
- when a parent closes while it has open contents tabs (`smart_children`), ask about them first;
- update every Smart Object whose `source.uid` matches, including nested ones, as one History step. Today there's a FAST note at lines 341–342 saying instances are not updated.

**3. D3 + D11: Save As over a file another tab uses.**
- Keep a registry from path to open document. Opening a path that's already open activates that tab.
- Save As onto a path another tab has open asks the user ("X is open in another tab: replace and close it / cancel") or is refused. Today `save_as` (`engine.rs` around 2808) only checks the document's own path.
- Before an incremental append (`fxd/container.rs` `append_to`, around 509–520), compare the handle's file ID with the path's current file ID (`GetFileInformationByHandle`: volume serial + file index). If they differ, do a fresh save and rebind the document to the path.
- **Don't** switch to rewriting the whole file on every save. The incremental append is a deliberate advantage.

**4. I1 + P5: opening a .fxd.**
- In `fxd/container.rs` `read_chunk` (around line 436), and wherever backed tile refs are built, reject any chunk whose `offset + len` is past the file's length **before** allocating. Today a crafted footer makes it allocate 1 TiB and abort.
- In `fxd/manifest.rs` (around 832–836), `zstd::bulk::decompress(payload, 1 GiB)` reserves the full 1 GiB on every open. Size the buffer from the zstd frame's content size with a cap (`zstd_safe::get_frame_content_size`), or stream through a decoder with `take(LIMIT)`.

**5. I2 + I3: hostile or huge images.**
- Estimate the decoded size from the header before decoding; refuse above a cap (e.g. 4 GiB of RGBA8) with a clear message.
- **JPEG** (`fx-io/src/jpeg.rs`): today a 600-byte header claiming 65,535² commits 16.4 GB at once.
- **TIFF** (`fx-io/src/tiff.rs`, `Limits::unlimited()` around lines 136–142): set real limits, about one band batch. Read one-strip files in pieces or row by row instead of one buffer for the whole strip.
- Use `Vec::try_reserve` at format boundaries so an allocation failure becomes an error, not an abort.

**6. D5: a damaged newest save.**
- In `fxd/open.rs` (around 32–53) and `container.rs` (around 324–347): when the newest footer's manifest or structure fails to load, scan back to the previous valid footer, open that version, and show a banner: "Recovered the version saved at …; the latest save was damaged".
- Put a save counter and timestamp in the footer's reserved bytes so a silent rollback can be detected and shown.
- Files written by the current version must still open: treat zeroed reserved bytes as "unknown".

**7. D6 + D7 + D10: atomic writes, leftovers, disk space.**
- **Exports** (`export.rs` around 97–121, `tiff_write.rs` around 266–276): `sync_all` the part file before renaming.
- **Replacing files on Windows:** for fresh `.fxd` saves (`fxd/save.rs` around 148–158) and exports, use `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`.
- **Preferences, brushes, patterns, shapes** (`prefs.rs` around 36–53, `brushes.rs` around 104–117, and the others that use `std::fs::write`): write a temp file, sync it, rename it, and keep a `.bak`. On load, fall back to the `.bak` if the main file doesn't parse.
- **Read-only files:** `FxdFile::open` (`container.rs` around 377) opens read-only and upgrades to read-write lazily at save, so read-only `.fxd` files can be opened.
- **Leftover `.part` files:** remove them on error or cancel (`save.rs` around 88–93). Give them unique names like exports already have. At start, sweep stale `*.fxd.part` files from folders of recently opened documents.
- **Error messages:** translate OS error codes into clear text. Error 183 on exFAT currently appears for a read-only target.
- **Free space:** nothing checks free disk space today.
  - Before a fresh save, require the document's estimated size plus a margin; before an incremental save, the estimated changed bytes.
  - If there isn't enough, refuse with "Not enough space on E: (needs X GB, Y GB free)" and keep the document dirty.

**8. D8: compaction.** `fxd/save.rs` has `needs_compaction` (around line 178), but nothing calls it. After a successful save, if `needs_compaction` holds and the file is over about 256 MB, run a background fresh save to `X.compact`, then replace `X`. Use item 3's file-ID check. On exFAT, close the old handles first or skip compaction.

**9. T2: preferences default.** `ui/js/native/prefs.js` (around line 170) shows 4,096 MB as the memory default while the engine uses 5 GiB. Opening Preferences and pressing OK silently changes the budget. Make them match.

**10. Grow the GPU atlas on demand.** `fx-render/src/gpu/atlas.rs` `TileAtlas::new` (around 46–74) creates every page up front, from `CompositorConfig::atlas_budget = 6 << 30` in `compositor.rs` (around 120–127). Each page is 1 GiB, holding 2,048 tile layers of `Rgba16Float`. That puts about 6.6 GB into the process's private bytes on the first frame, whatever the document's size.
- Start with one page.
- When the free list is empty and the page count is below the maximum, create the next page and add its slots to the free list. Evict with the existing clock hand only once every page exists.
- Set the maximum from the adapter's memory (e.g. half of VRAM, capped at today's 6 GiB), not a fixed 6 GiB.
- Read how the shader indexes pages first. If it binds all pages at once, rebuild the bind group when a page is added, and bind a one-layer placeholder texture for pages that don't exist yet.

### Phase 2: crash recovery (D2)

Fotox has no autosave or recovery at all (`fx-app/src/crash.rs` around 53–60 tells the user unsaved work is lost).
- Every N minutes (setting, default 5) and after M edits, for each dirty document, take `open.doc.clone()`. It's cheap, because tiles are shared.
- Write it on a worker with the existing incremental `fxd::save` to `%LOCALAPPDATA%\Fotox\recovery\<session>\<doc>.fxd`. Append to that recovery file, so the cost is the changed tiles. Never block the engine or render thread.
- When the render thread dies or another recoverable crash path runs while the engine thread is alive, write recovery snapshots before exiting. This is best effort; the periodic snapshots are the main protection.
- At start, list recovery files from sessions that are no longer running and offer to reopen them. Delete a document's recovery file on a successful save or a clean close.
- Keep recovery files out of the scratch file, which is deleted on close.

### Phase 3: large documents

- **P2:** move these off the engine thread and onto the pixel-job path (`engine.rs` around 4729–4767):
  - pixel Free Transform, which blocks for 8.6 s at 16K today;
  - Fill;
  - Convert to Smart Object;
  - Rasterize;
  - the Edit Contents save-back.
- **P4:** cancel tokens for open, import, save and export. Today their progress closures always return `true` (`engine.rs` around 1628, 1730, 2859). Add `UiToEngine::CancelTask { task }` and a cancel button on the progress bar. fx-io already stops when a progress callback returns `false`.
- **SO1:** in `fx-core/src/command/m12.rs` `convert_to_smart` (around 74–115), the nested document is created with the parent's canvas size, so anything outside the canvas is cropped.
  - Make the nested canvas the union of the converted layers' bounds (or the placed image's own size), and let the transform place it.
  - Place of a `.fxd` (`engine.rs` around 1652) should keep its layers instead of flattening.
- **P1, the rest:** budgets sized from the machine, warm tier, backpressure. **Second prompt; skip.**

### Phase 4: toward 6,000 layers

Rob's goal: a 16K or 30K canvas, 8-bit, with about 6,000 layers that each cover part of the canvas.

- **4.1 Effects by dependency.** `effects::invalidate` (`effects.rs` around 16–40) marks every styled layer dirty on any content change. Re-derive only the layers whose own content, style or mask changed.
- **4.2 Layer list patches.** Structural edits (add, delete, move, group) send insert / remove / move patches to the UI instead of the whole layer list. Property edits already use `layers_patch`; follow that pattern on both the engine and the `ui/js` side.
- **4.3 Sparse slot map, 4.4 composite cache, 4.5 effects by region:** **second prompt; skip.**

### Not now

Don't start any of these:
- D9 (cross-process locking, content-bound chunk refs; a format change);
- multi-disk scratch;
- recovering from wgpu device loss;
- decoding foreign files in a separate process;
- an 8-bit atlas;
- linked, Replace, Export or Relink Smart Objects, and filter masks;
- per-filter blend modes.

Exception: if the per-filter blend-mode control is visible in the UI, hide it, because it's stored but ignored when rendering (`smart_filters.rs` around line 9).

## Each commit

1. Make the change, with `// AUDIT-FIX(<ID>)` markers.
2. Run `cargo check --workspace --all-targets` if you can run commands.
3. Commit with a message naming the finding ID, ending with:

   ```
   Co-Authored-By: <your model name> <noreply>
   ```

4. Append to `docs/reports/FIX-2026-10-01.md`:
   - the finding ID;
   - the files and functions changed;
   - what you did and why;
   - assumptions and anything you weren't sure about;
   - **what we should test first.** The audit's harness and test names are in report §9 under each finding; name the ones that apply.

## When you stop

Finish with a summary covering:
- the starting and ending state (branch, commits);
- a table with one row per item: implemented (untested) / design note written / skipped (why) / blocked (why);
- every audit test file you had to touch to keep it compiling;
- the places you think are most likely to be wrong.

Don't claim that anything works, or that Fotox matches or beats Photoshop. Nothing has been tested yet.
