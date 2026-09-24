# Core engine pieces implemented by Claude

Date: 2026-09-24 · Status: done, tested · Reviewer: Rob

These are the parts of M1/M2 that are hardest to get right (concurrency,
GPU compositing, exact blend math). They are finished and tested. **Build on
them; do not restructure them.** If you find a bug, write a failing test,
fix the smallest thing possible, and explain it in your report.

## What exists

| Area | Files | Tests |
| --- | --- | --- |
| Tile store tiers: hot / warm (LZ4) / cold (scratch file), LRU trim with hysteresis, background trim thread, exact accounting, `scratch_full` | `fx-tiles/src/store.rs`, `scratch.rs` | 20 (incl. concurrent get + trim stress, allocator) |
| Tile programs: per-tile op lists from a document snapshot; skips everything invisible here; content-identity cache keys; dirty-mip reporting | `fx-render/src/program.rs` | `program_tests.rs` (11) |
| Blend modes (27) + source-over / source-atop compositing, f64 | `fx-render/src/blend.rs` | 5 |
| Adjustment LUTs: Invert, Levels, Curves, Exposure (+ `LutCache`) | `fx-render/src/adjust.rs` | 4 |
| CPU reference compositor (defines correct output) | `fx-render/src/reference.rs` | via program + GPU tests |
| GPU atlas: straight f16 pages, clock eviction, parallel conversion | `fx-render/src/gpu/atlas.rs` | via GPU tests |
| GPU compositor: one dispatch per frame for all tiles, result cache by key, upload budget with guaranteed progress, deferral with missing-tile lists, prefix cache for the edited layer | `fx-render/src/gpu/compositor.rs`, `composite.wgsl` | `gpu/tests.rs` (6): every blend mode and a complex stack vs CPU, cache hits, deferral, budget convergence, prefix exactness |
| Frame planner: target tiles, coarser fallbacks, prioritised requests, coverage level | `fx-render/src/frame.rs` | 4 |
| Viewport pass: background, checkerboard, tile quads (nearest ≥ 100 %) | `fx-render/src/gpu/viewport.rs`, `viewport.wgsl` | 1 (renders and reads back pixels) |

Totals at hand-over: 64 tests pass, 4 ignored (M1-T05 mip spec tests for
you), `cargo clippy --workspace --all-targets -D warnings` clean.

GPU tests run on any adapter, including software Vulkan (Linux: `mesa-vulkan-drivers`
/ lavapipe). With no adapter they print "skipped" and pass.

## How the pieces fit (render thread, per frame)

```text
plan_frame(view, …, ready)         → draws + requests (priority order)
build_program(doc, level, tx, ty)  → Ok(program) | Err(dirty mips → mip scheduler)
compositor.composite(programs, try_get_hot)
                                   → Ready{slot} | Empty | Deferred{missing → loader}
ViewportRenderer::render(plan, compositor.composite_view())
```
Details and the integration steps: `docs/tasks/M1.md` M1-T07.

## Invariants you must keep

1. **Every `TiledImage` in a document has the document's size** (placement
   is the layer `offset`). The program builder relies on equal level counts.
2. **The render thread only calls `TileStore::try_get_hot`.** Loading is a
   worker job; `Deferred { missing }` tells you what to load.
3. **Tiles are never mutated.** New pixels = new tile (`TiledImage::put_buffer`).
4. **`BlendMode::shader_id` = enum order = the ids in `composite.wgsl`.** Never reorder.
5. **`GpuOp` / `GpuJob` / `Globals` layouts** in Rust and WGSL must match
   byte for byte (176 / 32 / 16 bytes). Any change: update both + run the GPU tests.
6. The viewport target is **non-sRGB** (`Rgba8Unorm`/`Bgra8Unorm`).
7. The wgpu device must be created with the adapter's
   `max_texture_array_layers` (atlas pages hold up to 2048 tiles).

## Known limitations (documented, not bugs)

* Brightness/Contrast and Hue/Saturation adjustments: M2-T04 (yours).
  Programs containing Hue/Saturation are rejected by the GPU compositor with
  `CompositeError::Unsupported` until then.
* Masks use the colour atlas (4× the memory a single-channel atlas would
  need). Optimise only if VRAM becomes the limit.
* Linear sampling below 100 % clamps at tile edges: faint seams may be
  visible at some zoom levels. Fix later with a 1-pixel tile gutter if Rob
  notices it.
* No cache for the layers *above* the edited one (see M2-T05).
* Formulas marked VERIFY in `BLEND_MODES.md` / `adjust.rs` are checked against
  Photoshop in M7.
