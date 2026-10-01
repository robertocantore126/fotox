# Fotox: the big changes from the 2026-10-01 audit (code only)

This is the **second** prompt. It runs after `fotox-fix-prompt.md` has been completed, on the same branch.

- **Repository and branch:** worktree `E:\fotox-fix`, branch `fix/audit-2026-10-01`.
- **Read before anything else:**
  - the audit report `docs/reports/AUDIT-2026-10-01-large-docs-save-smart-objects.md`, sections 2, 3, 6 and 9 (P1, P3, X1);
  - the log of the first prompt, `docs/reports/FIX-2026-10-01.md`. Several items here build on its changes, especially item 10, the GPU atlas grown on demand.

**This is a code-only task.**
- Write the changes; don't write or run tests.
- Rob and Claude will test everything afterwards.
- Because nothing is tested, the rules below about switches, markers and the log matter more than usual: they're how we'll find, compare and fix your work.

## Goal

Rob's target is a 16K or 30K canvas, 8-bit, with about 6,000 layers that each cover part of the canvas, while staying smooth on a 16 GB PC with an RTX 3060 12 GB. The four changes below remove the costs that grow with memory pressure and with layer count.

## Rules

1. **Code only.**
   - No new tests, benchmarks or harnesses. Don't run test binaries, the app or any long-running process.
   - Don't edit or delete `crates/*/tests/audit_*.rs`. If an API change breaks how they compile, make the smallest change that compiles them again, and log it.
2. **It must compile.** If you can run commands, `cargo check --workspace --all-targets` must be green after every commit.
   - Set `CARGO_TARGET_DIR=E:/fotox-fix-target`.
   - Use `cargo check` only.
3. **Design note first, then code.** For each change, write `docs/design/<name>.md` first. Keep it to one or two pages:
   - what the code does today, with file:line;
   - the new design;
   - the invariants it must keep;
   - what can go wrong.

   Then implement it. If reading the code shows the design below is wrong, follow the code, and explain the difference in the note.
4. **Every behaviour change gets an off switch,** except item C, where it's impossible. The switch is an environment variable read once at start, which restores the old code path exactly:
   - `FOTOX_OLD_BUDGETS=1`
   - `FOTOX_NO_BACKPRESSURE=1`
   - `FOTOX_NO_COMPOSITE_CACHE=1`
   - `FOTOX_NO_REGION_EFFECTS=1`

   We'll compare old against new pixel by pixel, so the old path must stay intact and selectable.
5. **Markers.** Mark every change `// AUDIT-FIX(<ID>): <one line why>`. The IDs are `P1`, `X1`, `SPARSE`, `COMPCACHE` and `FXREGION`.
6. **Keep the hard rules:**
   - the render thread never blocks or waits;
   - no document-sized buffers;
   - working code is not rewritten for style;
   - 16-bit support is not removed or broken, though the work targets 8-bit.
7. **Small commits.** One commit per sub-step, so we can bisect, with the message naming the ID. Don't push or merge.
8. **Be honest in the log.** Write "implemented, untested". List every assumption and every place you're unsure.

## Order

Do them in this order: A (memory), B (scratch visibility), C (sparse slot map), D (effects by region), E (composite cache). E is the hardest and the most likely to be wrong; do it last.

### A. Memory budgets and backpressure (P1)

**Today:**
- `TileStoreConfig::reference_machine` (`fx-tiles/src/store.rs` around 184–193) fixes hot = 5 GiB, warm = 3 GiB and scratch = 60 GiB.
- Preferences only change the hot budget and the scratch folder (`fx-engine/src/lib.rs` around 321–327).
- The trim (`store.rs` around 726–772) snapshots and sorts every live tile each pass, and skips tiles anyone holds.
- Producers are never slowed down. Measured result: a 256 MiB hot budget ended at 2.0–2.4 GiB, and a 16K 16-bit blur reached 13.4 GB private.

**A1. Budgets from the machine.**
- At start, read total physical RAM (`GlobalMemoryStatusEx` on Windows) and use hot = 25 % and warm = 10 % of RAM by default.
- Both are preferences (`memory_budget_mb` already exists; add a warm one). If set, the preference wins.
- Show the effective values in Preferences next to the machine's total.
- `FOTOX_OLD_BUDGETS=1` restores the fixed 5 / 3 GiB.

**A2. Backpressure.** When hot + warm exceed their budgets by more than 25 %, `TileStore::insert` called from a thread that **opted in** waits on a `Condvar` that the trim signals after each pass.
- **Opting in** is a thread-local flag set only on pixel-job threads, the derived-job thread and the save/export/import workers.
- **Never opt in:** the render thread, the engine thread, or the shared rayon pool that the render thread uses to load missing tiles (`render.rs` around 516–529). Those would freeze the UI.
- **Wait at most 2 s per insert, then proceed** and log it once per job. The trim cannot evict tiles that someone holds, so an unbounded wait could livelock.
- `FOTOX_NO_BACKPRESSURE=1` disables it.

**A3. Cheaper trim.** Don't sort every live tile on every pass. Keep an approximate LRU, such as a clock over a ring of candidates or generation buckets, and stop as soon as the budget is met. Keep the same rule for held tiles.

**A4. The atlas maximum.** Item 10 of the first prompt sized it from VRAM. Make sure the composite cache (0.5 GiB today) is part of the same budget, and show the effective GPU budget in Preferences.

### B. Scratch failures visible, scratch safer (X1, report §6.2 items 3–6)

- **B1.** `MemoryStats` (`fx-protocol`, around line 306) gains `scratch_full`, `scratch_error: Option<String>` and `scratch_free_bytes`.
  - The status bar (`ui/js/canvas.js` around 146) turns amber when scratch is nearly full and red on full or error, with a clear message, e.g. "Scratch disk E: is full — free space or choose another folder".
- **B2. Free-space reserve.** The scratch file never grows when its volume has less than max(5 GB, 5 %) free. Treat that like `scratch_full`.
- **B3. CRC32 for each scratch extent.** Store it in the in-memory extent table, not the file, and check it on read. A mismatch becomes `TileError::Corrupt`, which is logged and shown in the status bar.
- **B4. Scratch folder preference.** Validate it on OK: the folder exists, is writable, and its free space is shown. A missing folder is never silently ignored at start (`lib.rs` around 321–323): fall back to the default **and** tell the user.

### C. Sparse slot map (SPARSE)

**Today:** `TileGrid` (`fx-tiles/src/image.rs` around 46–63) keeps, for every mip level, a dense `slots: Vec<TileSlot>` and a dense `dirty: Vec<bool>`, one entry per tile position, empty or not. A canvas-sized image costs about 0.3 MB at 30,000², even when empty. With 6,000 layers plus masks and caches that's gigabytes.

**Design:**
- Use a two-level table: chunks of 16 × 16 tile positions, `Vec<Option<Box<Chunk>>>` indexed by chunk position, where a `Chunk` holds 256 slots and 256 dirty flags.
- A chunk exists only if at least one of its slots is non-empty or dirty. Free the chunk when it becomes all-empty and clean.
- Use a two-level table, not a `HashMap`. It keeps O(1) access and makes row-major iteration cheap and deterministic. **Iteration order must stay row-major**, because save writes tiles in that order and identical documents must produce identical manifests.
- **Keep `TileGrid`'s and `TiledImage`'s public API and behaviour identical:**
  - every getter returns `TileSlot::Empty` for missing chunks;
  - `cols` and `rows` are unchanged;
  - a dirty level's "all dirty" initial state must still be represented. Use a per-grid "default dirty" flag rather than allocating every chunk.
- `Clone` must stay cheap. Documents are cloned for undo, save and derived jobs. Consider `Arc<Chunk>` with copy-on-write, and say in the note which you chose and why.
- No off switch for this one. Instead, keep the old dense implementation in the same file behind a `cfg(feature = "dense-grid")` cargo feature, so we can build the old one for comparison.

### D. Effects re-derived by region (FXREGION)

**Today:** the first prompt's item 4.1 limited `effects::invalidate` (`fx-engine/src/effects.rs` around 16–40) to the layers whose content, style or mask changed. Those layers still re-derive their whole effect cache.

**Design:**
- Pass the edit's dirty region (tile rectangle) into the invalidation.
- For each affected styled layer, mark dirty only the effect-cache tiles inside that rectangle grown by the effect stack's **apron**: the furthest distance any enabled effect reaches.
  - For a drop shadow, that's distance + size + spread.
  - For a glow or a stroke, size.
  - Write one `apron(&Styles) -> u32` in pixels, and round up to whole tiles.
- When the region is unknown (structural edits, a style change, mask replacement), fall back to the whole layer.
- `FOTOX_NO_REGION_EFFECTS=1` always uses the whole layer.

### E. Composite cache around the active layer (COMPCACHE)

**Goal:** when painting on layer 3,000 of 6,000, the cost of a stroke should not depend on the other 5,999 layers.

**E1. Investigate first.** Read the compositor (`fx-render/src/gpu/compositor.rs`, its composite cache of `composite_slots` and `TileOutcome`) and the render pipeline in `fx-render`. Write down in the design note, with file:line, what happens today on a brush stroke on layer k:
- which tiles are recomposited;
- how many layers' tiles are uploaded and blended;
- what the composite cache keys on.

If the existing cache already avoids recompositing the unchanged layers, say so, and implement only what's missing.

**E2. The "below" cache.**
- For each visible composite tile at the current mip level, cache the composite of everything **below** the active layer: one GPU texture slot.
- **Key:** the document, the tile, the level, the active layer, and a generation that changes when any layer below changes (content, properties, order, visibility, mask, style, adjustment).
- A stroke on the active layer then composites `below cache` + the active layer (with its clipped layers, mask and style) + the layers above.

**E3. The "above" cache, only where it's correct.** Porter-Duff "over" is associative, so the layers above can be pre-composited into one premultiplied image **only** if every one of them is a visible Normal-mode pixel (or fill) layer, with:
- no adjustment layer;
- no clipping onto layers below them;
- no pass-through group, and no group with a non-Normal mode or with opacity applied to its contents;
- no layer-style blend that reads the backdrop.

Otherwise, recomposite the layers above per tile as today. State the exact rule in the note.

**E4. Groups.**
- If the active layer is inside a group, the caches apply inside the group's own stack only when the group is isolated (not pass-through). Otherwise fall back to the full composite.
- Start with the simplest correct rule. Covering every case matters less than never drawing wrong pixels.

**E5. Memory and invalidation.**
- The caches take slots from the same GPU budget as the atlas. Evict them first under pressure.
- Any change outside the active layer, or switching the active layer, invalidates the relevant caches.
- The render thread never waits for a cache. A missing cache means the old full composite for that frame.

`FOTOX_NO_COMPOSITE_CACHE=1` disables all of E.

## Log and summary

**After each commit,** append to `docs/reports/FIX-2026-10-01.md`, under a heading "Second prompt":
- the ID;
- the files and functions changed;
- the switch that turns it off;
- assumptions and doubts;
- **what we should test first**, including the old-versus-new comparisons the switch allows.

**When you stop,** finish with a summary covering:
- the commits;
- a table with one row per item (A1–A4, B1–B4, C, D, E1–E5): implemented (untested) / partly done (what's missing) / skipped (why);
- the audit test files you had to touch;
- **the places most likely to be wrong**, ranked.

Don't claim that anything works or is faster, or that Fotox matches or beats Photoshop. Nothing has been tested yet.
