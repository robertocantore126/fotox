# Deep Code Review — Exhaustive Bug Hunt

**Repository:** `fotox` (Rust workspace + CEF/JavaScript UI)
**Revision reviewed:** `bughunt/deepseek`, `e8ebc02`
**Date:** 2026-09-26
**Scope:** repository-wide static review of current implementation and feature lifecycles. Read-only: no source code was changed; this report is the only artifact added.
**Method:** repository model → feature coverage → cross-layer call tracing → invariants → adversarial ordering and invalid-state analysis → focused tests/checks. Findings are based on current code, not on task-card claims.

## 1. Executive Summary

The highest-risk defects are in ordinary save/edit workflows: repeated saves can interleave writes to one native file, closing during a background pixel operation silently discards that operation, and large filters/merge operations retain output for the entire affected image. A corrupt lazy tile can still panic a worker before it posts completion, leaving the document busy indefinitely.

Additional confirmed user-visible defects affect option-bar values, eyedropper sampling, tool changes during a stroke, cross-document tool state, export dialog ownership, and reduced-resolution Generative Fill alignment. The standard Rust suite passes, but an ignored eyedropper test fails when explicitly enabled; the existing UI data checker does not detect these behavioral mismatches.

## 2. Severity and Confidence

| Severity | Meaning |
|---|---|
| **S1 — Critical** | Data loss/corruption, a severe resource failure on intended workloads, or an effectively unrecoverable editing state. |
| **S2 — High** | A reachable operation produces wrong data or loses expected operation/history semantics. |
| **S3 — Medium** | Significant responsiveness/resource cliff or a failure restricted to a narrower input path. |
| **S4 — Low** | A visible option is inert or an isolated robustness defect. |

*Confirmed* means the implementation chain establishes the failure. *Probable* means the failure mechanism is established but depends on timing or an external setup that was not executed end-to-end. No production code was instrumented or modified for this review.

## 3. Repository Model

### 3.1 Architecture and boundaries

| Subsystem | Responsibility and principal boundary |
|---|---|
| `fx-app` | winit shell, native dialogs, OS clipboard, window/GPU setup, CEF host. Routes viewport input directly to the engine and UI messages through the bridge. |
| `ui/js` | Menus, dialogs, panels, options, shortcuts and document presentation. `native/*` modules bridge UI state to `UiToEngine`; `mock-engine.js` supports browser-only mode. |
| `fx-protocol` | JSON control messages and binary image frames between UI, shell and engine. |
| `fx-engine` | Owns documents, command/history orchestration, view/tool state, import/export, selection, model/ComfyUI integration and render requests. |
| `fx-core` | Document/layer/path/selection models, commands, history, geometry, and serialization-facing data types. |
| `fx-tiles` | Immutable tile handles, hot/warm/cold/backed residency, scratch storage, mip images and memory trimming. |
| `fx-ops` | Pixel algorithms: brush, filters, flood/selection, resampling and warp operations. |
| `fx-render` | Tile program construction, CPU reference composition, GPU atlas/compositor, viewport and overlays. |
| `fx-io` | Lazy `.fxd` container/manifest, PNG/JPEG/TIFF and ABR read/write. |
| `fx-ai` | ONNX runtime/model download and local ComfyUI HTTP client. |
| `fx-cli` / `tools/xtask` | Headless operations, benchmarks and application launch/build orchestration. |

The app has an engine thread, render thread, rayon worker pool, tile-trim thread, and short-lived job threads. The engine serializes input and owns authoritative `Document`/`History` state. Pixel jobs work on a document snapshot and return `PixelJobDone`; saves/imports/exports return internal completion messages. The render thread consumes document snapshots and must avoid blocking tile loads. `.fxd` persistence is append-only with lazy backed tiles and per-chunk checksums. UI state is partly authoritative for control presentation and partly mirrored into engine `Settings`; options are sent as complete per-tool objects.

### 3.2 Important state and lifecycle

| State | Creation / mutation | Consumer and invalidation / cleanup |
|---|---|---|
| `OpenDoc.doc` | Open/import/new; commands, strokes, pixel-job completion, undo/redo. | Snapshot/render, export and save. Closing drops document handles; in-flight jobs can outlive it. |
| `OpenDoc.history` | History commands, committed strokes and completed pixel jobs. | Undo/redo and history UI; limited history drops old snapshots. |
| `OpenDoc.generation` / snapshot | `changed()` and snapshot refresh. | Render cache generation and async consumers; stale results need identity/version checks. |
| `OpenDoc.dirty` / `busy` | Content edits/save completion; background job start/completion. | Close/save guards. `busy` is not currently part of close eligibility; dirty is delayed until pixel-job completion. |
| `Engine.tools` and option settings | Engine construction, tool options and pointer events. | Tool pointer/overlay paths across whichever document is active; tool objects are not per-document. |
| `TileStore` tiers | Tile creation/load, mip/filter work, trim. | Renderer, commands and workers. Backed/cold reads can fail after open because payloads are lazy. |
| Render caches | Render-thread snapshot/generation/frame processing. | Viewport output; cache lifecycle is mostly generation/key-based. |
| AI task/cancel/embedding | `engine/m13.rs` actions and object selection. | Worker completion updates selection/layers; one process-wide AI task slot and a cached embedding keyed by document generation. |
| UI option values | `optionsbar.js` controls and per-tool `memory`. | Engine `Settings` via `ToolOptions`; popup-based selects do not emit the event that invokes the bridge. |

### 3.3 Entry points and feature coverage

| Feature/lifecycle | Current path | Coverage/status |
|---|---|---|
| Startup/render | `fx-app` startup → CEF/winit → render thread → snapshot/tile requests | Traced; GPU and viewport unit tests pass. |
| UI actions/shortcuts | key/menu/control → `actions.js`/bridge → protocol → engine dispatcher | Traced for findings; data consistency check passes but does not verify value semantics. |
| Pointer tools/strokes | shell routing → `PointerInput` → engine tool registry → live stroke → `Command::Stroke` on completion | Traced; tool switch and cross-document state defects remain. |
| Commands/history | UI or tool → `Command` → sync apply or pixel job → history/effect notifications | Traced; normal command atomicity was hardened, but closing during a pending pixel job is not guarded. |
| Selection/channels/paths/warps | tool/action → command/job → document data and selection overlay → `.fxd` manifest | Broadly covered by current tests and M9–M11 modules; external behavior/model quality not end-to-end verified. |
| Smart Objects/Filters | commands → source composite/mapping/filter stack → derived tile requests/cache → save manifest | Source and render code inspected; documented unfinished filter semantics exist, but were not counted as accidental bugs here. |
| Native open/save | native dialog or drop → import/open worker → lazy tile-backed document → save worker → append/fresh `.fxd` | Traced; overlapping saves, pending-job close, and worker panic cases remain. |
| Export | UI action → native dialog → `EngineInput::Export` → active-document snapshot → format writer | Traced; document identity is lost across the dialog. |
| Generative Fill/Expand | selection/crop → bounded model input → ComfyUI HTTP job → variation layers + selection mask → history/save | Traced; downsampled Fill input can be spatially misregistered. No real ComfyUI service run. |
| Model management/selection | Preferences/action → model download/runtime → bounded composite → model worker → selection/mask command | Traced; preprocessing happens synchronously on engine thread; real model test remains ignored. |
| Preferences/resources/CLI | settings files, model/brush/pattern paths, command-line and xtask entry points | Module inventory inspected; no full external install/runtime validation. |

## 4. Findings

### S1-01 — Confirmed: concurrent Save actions corrupt an incrementally saved `.fxd`

**Locations:** `ui/js/shortcuts.js:148` (keydown dispatch), `crates/fx-engine/src/engine.rs:2414` (`start_save`), `crates/fx-io/src/fxd/container.rs:467,502,526`.

The shortcut handler does not filter `KeyboardEvent.repeat`, so holding Ctrl+S dispatches repeated `doc:save` actions. `start_save` takes the same current `FxdFile` handle and starts a new thread for every request; it has no per-document save guard. Each `FxdWriter::append_to` initializes its own `pos` from the same footer end offset. `write_chunk` performs positional writes without a file-wide lock or offset reservation, and `commit` writes its own footer then truncates the file to its own `end_offset`.

Two saves can therefore overwrite chunks/manifest bytes at the same offsets and truncate the other writer's tail. The resulting footer can point at bytes with another writer's contents or the latest save can be truncated. This is reachable on ordinary held-key repeat and on repeated Save/Save As actions, not just a synthetic API call.

**Impact:** silent corruption of the user's native document. **Fix direction:** serialize writes per file/document and reject/coalesce in-flight saves; UI repeat suppression is useful but not a storage invariant.

### S1-02 — Confirmed: closing a clean document while a pixel job runs silently discards the edit

**Locations:** `crates/fx-engine/src/engine.rs:1995` (`start_pixel_job`), `2050` (`pixel_job_done`), `2305` (`close`), `2344` (`close_requested`).

`start_pixel_job` sets `open.busy`, but leaves `dirty` unchanged until `pixel_job_done` successfully installs the result. Both `close` and `close_requested` decide whether to prompt from `dirty` and do not reject `busy` documents. Repro: open a clean/imported document, start a filter/flatten/merge pixel job, then close the tab or window before completion. The clean document is closed/allowed to exit; the worker's later completion either finds no document and returns or is lost during process shutdown.

The command operates on a snapshot, so it cannot be recovered from the closed document. This is distinct from the live-stroke case fixed by HARDEN: the in-flight operation is a worker job, not a live tool gesture.

**Impact:** a user-requested edit is lost without a save prompt. **Fix direction:** treat pending content work as unsaved for close, or wait/cancel explicitly before destroying the document/window.

### S1-03 — Confirmed: filters and composite operations retain a whole image of output tiles

**Locations:** `crates/fx-engine/src/ops.rs:53` (`EngineOps::filter`), `crates/fx-engine/src/export.rs:243` (`composite_layers`).

Both paths collect every rendered output `TileBuffer` into a `Vec` before inserting the buffers into the destination `TiledImage`. For an opaque or fully populated 30,000 × 30,000 RGBA16 image, the 118 × 118 grid contains 13,924 tiles; one 256² RGBA16 tile is 512 KiB, so the vector alone is about 6.8 GiB. `filter` also retains the source image and destination slots. Merge/Flatten/Stamp Visible reach `composite_layers`; destructive filters reach `EngineOps::filter`.

The same crate uses bounded tile rows/batches for export and bounded batches for resampling, so this memory cliff is an avoidable outlier and violates the project's document-size memory invariant.

**Impact:** multi-gigabyte transient allocation, paging or OOM on intended document sizes. **Fix direction:** bound in-flight work and insert/commit output tiles as batches or rows complete.

### S1-04 — Confirmed: a worker panic skips completion and leaves the document permanently busy

**Locations:** `crates/fx-engine/src/engine.rs:1995–2031`, `crates/fx-engine/src/ops.rs:390`, `crates/fx-engine/src/export.rs:130,177,239`, `crates/fx-engine/src/stroke.rs:142`.

`start_pixel_job` sets `busy` and sends `PixelJobDone` only after `command.apply` returns. Multiple render/fetch closures use `store.get(handle).expect("tile of a live document")`. `.fxd` tile payloads are intentionally read lazily; a checksum mismatch is returned as a tile error when the chunk is first fetched, not necessarily during open. If that error reaches one of these `expect`s inside a rayon computation, the worker thread unwinds before sending `PixelJobDone`; no cleanup path clears `busy`.

Subsequent commands/history/strokes are rejected as busy. The same completion-after-work shape exists in `engine/m13.rs:213–255` for AI work, so an unexpected model/job panic can also leave its AI task state uncleared.

**Impact:** corrupted/missing lazy tile or another panic causes an in-session editing lockout and stuck progress. **Fix direction:** propagate tile errors and guarantee completion/cleanup with a panic-safe task guard.

### S2-01 — Confirmed: changing tools during a paint gesture can leave painting active

**Locations:** `crates/fx-engine/src/engine.rs:1418–1440`, `crates/fx-engine/src/view.rs:92–95`, `crates/fx-engine/src/tools/paint.rs` (`Paint::pointer`).

The tool action deactivates the old tool, but the paint tool inherits the default no-op `deactivate`; the path does not end the engine's live stroke. Tool selection then changes immediately. Pointer-up is routed using the newly active tool, while `Paint` clears `stroking` only on its own pointer-up path. Returning to the paint tool can therefore treat hover moves as stroke samples; the still-open engine `StrokeSession` can combine unintended samples into the prior history step.

**Impact:** unrequested pixels and undo steps that no longer match user gestures. **Fix direction:** cancel/end the active gesture and engine stroke on tool/document transitions; never interpret unbuttoned hover as an active stroke.

### S2-02 — Confirmed: paint-tool state is shared between documents

**Locations:** `crates/fx-engine/src/engine.rs` (`Engine::tools` and active-document change path), `crates/fx-engine/src/tools/mod.rs` (`Tools` registry), `crates/fx-engine/src/tools/paint.rs` (`source`, `aligned`, `hover`, `stroking`).

One engine-wide tool registry owns paint/clone/heal state and `ToolContext` has no document identity. The clone/heal source and aligned offset are document coordinates; switching to another document preserves these values. Clone/heal can then sample the wrong location in the newly active document. Hover/cursor and any unfinished-stroke fields can also cross the document boundary.

**Impact:** wrong clone/heal pixels and stale overlays; unfinished input can cross tabs. **Fix direction:** reset per-document transient tool state on active-document change or key tool instances/state by `DocId`.

### S2-03 — Probable: Export As writes the document active when the native dialog returns

**Locations:** `crates/fx-engine/src/lib.rs:143` (`EngineInput::Export`), `crates/fx-app/src/app.rs:272–300,438`, `crates/fx-engine/src/engine.rs:1520`.

The UI action opens a native asynchronous export dialog. The shell's `ExportTo` event carries only path and export options, and `EngineInput::Export` also has no document id. `Engine::export` snapshots `docs.active_mut()` only after the dialog returns. Switching tabs while the dialog is open therefore changes which document is exported to the chosen path.

**Impact:** a valid export path can receive a different open document than the one the user invoked Export As from. The static call chain proves the ownership loss; the native-dialog/tab-switch sequence was not run interactively.

**Fix direction:** carry the originating `DocId` through the dialog event and reject if that document has closed.

### S2-04 — Confirmed: Generative Fill's mip read is resized as if its rounded bounds were the requested rectangle

**Locations:** `crates/fx-engine/src/ai.rs:381–404`, called from `crates/fx-engine/src/engine/m13.rs:469–516`.

For a large Fill region, `generative_input` selects a mip level, rounds the requested rectangle outward to mip-cell boundaries in `lr`, reads that larger rectangle, then resizes all of it to dimensions calculated from the original unrounded `rect`. It does not crop the mip pixels back to the requested bounds or use `lr`'s document-space origin when mapping output pixels. Whenever the region is downsampled and its origin/extent is not aligned to `2^level`, the conditioning image is shifted/scaled relative to the `rect` later used to place the generated layer and selection mask.

This is reachable for large documents/selections; Generative Expand uses origin `(0,0)` and does not hit the same non-aligned-origin case.

**Impact:** generated Fill content is based on a slightly different part/scale of the canvas than the area where the result is installed, producing seams or context mismatch at selection edges. **Fix direction:** sample/crop the mip rectangle in document coordinates before resizing, and use one shared transform for input and output placement.

### S2-05 — Confirmed: option-bar dropdown picks update the UI but do not send the value to the engine

**Locations:** `ui/js/optionsbar.js:34–47,234–249`; popup rows are created outside the option-bar container by `ui/js/popup.js`.

`onOptionsChange` listens for events on the bar container. The dropdown's `onPick` changes the displayed value, calls `changed()` and emits a mock toast. `changed` is the local `sync()` function, which only updates dependent-field enablement; it does not call the registered engine change handler. The selection happens in the separate popup layer, and no bubbling `change` is dispatched. The displayed mode/value and the value used by the engine can consequently diverge until some later unrelated bar event happens to resend the complete options object.

**Impact:** every generic select-backed option can look selected while brush modes, sampling settings, gradient options and other tool behavior continue using an older value. Toggle controls explicitly dispatch a bubbling change; select controls do not.

**Fix direction:** notify the option bar's change handler after `onPick`, with a regression test asserting the emitted `ToolOptions` value.

### S2-06 — Confirmed: Eyedropper 11×11 and 51×51 choices silently point-sample

**Locations:** `ui/js/data/options.js:110`; `crates/fx-engine/src/tools/eyedropper.rs:32–39,268–297`.

The bar offers 1, 3, 5, 11 and 51 pixel sample sizes. `Options::from` recognizes only 3 and 5; both larger values fall through to area 1. The ignored regression test explicitly checks all offered values and fails at `11 by 11 Average` when run:

```text
left: 1
right: 11
```

**Impact:** visibly wrong foreground/background colors for two offered settings, and the normal `cargo test` run hides the known failure because it is ignored.

### S3-01 — Confirmed: AI image preparation can block the engine event loop on a large cold document

**Locations:** `crates/fx-engine/src/engine/m13.rs:310–326,357–378`; `crates/fx-engine/src/ai.rs:85–150`; `crates/fx-engine/src/mips.rs:31–120`.

Select Subject and the first Object Selection request call `working_composite` before spawning the model worker. It builds the composite on the engine thread; missing mip/vector/effect requests are served inline, and `ensure_mip` recursively computes missing lower levels and synchronously waits for its tile work. The output buffer is bounded to roughly 1024², but a cold 30k² image may require reading/computing a substantial portion of its source to create those mips. While that happens, the engine cannot process input, save/close requests, or update engine-side status.

**Impact:** long UI/command stalls on the target large-document workload, especially immediately after open or before the render pipeline has warmed the required mips. **Fix direction:** schedule composite/mip preparation as cancellable worker work and return the bounded model input to the engine asynchronously.

### S4-01 — Confirmed: “Show Sampling Ring” is an inert eyedropper option

**Locations:** `ui/js/data/options.js:110`; `crates/fx-engine/src/tools/eyedropper.rs:20–80`; `crates/fx-engine/src/tools/mod.rs:795–799`.

The toggle is stored in the settings test but production `Options` reads only sample area and current-layer mode. The eyedropper has no overlay implementation, so enabling the option cannot draw a sampling ring.

**Impact:** a user-facing toggle has no effect. Either implement the ring or remove/disable the option until supported.

## 5. Invariant Audit

| Invariant | Current enforcement | Result |
|---|---|---|
| Concurrent saves cannot mutate one file simultaneously | None in `start_save` or `FxdWriter` | Violated by S1-01. |
| A pending content edit prevents silent close | `busy` guards edit/history paths, not `close`/window close | Violated by S1-02. |
| Operation memory is bounded by changed/screen area | Export is row-based; filter/merge collect whole result sets | Violated by S1-03. |
| Worker completion always releases task/document state | Completion send follows fallible work; panic bypasses it | Violated by S1-04 and the AI completion path. |
| A gesture ends when its tool/document changes | Tool deactivation defaults to no-op for paint; live stroke is engine-owned | Violated by S2-01/S2-02. |
| Async result is applied to the request's document | Save inputs carry DocId; Export does not | Violated by S2-03. |
| UI option equals engine option | Select updates UI state but does not notify bridge | Violated by S2-05; B-03 separately has parser mismatch. |
| Model input and generated output use the same canvas transform | Rounded mip read resized without crop/origin correction | Violated by S2-04. |
| Lazy tile corruption is a recoverable error | Some paths propagate errors; several still `expect` | Violated by S1-04. |

## 6. Adversarial Execution Notes

- **Rapid actions:** holding Ctrl+S can start overlapping writers; tool switch during a held stroke loses the paint tool's matching release; switching documents preserves paint-tool fields. Repeated filter/merge is normally blocked by `busy`, but close is not.
- **Invalid state:** corrupt `.fxd` payloads are detected only when a backed tile is read; several pixel consumers turn that error into a panic. Model tensor shapes are trusted after ONNX output and no real model fixture ran in this environment.
- **Extreme input:** 30k² RGBA16 filter/merge paths can allocate about 6.8 GiB of tile outputs. AI input memory is capped, but building cold mips still does work proportional to the source area on the engine thread. Very large generated variations are accepted into pixel-layer buffers without a global memory budget beyond ComfyUI response size.
- **Async ordering:** save completion messages have no “latest save” identity/serialization; export dialog results do not retain origin DocId; pixel/AI completion messages are not guaranteed after panic; closing drops documents while their worker snapshots can still be running.
- **History/persistence:** the recent hardening makes normal command application transactional and commits live strokes before save/close. That does not protect pending worker operations or two independent writers. Generative variation layers are ordinary history commands, but their output is already spatially misregistered when the model input came from a non-aligned reduced mip.

## 7. Verification Performed

- `cargo test`: passed for default workspace members. The `fx-engine` test suite reported 116 passed and 1 ignored; the AI real-model flow is also ignored without the model/runtime.
- `cargo test -p fx-engine --lib eyedropper -- --ignored`: failed as expected in `the_samples_sizes_the_option_bar_offers_are_the_ones_the_reader_knows`, at `11 by 11 Average` (`left: 1`, `right: 11`).
- `node ui/tools/check-data.mjs`: passed, reporting 482 menu leaf commands, 357 handled and 125 on fallback. This checks existence/registration, not semantic propagation of selected values.
- No native interactive dialog/tab-switch repro, ComfyUI run, real ONNX inference, GPU DirectML run, or 30k² memory stress test was performed.

## 8. Coverage Limits and Disposition of Older Leads

The source inventory covered all workspace crates, engine entry points, current UI modules, and current milestone module split. The deepest path tracing focused on engine job/save/export/stroke lifecycle, `.fxd` writes, option forwarding, M13 AI/generative processing, and the M12 smart-object/filter boundary. Numeric kernels, every WGSL branch, all platform-specific installer/runtime combinations, and every UI action were not individually proven correct; the report does not claim they are bug-free.

The 2026-09-25 report at `docs/reviews/2026-09-25-deep-code-review.md` targets `b0bd9ef` and must not be read as a current finding list. Current review rechecked its highest-impact leads: whole-output filter/merge allocations, concurrent save writers, export document identity, and tile-read worker panics remain; the live-stroke close case has been addressed by `commit_live_edits`, but close during a background pixel job remains unfixed. The current `docs/reports/BUGHUNT-2026-09-26.md` records the dropdown and eyedropper findings; this report independently rechecked them against current production code.

---

*Reviewed revision:* `bughunt/deepseek` @ `e8ebc02`. *Artifacts produced:* this review document only.

## 9. Fix Follow-Up — 2026-09-26

The findings above describe the reviewed revision, not the current working tree. Fixes have since been applied:

| Finding | Follow-up |
|---|---|
| S1-01 | Per-path writer leases serialize `.fxd` writes; incremental writers reread the latest valid footer under the lease. The stale-handle regression passes. |
| S1-02 | Tab close refuses while a job/save is active; window close waits and resumes after completion. A filter-job close regression passes. |
| S1-03 | Filter and merge/flatten output is committed in batches of at most 16 tiles. Composite output still matches the viewport test. |
| S1-04 | Pixel and AI workers catch unwinds and send failed completion messages, releasing busy/progress state. |
| S2-01 / S2-02 | Tool changes finish the live stroke and deactivate painting; document transitions commit live edits and clear per-document transient tool instances. A mid-stroke tool-switch regression passes. |
| S2-03 | Export requests carry the initiating `DocId` through the native dialog. The first document remains the export source after switching tabs in the regression test. |
| S2-04 | Generative Fill resampling maps output sample centers through the requested canvas rectangle and mip origin. The non-aligned mip regression passes. |
| S2-05 / S2-06 | Dropdown picks emit a bubbling change; Eyedropper supports 11×11 and 51×51. Its formerly ignored option-contract test now passes. |
| S3-01 | Select Subject/Object Selection composite and mip preparation now runs on the AI worker rather than the engine event loop. |
| S4-01 | The unimplemented “Show Sampling Ring” control was removed. |

Verification after fixes: `cargo test` passed for default workspace members (the real-model ONNX flow remains ignored without its runtime/models), `cargo check --workspace --all-targets`, `cargo build -p fx-app`, `cargo fmt --check` for touched crates, JavaScript syntax checks and `node ui/tools/check-data.mjs` all passed. A real ComfyUI/ONNX run and a 30k² memory stress test were not performed.