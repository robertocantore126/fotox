# Workspace bug check — `feat/layer-style-overhaul`, 2026-10-01

Read-only review. **No builds, no tests** (per request): everything here is from
reading code in the current working tree. Method: enumerate the first-party
crates, sweep every crate for risky patterns (`unwrap`/`expect`/`panic`/
`unsafe`/`todo`), read the core data-path algorithms in full, and re-check the
findings of the three earlier reviews against the present code (`git` history).

Coverage, stated honestly: I read **in full** `fx-tiles/{format,image,mip}.rs`,
`fx-ops/{morph,gaussian,neighbourhood,filter,raster,flood}.rs`, all of `fx-ai`,
and the whole layer-style change set (`fx-core/styles.rs`, `command.rs`
`create_effect_layers`/`scale_effects`/`set_global_light`, `fx-engine/effects.rs`,
`fx-render/program.rs`+`reference.rs`+`compositor.rs`+`composite.wgsl`, and the
UI files). I swept all `crates/**` for the patterns above and spot-checked the
other crates. I did **not** read every line of `fx-core/command.rs` (7 600
lines), the brush stack, the TIFF/PNG codecs, `fx-app` Win32 code, or `ui/js`
beyond the changed files — findings there are not claimed.

## 1. Previously reported bugs — current status

The 2026-09-25 and 2026-09-26 deep reviews found 12+19 issues; the 2026-09-27
review (R01–R12) says each was fixed with a regression test. I re-checked the
code paths, and the critical ones are genuinely fixed in this tree:

| Prior finding | Status now | Evidence |
|---|---|---|
| Concurrent saves corrupt `.fxd` (S1-01/S1-02) | **Fixed** | `OpenDoc.saving` (`documents.rs:31`); `start_save` refuses while saving (`engine.rs:2763,2810`); path lease |
| Worker panic wedges the document (S1-03/S1-04) | **Fixed** | `catch_unwind` + unconditional `PixelJobDone` (`engine.rs:2268-2271`) |
| Filter/Merge whole-image tile vector (S1-01 09-25 / R06) | **Fixed** | streamed derivation, `derived::render_tiles` |
| Import panic (R09) | **Fixed** | `catch_unwind` in the open worker (`engine.rs:1639`) |
| `Export` loses the document id (S2-04) | **Fixed** | `EngineInput::Export { doc, .. }` (`fx-engine/src/lib.rs:145`) |
| Evicted derived tiles never regenerate (R01) | **Fixed** | `is_evicted`, `build_program_checked` |
| Rasterize applies masks twice (R02) | **Fixed** | `command.rs:1646` area reworked |
| Concurrent exports (R04), Revert (R05), profile conversion (R10), adjustment Cancel (R11), pixel+vector masks (R12) | **Fixed** | see their commits/tests |

Still open in the tree (carried from the 2026-09-27 report’s own “deliberately
left” list, plus two I re-confirmed by reading):

* **`zoom:fill` / `zoom:print` are still hardcoded** — `ui/js/actions.js:247-248`
  (`zoomTo(200)` / `zoomTo(72)`). The engine has `zoom:fit`; neither of these is
  wired, so “Fill Screen” ignores the viewport and “Print Size” ignores `doc.ppi`.
* **`ui/js/shortcuts.js` still runs on every `keydown` without checking
  `event.repeat`** (the only `event.repeat` in the tree is for Space in
  `fx-app/src/input.rs:198`). The new save guard absorbs held `Ctrl+S`, but held
  `Ctrl+Z`, toggles and other bindings still fire at the auto-repeat rate.
* R03 — Bézier-warp Smart Objects under Perspective Crop / Free Transform, and a
  Linear/Angle gradient fill that cannot mirror under a flip.
* R06/R07 — Free Transform preview still draws a Smart Object’s whole level-0
  cache; `layer_content` walks every canvas tile; under a hot budget smaller than
  the visible set the viewport can re-request tiles; some preparation stays on
  the engine loop.
* S4-03 (09-25) — no nesting-depth clamp; `Document::walk`/`path_of` and the
  manifest builders recurse without a limit. `GroupLayers` can nest arbitrarily.
* S4-02 (09-25) — blocking `TileStore::get` on the engine thread in selection
  hit-testing, clipboard and eyedropper paths.
* Design points 1–8 and the README/AGENTS/ARCHITECTURE status inconsistencies
  from the 2026-09-27 report are untouched (decisions for Rob).

## 2. New findings (not in any earlier review)

### N1 — `Create Layers` reorders effects and changes the picture — HIGH
`crates/fx-core/src/command.rs:1826`

```rust
for (k, layer) in inside.into_iter().chain(outside).enumerate() {
    siblings.insert(index + 1 + k, layer);
}
```

The layers above the original are stacked “clipped-inside effects first, then
the rest”, which is not the live order. `fx-render/src/program.rs` (`with_effects`)
draws above-effects in `slots()` order, where a non-inside **Stroke**
(Outside/Center) sits *below* an inner **Bevel**. `stays_inside` says Stroke
Outside = false, Bevel Inner = true, so the created stack becomes
Bevel → Bevel → Stroke — the stroke paints over the bevel, while the live style
painted the bevel over the stroke. Fix: emit sorted by the original slot index;
use `stays_inside` only for `made.clipped`.

### N2 — Layer Style number boxes commit out-of-range values — MEDIUM
`ui/js/native/styles.js:150-152`

```js
range.value = String(clamp(v, min, max));
out.value = fmt(v, step);   // unclamped
if (fire) onInput(v);       // ...applied too
```

Only the `range` element is clamped. Typing `1000` into a `max: 250` field
(Size, Spread, Soften) shows 1000, parks the slider at 250, and sends 1000 to
the engine; the same for `Scale`/`Altitude` and `openScaleEffects`.

### N3 — Masked-layer effect alpha recomposites the whole document per tile — MEDIUM
`crates/fx-engine/src/effects.rs:163` (`source_alpha`) → `:279` (`alone_alpha`)

`alone_alpha` does `doc.clone()` + `keep_layers` + a full `composite_rect` **per
requested effect tile**, and `source_alpha` now routes *every masked layer*
(not just groups, as before) through it. Only `layer_bounds` is cached, not the
alpha. On the documents this app targets, nudging a masked styled layer clones
the whole document once per visible effect tile per frame. Memoize per
`(LayerId, level, tile)` for one `draw_effect_requests` call, or fold the mask
into `read_alpha`.

### N4 — `LayerStyles::scale` turns NaN into zero — LOW
`crates/fx-core/src/styles.rs:1136` — `factor.max(0.0)` returns `0.0` for `NaN`,
silently zeroing every size/distance. `scale_effects` validates first, but the
layer-scale path does not. Reject or ignore non-finite factors.

### N5 — Stale broken intra-doc link — LOW
`crates/fx-core/src/layer.rs:339` still points at `EffectKind::index`, deleted in
this change set (caches are indexed by `LayerStyles::slots()`).

### N6 — `Blend Interior Effects as Group` is a no-op on groups — LOW
`crates/fx-render/src/program.rs` computes
`grouped = interior_as_group && !matches!(layer.kind, Group)`, but the Blending
Options page offers the checkbox for group layers too (`styles.js`
`blendingPage`). Either support groups or hide the control there.

### N7 — Model download integrity only checks existence — LOW
`crates/fx-ai/src/models.rs`: `installed()` returns true when the files simply
exist; the SHA-256 is only checked on a fresh download. A manually truncated or
replaced file is treated as installed and loaded. Verify the checksum (or at
least the size) in `installed()`.

### N8 — Concurrent model downloads share one `.part` — LOW
`crates/fx-ai/src/models.rs::download` writes `<file>.part` with no in-flight
guard, so two download requests for the same model (double click, or a retry
after a UI timeout) can interleave into one temporary file and rename a torn
file into place. There is one engine AI task slot, but the guard lives in the
engine, not in this reusable API.

### N9 — `Contour::custom` accepts points with duplicate inputs in the UI path — LOW
`ui/js/native/styles.js::customContour` de-duplicates only after sorting and
pads short lists with `[0,0]`; the Rust `Contour::custom` then `dedup_by_key`s
and truncates. The two agree today, but the padded `[0,0]` sentinel is
indistinguishable from a real point at `(0,0)`, so a contour whose *first* point
is `(0,0)` and that has fewer than 16 points is passed with trailing sentinels
that only survive because of the dedup. Worth a `None`/`len`-based encoding.

## 3. Things I checked and found correct (so they need not be re-checked)

* `fx-tiles`: `mark_rect_dirty` tile math, `put_buffer` uniform collapse,
  `downsample_2x2` premultiplied averaging, `TILE_SIZE`/format sizes.
* `fx-ops`: `edt_2d`/`edt_1d` bounds (no underflow at `z[0] = -∞`), the separable
  Gaussian’s interior sizing, `neighbourhood::gather` clamping and tile cache,
  `raster::polygon_band` span/diff buffers (indices stay in `0..=width`),
  `flood::fill_tile` border seeding.
* `fx-ai`: `download` resets the hasher per file and deletes on mismatch;
  `runtime::ensure` is a `OnceLock`; `best_mask` index math is bounded.
* The GPU/CPU `EndChannels` implementations match each other, and `EndChannels`
  is treated as a depth-closing op in both `check_supported` and the hot-split
  loop.
* `alone_alpha` does **not** strip a group target’s children styles (`plain`
  recurses only while `!own`), and the Create-Layers below/above insertion
  indices are otherwise correct.
* `Command`’s `#[serde(tag = "op")]` covers `scale_effects`,
  `set_global_light { altitude }` and `create_effect_layers` without extra
  dispatch; `EditKey::of` handles the new `SetGlobalLight { .. }`.
* `PixelFormat` has only RGBA8/16 + Gray8/16, so `paint_glow`’s `_ => bytes_mut()`
  is always RGBA8.

## 4. Suggested order

1. N1 (`Create Layers` order) and N2 (slider clamp) — small, user-visible.
2. N3 (per-tile document clone) — the only large-document invariant risk in the
   new style code.
3. `zoom:fill`/`zoom:print` and shortcut repeat — still open from the 09-25
   review; small.
4. N4–N9 — hardening.
5. Carry the R03/R06/R07 and recursion/blocking-read items into the next
   hardening pass.

*Artifacts: this file and `2026-10-01-bug-review-layer-styles.md`.*
