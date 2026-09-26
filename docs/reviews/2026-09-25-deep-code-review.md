# Deep Code Review — Exhaustive Bug Hunt

**Repository:** `fotox` (Rust workspace + CEF/JS UI)
**Revision reviewed:** branch `m6`, `b0bd9ef` *Merge M5: selections and painting* (working tree clean at review time)
**Date:** 2026-09-25
**Scope:** whole repository, adversarial. Read-only: **no source file was modified**; this document is the only artifact added.
**Method:** repository model → feature-coverage matrix → repo-wide symbol search → invariant verification → adversarial execution analysis (rapid actions, invalid state, extreme input, async/concurrency) → state & history audit. Every finding below is anchored in code that was read, not in documentation.

> *Nota:* the report is written in English to match the repository's own documents (`AGENTS.md`, `docs/ARCHITECTURE.md`, the `docs/reports/*` cards). All anchors are `path:line` at `b0bd9ef`.

---

## 0. Severity and confidence legend

| Severity | Meaning |
|---|---|
| **S1 — Critical** | Data loss/corruption, permanent unusable state, or a crash class the user cannot escape without restarting the app. |
| **S2 — High** | Wrong user-visible result, or a defect that is reachable in ordinary use and silently destroys work-in-progress semantics. |
| **S3 — Medium** | Incorrect behaviour in a specific but realistic path, resource/perf cliff, or a fragile coupling that fails under load. |
| **S4 — Low** | Robustness, hardening, dead code, contradicting a documented contract without breaking a user flow. |

*Confirmed* = traced through the code and the reachability chain; *probable* = the mechanism is proven but a full end-to-end repro was not executed (no code was modified, so no instrumentation was added); *latent* = the trap exists but today's call graphs do not step on it.

---

## 1. Repository model

### 1.1 Processes and threads

| Thread | Owner | Responsibility | Blocks on |
|---|---|---|---|
| main | `fx-app` | winit event loop, final composite of the CEF UI over the viewport | GPU present |
| engine | `fx-engine/src/engine.rs:258` (`Engine::run`) | single-threaded event loop over `EngineInput`, `select_biased!` on input / `internal_rx` / `mips_rx` / timeout | **never** on `TileStore::get` by design (but see S4-02) |
| render | `fx-engine/src/render.rs` | `TilePipeline::frame` → plan, build programs, composite on GPU, draw, tessellate overlays | `try_get_hot` only (non-blocking) |
| rayon pool | shared | import/export/mips/filters/tile decompression/thumbnails | expected |
| trim | `fx-tiles/src/store.rs:869` | LRU demotion hot→warm→cold, drop derived tiles | expected |
| job threads | `std::thread::Builder` in `engine.rs` | `pixel-job-N`, `export-N`, `save-N`, `import-N` | expected |

### 1.2 Subsystems and external boundaries

- **fx-protocol** — JSON control + binary frames; `UiToEngine` / `EngineToUi` / `EngineInput` / `EngineOutput`, `UI_LOCAL_ACTION_PREFIXES` (`lib.rs:157`). Boundary: CEF ↔ engine, engine ↔ shell.
- **fx-core** — pure document model: `Document` (cheap clone: `Arc<Layer>`, tile handles), `Layer`, `History` (snapshots, limit 50), `Command` + `CommandEffect`, `Selection`, `pixels`, `stroke`.
- **fx-tiles** — `TileStore` (hot/warm/cold/backed), `TiledImage`, `TileBuffer`, mip downsampling, scratch file, `unsafe` casts (`store.rs:104,126`).
- **fx-ops** — filters, neighbourhood/gaussian, raster, morph, flood, brush (stroke/heal).
- **fx-render** — plan/program/reference compositor, overlay tessellation, GPU compositor + atlas + prefix cache, WGSL.
- **fx-io** — `.fxd` container (append-only, CRC-32 per chunk, footer recovery), PNG/JPEG/TIFF, bands, export.
- **fx-engine** — orchestration: documents, view, ops (CPU `PixelOps`), stroke session, selection overlay, mips, thumbs, export, layers/filters/stats.
- **fx-app** — winit window, CEF host, input routing (`input.rs`), clipboard (`unsafe`), Windows/macOS/Linux specifics.
- **ui/js** — the real UI: `main.js`, `state.js`, `native/*` bridging the engine, `data/*` menus/dialogs.

### 1.3 Authoritative vs derived state

| State | Owner | Mutated by | Invalidated by | Consumers |
|---|---|---|---|---|
| `Document` (+`revision`) | `OpenDoc` | every `Command`, strokes, undo/redo | `changed()` also bumps `generation` (`documents.rs:206`) | render snapshot, history, export |
| `OpenDoc.generation` | `OpenDoc` | `changed()` only (`documents.rs:205`) | — | `render_generation()` = render cache key |
| `OpenDoc.snapshot` (`Arc<Document>`) | `OpenDoc` | `snapshot()`; new `Arc` when `(revision, preview_rev)` changes or `snapshot_stale` | `invalidate_snapshot()` (mips) | `TilePipeline` caches keyed on **pointer identity** + `generation` |
| `History` | `OpenDoc` | `execute`/`record`, `undo`/`redo` | `redo.clear()` on a new step | History panel, `can_undo/redo` |
| `Document.selection` | `Document` | selection commands, `history_only` effects | not persisted (D-028) | tools, overlays, pixel ops |
| `OpenDoc.busy` | engine | `start_pixel_job` / `pixel_job_done` | — | gate for `command`, `step_history`, stroke `Begin` |
| `Engine.stroke` (live session) | engine | `stroke_event` | `end_stroke()` | stroke pixels, one `Command::Stroke` |
| `TileStore` tiers | store | inserts, `attach_backing`, trim | trim thread | render (`try_get_hot`), ops (`get`) |
| `TilePipeline.{ready,programs,current,snapshot,mips_sent}` | render | `frame()` | generation change (ready/programs), snapshot change (mips_sent) | frames |
| `GpuCompositor.{cache,composite_slot_owner,atlas}` | render | `composite()` | frame counter / LRU | frames |
| `Engine.selection_overlay` | engine | `selection_overlay()` | `command()` when `history_only`, explicit `None` | overlay |
| `Engine.tools` | engine | tool registry | **never across documents** (see H-02) | pointer routing |

### 1.4 Entry points

Startup (`fx-app/src/main.rs`, `Engine::run`), CLI (`fx-cli`), UI actions (`ui/js/actions.js:runAction`), shortcuts (`shortcuts.js`), pointer/keyboard (`input.rs` → `EngineInput`), file open (`EngineInput::Open`, drag/drop, CLI), save/export (`doc:save`, `EngineInput::Save/SaveAs/Export`), render loop (`RenderRequest::Frame/Wake/Stop`), timers (`view_message_deadline`, `hot_expiry`, `thumbs_due.min()` as the loop deadline, `send_status_if_due`), async returns (`Internal::{PixelJobDone, Exported, Copied, Saved, Thumbnail, PreviewTiles, Imported, OpenedFxd, B3Built, Progress}`), worker boundaries (`MipWork`, `RenderRequest`).

### 1.5 Invariants asserted by the project

1. **The One Rule** (`AGENTS.md`): no operation may cost memory proportional to the document; bands and tiles only.
2. The render thread never blocks on disk, `TileStore::get`, or the engine.
3. `expect`/`unwrap` in non-test code only for a true invariant, with an explanatory string.
4. `unsafe` only in the three files listed in `AGENTS.md`, each with `// SAFETY:`.
5. No `todo!`/`unimplemented!`/commented-out code in scope (grep over `crates/*/src` and `ui/js` returns **nothing** — verified again at this revision).
6. Only approved dependencies in the root `Cargo.toml` (adding one requires a `docs/DECISIONS.md` line).
7. A tile handle is referenced only while its document is alive; a tile of a live document is always readable.
8. Selection is a history step but not content, and not saved (D-028).

Invariants 1, 2 and 7 are the ones this review found breakable (S1-01, L-03, S1-03).

---

## 2. Feature coverage matrix

Status: **OK** = traced end-to-end and found consistent; **BUG** = a finding below; **GAP** = a stage of the lifecycle is missing/bypassed.

| Feature | Entry | State mutation | Persistence | Rendering | Cleanup | Status |
|---|---|---|---|---|---|---|
| Open `.fxd` (lazy, backed tiles) | `EngineInput::Open` → `Internal::OpenedFxd` | `docs.add`, `insert_backed` | footer+manifest checked, **chunks lazy** | first frame requests mips | close drops handles | OK, but a corrupt chunk surfaces much later → S1-03 |
| Open PNG/JPEG/TIFF | same | import job, mips | n/a | OK | OK | OK |
| New/duplicate/delete layer | action → `Command` | `History::execute` | on save | `after_edit` | — | OK |
| Move/group/reorder | `Command::MoveLayer/GroupLayers` | `children_mut` | on save | OK | — | OK (see §6 non-findings) |
| Paint stroke (brush/pencil/eraser/clone/heal) | pointer → `StrokeEvent` | live `TiledImage` + revision, one `Command::Stroke` at up-end | on save | mips for touched tiles | `end_stroke` on command/undo/Begin | **BUG** S2-01, S2-02 |
| Tool switch | `tool:<id>` action (`view.rs:91`) | `view.tool` only | — | overlay of the new tool | **none** for the old tool | **BUG** S2-01 |
| Selections (marquee/lasso/wand/modify) | tool → `Command::Select/ModifySelection/MagicWand` | `doc.selection`, `revision` | **not saved** (D-028) | overlay cache + `history_only` path | `selection_overlay = None` on history-only | OK |
| Selection outline drag / nudge | `OutlineDrag`, arrow keys | `OffsetSelection` | not saved | ants translated, cached key unchanged | — | OK |
| Filters (Gaussian, Unsharp, …) | `ApplyFilter` pixel job | job thread → `pixel_job_done` | on save | thumbnail refresh | preview cancelled on done | **BUG** S1-01, S3-01 |
| Merge/Flatten/Stamp Visible/Copy Merged | pixel job / clipboard | `composite_layers` | on save | OK | — | **BUG** S1-01 |
| Levels/Curves/adjustments | `SetAdjustment` | `History::execute` | on save | program rebuild | — | OK |
| Undo/redo | `hist:undo/redo` | `History::{undo,redo}` + `generation` | on save | full refresh | thumbnails on show | OK (dirty flag caveat S3-02) |
| Save incremental | `doc:save` | `save-N` thread, `attach_backing` | `.fxd` append | — | `pending_close` | **BUG** S1-02 |
| Save As / fresh save | `SaveAs` | part file + rename | `.fxd` | — | `pending_close` | **BUG** S1-02, S2-04 |
| Export flattened | `EngineInput::Export` | `export-N` thread, band by band | PNG/TIFF/JPEG | — | progress done | **BUG** S3-03 |
| Close document / window | `CloseDocument`, `CloseRequested` | `force_close`, `MayClose` | prompt when `dirty` | — | thumbs retained-cleared | **BUG** S3-01 (in-flight job) |
| Zoom/pan/Fit/Fill/Print | view actions + `SET_ZOOM` | `ViewState` | not saved | render plan | — | **BUG** S2-05 |
| Thumbnails | `RequestThumbnails` | `thumbs_last/due/wanted` | — | binary frames | `thumbs_due` throttle, retained on close | OK |
| Colour management / proof / CMYK | actions, jobs | LUT caches | profile on save | display LUT | LUT dropped on profile change | OK |
| Clipboard (OS + internal) | `clip:*` | `SharedClipboard`, paste command | — | — | — | OK, L-03 |
| B3 benchmark load | `debug:b3` | **no history**, history reset | — | — | — | OK (documented) |

---

## 3. Findings

### S1-01 — Critical, confirmed: `composite_layers` and `EngineOps::filter` materialise the whole document

**Rule violated:** the One Rule (`AGENTS.md`), for two ordinary operations.

`fx-engine/src/export.rs:195`:

```rust
let tiles: Vec<((u32, u32), fx_tiles::TileBuffer)> = programs
    .par_iter()
    .map(|(pos, program)| { /* render one 256² tile */ (*pos, tile) })
    .collect();
let mut image = fx_tiles::TiledImage::new(doc.width, doc.height, format);
for ((tx, ty), tile) in tiles { image.put_buffer(store, tx, ty, tile); }
```

`fx-engine/src/ops.rs:44` (identical pattern for filters):

```rust
let results: Vec<Result<FilteredTile, TileError>> = tiles.par_iter().map(...).collect();
let mut out = image.clone();
for result in results { let ((tx, ty), tile) = result?; out.put_buffer(store, tx, ty, tile); }
```

Every output tile is resident at once, on top of the returned image and (for the filter path) the source. Numbers: a 30 000 × 30 000 16-bit document is 13 736 tiles × 512 KiB ≈ **6.7 GiB**, plus the same again for a filter; a 10 000² document is ≈ 780 MiB. Reachability is not theoretical: `is_pixel_job` (`engine.rs:2659`) routes `ApplyFilter`, `MergeLayers`, `Flatten`, `StampVisible`, `ConvertProfile` through exactly these two functions, and `Command::CopyMerged` reaches `composite_layers` from the clipboard path too.

The codebase contradicts itself in the same crate: `export_document` (`export.rs:126`) renders **one band of tiles at a time** and reuses the buffer, and `EngineOps::convert` (`ops.rs:69`) collects *slot handles* only. So the streaming shape is known and used — these two call sites are the outliers.

**Impact:** OOM / swap death on large documents (`docs/PERFORMANCE.md` targets exactly these sizes). Bounded only by the OS.
**Fix direction:** write each rendered tile into the store inside the parallel map and keep only the handles, exactly as `convert` does; or render band by band as `export_document` does with a row of results reused.

---

### S1-02 — Critical, confirmed: holding `Ctrl+S` runs several writers on the same `.fxd`, corrupting it

Three facts combine:

1. `ui/js/shortcuts.js:175` resolves the shortcut and calls `runAction` on **every** `keydown`, and the handler never looks at `event.repeat` (grep for `e.repeat` over `ui/js` returns nothing). A held `Ctrl+S` therefore fires `doc:save` at the keyboard auto-repeat rate (≈30/s after the initial delay).
2. `runAction` (`ui/js/actions.js:24`) forwards each one to the engine: `Engine::action → "doc:save" → save(id) → start_save(id, SaveTarget::Incremental(file))`.
3. `start_save` (`engine.rs:1548`) has **no in-flight guard**: `OpenDoc` has no `saving` flag (its fields are `dirty, generation, last_edit, hot, snapshot, snapshot_stale, file, path, preview, preview_rev, busy, snapshot_key, proof, proof_colors, gamut_warning, mask_target`), and it spawns a fresh `save-N` thread per call.

Each of those threads calls `FxdWriter::append_to(file.clone())` (`container.rs:467`) with `pos = file.footer.end_offset` — **the same start offset** — and `FxdWriter` holds only `Arc<File>` plus a `pos: u64`; `write_chunk` (`container.rs:502`) does positional `write_all_at` with no lock, no file-length check, no reservation. So the writers overlap: A's chunk at offset X can be overwritten by B's different-length chunk at X, each writer's `pos` advances independently, and `commit` (`container.rs:526`) then writes its footer at *its own* `pos` and calls `set_len(footer.end_offset)`, truncating the other writer's tail. The result is a file whose footer/manifest may reference offsets holding another save's bytes (CRC mismatch on read) or, if lengths coincide, silently wrong pixels.

`Engine::save` is also reachable from the close-dirty "Save" answer, so a user-initiated save racing the close prompt does the same thing.

**Impact:** silent corruption of the user's only copy of a document. The window is the duration of a save, i.e. seconds on the documents this app targets.
**Fix direction (any one, preferably all):** (a) ignore `event.repeat` in `shortcuts.js` (and/or debounce `doc:save` in `runAction`); (b) add a `saving: Option<TaskId>` to `OpenDoc` and make `start_save` return early with a toast; (c) give `FxdWriter` exclusive access to the file (lock the `FxdFile` for the duration of an append, or recompute `pos` under that lock) so two writers can never interleave.

---

### S1-03 — Critical, confirmed: a panic on a worker thread wedges the document forever

`Engine::start_pixel_job` (`engine.rs:1170`) sets `open.busy = Some(label)` at line 1173 and never clears it except in `pixel_job_done`, which is fed by `Internal::PixelJobDone` **sent from the job thread after `command.apply` returns** (`engine.rs:1203`). Every guard then refuses work:

- `command()` (`engine.rs:1610`, guard at `1618`) → `if let Some(job) = &doc.busy { toast("Wait until {job} is finished"); return; }`
- `step_history()` (`engine.rs:1688`, guard at `1692`) → same
- `stroke_event(Begin)` (`engine.rs:550`, guard at `564`) → same

The job body can panic on ordinary error paths, because several closures turn a `TileError` into a panic:

| Site | Panic |
|---|---|
| `export.rs:130`, `export.rs:191` | `store.get(h).expect("tile of a live document")` |
| `ops.rs:209` | `self.store.get(h).expect("tile of a live document")` |
| `stroke.rs:112` | `self.store.get(h).expect("tile of a live document")` |

`TileStore::get` returns `TileError::Corrupt` for a chunk whose CRC fails (`fxd/container.rs:412`, `store.rs:858`) and `TileError::Corrupt("cold tile without scratch file")` (`store.rs:652`). A corrupted or torn `.fxd` chunk, or a missing scratch file, therefore panics *inside a `rayon` closure*; rayon re-raises the panic on the job thread; the thread dies before `internal.send(...)`; `busy` stays `Some(label)` for the lifetime of the process. The `EngineToUi::Progress { task }` was already sent (`engine.rs:1177`), so the status bar shows a job that never finishes, and every subsequent command, undo and stroke is refused with "Wait until … is finished". There is no `catch_unwind`, no timeout, no watchdog.

Note the compounding path: S1-02 produces a corrupted `.fxd`; opening it succeeds (only the footer and the manifest are read at `open`); the corruption surfaces on the first `store.get` of the bad tile inside a pixel job → the document is permanently unusable.

**Impact:** unrecoverable in-session state (the user must restart and lose the document), from a file-level fault the app already detects as `Corrupt`.
**Fix direction:** make the fetch closures return the error (`reference.rs`'s `render_tile` already takes a fallible `fetch`-like abstraction — `export_document` itself maps a program failure to `IoError`), and wrap the job body so that `Internal::PixelJobDone` is sent on *every* exit path (a drop guard around the send is enough).

---

### S2-01 — High, confirmed: changing tool mid-stroke leaves the painting tool stuck in "stroking"; hover then paints

- `ViewState::action` (`view.rs:90`) handles `tool:<id>` as a bare assignment (line 92): `self.tool = tool.to_owned(); return Some(Changed::default());` — no tool teardown, no `end_stroke()`, no per-tool reset.
- `Engine::tool_pointer` (`engine.rs:420`) picks the tool from `open.view.tool` **at event time**, so the `PointerKind::Up` that ends a gesture is delivered to whatever tool is active *then*.
- `Paint::pointer` (`tools/paint.rs`) treats `PointerKind::Move if self.stroking` (line 251) as stroke input and only clears `stroking` in `PointerKind::Up if self.stroking`. It never requires a pressed button on `Move`.

Sequence: press and hold the Brush → press a tool shortcut (`E`, `V`, …) or click a toolbar button → release. The release goes to the new tool, so `Paint.stroking` stays `true`. Switching back to the brush and merely *hovering* now emits `StrokeEvent::Add` samples and paints, and the next release records a `Command::Stroke` for a gesture the user never made. If the new tool is an unimplemented one (`NotYet`) or `quick-select`, its `Up` does nothing at all and the engine's live `Session` (`self.stroke`) is left open with the already-painted pixels in place; the eventual `end_stroke()` (triggered by the next command or undo) records a single history step whose `before` snapshot spans **two** gestures, so one Undo discards both.

**Impact:** unintended pixels; history steps that do not correspond to a user action; a live session outliving its gesture.
**Fix direction:** a `Tool::cancel(&mut self)` (or `Tool::end_gesture`) called when `view.tool` changes, plus `end_stroke()` when the active tool changes, plus requiring `buttons != 0` on `Move` before feeding samples.

---

### S2-02 — High, confirmed: tool state is engine-global, not per document

`Tools` is one `HashMap<String, Box<dyn Tool>>` on the `Engine` (`tools/mod.rs:326`), created once and never keyed by `DocId`. `Paint` caches `hover`, `pen`, `diameter`, `source`, `aligned`, `stroke_offset`, `zoom`, and `stroking`. `ToolContext` carries no document identity, so a tool cannot even tell that the document changed.

Consequences with two documents open:
- The clone/heal source is stored in **document coordinates** (`paint.rs`: `self.source = Some((event.x, event.y))`) and survives a document switch: with `Aligned` on, `self.aligned` (the destination − source offset) is reused on the *other* document, sampling from `(dst − offset)` in that document's space — a wrong sampling origin that is silently the wrong pixels.
- `Paint.hover` is reused for the overlay of the newly active document, so the brush outline appears at coordinates taken from the other document until the first pointer move, and `Paint.diameter`/`zoom` are stale.
- The live-stroke fields (`stroking`, `pen`) are shared, so the S2-01 failure mode also crosses tabs.

**Impact:** wrong clone/heal output; a misleading overlay; a class of bugs that cannot be fixed locally in `Paint`.
**Fix direction:** key the tool registry by `(DocId, tool)` or reset tool state on `ActiveDocument` change; carry the `DocId` in `ToolContext` and validate it in each tool.

---

### S2-04 — High, probable: `Export` writes whichever document is active when the dialog resolves

`EngineInput::Export { path, choice }` (`fx-engine/src/lib.rs:122`) carries **no document id**, `Engine::export` (`engine.rs:998`) resolves `self.docs.active_mut()`, and the shell forwards it the same way (`fx-app/src/app.rs:434`). The `export:as` action first calls `ask_save_path(doc, name)` with the *then*-active document, but the shell's file dialog is asynchronous (the engine keeps the event loop running), so if the user activates another tab before confirming, the chosen file receives the wrong document's composite. Note that `EngineInput::Save { doc }` and `SaveAs { doc }` do carry the id — `Export` is the only file-writing input that does not, which looks like an oversight rather than a decision.

**Fix direction:** add `doc: DocId` to `EngineInput::Export` (the `NeedSavePath`/`ExportAs` action already knows it) and answer with a toast if the document has been closed meanwhile.

---

### S2-05 — High, confirmed: "Fill Screen" is a hardcoded 200 %, "Print Size" a hardcoded 72 %

`ui/js/actions.js:121-122`:

```js
if (a === "zoom:fill") { zoomTo(200); status("Fill screen"); return; }
if (a === "zoom:print") { zoomTo(72);  status("Print size"); return; }
```

and the status-bar menu does the same (`ui/js/main.js:167`). In the native app `zoomTo` (`ui/js/canvas.js:306`) forwards a raw `SET_ZOOM { zoom: z / 100 }`, so View ▸ Zoom ▸ Fill Screen sets exactly 200 % regardless of the viewport size and of the document size — for a 10 000 × 10 000 document in a 1 600 px-wide viewport that is not "fill", it is "fit about 3×". "Print Size" ignores `doc.ppi` entirely (it is stored, exported and shown in the UI), so a 300 ppi document prints-size at 72 %, roughly 3× too large on screen.

Meanwhile the engine's real fit action exists and is reachable: `view.action("zoom:fit")` (`view.rs:99`) is consulted *before* the UI-local prefix check (`engine.rs:923`), and it is what `Ctrl+0` uses. So `zoom:fill` is simply not wired to anything on the engine side, and the local fallback is a placeholder that never got replaced.

**Fix direction:** give the engine a `zoom:fill` action (zoom so the document covers the viewport, `ViewTransform::fill`) and a `zoom:print` that divides by the ratio between the document ppi and the monitor ppi reported by the shell.

---

### S3-01 — Medium, confirmed: the dirty flag lags the in-flight edit, so a close can drop it silently

`start_pixel_job` (`engine.rs:1170`) does **not** set `open.dirty = true`; only `pixel_job_done` does, when the job lands (`engine.rs:1243`). While a job runs, `dirty` still reflects the state before it. Consequences:

- A clean document (freshly imported/opened) with a running `Flatten` or `ApplyFilter` is closed by `Ctrl+W` with no "save changes?" prompt, and `pixel_job_done` then early-returns (`let Some(open) = self.docs.get_mut(id) else { return }`) — the whole job is discarded with no user-visible event.
- `CloseRequested` (`engine.rs:1484`) scans `self.docs.iter_mut().find(|open| open.dirty)`; the same hole means the app can exit while a job thread is still writing pixels into a document that is about to be dropped.
- `close_answer(Save)` → `save(id)` snapshots `open.doc.clone()` — the **pre-job** document — so the saved file can legitimately lack the edit the user just asked for, and `dirty` is then set to `open.generation != generation`, which happens to be `true` (the job bumped the generation), leaving the document dirty and the job result unsaved. The user asked to "save" and got the state before their last edit plus an unexplained dirty marker.

**Fix direction:** set `dirty = true` (and a `has_pending_work` flag) when a job starts, and refuse `close`/`CloseRequested` while `busy.is_some()` (or offer to wait/cancel).

---

### S3-02 — Medium, confirmed: a stale `Internal::Saved` can re-mark a saved document dirty

`Internal::Saved` (`engine.rs:1396`) sets `open.dirty = open.generation != generation` using the generation captured when *that* save started. There is no ordering or "latest wins" rule, and (per S1-02) no in-flight guard. Two saves on a document edited in between: the newer save finishes first and clears `dirty`; the older one lands afterwards and sets `dirty = true` because `open.generation` has moved on. The document is fully saved but the UI shows unsaved changes, and the next close prompts to save again. Symmetrically, `open.file`/`open.path` are overwritten by whichever completion arrives last, so a stale save can install an older `FxdFile` handle.

**Fix direction:** keep `last_saved_generation` on `OpenDoc`, compute `dirty = generation != last_saved_generation` from a monotonic counter, and ignore a completion whose captured generation is older than the current one.

---

### S3-03 — Medium, probable: the mip pipeline can leave a tile permanently blank and stop waking

The render thread's contract is "return `true` to be woken again immediately" (`render.rs`, `Ok(!plan.complete && (progressed || budget_spent))`). When a program cannot be built because mips are missing, the missing tiles are sent as `MipWork` and are **also** inserted into `mips_sent` (`render.rs:354`); that set is cleared **only** when the snapshot `Arc` identity changes (`render.rs:318`), i.e. it relies on `invalidate_snapshot()`/`changed()` churn to re-arm.

The engine side drops a batch silently in one case: `compute_mips` (`engine.rs:2213`) begins with (the check is at `2217`)

```rust
if doc.doc.revision != work.revision {
    // The document changed meanwhile; the next frame asks again.
    return;
}
```

— and does **not** call `invalidate_snapshot()` nor `request_frame()`. Here the comment is right (a revision change comes with `changed()`, which marks the snapshot stale and produces a new `Arc`), but the coupling is invisible and fragile: the recovery depends on a *different* code path having already requested a frame.

The permanent case is worse: `mips::ensure_mip` errors are only logged (`tracing::warn!("mip {request:?} failed: {error}")`), and a `TileError::Corrupt` (corrupt `.fxd` chunk below a missing mip, or a scratch file that vanished) never becomes available. Those keys stay in `mips_sent`, the tile is never retried for the same snapshot, `plan.complete` stays `false` and `progressed`/`budget_spent` stay `false`, so `frame()` returns `false` and the render thread goes to sleep with a blank tile on screen. The condition is invisible to the user (a hole in the image, no toast) and to the UI (no `Error` message).

**Fix direction:** on a failed request, drop the key from `mips_sent` and report one `EngineToUi::Error`/toast; make the revision-mismatch path explicitly re-request a frame.

---

### S3-04 — Medium, confirmed: the save path panics on a missing level-0 chunk (a trap, currently unreachable)

`fxd/manifest.rs:272`:

```rust
TileSlot::Data(handle) => match tile_ref(handle) {
    Some(chunk) => slots.push(SlotEntry::Tile { tx, ty, chunk }),
    None if level != 0 => {}
    None => panic!("level-0 tile {tx},{ty} was not written by the save"),
},
```

Skipping a missing chunk is deliberately allowed for derived levels (they are rebuilt after opening), and `save` skips a *derived* tile when `store.get` reports `Evicted` (`save.rs:112`). Whether a level-0 slot can be in that position depends on `collect_tiles`'s reasoning: it deduplicates handles globally by id and records `derived: level != 0` **from the first visit**, so if one handle were ever recorded from a level ≥ 3 slot and then found again at level 0, an eviction at save time would route it into the `continue` branch and the manifest builder would panic — on the `save-N` worker thread, i.e. the S1-03 wedge: no `Internal::Saved`, a "Saving…" progress bar that never completes, `pending_close` stuck, and a `.part` file left behind.

At this revision the situation is unreachable because `TileStore::insert` (`store.rs:564`) always allocates a fresh `TileId` (no content dedup), so one handle cannot appear at two levels; and `collect_tiles` visits level 0 before levels ≥ 3 within each image. It stays a trap: the invariant "a level-0 handle is always written" is enforced by an accident of construction plus a panic, in a worker thread, with no cleanup and no user-visible error.

**Fix direction:** return a `Result` from `image_entry`/`to_manifest` and surface a save error instead of panicking.

---

### S4-01 — Low, confirmed: silent no-ops contradict "no menu item stays mute"

`Engine::action` ends with a `not implemented yet` toast for anything unrecognised *unless* the id starts with a `UI_LOCAL_ACTION_PREFIXES` entry (`engine.rs:923`), in which case it is only a `tracing::debug!`. Two engine-owned paths return `true`/nothing without any feedback:

- `edit_action("clip:clear")` with no selection: does nothing and produces no toast (`Delete`/`Backspace` routes here through `tool_key`).
- `layer_action("layer:merge-visible")` with fewer than two visible root layers: returns `true` silently.
- `zoom:fill`/`zoom:print` are swallowed by the engine (they match the UI-local `zoom:` prefix and are absent from `view.action`), so the engine cannot complain; the UI's placeholder handlers then do something different from the label (S2-05).

**Fix direction:** toast on the no-op branches ("Nothing to clear", "Merge Visible needs two visible layers").

---

### S4-02 — Low, confirmed: blocking tile reads on the engine thread

The design rule "the render thread never blocks" is honoured, but the engine thread performs synchronous `TileStore::get` in several places, and `get` can hit the cold tier (file read + lz4) or the backed tier (file read + CRC + zstd):

- `clipboard::os_pixels` / `alpha_bounds` (`clipboard.rs:103`) — bounded by `OS_LIMIT = 8192` px, but still whole-image reads on the event loop.
- `tools/mod.rs:217` — `inside_selection` calls `selection.tile_coverage(ctx.store, …)`, which is `store.get` per tile, i.e. a disk read possible **on every pointer press** of a selection tool while a selection exists.
- `tools/eyedropper.rs:128`, `engine.rs:1780` (`thumbs`), and `Selection` helpers in `fx-core` follow the same pattern.

**Fix direction:** prefetch selection tiles through the existing loader machinery, or at least keep a hot copy of the selection's tiles while a selection tool is active.

---

### S4-03 — Low, confirmed: unbounded recursion over the layer tree

`Document::walk`, `Document::path_of` (`document.rs:193`), `Document::layer`/`layer_mut`, `Document::panel_order`, `keep_layers` (`export.rs`), `layers::flatten`, `command.rs::{collect_subtree, deep_copy, layer_entry, layer_from_entry}` (`manifest.rs`) are all recursive with no depth guard. The UI can nest groups arbitrarily (`GroupLayers` on a group's child creates a sub-group; nothing bounds the depth), so a pathological document (a script, a corrupt manifest, or a user with a lot of patience) overflows the stack. Secondary: `resolve_all` calls `path_of` once per selected id (`command.rs:744`), and each `path_of` walks the whole tree → O(n · depth) for a large multi-selection.

**Fix direction:** iterate with an explicit stack in `walk`/`path_of` (and the panel-order builder), and clamp nesting depth where a document is loaded.

---

### S4-04 — Low, confirmed: `revision` vs `generation` is easy to get wrong for consumers

`documents.rs:207` says it explicitly: "Undo can bring back an earlier revision number: never trust it alone." The render thread uses `render_generation()` (= `generation × 1 000 003 + preview_rev`, `documents.rs:194`), and the snapshot identity is `(revision, preview_rev)` plus `snapshot_stale`. But the engine still publishes raw revisions to the UI and uses them as cache keys elsewhere: `EngineToUi::Layers { revision }` (`engine.rs:1719`, `2205`), `DocumentInfo.revision`, and `Internal::Thumbnail { revision }` forwarded unchecked (`engine.rs:1334` — the engine never compares it against `doc.doc.revision`, and neither does the panels code, which keys only on `doc:layer` in `requested`). Undo therefore makes "revision" ambiguous for any consumer that treats it as a monotonic counter.

**Fix direction:** publish `generation` (monotonic) alongside or instead of `revision` in the messages whose consumers cache by it.

---

### S4-05 — Low, confirmed: `zoom:` is both a UI-local and an engine-owned prefix

`UI_LOCAL_ACTION_PREFIXES` (`fx-protocol/src/lib.rs:157`) lists `"zoom:"`, but `ViewState::action` implements `zoom:in`, `zoom:out`, `zoom:100`, `zoom:fit` and is consulted first (`engine.rs:916`). The list entry is therefore wrong for those four ids (it would suppress the "not implemented yet" toast if they were ever removed) and is what silently swallows `zoom:fill`/`zoom:print`. Two owners for one prefix is what allowed S2-05 to hide.

---

### S4-06 — Low, confirmed: the FPS readout is a fixed-window average

`stats.rs::summary` (`stats.rs:82`) reports `fps = frames.len() / 2 s`. This is a windowed average, not an instantaneous rate: a burst of 60 frames in one second (then idle) reports 30 fps, and the first frames after start-up report a fraction of the truth. p50/p99 are computed over the same ≤ ~120 samples, so a single hitch is quantised. Mostly cosmetic (the overlay is a debug aid), but the number is easy to misread as "the current frame rate" in `docs/PERFORMANCE.md`-style measurements.

---

## 4. Invariant audit

| Invariant | Established | Enforced | Violated by |
|---|---|---|---|
| One Rule (document-size memory) | `AGENTS.md`, `docs/ARCHITECTURE.md` | `export_document`, `convert`, the whole tile design | **S1-01** (two sites) |
| Render thread never blocks | `render.rs` uses `try_get_hot` | `TilePipeline::frame`, `load()` on rayon | holds; L-03-equivalent blocking lives on the *engine* thread instead (S4-02) |
| A live document's tiles are always readable | assumed by `expect("tile of a live document")` | **nothing** | **S1-03** (`Corrupt` from a bad chunk/scratch file) |
| `expect` only for true invariants | `AGENTS.md` | review | S1-03, S3-04 |
| Every action gives feedback | `actions.js` header comment, `NotYet` tool | partial | S4-01 |
| IDs unique, no cycles | type-level (`LayerId` from `allocate_layer_id`) | `path_of` assumes a tree | **S4-03** (depth, not cycles) |
| Selection is a history step, not content, not saved | D-028 (`command.rs`, `engine.rs:1667`) | `history_only`/`selection_only` effect flags | OK — but several `revision`-keyed consumers (S4-04) |
| History ⇔ mutations | `History::{execute,record}` | one step per command/stroke | **S2-01** (one step can cover two gestures) |
| `dirty` ⇔ "there are unsaved changes" | `after_edit`, `pixel_job_done` | close prompt, window close | **S3-01** (in-flight), **S3-02** (stale save) |
| Cached data corresponds to its source | `generation` + snapshot identity | render pipeline | **S3-03** (mips), S4-04 |
| Persisted state is reconstructible | `FILE_FORMAT.md`, CRC per chunk | container read path | **S1-02** (concurrent writers), **S3-04** (manifest panic) |
| GPU resources correspond to live tiles | atlas LRU + composite cache | `evict()` | verified safe (see §5) |
| Undo restores the exact previous state | snapshot history | every command | **S2-01/S2-02** (stroke `before` spans gestures) |

---

## 5. Adversarial execution analysis — what was tried, and the result

**Rapid actions.** *Hold `Ctrl+S`* → S1-02 (concurrent writers). *Hold a toggle key* (`toggle:grid`, `panel:toggle:layers`) → the state flips at the repeat rate; visually stable but the engine receives ~30 messages/s and `toggle:panels` on Tab collapses/expands rapidly. *Hold `Ctrl+Z`* → one undo per repeat (Photoshop-like, acceptable) but each `step_history` sends `Layers` + `History` + `DocumentChanged` plus a full thumbnail set (`engine.rs:1708`), so a held undo on a many-layer document queues a lot of UI work. *Double-click a tool that starts a gesture* (`OutlineDrag`, double-click to end a polygonal lasso) → traced, consistent. *Press `Ctrl+S` while a save is running* → S1-02.

**Invalid state.** *Corrupt/truncated `.fxd`* → opens clean (footer+manifest only), then the first `get` of the bad chunk returns `Corrupt`; every `expect("tile of a live document")` site panics → S1-03. *Missing scratch file for a cold tile* → `TileError::Corrupt("cold tile without scratch file")` (`store.rs:652`) → same panic path inside a pixel job. *Close a document while a job runs* → S3-01 (job silently dropped). *Empty document / zero layers* → `active_layer()` returns `None`, `stroke_event(Begin)` toasts "Select a layer to paint on", `composite_layers` with an empty selection is rejected by `Command` validation before it gets there. *Group nesting* → S4-03 (stack) and see §6 for the group/selection interactions that are handled correctly. *Extreme coordinates*: `OffsetSelection`/`Info::offset` go through `checked_offset`; `grow_to_canvas` (`pixels.rs:257`) widens to `i64` internally and clamps against the canvas, so the `i32` offsets used to express layer placement are validated on the `Command` side rather than in the op (worth a property test, but no concrete wrap was found in the paths read).

**Extreme input.** 0/1/10 layers → normal. 1 000 layers → every `after_edit` builds the full flat `Layers` list (`layer_list`) and the UI re-renders it; `resolve_all` is O(n·depth). 10 000–100 000 layers → quadratic paths above plus per-edit `History` snapshot clones (cheap per layer, but O(n) per command, including `selection_only` commands which clone the document and then discard it — `History::execute` clones `before` before checking `effect.selection_only`). Huge canvases (30 000²) → S1-01. Very deep trees → S4-03. Very wide trees → `panel_order`/`layer_list` are linear, fine.

**Async and concurrency.** Every `Internal` message was followed from producer to consumer: `PixelJobDone` (S1-03 when the producer dies), `Exported` (progress always closed, `Err` reported), `Copied` (both `Ok(None)` and `Err` reported), `Saved` (S3-02 ordering, `pending_close` handling is otherwise correct including `Cancelled`), `Thumbnail` (revision unchecked, S4-04), `PreviewTiles` (guarded by `request == preview.request` — correct), `PreviewFailed` (only reported if still current — correct), `Imported`/`OpenedFxd` (cancellation handled), `B3Built` (documented as a benchmark, history reset). `MipWork` → S3-03. `RenderRequest::{Frame, Wake, Stop}`: `load()` guards duplicate loads with a `loading` set and always wakes; a dropped `Stop` would leave the render thread idle but `Engine::run` sends it after the loop and the process exits anyway. Cancellation of a save/export exists only via `Progress` returning `false` (`IoError::Cancelled`), which the engine never does.

**State and history.** edit → undo → redo round-trips through `std::mem::swap` of whole documents; the `redo` stack is cleared on a new step; the 50-step limit drops from the front. The merge path (`MERGE_EDITS_WITHIN`, `engine.rs:1628`) undoes the previous step, applies the new value, and restores `before_merge` if the new value is refused — that path was traced and is correct, including `doc.history.labels().count()` bookkeeping. A stroke that fails mid-way restores `open.doc = before` (`engine.rs:698`). What does *not* round-trip is a gesture split by a tool change (S2-01).

---

## 6. Things that look wrong but are correct (checked, so they do not have to be re-checked)

- **`TileAtlas::evict`** (`atlas.rs`): the first sweep requires `last + 1 < self.frame` and the fallback requires `last < self.frame`; a slot touched this frame (via `lookup`, which stamps `entry.1 = self.frame`) can therefore never be evicted. The comment matches the code.
- **Prefix cache keys** (`compositor.rs:509`): `AtlasKey::Prefix` hashes `(level, tx, ty, split)` **plus** `hash_ops(&program.ops[..split])`. Editing the hot layer does not change the ops below the split (tile handles are part of the op hash), and editing anything below changes them, so the cache cannot serve a stale prefix.
- **`group_layers`** (`command.rs:465`): `resolve_all(doc, layers, /*drop_descendants=*/true)` removes descendants of a selected ancestor *before* any mutation, and `insert_at` is computed against the pre-removal sibling list while excluding the selected ids. Selecting a group and one of its children cannot panic and cannot double-count. The "pull a layer out of another group in panel order" behaviour is the documented, tested Photoshop semantic, not a bug.
- **`zoom:fit` / `zoom:100` do reach the engine**: `self.view_mut().action(id)` runs *before* the `UI_LOCAL_ACTION_PREFIXES` check (`engine.rs:916` vs `923`), so `Ctrl+0` and `Ctrl+1` work; only `zoom:fill`/`zoom:print` fall through (S2-05).
- **`composite_layers` renders only contributing tiles**: the `if background.is_some() || !program.is_empty()` filter is honoured, so a Merge of one small layer does not render the full canvas *grid* — the memory blow-up comes from collecting the rendered buffers, not from rendering empty tiles.
- **The live filter preview is correctly bounded** (`filters.rs:57`): only the visible tiles at the view level are computed, in batches of 8, with a latest-request-wins check between batches. This is the shape S1-01 should copy.
- **`TileStore::insert` allocates a fresh id** (`store.rs:564`) with no content dedup, so a handle cannot be shared between levels, which is why S3-04 is currently unreachable.
- **`History` limit and redo semantics** are correct, including `redo.clear()` on a new step and restoring `before_merge` on a refused merged edit.
- **`fxd` crash safety**: `FxdWriter::commit` syncs data, writes the footer, syncs again, and truncates the torn tail; a crash mid-save leaves the previous footer valid. Correct as documented — the corruption risk is specifically *two concurrent* writers (S1-02).
- **`mips::ensure_mip` retry loop**: `MAX_RETRIES` with recomputation of only what was lost is correct; the failure signal (evicted child) is the only one it retries, which is right.
- **`unsafe` sites**: `fx-app/src/window/clipboard.rs`, `fx-app/src/window/win.rs`, `fx-tiles/src/store.rs:104,126` are the only ones, as `AGENTS.md` claims.

---

## 7. Not reviewed (honest coverage statement)

Read in depth: `fx-engine` (all of `engine.rs`, `render.rs`, `export.rs`, `ops.rs`, `documents.rs`, `view.rs`, `stroke.rs`, `mips.rs`, `thumbs.rs`, `selection.rs`, `stats.rs`, `tools/*`), `fx-core` (`command.rs` incl. tests, `history.rs`, `document.rs`, `pixels.rs`, `selection.rs`, `layer.rs`), `fx-tiles/store.rs`, `fx-io/fxd/*`, `fx-render/{program,frame,viewport,reference,overlay,gpu/*}.rs`, `fx-app/input.rs`, `ui/js/{main,state,canvas,actions,shortcuts,popup,dialogs}.js`, `ui/js/native/*`.

Not read line by line (so findings there are *not* claimed): `fx-ops/{brush/*, flood.rs, morph.rs, raster.rs, filter.rs, gaussian.rs, neighbourhood.rs}` numeric kernels; the WGSL shaders; `fx-io/{tiff*.rs, png.rs, jpeg.rs, band.rs, export.rs}`; `fx-color`; `fx-app/{app.rs, window/*, bridge.rs, gpu.rs, event.rs, preferences.rs}`; `ui/js/{menu,panels,optionsbar,tooltip,el,icons}.js` and `ui/js/data/*`; `fx-cli`; `tools/xtask`. The `.fxd` codec and the GPU compositor were read at the level needed to confirm the findings above, not exhaustively for numerical parity.

No build or test run was performed (the review is read-only and the findings are static); `cargo test`/`clippy` would be the natural confirmation step for S1-01/S1-03 once a fix is attempted.

---

## 8. Prioritised fix list

| # | Finding | Sev | Effort | Notes |
|---|---|---|---|---|
| 1 | Ignore key auto-repeat in `shortcuts.js` | S1 | 1 line | Smallest change that removes the practical trigger of S1-02 |
| 2 | Guard `start_save` with an in-flight flag per document | S1 | small | Also fixes S3-02 ordering; needs an `OpenDoc` field |
| 3 | Make `FxdWriter` append exclusive (or reserve `pos` under a lock) | S1 | small | Defence in depth; the container is used by the CLI too |
| 4 | Propagate errors instead of `expect("tile of a live document")`; guarantee `PixelJobDone` via a drop guard | S1 | medium | Touches `export.rs`, `ops.rs`, `stroke.rs`, `engine.rs` |
| 5 | Stream `composite_layers` / `EngineOps::filter` tile by tile | S1 | medium | Model on `EngineOps::convert` |
| 6 | Reset the previous tool (and end the stroke) when `view.tool` changes | S2 | small | Add `Tool::cancel` |
| 7 | Key tool state by document (or reset on `ActiveDocument`) | S2 | medium | Kills the clone-source cross-document bug |
| 8 | Add `doc` to `EngineInput::Export`; wire real `zoom:fill` / `zoom:print` | S2 | small | Two independent contract bugs |
| 9 | `dirty` on job start + refuse close while `busy` | S3 | small | Protects in-flight edits |
| 10 | Re-arm `mips_sent` on failure and report mip errors | S3 | small | Removes the silent blank-tile state |
| 11 | Return `Result` from `manifest::to_manifest` instead of panicking | S3 | small | Removes the last panic in the save thread |
| 12 | Toast the no-op action branches; block-free selection hit-testing | S4 | small | Contract and latency |

---

*Reviewed revisions:* `m6` @ `b0bd9ef`. *Artifacts produced:* this document only.
