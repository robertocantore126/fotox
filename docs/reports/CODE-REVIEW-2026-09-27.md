Fotox code review — 2026-09-27
================================

Reviewed `bughunt/round2` at `fac522c`, including the uncommitted working-tree changes present during this review. Those changes were preserved. This report is the only repository file added by the review; no fixes, commits, or pushes were made.

The main conclusion: the tiled core is a useful foundation, but several feature combinations violate its invariants. The highest priorities are preserving output correctness, making evicted derived data recoverable, and bounding work before it reaches the tile store. A rewrite of the UI or rendering stack would not address these failures.

This was a broad review of application code, especially document/history, storage, rendering, geometry, export/import, derived layers, color conversion, and UI command wiring. It was not a line-by-line audit of every file or vendored dependency. The native app was not launched for interactive testing, and no large-canvas performance measurements or real AI model downloads were performed.

**Validation performed**

- `cargo check --workspace --all-targets`: passed, with CMake on PATH.
- `cargo test --lib --tests`: passed for the workspace's default members. This excludes the GUI/vendor crates; explicitly ignored real-model tests remained ignored. Some tests can return early when their GPU/runtime prerequisites are unavailable, so a green result is not a claim that every hardware-dependent path ran.
- `node ui/tools/check-data.mjs`: passed; reported four dialogs reachable only through disabled entries or no entries. This checks references and generated capability data, not actual behavior.
- Two standalone Rust reproduction programs, outside the repository, linked against the project's compiled libraries. They confirmed eviction, rasterization, geometry, export-opacity, overlapping-export, and malformed-image-entry failures. Outputs are included below. These small checks did not allocate huge images.

**Severity**: P1 = high priority correctness, data loss, or failure of the main large-document use case. P2 = significant behavior or robustness issue. “Reproduced” refers to the standalone checks; “source-confirmed” means a complete code path was traced, without native UI reproduction.

**Fix status (Claude, same day, branch `bughunt/round2`)**

Every finding was fixed and has a regression test that fails without the fix (checked by reverting for R02; the others reproduce the review's own programs). `cargo test --workspace` (623 tests) and `cargo clippy --workspace --all-targets -D warnings` pass. Nothing was run in the native app. The uncommitted working-tree changes the review mentions were committed first, unchanged, as `ae99223`.

| ID | Commit | What changed | Test |
|---|---|---|---|
| R01 | `3111860`, `81e5d0d` | `TileStore::is_evicted`; the render thread builds programs with `build_program_checked`, which asks for a dropped derived tile like a dirty one, and forgets a cached program whose input was dropped. Effect inputs and Smart Object source mips count dropped tiles as missing. Mip chains hold each computed level until the next one read it (`insert_held`). | `derived::tests::a_reader_recomputes_an_evicted_mip`, `stress::readers_survive_a_trim_thread_dropping_their_inputs` |
| R02 | `d474f4e` | Rasterize composites the layer alone with masks, styles, clipping and groups left out; they stay on the layer. | `review_2026_09_27::rasterize_does_not_apply_a_mask_twice` |
| R03 | `3be5545` | `command/canvas_space.rs`: one plan, computed before anything changes, moves vector-mask and document paths, Smart Object transforms, gradient/pattern fills, artboards, slices, annotations, axis-aligned guides and comps; alpha channels move with every raster helper; `set_canvas` resizes every canvas-sized cache. | `rotating_the_canvas_turns_the_whole_document`, `canvas_size_moves_the_alpha_channels_with_the_content`, `stress::full_turns_and_double_flips_are_the_identity_for_every_part` |
| R04 | `d5b6c20` | Exports hold the process-wide path lease for the whole write and rename, with a job-unique temporary name. | `fx-io export::overlapping_exports_to_one_path_do_not_mix`, `stress::eight_simultaneous_exports_to_one_path_leave_one_whole_image` |
| R05 | `d5b6c20` | Revert reloads the file the document was opened from or last saved to into the same tab; refused while a job or save runs, or for a never-saved document. | `harden_flow::revert_reloads_the_saved_file_not_the_history_start` |
| R06 | `3111860` | `derived::render_tiles` renders a batch for a reader, computing only that batch's inputs on the reader's copy and holding each tile's inputs while it renders; export no longer prepares the whole document; `prepare_level0` draws 64 tiles at a time. | `vector::tests::a_whole_document_reader_draws_the_shape_tiles_it_reads`, `stress::preparing_a_small_shape_on_a_huge_canvas_stays_bounded` (ignored; 1.5 s for 6 241 tiles) |
| R07 | `3111860` | Frame tile requests run as one background job at a time on a copy, merged back only for the same content generation; pixel jobs prepare on their worker; slice/artboard export runs on a worker with progress. | engine flow tests (frames still converge) |
| R08 | `830f02f` | `opaque_background` refuses a bottom layer with a vector mask or clipping. | `a_vector_masked_background_keeps_the_alpha_channel` |
| R09 | `830f02f`, `d5b6c20` | Every nested image entry is checked (size, format role, level, tile coordinates) before construction; the import worker reports a panic as an error. | `manifest::tests::malformed_nested_images_are_refused_before_construction` |
| R10 | `e217be4` | Convert to Profile is exhaustive over layer kinds: gradient fills, style colours, artboard backgrounds, patterns and Smart Objects (composite and nested document) convert; adjustments and masks deliberately do not. | `command::tests::convert_to_profile_reaches_every_colour_bearing_kind` |
| R11 | `708e368` | `adjustment_preview` / `adjustment_preview_end` messages: the dialog previews in the render snapshot only; OK adds one step (none if unchanged), Cancel adds none and keeps redo. | `harden_flow::an_adjustment_preview_keeps_history_and_redo`, `documents::tests::an_adjustment_preview_is_on_screen_only` |
| R12 | `fb340da` | Pixel and vector masks multiply; when both vary, the op is nested in a group carrying the second mask (exact). | `program_tests::two_varying_masks_equal_their_product`, `pixel_and_vector_masks_multiply` |

Follow-up (same branch), closing four of the items first left open:

| Commit | Change | Test |
|---|---|---|
| `8bd6d11` | Pattern fills gain an origin and a mirror; the canvas mapping composes with the pattern's matrix, so Flip, Rotate, Canvas Size and Crop move the fill exactly (the earlier angle update also turned it the wrong way). `m12::compose` holds a Bézier warp under an affine map and over an axis-aligned scale; Liquify/Puppet geometry (a session-only registry) is never kept on a Smart Object, and an FXD naming one is refused. | `a_pattern_fill_follows_the_canvas`, `a_warped_smart_object_follows_the_canvas`, `a_stored_custom_mapping_is_refused` |
| `ffe51fa` | No whole-document preparation before pixel jobs, Rasterize, Convert to Smart Object, Edit Contents save or Define Pattern: they composite through `derived`, which draws what each tile reads. Copy and Align take a shape's content from `derived::layer_content` (ordinary tiles, not the raw cache). | `a_never_drawn_shape_is_copied_and_merged_from_its_geometry`, `copying_a_small_shape_on_a_huge_canvas_stays_bounded` (ignored; 20 000² in 1.6 s, 4 tiles stored) |
| `e4d1fe4` | A Smart Object's source mips are computed as the sampler reads them and held until the draw ends (the old whole-pyramid build also failed with `Evicted` under a busy trim). | `a_smart_object_computes_only_the_source_mips_it_samples` |

Still open, deliberately:

* R03 — a Smart Object whose transform is a Bézier warp refuses Perspective Crop (a projective map of a Bézier patch is not one), and a warp over a turned Smart Object is refused by Free Transform; shapes and text stay put under Perspective Crop (as before). A Linear/Angle gradient fill cannot mirror its sweep under a flip.
* R06/R07 — the Free Transform preview of a Smart Object still draws that layer's whole level-0 cache; `layer_content` walks every canvas tile (cheap for empty ones, 1.6 s at 20 000²); under a hot budget smaller than the visible working set the viewport can keep re-requesting tiles. Large-canvas timings and peak memory were not measured in the app.
* R11 — the UI side (`ui/js/native/layers-panel.js`) was checked with `node --check` and `check-data.mjs` only; the dialogs need a native smoke test.
* Design points 1–8 and the documentation inconsistencies below are untouched: they are decisions for Rob.

| ID | Priority | Finding | Evidence |
|---|---|---|---|
| R01 | P1 | Evicted derived tiles never trigger regeneration in the renderer | Reproduced + source trace |
| R02 | P1 | Rasterize applies masks twice and retains baked effects | Mask reproduced |
| R03 | P1 | Canvas transforms omit Smart Objects and other document geometry | Reproduced + source trace |
| R04 | P1 | Concurrent exports share one temporary file and corrupt each other's output | Reproduced |
| R05 | P1 | Revert restores an undo state, then incorrectly marks it saved | Source-confirmed |
| R06 | P1 | Derived-layer preparation allocates whole-canvas pixel batches | Source-confirmed |
| R07 | P2 | Mip generation and export preparation block the engine loop | Source-confirmed |
| R08 | P2 | Automatic export drops vector-mask transparency | Reproduced |
| R09 | P2 | Nested FXD image entries can panic the unguarded import worker | Reproduced + source trace |
| R10 | P2 | Profile conversion skips later color-bearing layer types | Source-confirmed |
| R11 | P2 | Adjustment-dialog Cancel changes history and destroys redo | Source-confirmed |
| R12 | P2 | Adding a pixel mask disables the vector mask's contribution | Source-confirmed; marked shortcut |

**R01 — [P1] Regenerate evicted derived tiles instead of trying to reload them forever**

Locations: [program.rs:752](../../crates/fx-render/src/program.rs#L752), [image.rs:282](../../crates/fx-tiles/src/image.rs#L282), [render.rs:484](../../crates/fx-engine/src/render.rs#L484), [store.rs:757](../../crates/fx-tiles/src/store.rs#L757).

`build_program` requests work only when `image.is_dirty()` is true. That method reads a stored dirty bit, which eviction does not change. The store can discard the only copy of a clean derived tile. Its handle remains in the image and in cached programs. The compositor then reports the tile missing; the render loader calls `store.get`, logs `Evicted`, and wakes the render loop without scheduling regeneration or replacing the handle.

Reproduction: install a derived level-1 tile in a 512×512 pixel layer, trim a store with a zero hot budget, then build its level-1 program. Output: `get=Some(Evicted); build_program_ok=true`. A regeneration request was required. `ensure_mip` itself understands eviction, but this path never reaches it. This also affects generated layer caches, not just pixel-layer mips. A GPU atlas/cache hit can hide the failure until those caches also miss.

Impact: missing or stale image regions after memory pressure, potentially with repeated load/wake work. This undermines the large-document design even when the tile-store unit tests pass.

Fix direction: distinguish reloadable cold tiles from discarded derived tiles without blocking the render thread. Keep producer information for every derived source, schedule its regeneration, and invalidate programs that still reference the discarded handle. Test the complete trim → program → request → regenerated frame path.

**R02 — [P1] Rasterize must not bake properties that remain attached to the layer**

Location: [command.rs:1646](../../crates/fx-core/src/command.rs#L1646), especially the temporary layer neutralization at line 1661 and replacement at line 1677.

Rasterize neutralizes opacity, fill, visibility, and blend mode before compositing, but leaves pixel masks, vector masks, styles, and effects attached. Their contributions enter the rasterized pixels. It then replaces only `layer.kind`, so those properties apply again when the new pixel layer renders.

Reproduction: a solid fill with a uniform 50% pixel mask, rendered before and after `Command::Rasterize` through the real `EngineOps`: `alpha_before=0.5000, alpha_after=0.2510`. This is an immediate visible change from an operation that should preserve appearance. The retained effects follow the same bake-and-retain path, although individual effect outputs were not separately reproduced.

Fix direction: rasterize intrinsic content with retained masks/styles temporarily removed, or explicitly bake and remove them as a separate operation. Include clipping and enclosing group properties in the rasterization contract. Add appearance-equivalence tests with masks and styles.

**R03 — [P1] Canvas geometry commands do not cover the complete document model**

Locations: [command.rs:2619](../../crates/fx-core/src/command.rs#L2619), [command.rs:2643](../../crates/fx-core/src/command.rs#L2643), [command.rs:2705](../../crates/fx-core/src/command.rs#L2705), [layer.rs:278](../../crates/fx-core/src/layer.rs#L278).

Canvas permutation transforms pixel images, pixel masks, active/reselect selections, and the generated placements returned by `derived_placement`. That helper handles Shape and Text only. Smart Object transforms and caches, vector-mask paths/caches, saved alpha channels, and other document-space objects do not participate in this operation. The resize helper likewise rebuilds only those Shape/Text caches. Similar helpers are reused by other canvas geometry commands.

Reproductions after rotating a 20×10 canvas 90°:

```text
canvas=10x20, vector_mask=20x10, channel=20x10
canvas=10x20, smart cache=20x10, smart transform_identity=true
```

Impact: mixed documents rotate only partly; masks and saved selections no longer line up with the image. Smart Objects remain unrotated and retain the old cache dimensions. A resize that exposes new cache tiles can also leave generated content absent there. Saved paths, guides, annotations, artboards, and slices need explicit transform semantics; their omission should not be accidental.

Fix direction: define one document-geometry operation covering every component, with exhaustive handling of layer variants and explicit policies for guides/slices/annotations. Test a mixed document containing pixel, shape, text, smart, fill, mask, channel, and path data through rotate, flip, crop, and resize, including undo.

**R04 — [P1] Concurrent exports can change an already-successful export's file**

Locations: [export.rs:94](../../crates/fx-io/src/export.rs#L94), [export.rs:118](../../crates/fx-io/src/export.rs#L118), [engine.rs:1557](../../crates/fx-engine/src/engine.rs#L1557).

Every export to a target uses the same `<name>.part`. Export workers can overlap, and this path does not use the path lease added to native FXD saving. One worker can truncate another's temporary file; renaming does not stop a worker with an open handle from subsequently writing to that file.

A deterministic Windows reproduction held export A inside its band callback, completed export B to the same path, then resumed A:

```text
export_B=Ok(())
export_A=Err(NotFound)
final_pixel=[255, 0, 0, 255]
expected B blue=[0, 0, 255, 255]
```

B reported success, but its final output contained A's red pixels. The failure is therefore more serious than a harmless temporary-file rename error.

Fix direction: use unique temporary files and a canonical destination lease covering the whole write/commit, with a defined ordering for repeated requests. Apply this consistently to flat exports and region exports. A job must only clean up its own temporary file.

**R05 — [P1] Revert can discard the saved state and suppress the save warning**

Location: [engine.rs:3471](../../crates/fx-engine/src/engine.rs#L3471).

`doc:revert` undoes all retained history entries and sets `dirty=false`. That is neither the last saved snapshot nor necessarily the initially opened state. Saving does not clear history, and history retains only a limited number of steps.

Example: open A, edit to B, save B, edit to C, choose Revert. The implementation goes back toward A, while the file contains B. After more than the history limit, it stops at whichever intermediate state is still retained. In both cases it labels the result clean, so closing can discard the mismatch without warning. The action also bypasses the usual command busy guard.

Fix direction: reopen the saved source into a replacement document, or retain an explicit saved snapshot/content identity independent of undo. Only clear dirty state after successfully restoring that state. Define behavior for a never-saved document and for running jobs.

**R06 — [P1] Whole-canvas derived rasterization bypasses the memory budget**

Locations: [vector.rs:34](../../crates/fx-engine/src/vector.rs#L34), [vector.rs:66](../../crates/fx-engine/src/vector.rs#L66), [text.rs:82](../../crates/fx-engine/src/text.rs#L82), [effects.rs:44](../../crates/fx-engine/src/effects.rs#L44).

`prepare_level0` collects all dirty level-0 tiles for generated layers. Shape, text, vector-mask, and fill rendering collect the entire requested batch as raw `TileBuffer`s before inserting them into `TileStore`. Effects similarly retain their input alpha windows and output buffers. These allocations are outside the trim system.

For a 30,000×30,000 16-bit layer, 118×118 output tiles alone occupy approximately 6.8 GiB before insertion. A 20,000×20,000 layer requires approximately 3.05 GiB. Even a tiny shape on an otherwise empty canvas initially has all derived-cache tiles dirty, so the allocation can include thousands of transparent tile buffers that are discarded only afterward. Parallelism does not bound this batch.

Fix direction: bound generated-tile batches by bytes or a small tile count, insert results immediately, and avoid rendering tiles outside known geometry bounds. Export and merge should prepare and consume the required band/tile region rather than preparing the entire document first. Pin only inputs currently being consumed; precomputed derived results must remain recoverable if trimmed.

**R07 — [P2] The engine event loop still performs expensive tile and disk work**

Locations: [engine.rs:3758](../../crates/fx-engine/src/engine.rs#L3758), [engine.rs:1568](../../crates/fx-engine/src/engine.rs#L1568), [engine.rs:2041](../../crates/fx-engine/src/engine.rs#L2041), [engine/m12.rs:410](../../crates/fx-engine/src/engine/m12.rs#L410), [smart.rs:39](../../crates/fx-engine/src/smart.rs#L39).

`compute_mips` calls `ensure_mip` synchronously; its child reads can block on `TileStore::get`, and its parallel work is joined before returning. Export and pixel jobs call full-resolution derived preparation before spawning their worker. Slice/artboard export performs compositing, encoding, and filesystem writes entirely on the engine thread. Smart Object preparation walks and builds the source pyramid rather than just the source region needed by a requested tile.

The render thread may continue presenting a previous frame, but input, tool changes, close requests, and new commands wait for the engine. This is a concrete architectural source of stalls; their wall-clock duration was not measured in this review. It is distinct from R06: making buffers smaller alone will not restore event-loop responsiveness.

Fix direction: move preparation as well as execution into cancellable jobs. Return derived results with document generation and source identity, apply only still-current results, and prioritize visible tiles. Use bounded work queues and coalesce replaceable view/preview requests without dropping brush input samples.

**R08 — [P2] Automatic export treats vector-masked content as opaque**

Location: [export.rs:55](../../crates/fx-engine/src/export.rs#L55).

`opaque_background` checks opacity, fill, and a pixel mask, but not an enabled vector mask. It returns true immediately for an opaque SolidFill, or after checking raw pixel alpha for a Pixel layer. An enabled vector mask can make either layer partially or entirely transparent. Default export consequently sets `alpha=false` and composites those areas onto white.

Reproduction: a solid fill with an enabled empty vector mask returns `opaque_background=true`, despite the mask hiding the content. Explicitly requesting transparency avoids this default, but automatic export must preserve the document.

Fix direction: conservatively retain alpha whenever masks or other compositing semantics prevent proving full coverage. This optimization should prefer an unnecessary alpha channel over irreversible transparency loss.

**R09 — [P2] FXD validates the canvas but trusts nested image dimensions and indices**

Locations: [manifest.rs:633](../../crates/fx-io/src/fxd/manifest.rs#L633), [engine.rs:1533](../../crates/fx-engine/src/engine.rs#L1533).

`from_manifest` validates root canvas dimensions, but `image_from_entry` passes nested dimensions directly to `TiledImage::new` and uses serialized level/tile coordinates directly as indices. A zero-sized layer image panics (`empty image`), and invalid levels/coordinates can hit bounds assertions. Excessive nested dimensions can allocate huge grids even when the canvas is small. This applies to masks/channels/previews as well as layer images.

Reproduction: calling `image_from_entry` with width 0 and height 10 panicked instead of returning `IoError`. The FXD import worker calls the decoder without the `catch_unwind` completion handling used by save/export/pixel jobs. A panic can terminate that worker before it sends `OpenedFxd`/the corresponding error completion, leaving progress unresolved. This is not claimed to terminate the entire process in the default unwind configuration.

Fix direction: validate every nested image's dimensions, checked grid size, format role, level range, tile coordinates, and relevant resource lengths before construction. Add per-import resource limits and guarantee a completion/error message for a worker panic. The manifest's current 1 GiB decompression ceiling is not a sufficient nested-allocation budget.

**R10 — [P2] Convert to Profile skips Smart Objects, gradient fills, and pattern content**

Location: [command.rs:2139](../../crates/fx-core/src/command.rs#L2139), especially the wildcard return before the document profile changes at line 2257.

The conversion visits Pixel, SolidFill, Shape, and Text, then silently skips other variants. Gradient/pattern fill colors, pattern resources, and Smart Object composites are still sampled as raw values by their rendering paths. The document profile is nevertheless changed. Colors from those sources are therefore reinterpreted under the new profile while neighboring supported layers have been converted. Styles also carry colors outside this layer-kind conversion.

Fix direction: define color provenance for every color-bearing resource and convert at the appropriate boundary. Smart Object sources can retain their own profile if sampling converts into the parent working space. Handle fill colors, pattern resources, styles, and cache invalidation explicitly. Add a same-color comparison across layer kinds before and after profile conversion.

**R11 — [P2] Cancelling an adjustment dialog is a real edit with history side effects**

Locations: [layers-panel.js:822](../../ui/js/native/layers-panel.js#L822), [layers-panel.js:842](../../ui/js/native/layers-panel.js#L842), [history.rs:59](../../crates/fx-core/src/history.rs#L59), [engine.rs:2631](../../crates/fx-engine/src/engine.rs#L2631).

Adjustment dialogs send `SetAdjustment` for live preview and again on Cancel to restore the original value. Those are normal content commands: they dirty the document, clear redo, and add history entries. Time-based coalescing only merges nearby changes; it cannot implement a dialog transaction. Even opening an adjustment and cancelling without changing a control sends an original-value command. If the user previously undid an edit, that command clears the redo branch.

Fix direction: use a preview session outside committed history, with one commit on OK and zero history/dirty changes on Cancel. Record the document/layer identity at session start. A no-op command check helps the no-change case but does not restore redo lost during a real preview.

**R12 — [P2] Pixel and vector masks are mutually exclusive in compositing**

Location: [program.rs:648](../../crates/fx-render/src/program.rs#L648).

When a pixel mask is enabled, the compositor ignores the enabled vector mask entirely. This is explicitly marked `FAST`, but the document model and command paths allow both. Adding a reveal-all pixel mask to a vector-masked layer can therefore reveal content outside the vector path. Disabling that pixel mask makes the vector clipping suddenly return.

Fix direction: multiply both masks' coverage, either with two mask inputs or a combined derived coverage tile. Preserve the independent enable/density semantics. Until supported, the UI should not present their combination as fully functional.

**Design decisions worth changing**

1. **Keep the tiled engine, but separate document truth from derived caches.** Currently caches live inside cloneable layers/history snapshots, while their pixels can disappear independently in the store. A cache service keyed by immutable source identity plus operation parameters would make eviction, regeneration, and memory budgeting explicit. Introduce this incrementally, beginning with the R01 request/response contract; do not rewrite the whole compositor at once.

2. **Replace scattered worker handling with one job lifecycle.** A typed job result should identify document, generation, task, cancellation state, and completion/error. Preparation belongs in the same job. Centralize busy-state cleanup and shutdown continuation, and budget both worker concurrency and bytes in flight. Dedicated threads per request and unbounded queues are convenient but do not provide aggregate memory control.

3. **Use exhaustive model operations for geometry and color.** The repeated pattern here is that an older operation knows about Pixel/Shape/Text while newer variants fall through. Small internal helpers or visitors with exhaustive enum matches can force an explicit decision whenever a layer variant is added. Include non-layer document data, rather than treating channels, paths, and artboards as unrelated side tables.

4. **Separate saved content identity, history position, and UI previews.** A monotonic generation is useful for rejecting stale worker results, but it is not a saved-state identity. Undoing a selection currently dirties the document in `step_history` even though selection commands are treated as non-content edits on execution. Track content identity/savepoints and dialog transactions directly instead of deriving them from a dirty boolean and coalescing time windows.

5. **Make resource limits apply before the tile store.** JPEG import decodes the whole image and permits sides up to 65,535 (`fx-io/src/jpeg.rs:23`), so RGBA output alone can approach 16 GiB before decoder overhead and tiled copies. Interlaced PNG is capped per side, but 16,384² RGBA16 still permits a roughly 2 GiB whole-frame buffer. Keep these documented deviations visible, add checked byte-based admission limits, and investigate streaming/banded decoders where their complexity is justified. Tile budgets cannot constrain external whole-image buffers.

6. **Keep Rust, the current compositor, and plain ES modules for this hardening phase.** The demonstrated failures are integration and ownership problems, not evidence that CEF, wgpu, or the UI's lack of a framework is the cause. Reduce browser-mock/native behavioral divergence by driving common panels with a protocol adapter. A mock that only returns plausible messages cannot establish that the native edit path works.

7. **Make the capability list describe behavior, not string presence.** `gen-implemented.mjs` and `check-data.mjs` are useful consistency checks, but a handler string does not prove a control has the advertised effect. Declare supported controls/options and shortcuts explicitly and validate their protocol payloads and state changes. Mark unsupported options unavailable instead of silently accepting them.

8. **Revise the performance contract to something achievable and measurable.** “The cost of any operation never depends on document size” is too broad: a complete export necessarily processes the complete output. Require bounded resident memory, responsive event loops, interactive work proportional to visible/changed regions, and streamed whole-document jobs with progress/cancellation. Measure first-frame latency, input delay, peak total process memory, and bytes processed—not only tile-store counters.

**Documentation and verification inconsistencies**

The README still says M0 has not started, despite implementations through later milestones. `AGENTS.md` describes fast mode and defers review/testing, while the task/report history and code contain HARDEN work and subsequent bug hunts. Architecture documentation describes worker-scheduled mip generation, but the engine currently waits for that work. These contradictions make it difficult to know which guarantees are intended to hold now. Update the status and workflow documents after deciding the next hardening scope.

The current green test suite covers many primitives well, including blend math, permutations, storage tiers, and ordinary save round trips. The failures above mostly sit between individually tested components. Prioritize a compact integration matrix: mask × rasterize, new layer kind × canvas geometry, color resource × profile conversion, eviction × renderer, overlapping jobs × destination, and preview × undo/redo/savepoint. Add malformed nested FXD fixtures, not just root-header corruption. Retain native smoke checks for popup/input routing, text entry/IME, and dirty-close behavior.

Existing reports and the uncommitted fixes were cross-checked. This review does not re-report the fixed canvas edge clipping, option-dropdown notifications, popup scrolling, gradient-map wiring, or layer-list scroll restoration as new bugs. Earlier claims that mip helpers and render caches are individually correct do not establish the missing eviction-to-regeneration integration in R01.

**Suggested order of work**

First address R04/R05/R02, which can replace expected output or discard expected state. Then fix R01 and R03, which affect ordinary navigation and document transformations. Bound all derived preparation (R06) before measuring large-canvas behavior, then move it off the engine loop (R07). Finish export opacity, import validation, color consistency, and transactional previews/mask composition. Each fix should gain a focused regression that exercises the public path responsible for the failure.

No native-app startup claim, AI-model quality claim, Photoshop parity claim, or measured 20k/30k performance claim is made by this review.
