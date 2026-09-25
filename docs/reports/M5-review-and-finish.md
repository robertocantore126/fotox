# M5 — review of the agent's work, and completion

Reviewer and finisher: Claude  ·  Branch: `m5` (the agent's T00–T03 commits and its
uncommitted T04 slice, committed as found before the review)

## What the agent had done

* **T00** decisions D-040…D-049 recorded; **T01** tools reach the engine
  (`DocPointer`, `Tool` trait, Eyedropper, `ToolOptions`/`SetColors`/`ColorPicked`);
  **T02** viewport overlays (model, tessellation, GPU pass, 8 Hz ants);
  **T03** selection core (model, commands, rasterisers, feather/expand/contract/
  border/smooth, marching-ants contour); **T04 (part)** marquee, single
  row/column, lasso, polygonal lasso, the `key` message. Careful work with honest
  reports and meaningful tests; all checks green.

## Review fixes

1. **Moved selections were read in the wrong place.** `combine`, and every
   reshape, listed the selection's *image* tiles as if they were canvas tiles:
   Add/Subtract/Border/Feather after Move Selection lost or misplaced the moved
   part. New `Selection::canvas_tiles` / `tile_coverage` / `PatchReader` read
   canvas tiles with the offset applied, in one place.
2. **Selections did not scale to B1.** The polygon rasteriser tested every
   pixel of the bounding box against every lasso point (a 20 000-point lasso over
   a large area: minutes to hours); combine/invert/feather read pixels through a
   per-pixel hash map, serially, even for uniform tiles. Now: a scanline
   converter (non-zero winding, 16 sub-scanlines, exact horizontal coverage, one
   row buffer per band), the ellipse as an adaptive polygon, solid tiles inside
   big shapes, rayon everywhere, and uniform tiles/patches answered without a
   per-pixel loop.
3. Modify Selection runs as a job and does not dirty the document; inverting a
   full selection leaves no selection.
4. Integration tests start one engine at a time (three in parallel ran the GPU
   out of memory).

## What I completed

* **T04**: Magic Wand (`fx-ops::flood`: tile-by-tile scanline flood in parallel
  rounds, a tile that matches entirely taken whole, non-contiguous thresholding,
  anti-aliased edge; `Command::MagicWand` as a job; current layer or all layers);
  moving the outline by dragging inside the selection (a click stays a click),
  arrow keys (Shift = 10 px); Alt in the freehand lasso draws straight segments.
* **T05**: Edit ▸ Fill (dialog, Alt/Ctrl+Backspace, Shift = preserve transparency),
  Clear on Delete, Layer via Copy/Cut, masks from the selection (menu and the
  panel's mask button, Alt = hide), filters limited by the selection, the
  clipboard (tiles stay internal; Copy, Copy Merged on a helper thread, Cut,
  Paste centred in view or in place, 8↔16-bit; ≤ 8192 px also to the Windows
  clipboard as CF_DIBV5, and an image copied by another program pastes as a
  layer). The blend maths moved to `fx-core::blend` (D-047).
* **T06**: the brush engine (`fx-ops::brush`): round tip (hardness, roundness,
  angle, supersampled small dabs), pencil tip, spacing independent of event
  batching, pressure dynamics, Photoshop's stroke buffer (flow builds up, opacity
  is a ceiling from the pixels at stroke start), blend modes, selection, lock
  transparency.
* **T07**: `Command::Stroke` + `PixelOps::stroke`; the engine's live stroke
  session (tiles into the live layer, the touched mips up to the view level at
  once, hot layer, one History step at pen-up). **Spec test**: the recorded
  command replayed on the document before the stroke equals the live result,
  tile by tile.
* **T08**: Eraser (to transparency; background colour on a mask or with the
  lock), Pencil, Clone Stamp (Alt-click source, Aligned, current/all layers,
  bilinear for sub-pixel offsets), Healing Brush (Poisson blend on a pyramid at
  pen-up), Spot Healing Brush (source chosen among 8 candidates by the ring SSD).
* **T09**: option bars remember values per tool; `[` `]` size in Photoshop's
  steps, Shift+`[` `]` hardness, number keys opacity (two quick digits exact);
  built-in round presets; Smoothing (pulled string, catch-up at pen-up) and the
  pen-pressure toggles; brush outline / crosshair overlay; Alt = eyedropper;
  painting on a layer mask (click the mask thumbnail, framed); Lock transparent
  pixels (D-049) in the panel and the `.fxd` manifest.
* **T10**: Select ▸ Modify dialogs, Reselect Shift+Ctrl+D, Feather Shift+F6, the
  Mode buttons show the modifier's mode while held, the marquee's W × H in the
  status bar (`tool_info`).
* **T11**: input → pixels latency p50/p99 in `status` and the frame-time overlay.

## Deviations and open points

* **Airbrush** (dabs while the pen stands still) is not implemented; the toggle
  is gone from the brush bar. The eraser's Pencil/Block modes use a hard (not
  aliased) tip. Clone's "Current & Below" is not offered.
* The paint tools run on the engine thread with rayon over tiles, not on a
  dedicated stroke thread (D-041 said "rayon + a dedicated stroke thread"): the
  per-event work is a few tiles. If S9 is missed on B1, move `Stroke::add` to a
  worker first.
* Menu items that need a selection (Clear, Layer via Cut…) stay enabled; the
  engine answers "nothing is selected" with a toast.
* W selects Quick Selection first (Photoshop's order); the Magic Wand is in the
  same toolbar slot.
* Formulas marked VERIFY: tip fall-off curve, feather σ = r/2, wand distance,
  wand anti-alias, ants speed.

## For Rob

* **S9**: 500 px brush on a 16-bit layer of B1 at 100 %: Ctrl+Alt+F shows the
  input latency p99 while painting (≤ 16 ms). Plus the 240 Hz phone video.
* **S13** wand (tolerance 32, contiguous, ~10 % of B1) ≤ 2 s; **S14** feather a
  20 000² rectangle by 50 px ≤ 5 s; **S15** a 500 px stroke at fit ≥ 30 fps.
* Feel of pressure, smoothing and spacing against Photoshop with the same brush.

## Verification

```text
cargo fmt --check / clippy --workspace -D warnings / cargo test --workspace: clean
  new tests: selection offsets and scale (fx-core, fx-ops raster), flood (5),
  pixels (6), using-the-selection commands (7), brush tip/path/stroke/heal (18),
  live == replay (fx-engine), engine flows: wand + outline drag, fill/clear/
  copy/paste/via-copy/mask, brush/eraser/pencil and painting on a mask
node ui/tools/check-data.mjs: ok
In the app (smoke-6000x4000.tif): brush stroke, ] size, marquee + Alt+Backspace
fill with ants, Clone Stamp with an Alt-click source; History shows each step.
```
