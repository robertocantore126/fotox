# Bug review — layer-style overhaul (`feat/layer-style-overhaul`)

Scope: the uncommitted change set on the branch (24 modified files, 2 new tests,
1215 new lines in `ui/js/native/styles.js`) plus the code it reaches in
`fx-core`, `fx-engine`, `fx-render` and `fx-io`. Read-only review: no tests
were run. Severity is about correctness, not polish.

## Confirmed

### 1. `Create Layers` reorders effects and changes the picture — HIGH
`crates/fx-core/src/command.rs:1826`

```rust
for (k, layer) in inside.into_iter().chain(outside).enumerate() {
    siblings.insert(index + 1 + k, layer);
}
```

The layers above the original are stacked "clipped-inside effects first, then
the rest", which is *not* the live compositing order. `fx-render/src/program.rs`
(`with_effects`) draws above-effects strictly in `slots()` order, and in that
order a non-inside Stroke (Outside/Center) sits **below** an inner Bevel.
`stays_inside` (styles.rs) says: Stroke Outside = false, Bevel Inner = true, so
`inside = [BevelShadow, BevelHighlight]`, `outside = [Stroke]`, and the created
stack becomes Bevel → Bevel → Stroke — the stroke now paints over the bevel,
while the live style painted the bevel over the stroke.

Repro: Drop-in: a layer with an outside 4 px Stroke *and* a default Bevel &
Emboss (Inner Bevel). Compare the canvas before and after Layer ▸ Layer Style ▸
Create Layers; the stroke's position relative to the bevel flips.

Fix: emit the created layers sorted by their original slot index, using
`stays_inside` only to choose `made.clipped` (as the comment already says).

### 2. Effect sliders commit out-of-range values — MEDIUM
`ui/js/native/styles.js:150`

```js
range.value = String(clamp(v, min, max));
out.value = fmt(v, step);      // unclamped: the box shows what was typed
if (fire) onInput(v);          // ...and the effect gets it too
```

The number box is not clamped (only the `range` element is), so typing `1000`
into a `max: 250` field (Size, Soften, Spread, Scale, Altitude, and
`openScaleEffects`'s Scale) shows 1000, the slider sits at 250, and the engine
gets 1000. Fix: `v = clamp(Math.round(v / step) * step, min, max)` before
writing `out.value` and calling `onInput`.

### 3. `source_alpha` clones the document per effect tile — MEDIUM (performance)
`crates/fx-engine/src/effects.rs:163` (`source_alpha`) → `:279` (`alone_alpha`)

`alone_alpha` does `doc.clone()` + `keep_layers` + a full `composite_rect` *per
requested effect tile*, and it is now reached for **every masked layer**, not
only groups (the old `group_alpha` was a group-only path). `draw_effect_requests`
calls it once per `(layer, effect, level, tx, ty)` request, and the only cache is
`bounds_of` (for `layer_bounds`), not the alpha. On a 30 000 × 30 000 document
with hundreds of layers, nudging a masked, styled layer clones the whole
document once per visible effect tile per frame.

Fix: memoize the alpha per `(LayerId, level, tile)` for the duration of one
`draw_effect_requests` call, or fold the mask into `read_alpha` instead of
re-compositing the layer alone.

### 4. `LayerStyles::scale` turns NaN into zero — LOW
`crates/fx-core/src/styles.rs:1136`

```rust
let k = factor.max(0.0);
```

`f64::max(NaN, 0.0)` returns `0.0`, so a non-finite factor silently zeroes every
size and distance. `scale_effects` validates `percent.is_finite()` first, but
`scale()` is also called when scaling a layer with its styles, where the factor
comes from the transform maths. Reject or ignore a non-finite factor.

### 5. Stale intra-doc link to a removed method — LOW
`crates/fx-core/src/layer.rs:339`

The `effects` field doc says "indexed by `[crate::styles::EffectKind::index]`",
but `EffectKind::index`/`from_index` were deleted in this change set; caches are
now indexed by `LayerStyles::slots()`. `cargo doc` emits a broken-link warning.

## Needs a decision / verify

* `with_effects` (program.rs:560) only honours `interior_as_group` for
  non-groups (`!matches!(layer.kind, Group)`), yet the Blending Options page
  offers the checkbox for groups. Either support it for groups or hide the
  checkbox there.
* `finish` (effects.rs) remaps a glow's falloff with `k = 0.5 / range` and is
  self-marked `VERIFY`; the direction (Range < 50 % stronger vs weaker) should
  be checked against Photoshop before it is trusted.
* Channels are applied by wrapping the layer's whole op run in a pass-through
  group; the unticked channels then come from the *backdrop below the group*.
  The new GPU `channels_match_the_reference` test proves CPU/GPU agree, but both
  may still differ from Photoshop for effects that used to blend directly.
* `Create Layers` uses `make.opacity = params.opacity`, ignoring `layer.opacity`
  / `layer.fill`. Photoshop also changes appearance here, so this may be fine —
  but the two new tests use opacity 1, so the difference is untested.

## Smaller UI notes

* `check()` (styles.js) calls `preventDefault()` but not `stopPropagation()`;
  a checkbox inside a clickable row can bubble into the row handler.
* `compact()` (styles.js) leaves `effects_visible: true` in the payload; harmless
  but noise on every live preview send.
* `openStyleDialog(id)` with an effect id calls `commit()` before the window is
  built, so opening a style via one effect's menu sends a `set_layer_style` that
  enables the first instance. Intended (matches Photoshop), but it always adds a
  history step (merged) even when the user cancels.

## Not bugs (checked)

* `alone_alpha` does **not** strip nested styles from a group's children: `plain`
  only recurses while `!own`, so the target group's children keep their styles
  and the group's effects are computed from its full content. (It does strip the
  target's own styles, which is correct.)
* `create_effect_layers`'s below/above insertion indices are correct: the
  `inside.chain(outside)` pass preserves the layer index, and the later `below`
  inserts each land directly under it in slot order.
* `Command` is `#[serde(tag = "op")]`, so `scale_effects`, `set_global_light`
  (with `altitude`) and `create_effect_layers` deserialize without extra
  dispatch; `EditKey::of` covers the new `SetGlobalLight { .. }`.
* `PixelFormat` only has RGBA8/16 and Gray8/16, so `paint_glow`'s
  `_ => bytes_mut()` is always RGBA8.
* `after_edit` re-sends the whole layer list on every edit
  (`engine.rs:3048`), so `CreateEffectLayers`'s new layers appear even though
  `props_changed` lists only the source layer.
* The GPU/CPU `EndChannels` implementations match each other (stack pop, keep
  unticked channels from the backdrop, result alpha), and `EndChannels` is
  counted as a depth-closing op in both `check_supported` and the hot-split loop.
