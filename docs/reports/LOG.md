# Fast-mode log

One entry per card, newest at the bottom (`AGENTS.md` §1). Keep it short.
HARDEN reads this file first, so be honest about what is missing.

```markdown
## M6-T07 — Text layers  (agent, date, commit)
- Done: what works now, in one or two lines.
- Skipped: what the card asks that is not there (or "none").
- FAST: the shortcuts worth knowing beyond the `// FAST:` comments (or "none").
- VERIFY: guesses about Photoshop's behaviour (or "none").
- Try it: how to see the feature in the app (menu, tool, shortcut).
```

---

## M6-T07 — Text layers  (Claude, 2026-09-26, 07649a8)
- Done: `LayerKind::Text` (model, commands `SetText`, name follows text; by the earlier agent) + engine wiring by Claude: layout cache and tile drawing through the shape request path (`fx-engine/src/text.rs`), `.fxd` save/load, Rasterize ▸ Type, clipboard, `LayerInfoKind::Text`, Type tool (`tools/type_tool.rs`: click = point text, drag = paragraph box, click on a text = enter it, caret/selection/frame overlay, Esc cancel, Ctrl+Enter / keypad Enter / click outside / tool change commit as one history step), hidden textarea bridge in `ui/js/native/type.js` (IME-friendly, UTF-8 byte offsets), system font list → option bar (`EngineToUi::Fonts`), I-beam cursor, "T" row in Layers.
- Skipped: vertical type, type mask, per-selection formatting (the option bar formats the whole layer), Faux Bold/Italic, tracking/leading UI, text thumbnails (the row shows "T"), the card's Tests.
- FAST: the live edit mutates the document outside history and the commit restores a whole-document snapshot (anything else changed during typing is lost); every keystroke marks the whole canvas dirty; one process-wide layout cache keyed by layer id; hit-testing assumes `walk` is bottom → top; `expect` on the font mutex.
- VERIFY: anti-alias labels → renderer; history label "Type Tool" for a new layer (Photoshop may name it differently); default font when the bar's font is not installed (parley fallback).
- Try it: T, click on the canvas, type, Ctrl+Enter. Click on the text again to edit. Double-click the T thumbnail in Layers. Layer ▸ Rasterize ▸ Type.

## M6-T08 — Layer styles  (Claude, 2026-09-26, c7f1dc3)
- Done: `fx_core::styles` (Drop Shadow, Outer Glow, Inner Shadow, Color Overlay, Stroke; `Document::global_light`), `Command::SetLayerStyle`, per-effect derived caches on the layer, effect tiles computed on demand in `fx-engine/src/effects.rs` (shift, EDT dilate/erode, 3-box blur, stroke rings, colour baked in), program emits shadow/glow under the content and the rest over it (effects use opacity, not fill), level-0 preparation for export/merge/flatten, `.fxd` save/load, `LayerInfo.styles`, live dialogs for the five effects + Blending Options (`ui/js/native/styles.js`), Copy/Paste/Clear Layer Style, "fx" marker in the row.
- Skipped: styles on groups, per-effect eye toggles under the row, Global Light dialog, noise/contour/knockout options, effects on clipped layers, the card's Tests.
- FAST: every content change marks every effect cache of every styled layer dirty; interior effects are composited over the backdrop+content instead of inside the layer (not true source-atop); inner shadow dialog always uses its own angle; the layer mask shapes the effects.
- VERIFY: Photoshop's order and blend of effects, spread/choke semantics (percent of size = dilation part), blur σ = size/2, default global light 120°, stroke anti-aliasing.
- Try it: Layer ▸ Layer Style ▸ Drop Shadow… on a shape or text layer; set Fill to 0 in Blending Options — the shadow stays.

## M6-T09 — UI  (Claude, 2026-09-26, a39e34a)
- Done: Free Transform bar X/Y/W/H/Angle fields (empty = keep), double-click on a text thumbnail edits it with everything selected; style dialogs and Blending Options live (T08). Image Size / Canvas Size / Rotate Arbitrary were already live (T02).
- Skipped: the transform fields are not updated from the box while dragging (the status bar shows the numbers); `[`/`]` for type size; shape/text real thumbnails.
- FAST: numeric entry turns the box into a rotated rectangle about its centre (skew/perspective lost), and re-sending the bar (any click on it) re-applies the typed values.
- VERIFY: none.
- Try it: Ctrl+T on a layer, type 50 in W:.

## M6-T10 — Acceptance: deferred to HARDEN (S16–S20, Photoshop comparisons).

## M7-T00 — Decisions  (Claude, 2026-09-26, a2dccfd)
- Done: the card's five recommendations recorded as fast defaults D-058..D-062.
- Skipped: none. FAST: none. VERIFY: none.
- Try it: `docs/DECISIONS.md`.

## M7-T01 — File ▸ New  (Claude, 2026-09-26, f3a7c23)
- Done: `doc:new` action → `OpenDoc::blank` (Solid tiles, or Empty + "Layer 1" for Transparent), Untitled-N, clean; native dialog with presets (`ui/js/native/newdoc.js`); Ctrl+N and the tab "+".
- Skipped: the Clipboard preset (shell does not report the clipboard size), name field, colour profile choice (always sRGB), units other than px.
- FAST: the preset fills the number boxes by poking the DOM.
- VERIFY: none.
- Try it: Ctrl+N, 30 000² preset.

## M7-T02 — Move tool  (Claude, 2026-09-26, 1cdedf4)
- Done: `tools/move_tool.rs` + `Command::OffsetLayers` (groups move their content, shape/text translate their matrix, locked refuse, "Move"); live drag outside history, one step at the drop; pixel selection → whole-pixel `Transform`; Shift 0/45/90°; Alt duplicates; Ctrl or Auto-Select (Layer/Group) picks the top opaque layer from its level-0 tiles; arrow nudges merged.
- Skipped: Show Transform Controls (box + handle drag), live preview of a selection move.
- FAST: Alt+drag is two history steps; a burst of nudges relies on the engine's 1 s merge window; auto-select ignores shape/text tiles never drawn at level 0.
- VERIFY: nudge history label.
- Try it: V, drag a layer; Ctrl+drag over another layer.

## M7-T03 — Place Embedded  (Claude, 2026-09-26, 9c3973f)
- Done: `EngineInput::Place` (menu Place Embedded/Linked via the shell's dialog, and drops): import job → pasted centred as a layer named after the file → Free Transform box, pre-scaled to fit when larger than the canvas.
- Skipped: placing a `.fxd` (it opens instead), Escape removing the placed layer, one "Place" history step (it is Paste + Rename + Transform).
- FAST: the clipboard is borrowed for the paste and restored.
- VERIFY: none.
- Try it: drop a PNG on an open document.

## M7-T04 — Arrange, Align, Distribute  (Claude, 2026-09-26, 05a4df8)
- Done: `order:*` within the parent; `align:*` / `dist:*` on exact content bounds against the selection / canvas (one layer) / union, one `MoveEach` step; Move bar buttons.
- Skipped: `dist:hspace|vspace` (equal gaps), groups and fill layers in align.
- FAST: `MoveEach` is not all-or-nothing on a refusal half-way; the bounds are computed on the engine thread (no job).
- VERIFY: MoveLayer's index semantics for "forward/backward" (assumed final index).
- Try it: select three layers, Layer ▸ Distribute ▸ Horizontal Centers.

## M7-T05 — Mask and Layer menus  (Claude, 2026-09-26, d83c25b)
- Done: mask reveal-all/hide-all/delete/apply, disable and link toggles (`SetMaskFlags`), Layer from Background, File ▸ Revert.
- Skipped: Background from Layer.
- FAST: Revert undoes every history step (exact only while the history kept them all).
- VERIFY: none.
- Try it: Layer ▸ Layer Mask ▸ Disable.

## M7-T06 — Guides, grid and snapping  (Claude, 2026-09-26, 8909afe)
- Done: `Document::guides` (in `.fxd`), `SetGuides` (history, not dirty); guides from a ruler drag, New Guide, New Guide Layout, Clear; engine-drawn guides and grid over the visible rect (`fx-engine/src/snap.rs`) from the UI's View flags; snapping of marquee/crop/shape/move/type/transform pointers to guides, grid and canvas edges/centre (8 screen px).
- Skipped: dragging or deleting a single guide, guide/grid colours from prefs, pixel grid in the app, snapping to layer bounds and selection edges.
- FAST: the Move tool snaps the pointer, not the moved box's edges.
- VERIFY: snap radius.
- Try it: drag from the top ruler; View ▸ Show ▸ Grid.

## M7-T07 — Zoom tool  (Claude, 2026-09-26, 8424af6)
- Done: Zoom tool click in/out about the point, scrubby drag; double-click Hand = Fit, Zoom = 100 %.
- Skipped: rectangle zoom (Scrubby off).
- FAST: none. VERIFY: scrubby speed (1 % per px).
- Try it: Z, click; Alt+click; drag right.

## M7-T08 — Tool kinds  (Claude, 2026-09-26, 15a33b8)
- Done: `fx_ops::brush::DabOp` (Paint, Erase, CloneSource, Veil; `op_for`), `StatefulDabOp` declared; `tools/kinds.rs` ClickTool/DragTool adapters + `registered`; HOWTO R11. The 18 brush tests pass after the refactor.
- Skipped: DragTool live preview, a toy op, the stored-hash test.
- FAST: none. VERIFY: none.
- Try it: nothing visible — M8 builds on it.

## M7-T09 — Menu honesty and Preferences  (Claude, 2026-09-26, fe8cad3)
- Done: `ui/tools/gen-implemented.mjs` → `js/data/implemented.js`; in the app unimplemented menu items are greyed with "Planned for Mx"; `check-data` fails when the list is stale. `fx-engine/src/prefs.rs` owns `%APPDATA%\Fotox\preferences.json` (grid, memory budget, scratch folder at start, recent files); Open Recent, Clear Recent; Preferences dialogs (Performance, Guides & Grid).
- Skipped: scratch folder editing, units, guide/grid colours, the other preference pages.
- FAST: the implemented list is scanned from string literals, so an id that appears in the engine only as a refusal counts as implemented; the Planned milestone is per action prefix.
- VERIFY: none.
- Try it: open a file, then File ▸ Open Recent; hover a greyed item.

## M7-T10 — Acceptance: deferred to HARDEN (S21–S23, the working-day checklist).

## M8-T00 — Decisions  (Claude, 2026-09-26)
- Done: the card's five recommendations recorded as fast defaults D-063..D-067.
- Skipped: none. FAST: none. VERIFY: none.
- Try it: `docs/DECISIONS.md`.

## M8-T01 — Brush tips, dynamics and presets  (Claude, 2026-09-26)
- Done: sampled tips (`fx_ops::brush::tip::SampledTip`, one master + box pyramid, registry by 52-bit hash id, `BrushParams.tip`); `Dynamics` in `BrushParams` (shape: size/angle/roundness jitter + Fade/Pressure/Tilt controls and minimums; scattering with count; transfer; colour dynamics) applied by `DabPath::emit`, seeded by `BrushParams.seed` set at pen-down, so live = replay; colour dynamics (`brush::color_dynamics`) jitter the stroke colour once; `.abr` v6+ sampled-tip import (`fx-io/src/abr.rs`, raw + PackBits); brush library `%APPDATA%\Fotox\brushes.json` (`fx-engine/src/brushes.rs`) with defaults, thumbnail strokes drawn by the dab placer + tips, `EngineToUi::Brushes`; `brush:*` actions (import-abr / save / rename / delete) in `engine/m8.rs`; File ▸ Open or drop of an `.abr` imports it; Brushes and Brush Settings panels live (`ui/js/native/brush-settings.js`), sent to the engine as `_brush` in every tool's options.
- Skipped: Photoshop's Smoothing modes other than the pulled string (Catch-up / Adjust for Zoom); the ABR `desc` section (names, spacing, dynamics of the file); computed tips' Flip X/Y; texture / dual brush / wet edges / noise; Tests (should check: a sampled tip stamps its image; live = replay with jitter; smoothing keeps a straight line straight; an ABR fixture imports).
- FAST: sampled tips live in a process-wide registry, a macro replayed in another session paints round; colour dynamics per stroke even with "Apply Per Tip"; opacity jitter acts like flow; one Brush Settings state shared by every painting tool; `BrushParams::clamped` does not clamp the dynamics; ABR reading and brushes.json writing happen on the engine thread.
- VERIFY: jitter distributions (uniform), hue jitter range (±180° at 100 %), the ABR skip sizes (47/301 bytes, from GIMP).
- Try it: Window ▸ Brush Settings, raise Size Jitter and Scatter, paint with B. Drop an `.abr` on the window, pick a tip in the Brushes panel.

## M8-T02 — Paint Bucket, Magic Eraser, Background Eraser  (Claude, 2026-09-26)
- Done: `Command::BucketFill` (wand flood ∩ selection, then `pixels::fill` of the colour or `pixels::fill_with` of a document pattern; mode, opacity, lock transparency) and `Command::MagicErase` (flood ∩ selection → `pixels::clear`; a Background layer becomes "Layer 0" in the same step), both run as jobs like the wand; `tools/bucket.rs` ClickTools registered in `kinds::registered` ("paint-bucket", "eraser-magic"); `kinds::blend_mode` parses a Mode drop-down. Background Eraser: `StrokeTool::BgEraser` + `brush::ops::background_eraser` DabOp (`alpha × (1 − k × match)`, soft 4-level edge, Protect Foreground Color), sampling Once / Background Swatch, Background → layer before the stroke. Groundwork for T06: `fx_core::pattern::Pattern`, `Document::patterns`, `fx_core::fill::FillSource`, `pixels::fill_with` (positional fill); option-bar `registerControl` hook for the pattern / gradient pickers.
- Skipped: Background Eraser limits Contiguous / Find Edges (all act as Discontiguous); continuous sampling (samples once at the press); Magic Eraser opacity; Tests (should check: a bucket fill on a two-colour image stops at the edge; non-contiguous fills both regions; the magic eraser leaves transparency and un-Backgrounds; the background eraser keeps the protected colour).
- FAST: the flood reads the *active* layer (the tools always send `LayerRef::Active`); magic erase on a transparency-locked layer is refused instead of painting the background colour; Background → layer for the background eraser is a separate History step.
- VERIFY: the match edge softness; Photoshop's bucket anti-alias on the flood edge.
- Try it: G's flyout ▸ Paint Bucket, click a flat area. E's flyout ▸ Magic Eraser / Background Eraser.

## M8-T03 — Gradient tool and Gradient Fill layers  (Claude, 2026-09-26)
- Done: `fx_core::gradient` (colour + opacity stops with midpoints, Perceptual = Oklab / Linear = linear light / Classic = sRGB, five geometries, Reverse, Transparency, 4 × 4 Bayer dither of ±½ level); `Command::FillGradient` through the selection with mode / opacity / lock transparency, as a job (`pixels::fill_with`); Gradient tool = `DragTool` (`tools/gradient.rs`, option bar Type / Method / Mode / Opacity / Reverse / Dither / Transparency, "fg"/"bg" stops resolved at release). Fill layers: `LayerKind::FillLayer { content: fill::FillLayer, cache }` (one variant for gradient and pattern), `NewLayer::Fill`, `Command::SetFillLayer`, derived tiles drawn per level by `fx_render::gradient::render_fill_tile` through the shape request path, compositor op, `.fxd` save/load, `LayerInfo.fill_layer`, Rasterize ▸ Fill Content / Layer. UI `ui/js/native/gradients.js`: gradient picker in the option bar (presets + Edit…), Gradient Editor (stops, midpoints, opacity stops, method), New Fill Layer ▸ Gradient and Layer Content Options / double-click on the fill thumbnail to edit.
- Skipped: live preview of the drag (line overlay only); the Properties panel for fill layers (the dialog is used instead); gradient presets saved by the user; noise gradients; "Align with layer"; Tests (should check: linear endpoints exact; radial symmetric; dithered 8-bit has no bands wider than the period; fill layer at level 2 ≈ level 0 downsampled within 2/255; undo).
- FAST: coarse levels of a fill layer sample one point per pixel (no averaging); the gradient editor is lists of numbers, not Photoshop's draggable stops; a fill layer bakes "fg"/"bg" colours when created.
- VERIFY: midpoint curve, Perceptual space, dither pattern, Angle direction, the fill layer's Scale meaning (fraction of the canvas extent along the angle).
- Try it: G, drag on a pixel layer; Layer ▸ New Fill Layer ▸ Gradient…, then double-click its thumbnail.
