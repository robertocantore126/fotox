# Review fixes 2026-09-25 — deep review S1-01…S4-06 and the second review BUG-01…BUG-08

Agent: Buffy (DeepSeek)  ·  Branch: `review-fixes`  ·  Status: ready for review

## Plan (written before coding)

1. Add tests for the part 1 fixes (commit `ed20502`): the mask pixel lock, `zoom:fill`,
   `try_render_tile`, `slot_for`, the paint tool's gesture handling, and three engine
   integration cases (second save, tool switch mid-stroke, `zoom:fill`).
2. S3-04: make `manifest::to_manifest` / `image_entry` return `Result<_, IoError>` so a
   missing level-0 chunk fails the save instead of panicking on the save thread.
3. S1-02 defence in depth: a shared `Arc<AtomicBool>` on `FxdFile` so two
   `FxdWriter::append_to` cannot interleave.
4. S3-03: report a mip error other than `Evicted` once per document.
5. This report.

One commit per task, checks before each commit.

## What was done

| Commit | What |
| --- | --- |
| `13253ad` | Tests for the part 1 fixes (task 1). |
| `18f3abe` | S3-04: the manifest builder returns `Result`, `save` propagates. |
| `3806a00` | S1-02 defence in depth: one `FxdWriter::append_to` per file. |
| `f26856b` | S3-03: one `EngineToUi::Error` per document for an unreadable mip. |

## Findings

### Deep code review (`docs/reviews/2026-09-25-deep-code-review.md`)

| Finding | Verdict | Where / why |
| --- | --- | --- |
| **S1-01** — `composite_layers` / `EngineOps::filter` materialise the whole document | **Fixed** | `ed20502`: each finished tile is inserted into the store inside the parallel map (only slots are collected), as `EngineOps::convert` already did. |
| **S1-02** — a held `Ctrl+S` runs several writers on one `.fxd` | **Fixed** | `ed20502`: `ui/js/shortcuts.js` ignores `e.repeat`; `OpenDoc.saving` + `start_save` refuse a second save; and `3806a00` adds the container-level guard (see below). Tests: `edit_flow::a_second_save_while_one_runs_is_refused_or_queued_behind_it`, `fxd::tests::two_appends_to_one_file_do_not_interleave`. |
| **S1-03** — a panic on a worker thread wedges the document | **Fixed** | `ed20502`: `guarded()` wraps every worker body and reports a panic as an error; `try_render_tile` propagates a failed fetch; the `expect("tile of a live document")` sites are gone. Test: `program_tests::try_render_tile_propagates_a_fetch_error_and_matches_render_tile`. |
| **S2-01** — changing tool mid-stroke leaves the paint tool stroking | **Fixed** | `ed20502`: `Tool::cancel` / `Tools::cancel`; `leave_tool()` ends the live stroke and cancels the old tool; a `Move` without the left button ends a stroke. Tests: `tools::paint::tests::*`, `edit_flow::switching_tool_mid_stroke_records_one_step_and_a_hover_records_none`. |
| **S2-02** — tool state is engine-global, not per document | **Fixed** | `ed20502`: `Tools::document_changed()` runs on `ActivateDocument`, ends the stroke and resets each tool (clone source, aligned offset, hover). The registry is still one tool per id, reset on document change — one of the two fixes the review suggested. Test: `tools::paint::tests::document_changed_forgets_the_clone_source`. |
| **S2-04** — `Export` writes whichever document is active when the dialog resolves | **Fixed** | `ed20502`: `Engine::export_doc` remembers the document the export dialog was opened for. |
| **S2-05** — "Fill Screen" is a hardcoded 200 %, "Print Size" 72 % | **Fixed** | `ed20502`: the engine owns `zoom:fill` (`ViewState::action`) and `zoom:print` (`SCREEN_PPI / doc.ppi`); the UI forwards both. Test: `view::tests::zoom_fill_covers_the_viewport_and_fit_is_unchanged`. |
| **S3-01** — the dirty flag lags an in-flight edit | **Fixed** | `ed20502`: `close`, `close_requested` and `start_save` refuse while `OpenDoc::blocked()` (a pixel job or a save is running), so an in-flight edit cannot be closed away or saved as its pre-job state. |
| **S3-02** — a stale `Internal::Saved` re-marks a saved document dirty | **Not reachable any more** | The S1-02 guard allows only one save per document at a time, so the two completion orders the finding describes cannot interleave. The generation comparison stays, but a stale completion cannot exist. No further change. |
| **S3-03** — a mip error leaves a silent blank tile and the render thread sleeps | **Fixed (message)** | `f26856b`: an error other than `TileError::Evicted` sends one `EngineToUi::Error` per document ("A part of the image could not be read"), tracked in a `HashSet<DocId>` cleared when the document closes. `Evicted` keeps today's behaviour (log only; the next frame retries). The render-side `mips_sent` re-arm was **not** changed — the task asked only for the message; the tile still re-arms when the snapshot identity changes. |
| **S3-04** — the save path panics on a missing level-0 chunk | **Fixed** | `18f3abe`: `manifest::to_manifest` / `image_entry` return `Result<_, IoError>` (`IoError::Decode` with the old message); `save` propagates it, so a save fails with an error. Test: `manifest::tests::a_level_0_tile_without_a_chunk_is_an_error`. |
| **S4-01** — silent no-ops contradict "no menu item stays mute" | **Fixed** | `ed20502`: toasts for a no-op `clip:clear` ("Nothing is selected") and `layer:merge-visible`; `zoom:fill` / `zoom:print` now have an owner (S2-05). |
| **S4-02** — blocking tile reads on the engine thread | **Left open** | Selection hit-testing and the eyedropper still call `TileStore::get` on the engine thread. Bounded, unchanged, not part of this work. |
| **S4-03** — unbounded recursion over the layer tree | **Left open** | No depth guard or iterative `walk`/`path_of` was added. |
| **S4-04** — `revision` vs `generation` for consumers | **Left open** | Messages still publish raw `revision`; no monotonic `generation` was added. |
| **S4-05** — `zoom:` is both a UI-local and an engine-owned prefix | **Left open** | `UI_LOCAL_ACTION_PREFIXES` still lists `"zoom:"`. The practical hole (S2-05) is closed because `view.action` is consulted first, but the list entry is unchanged. |
| **S4-06** — the FPS readout is a fixed-window average | **Left open** | Cosmetic debug aid; unchanged. |

### Second review (BUG-01…BUG-08)

| Finding | Verdict |
| --- | --- |
| **BUG-01** — atlas vs ready cache | **Not a bug**: ready tiles are re-composited each frame as cache hits, which re-stamp their composite slot; draws read the composite array, not source atlas slots. |
| **BUG-02** — rename over an open file | **Not a bug**: Save As onto the document's own file is an incremental save; compaction is never triggered. |
| **BUG-03** — `DeleteMask { apply: true }` ignores the lock | **Fixed** (`ed20502`); test added (`13253ad`): `command::tests::delete_mask_apply_refuses_a_pixel_locked_layer_and_leaves_the_mask`. |
| **BUG-04** — `mips_sent` after undo | **Not a bug**: every undo publishes a new `Arc` snapshot; the old one is still held, so pointers cannot be equal. |
| **BUG-05** — `MAX_APRON` | **Not a bug as described**: the limit is the feather/expand radius, not the selection size; reshaping works tile by tile. |
| **BUG-06** — lock inversion | **Not a bug**: no path holds a shard lock and an entry lock at once; `Drop` uses `get_mut`. |
| **BUG-07** — unlinked mask on a moved layer | **Known limitation, left open.** |
| **BUG-08** — torn bytes and a stale footer | **Not a practical risk**: footers carry a CRC and new footers are always written past the previous end. |

## Files changed

| File | Change |
| --- | --- |
| `crates/fx-core/src/command.rs` | Test: `DeleteMask { apply: true }` respects the pixel lock and leaves the mask. |
| `crates/fx-engine/src/view.rs` | Test: `zoom:fill` covers an 800×600 viewport at 0.6, `zoom:fit` unchanged. |
| `crates/fx-render/src/program_tests.rs` | Test: `try_render_tile` propagates a fetch error and matches `render_tile`. |
| `crates/fx-tiles/src/image.rs` | Test: `slot_for` gives `Empty` / `Solid` / `Data`. |
| `crates/fx-engine/src/tools/paint.rs` | Tests: move without the button ends the stroke; `cancel`; `document_changed` forgets the clone source. |
| `crates/fx-engine/tests/edit_flow.rs` | Tests: second save, tool switch mid-stroke, `zoom:fill`. |
| `crates/fx-io/src/fxd/manifest.rs` | `to_manifest` / `layer_entry` / `image_entry` return `Result`; test for a missing level-0 chunk. |
| `crates/fx-io/src/fxd/save.rs` | Propagate the manifest error. |
| `crates/fx-io/src/fxd/container.rs` | `FxdFile.appending: Arc<AtomicBool>`; `append_to` guard + `AppendGuard` (`Drop`). |
| `crates/fx-io/src/fxd/tests.rs` | Test: two `append_to` on one file do not interleave. |
| `crates/fx-engine/src/engine.rs` | `mips_reported: HashSet<DocId>`; one `Error` per document on a non-`Evicted` mip failure. |

## Verification

```text
cargo fmt -p fx-tiles -p fx-core -p fx-protocol -p fx-color -p fx-io -p fx-render \
  -p fx-ops -p fx-engine -p fx-cli -p fx-app -p xtask -- --check        → clean
cargo clippy --workspace --all-targets -- -D warnings                   → clean
cargo test --workspace                                                  → all targets ok
node ui/tools/check-data.mjs                                            → no problems
```

Targeted runs while iterating:

```text
cargo test -p fx-io fxd                    → 24 passed (incl. the two new tests)
cargo test -p fx-engine --lib              → 83 passed (incl. the view + paint tests)
cargo test -p fx-engine --test edit_flow   → 5 passed, 3 consecutive runs
cargo test -p fx-core -p fx-render --lib   → ok
```

No automated test covers S3-03: reaching a non-`Evicted` mip failure needs a corrupted
`.fxd` chunk below a missing mip, which the integration harness cannot build today.

## Deviations from the task card

* The integration save test does not follow the literal "Save As then `doc:save`" step: the
  document has no file yet, so that second `doc:save` goes to `ask_save_path` (the shell's
  "where do I write?" dialog), not to the save guard. The test first does Save As, then two
  `doc:save` calls in a row — which is what exercises the guard. Both outcomes are accepted
  (the second save refused, or run after the first) and the reopened `.fxd` must load.
* S3-03 adds only the user-visible error; the render-side `mips_sent` re-arm was not touched.

## Blocked

- none

## Open problems / suggestions

* While a *fresh* Save As is running (`file` is still `None`), a second `doc:save` asks for a
  path again instead of being refused: the guard lives in `start_save`, but `save()` short-
  circuits to `ask_save_path` first. Guarding `save()` (or reserving `file`/`path` when the
  fresh save starts) would close it.
* S3-03: dropping the failed key from the render thread's `mips_sent` (and explicitly
  re-requesting a frame) would match the finding's fix direction more closely.
* S4-02…S4-06 remain as listed above; none blocks a user flow.
