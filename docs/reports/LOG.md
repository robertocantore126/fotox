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

## M8-T04 — Dodge, Burn, Sponge  (Claude, 2026-09-26)
- Done: `StrokeTool::{Dodge, Burn, Sponge}` + `brush::ops::tone` (`Tone`, `Sponge` DabOps): Range weights (Shadows `(1−v)²`, Midtones `4v(1−v)`, Highlights `v²`), dodge/burn by half the weighted strength, Protect Tones on luminance with a saturation clamp, Sponge saturate/desaturate about the grey with Vibrance weighting by unsaturation; Exposure / Flow are the stroke's flow at 100 % opacity; `DabOp::gray` so a mask target dodges/burns its grey and the sponge leaves it alone; tools "dodge", "burn", "sponge" wired with Photoshop's option bars (+ Hardness).
- Skipped: Sponge's skin-tone protection; Tests (should check: midtone dodge lifts 50 % grey more than black/white; Protect Tones keeps hue; desaturate reaches grey; vibrance protects a saturated pixel).
- FAST: none beyond the `// FAST:` marks.
- VERIFY: every formula (D-064), Photoshop's default Protect Tones (on), Exposure as flow.
- Try it: O, paint over a photo's midtones; Shift+O cycles to Burn / Sponge.

## M8-T05 — Blur, Sharpen, Smudge  (Claude, 2026-09-26)
- Done: `StrokeTool::{Blur, Sharpen, Smudge}`. Blur / Sharpen lay down a filtered source through the source window: `brush::ops::focus::FilteredTiles` filters the layer (or the composite with Sample All Layers) per canvas tile with a kernel apron (Gaussian σ 2, unsharp ×1.2 or ×0.6 with Protect Detail), cached per stroke; Strength = flow at 100 % opacity; the Mode drop-down is the blend mode. Smudge: a new sequential path in the stroke engine, `brush::op::DabSequence` (the dab's rectangle of the *current* pixels, dab by dab; lock transparency kept) — `ops::smudge` carries the previous dab's result to the new position (Finger Painting starts with the foreground). Tools "blur", "sharpen", "smudge" wired.
- Skipped: accumulation of Blur/Sharpen within one stroke (one pass per stroke), Smudge's Sample All Layers, Tests (should check: blur flattens a step monotonically with strength; sharpen raises edge contrast, flat stays flat; smudge drags a colour and fades with strength < 1; live = replay for the three).
- FAST: `DabSequence` runs on one thread; M7's `StatefulDabOp` is left unused (the sequence trait replaces it); the smudge offset is rounded to whole pixels.
- VERIFY: kernel size, sharpen amount, smudge strength curve.
- Try it: R, scrub over an edge; Shift+R to Sharpen / Smudge.

## M8-T06 — Patterns and the Pattern Stamp  (Claude, 2026-09-26)
- Done: `fx_core::pattern::Pattern` (≤ 2048² RGBA16, id = hash) in `Document::patterns`, saved in the `.fxd` manifest; the user library `%APPDATA%\Fotox\patterns.json` (`fx-engine/src/patterns.rs`, generated defaults, PNG import) sent as `EngineToUi::Patterns`; Edit ▸ Define Pattern (canvas or selection bounds, composite of the visible layers) → library + `Command::DefinePattern` (history, not dirty); `Command::FillPattern` and Edit ▸ Fill "Use: Pattern"; Paint Bucket's pattern source; `StrokeTool::PatternStamp` (source window of `brush::ops::pattern::PatternTiles`, Aligned = anchored at the canvas origin, else at the stroke start; Impressionist jitter); pattern fill layers (`FillLayer::Pattern { pattern, scale, angle }`, bilinear per level); a library pattern is copied into the document before a command or stroke reads it. UI `ui/js/native/patterns.js`: Patterns panel (Window ▸ Patterns: pick, rename by double-click, define from selection, import PNG, delete), the option bars' pattern picker, New Fill Layer ▸ Pattern and its Content Options.
- Skipped: `.pat` import, pattern export, Tests (should check: a pattern tiles seamlessly across tiles; aligned stamping continues between strokes; the fill layer round-trips through `.fxd`).
- FAST: manifest patterns are JSON number arrays (big for large patterns); library patterns are copied into the document outside History; Define Pattern composites on the engine thread and prepares every dirty shape tile first; the picker keeps a refresh callback per option-bar render.
- VERIFY: Impressionist (Photoshop's is a painterly blotch, not a jitter).
- Try it: select an area, Edit ▸ Define Pattern from Selection; S's flyout ▸ Pattern Stamp; Edit ▸ Fill ▸ Use: Pattern; Layer ▸ New Fill Layer ▸ Pattern…

## M8-T07 — History Brush, Art History Brush  (Claude, 2026-09-26)
- Done: `History::state(row, current)` reads any History panel row's snapshot (D-067); the panel's source column (`hist:source` → `EngineToUi::HistorySource`); `StrokeTool::HistoryBrush { state }` = the same layer in that state as the source window at offset 0 (through Mode / Opacity / Flow), `StrokeTool::ArtHistory { state, style, area, tolerance }` = a `DabSequence` (`brush::ops::history`) that spawns seeded short strokes per dab (ten styles: count, length, curl, looseness) coloured from the state, skipping pixels within the tolerance; Photoshop's refusals when the state has no such layer or another canvas size. Tools "history-brush", "art-history" wired.
- Skipped: History snapshots (Photoshop's named snapshots); painting a mask with them; Tests (should check: History Brush from the first state after a Levels restores the pixels; the size-mismatch refusal; an Art History stroke is deterministic for a seed).
- FAST: the source row is an index into the panel, so once History drops its oldest steps (limit 50) the rows shift under it; `Command::Stroke` of these tools cannot be replayed outside the engine (the command context has no History), which only matters for macros.
- VERIFY: Art History stroke shapes and counts; the refusal wording.
- Try it: paint, apply a filter, click the brush icon left of "Open" in History, Y and paint.

## M8-T08 — Color Replacement and Red Eye  (Claude, 2026-09-26)
- Done: `StrokeTool::ColorReplace` + `brush::ops::replace` (the foreground blended by Hue / Saturation / Color / Luminosity through `blend::composite` with alpha kept, weighted by the sample match); Sampling Once / Background Swatch; tool "color-replace". Red Eye: `Command::RedEye { point, pupil_size, darken }` (written with T03's commands in `command/m8.rs`): reads a ±120 px window, seeds at the reddest pixel within 15 px of the click (`r − max(g, b)`, alpha-weighted), floods the red blob (the pupil size lowers the threshold), feathers with two 3 × 3 box passes, sets red to the green-blue mean and darkens; `RedEye` ClickTool "red-eye".
- Skipped: Color Replacement Limits Contiguous / Find Edges and continuous sampling; Tests (should check: replacing a red area's hue with blue keeps luminosity; Red Eye turns a synthetic red pupil dark and leaves the iris).
- FAST: the red-eye window is a fixed ±120 px; it writes back every tile of the window.
- VERIFY: the red-eye formula and threshold; Color Replacement's match edge.
- Try it: B's flyout ▸ Color Replacement, paint over a coloured area; J's flyout ▸ Red Eye, click a red pupil.

## M8-T09 — Mixer Brush  (Claude, 2026-09-26)
- Done: `StrokeTool::Mixer { wet, load, mix }` + `brush::ops::mixer` (a `DabSequence`, D-066 **approximation**): the reservoir (foreground, an amount Load spends dab by dab), the pickup (the previous dab's paint moved to the brush, mixed with the canvas by Wet), each dab lays `mix(reservoir, pickup, Mix)` by the dab coverage (Flow); Photoshop's preset combinations (Dry … Very Wet, Heavy Mix) and Custom Wet / Load / Mix in the option bar; tool "mixer-brush" (the old "not implemented" test now expects it).
- Skipped: Load / Clean Brush menus and the "after each stroke" toggles (always load + clean per stroke), Sample All Layers, the current-brush-load swatch, Tests (should check: Dry with a loaded reservoir paints the reservoir colour; Very Wet mixes a two-colour field along the path; clean after each stroke resets).
- FAST: the reservoir depletion rate is a guess (2 % × (1 − Load) per dab).
- VERIFY: the whole model and the preset values (D-066).
- Try it: B's flyout ▸ Mixer Brush, Preset "Very Wet", drag across two colours.

## M8-T10 — UI  (Claude, 2026-09-26)
- Done: option bars of every M8 tool (Photoshop's fields: Gradient Type / Method, bucket Anti-alias / Contiguous / pattern, Background Eraser and Color Replacement Sampling, Dodge/Burn Protect Tones, Sharpen Protect Detail, Pattern Stamp pattern, Art History Style / Area / Tolerance, Mixer presets + Wet / Load / Mix, Hardness where missing); the option bar's `registerControl` pickers (gradient, pattern); Brushes and Brush Settings panels, Gradient Editor, Patterns panel, the History panel's source column (T01–T07); `gen-implemented.mjs` now also lists the engine's tools as `tool:<id>`, and in the app the flyouts dim the tools the engine does not build yet ("planned for a later milestone").
- Skipped: a Properties panel for fill layers (their dialog instead); the Tool Presets panel stays a mock.
- FAST: the UI-only tools (hand, zoom, rotate view, quick mask, screen) are a hand-written list in `main.js`.
- VERIFY: none.
- Try it: open the toolbar flyouts in the app — only Quick Selection, Object Selection, Magnetic Lasso, Custom Shape and the rest of M9+ are dimmed.

## M8-T11 — Acceptance: deferred to HARDEN (S24–S26, the tool-by-tool comparison with Photoshop feeding M14's VERIFY list).

M8 note for HARDEN (Claude, 2026-09-26): `cargo test -p fx-core` has 10 failures that are already on `main` (M6 shape/text/rotate tests: rotate needs the engine's ops in fx-core tests, shape crop/dirty-tile expectations, "Type 1" naming, two vector tests); `cargo test -p fx-engine --lib` aborts (SIGABRT) in a GPU-less Linux container on `main` too. `fx-ops` (78) passes. M8 changed one test: `every_name_kind_has_a_counter` (two name kinds appended).

## M9-T00 — Decisions  (Claude, 2026-09-26)
- Done: the card's five recommendations recorded as fast defaults D-068..D-072.
- Skipped: none. FAST: none. VERIFY: none.
- Try it: `docs/DECISIONS.md`.

## M9-T01 — Channels, Save / Load Selection, Quick Mask  (Claude, 2026-09-26)
- Done: `fx_core::channel::Channel` in `Document::channels` (canvas-aligned grey, colour, opacity; `canvas_aligned` rewrites a selection tile by tile), saved in the `.fxd` (manifest `channels`, tiles written with the layers'); commands `SaveSelection` (new channel or Replace / Add / Subtract / Intersect into one), `LoadSelection` (Invert + the four modes), `DeleteChannel`, `DuplicateChannel`, `SetChannel`; `EngineToUi::Channels` with 48² thumbnails, resent when the list's signature changes; Channels panel (`ui/js/native/channels-panel.js`: RGB rows, alpha channels, Ctrl(+Shift/Alt)+click thumbnail = load, double-click = rename, load / save / duplicate / delete buttons) and native Save / Load Selection dialogs. Quick Mask: `Command::QuickMask { on }` + Q (`sel:quick-mask`): the selection becomes a red 50 % "Quick Mask" solid-fill layer on top whose mask is the unselected amount, painting targets that mask with the grey inverted (black adds red, as in Photoshop), Q again turns the mask back into the selection. Also added now for later cards: `Document::annotations`, `Command::{SelectBy, TransformSelection, SetAnnotations, PerspectiveCrop}`, `PixelOps::select_op`.
- Skipped: showing R / G / B (or an alpha channel) as grey in the viewport and the channel eye toggles; drag rows onto the buttons; Quick Mask Options (colour, opacity, masked vs selected); a new empty channel; Tests (should check: save then load = same coverage; the four modes; channels round-trip through `.fxd`; Quick Mask paint then exit = the painted selection).
- FAST: Quick Mask is a real layer (export / merge / flatten see it while it is on; it is found by its name); thumbnails point-sample level 0; the channel signature is a string of the first slots.
- VERIFY: none.
- Try it: make a selection, Select ▸ Save Selection; Deselect; Ctrl+click the thumbnail in Channels. Press Q, paint black / white, press Q.

## M9-T02 — Grow, Similar, Transform Selection  (Claude, 2026-09-26)
- Done: `fx_ops::select` (M9's selection algorithms: `assemble` builds a selection one row of tiles at a time, `Windows` reads apron windows across tiles); `select::grow` — the selected colours quantised into a 32³ table dilated by the tolerance; Similar = every matching pixel, Grow = a flood across tiles over the matches from the selection's boundary pixels; both keep the old selection. `Command::SelectBy { select: SelectOp, mode }` + `PixelOps::select_op` (engine `EngineOps::select`, sources = active layer or composite) run as jobs; `sel:grow` / `sel:similar` use the Magic Wand's Tolerance and Sample All Layers. Transform Selection: `sel:transform` puts M6-T04's box over the selection's bounds (`Session::selection`), Enter commits `Command::TransformSelection` (the canvas-aligned coverage resampled, pixels untouched, "Transform Selection").
- Skipped: the live preview of the transformed ants (the box only); Tests (should check: Grow stops at an edge; Similar selects a separate same-coloured region; Transform Selection rotates the coverage and leaves the pixels).
- FAST: the colour test is quantised to 32 levels per channel; Grow's flood runs on one thread with a bitset per reached tile.
- VERIFY: Photoshop's Grow / Similar distance.
- Try it: magic-wand a sky patch, Select ▸ Grow, Select ▸ Similar; Select ▸ Transform Selection, rotate, Enter.

## M9-T03 — Color Range  (Claude, 2026-09-26)
- Done: `SelectOp::ColorRange` + `fx_ops::select::range` (soft coverage per pixel, per tile, row by row): Sampled Colors (`1 − d / fuzziness`, nearest sample, optional localized fade), Reds … Magentas (±30° hue windows × saturation), Highlights / Midtones / Shadows (luminance against split points with 20-level ramps), Skin Tones (a hue / saturation / luminance box), Invert; runs on the composite as a job ("Color Range"). Native dialog in `ui/js/native/selections.js` (with Focus Area and Select and Mask).
- Skipped: the dialog's live preview modes (Selection / Grayscale / Black / White Matte / Quick Mask — the result shows after OK), the image eyedropper with +/− samples (the swatches are the samples), Localized Color Clusters' centre, Out of Gamut, Detect Faces (M13); Tests (should check: sampled red on a hue ramp selects a band growing with fuzziness; Highlights pick the bright end; localized clusters drop the far region).
- FAST: samples come from the foreground / background swatches.
- VERIFY: every curve (D-064).
- Try it: set the foreground to a colour in the image, Select ▸ Color Range…, Fuzziness 60.

## M9-T04 — Focus Area  (Claude, 2026-09-26)
- Done: `SelectOp::FocusArea` + `fx_ops::select::focus` (per tile with a 16 px apron: luminance detail at σ 1 and 2 as a DoG stand-in for the LoG, 9 × 9 energy, noise floor, `e / (e + 0.02)` normalisation, soft threshold at `1 − In-Focus Range`, Soften Edge = σ 2 blur), on the composite, as a job; dialog with In-Focus Range, Image Noise Level, Soften Edge. Shared helpers `focus::blur` / `box_mean`.
- Skipped: the dialog's add / subtract brushes, preview and view modes, output to mask / new layer (use Select and Mask's output or Layer ▸ Layer Mask afterwards); Tests (should check: sharp-left / blurred-right synthetic selects the left half within a few pixels; the noise floor keeps a noisy flat area out).
- FAST: each tile reads a 288² window (apron) through a small tile cache.
- VERIFY: normalisation constant, threshold curve, Photoshop's slider meaning.
- Try it: a photo with a shallow depth of field, Select ▸ Focus Area…

## M9-T05 — Select and Mask  (Claude, 2026-09-26)
- Done: `SelectOp::Refine(Refine)` + `fx_ops::select::refine` (D-070): only tiles whose apron is not uniformly 0 or 1 are worked (a big document costs its boundary); in the band a grey guided filter (He et al.) of the coverage guided by the luminance, window = Radius, Smart Radius keeps more of the input on strong guide edges; then Smooth, Feather, Contrast, Shift Edge. Select ▸ Select and Mask… (Alt+Ctrl+R) dialog: Radius, Smart Radius, Smooth, Feather, Contrast, Shift Edge, Output To Selection / Layer Mask / New Layer with Layer Mask (the output is queued and made once the job is done: `select-mask:output`, `after_job_m9`).
- Skipped: Photoshop's modal workspace (view modes, Show Edge / Original, its Quick Selection / Refine Edge / Brush / Lasso tools), Decontaminate Colors, output to New Layer / New Document; Tests (should check: a soft-haired synthetic's band error halves; Shift Edge +50 % grows the selection).
- FAST: a dialog without preview instead of the workspace; Radius ≤ 64, Feather ≤ 50; Shift Edge only moves partially-selected pixels.
- VERIFY: guided-filter ε and window, the global refinements' curves.
- Try it: a rough lasso around hair, Select ▸ Select and Mask…, Radius 20, Output To Layer Mask.

## M9-T06 — Quick Selection tool  (Claude, 2026-09-26)
- Done: `SelectOp::QuickSelect { dabs, sample_all, enhance_edge }` + `fx_ops::select::quick` (D-069): the stroke's window (dabs' bounds + 4 radii, ≤ 1536²) read once; the seeds' mean and spread give a colour tolerance; a 4-connected flood from the pixels under the dabs stops where the luminance gradient exceeds an edge threshold; Auto-Enhance softens the border. Tool `tools/quick_select.rs` ("quick-select", W): dabs every half radius, brush circle + path overlay, one "Quick Selection" step at release — Replace for the first stroke, then Add, Alt (or the Subtract mode) subtracts; Size from the option bar (`[`/`]` as for brushes).
- Skipped: the live outline during the stroke (it updates at release); the coarse-level pass + boundary refinement of D-069; Select Subject (M13); Tests (should check: a stroke in a flat region bounded by a strong edge stops at the edge; Alt subtracts; live = replay).
- FAST: level-0 window capped at 1536²; the tolerance and edge threshold are constants.
- VERIFY: constants against Photoshop on Rob's photos (M9-T10).
- Try it: W, drag inside an object with clear edges; Alt+drag to remove.

## M9-T07 — Magnetic Lasso  (Claude, 2026-09-26)
- Done: `fx_ops::select::livewire` (Sobel gradient → cost `1 − g + 0.02` with Contrast ignoring weak edges; 8-connected Dijkstra between two window pixels); tool `tools/magnetic.rs` ("lasso-magnet"): click to start, the live wire from the last anchor to the strongest edge within Width of the pointer (the composite's luminance, tiles cached for the gesture), a click adds an anchor, Frequency adds anchors automatically (spacing `20 + (100 − f)·3` px), Backspace removes the last anchor, Enter or a double-click closes into one `Command::Select` polygon with the mode / feather / anti-alias of the bar, Escape cancels.
- Skipped: Alt switching to the freehand / polygonal lasso, pen pressure = width, a magnetic closing segment; Tests (should check: tracing near a synthetic disc closes on its edge within 1.5 px).
- FAST: the live wire runs on the engine thread per pointer move (window up to ~2048²; composite tiles rendered on first use); the closing segment is straight.
- VERIFY: cost function, Frequency spacing.
- Try it: L's flyout ▸ Magnetic Lasso, click on an edge and move along it, double-click to close.

## M9-T08 — Color Sampler, Ruler, Note, Count  (Claude, 2026-09-26)
- Done: `fx_core::annotations` (notes, count groups, samplers) in `Document::annotations`, saved in the `.fxd` (D-072), edited by `Command::SetAnnotations` steps; `tools/measure.rs`: Color Sampler ("sampler", ≤ 10 points, Alt+click removes, Clear All), Ruler ("ruler-tool": X / Y / W / H / angle / length on the status line and in Info, Shift = 45° steps, Straighten Layer rotates the active layer so the line is level, Clear), Note ("note-tool": click adds, Alt+click deletes, Author, Clear All), Count ("counting": click adds to the current group, Alt+click removes, New Group, Clear). `EngineToUi::Annotations` after every edit with the samplers' values from the composite averaged over Sample Size (Point … 101 × 101); Info panel (sampler RGB, ruler, counts) and Notes panel (edit / delete notes) in `ui/js/native/info-panel.js`. The option-bar buttons' actions reach the active tool as keys.
- Skipped: the protractor (Alt-drag from a ruler end), Straighten's crop, the sampler's second readout mode (HSB / Lab…), dragging samplers / notes, count labels and group colours / sizes, showing annotations while another tool is active, Tests (should check: a sampler's average; the ruler's angle; Straighten levels a tilted line; notes and counts round-trip through `.fxd`).
- FAST: annotations are drawn only while their tool is active, as crosshairs / handles without numbers; the samplers are re-read from the composite on the engine thread at every edit.
- VERIFY: Straighten's sign convention against Photoshop.
- Try it: I's flyout ▸ Color Sampler, click the image, open Window ▸ Info; the Ruler along a tilted horizon, Straighten Layer.

## M9-T09 — Perspective Crop  (Claude, 2026-09-26)
- Done: `Command::PerspectiveCrop { quad, width, height }` (in `command/m9.rs`: the inverse of M6-T01's rectangle→quad homography resamples every pixel layer and mask through `resample_document`, clipped to the new canvas, selection dropped, "Perspective Crop", run as a job); tool `tools/perspective_crop.rs` ("crop-persp"): drag a box, move each corner (convex only), drag inside to move it, 3 × 3 grid, Enter / ✓ commits with the bar's W × H or the quad's average sides, Escape / ✗ cancels.
- Skipped: Resolution, Front Image, Show Grid toggle; Tests (should check: a synthetic rectangle in perspective comes out axis-aligned within a pixel).
- FAST: Bicubic Automatic always; shape and text layers keep their geometry (only pixel layers and masks are resampled, like Crop's straighten).
- VERIFY: none.
- Try it: C's flyout ▸ Perspective Crop, drag over a photographed document, move the corners onto its edges, Enter.

## M9-T10 — Acceptance: deferred to HARDEN (S27–S29, Rob's photo comparisons of Color Range, Select and Mask and Quick Selection).

M9 note for HARDEN (Claude, 2026-09-26): the same 10 `fx-core` failures as before M8, plus `fx-io` `a_shape_layer_round_trips_through_a_file`, which also fails on the commit before M8 (76ab901). `fx-ops` (78) passes. No M9 test was added (fast mode).

## M10-T00 — Decisions  (Claude, 2026-09-26)
- Done: the four recommendations recorded as fast defaults D-073..D-076.
- Skipped: adding `i_overlay` itself (D-074 says why). FAST: none. VERIFY: none.
- Try it: `docs/DECISIONS.md`.

## M10-T01 — Paths and the Paths panel  (Claude, 2026-09-26)
- Done: `fx_core::path` (D-073: `Path` / `Subpath` / `Anchor` with in / out handles and smooth flag / `PathOp`; lossless `to_elements` / `from_elements`; flattening, bounds, hit tests, de Casteljau `split`, RDP `simplify`, Catmull-Rom `smooth_through`; `PathTarget` Work / Saved); `Document::{work_path, paths, active_path}` (the first two saved in `.fxd`); commands `SetPath`, `DeletePath`, `RenamePath`, `SaveWorkPath`, `SelectPath` (not a History step), `PathToSelection` (subpaths combined by their operation), `SelectionToPath` (the outline traced on a ≤ 1024² grid, simplified, smooth except at sharp turns), `FillPath` (colour or pattern, through the path's coverage); Stroke Path (engine: one `Command::Stroke` per subpath with the Brush / Pencil / Eraser option bar's brush, optional simulated pressure); `EngineToUi::Paths`; Paths panel (`ui/js/native/paths-panel.js`: Work Path + saved, select, Ctrl+click = load, rename, fill / stroke / load / make from selection / save or new / delete).
- Skipped: Schneider's curve fit (a traced grid + RDP + Catmull-Rom instead); Fill / Stroke Path dialogs (the panel buttons use the foreground and the Brush); path thumbnails; Tests (should check: a path round-trips through `.fxd`; selection → path → selection within a pixel; Stroke Path with the Brush equals painting the same points).
- FAST: path operations are winding approximations (D-074); Selection to Path traces at most 1024 cells per side (coarse for big selections).
- VERIFY: none.
- Try it: make a selection, Paths panel ▸ Make work path; Ctrl+click it; stroke it with the Brush.

## M10-T02..T05 — Pen, Curvature Pen, Freeform Pen, anchor tools, Direct Selection  (Claude, 2026-09-26)
- Done: `tools/pen.rs`, one tool type with seven kinds. **Pen** (P): click = corner, drag = smooth with symmetric handles, Alt-drag breaks the symmetry, Alt-click the last anchor drops its out handle, a click on the first anchor closes, Enter / Escape / a double-click / another tool finishes, rubber band to the pointer; Mode Path (into the selected path or a new Work Path, the bar's path operation on the new subpath) / Shape (a shape layer filled with the foreground) / Pixels (Work Path + Fill Path); Auto Add/Delete on the target path. **Curvature Pen**: clicks → Catmull-Rom smooth curve, double-click toggles a corner, drag a point to move it, Backspace removes the last, click the first point closes. **Freeform Pen**: a drag simplified within Curve Fit and made smooth except at sharp turns. **Add / Delete / Convert Anchor Point**: de Casteljau split keeps the curve; delete; click = corner, drag = smooth handles. **Direct Selection** (A's flyout): drag an anchor (with its handles) or the selected anchor's handles (Alt breaks the pair), arrows / Shift+arrows nudge, Delete removes; on the selected path, else the active shape layer's outline (a live shape becomes a path shape, with a toast), else the Work Path. Each edit is one step (`SetPath` or `SetShape`).
- Skipped: Ctrl switching to Direct Selection, the Freeform Pen's Magnetic option, marquee / Shift multi-selection of anchors, Path Selection on document paths (select / move / align subpaths, Merge Shape Components — needs D-074's crate); Tests (scripted clicks build the expected anchors; closing; Shape mode adds a layer; Alt breaks the symmetry; three curvature clicks give a smooth curve; a freehand circle fits within Curve Fit; adding an anchor keeps the curve; direct selection moves one anchor).
- FAST: a new drawing is one History step when finished (Photoshop records every anchor); Delete Anchor does not refit the neighbours; the target path is drawn only while a pen tool edits it; a shape edited by Direct Selection is stored in document coordinates with an identity matrix.
- VERIFY: handle behaviour against Photoshop (M10-T09, the logo test).
- Try it: P, click-drag a few anchors, click the first to close; A's flyout ▸ Direct Selection, drag an anchor; Paths panel shows the Work Path.

## M10-T06 — Vector masks  (Claude, 2026-09-26)
- Done: `Layer::vector_mask: Option<VectorMask { path, enabled, feather, density, cache }>` (the path in document coordinates, `cache` a derived grey coverage image); `Command::SetVectorMask` (`VectorMaskSpec` as data); the program uses it as the layer's mask (`SourceTile::VectorMask` → `VectorRequest { vector_mask: true }`, `1 − density` outside the path), drawn per tile and level by `fx_render::vector::render_vector_mask_tile` through the engine's derived-tile requests (`vector::draw_vector_mask_requests`); `.fxd` save/load (`LayerEntry.vector_mask`); `LayerInfo.vector_mask`; Layer ▸ Vector Mask ▸ Reveal All / Hide All / Current Path / Delete / Disable-Enable / Edit Path; a second (pen) thumbnail in Layers (Shift+click disables, double-click edits).
- Skipped: multiplying with a pixel mask (with both, only the pixel mask applies); Feather; Rasterize Vector Mask; Ctrl+click = load as selection; vector masks on groups; Properties' Density / Feather fields; Tests (should check: level 2 ≈ level 0 downsampled within 2/255; multiplication with the pixel mask; `.fxd` round trip).
- FAST: Edit Path copies the mask's path into the Work Path (edit it, then Vector Mask ▸ Current Path to apply); the thumbnail's actions act on the active layer; canvas-size / crop / rotate do not move vector mask paths.
- VERIFY: none.
- Try it: draw a closed path with P, select a layer, Layer ▸ Vector Mask ▸ Current Path.

## M10-T07 — Triangle, Custom Shape  (Claude, 2026-09-26)
- Done: `VectorShape::Triangle { w, h, radius }` (corners rounded by a quadratic through each corner; "Triangle N" names) and the Triangle tool ("shape-triangle", Radius); the custom shapes library (`fx-engine/src/shapes_lib.rs`, D-076: eight built-ins in a unit box + the user's `%APPDATA%\Fotox\shapes.json`), put into the tool settings as `_custom_shapes` and sent as `EngineToUi::Shapes`; the Custom Shape tool ("shape-custom", the option bar's Shape picker in `ui/js/native/shapes-lib.js`) scales the chosen path into the drag's box as a path shape; Edit ▸ Define Custom Shape (`misc:define-shape`) stores the selected path / Work Path, normalised.
- Skipped: Live Shape Properties in Properties (W / H / X / Y, radii, sides, star ratio), stroke options UI (caps, joins, dashes — the renderer has dashes), Line arrowheads, gradient / pattern paint for shapes, Tests (a triangle's corners; a custom shape round-trips; dash lengths; a gradient fill equals the renderer clipped).
- FAST: custom shapes are stored in documents as plain path shapes (no link to the library).
- VERIFY: Photoshop's triangle corner rounding (arcs, not quadratics).
- Try it: U's flyout ▸ Custom Shape, pick Heart, drag. Draw a path with P, Edit ▸ Define Custom Shape.

## M10-T08 — Type completion  (Claude, 2026-09-26)
- Done: Warp Text — `TextContent::warp` / `LayerKind::Text.warp` (`Warp { style, bend }`: Arc, Arch, Bulge, Squeeze, Flag, Wave, Fish, Rise), applied to every glyph outline over the glyphs' box at layout time (lossless: the text stays editable), Type ▸ Warp Text… dialog (`type:warp` → `SetText`), `.fxd` via the content; `TextLayout::outline_elements` (glyph outlines in document coordinates) behind `PixelOps::text_outline`; Type ▸ Create Work Path (`TextToWorkPath`), Type ▸ Convert to Shape (`TextToShape`: the layer becomes a path shape of its outlines, first run's colour); the Horizontal / Vertical Type Mask tools ("type-mask", "type-mask-vertical": `TypeTool::mask`, the commit is `TextToSelection` — all contours as one nonzero polygon separated by non-finite points, so counters stay open).
- Skipped: vertical type (D-075) — the Vertical Type tool stays planned and the vertical mask tool types horizontally; type on a path and area text; live Character / Paragraph panels (tracking, kerning, baseline shift, faux styles, caps, indents, spacing); Warp's Horizontal / Vertical and the distortions; Tests (vertical glyph positions; a type mask's coverage; text on a circle; warp preset hashes; Convert to Shape = the rendered text).
- FAST: warp moves Bézier control points (no refit); Convert to Shape keeps one colour; Arc and Fish curves are my approximations.
- VERIFY: the warp shapes against Photoshop's presets.
- Try it: type some text, Type ▸ Warp Text…, Arc, Bend 50. Type ▸ Convert to Shape. T's flyout ▸ Horizontal Type Mask, type, Ctrl+Enter.

## M10-T09 — Acceptance: deferred to HARDEN (S30–S31, the logo redraw and the vertical Japanese paragraph).

## M11-T00 — Decisions  (Claude, 2026-09-26)
- Done: the five recommendations recorded as fast defaults D-077..D-081 (D-079 notes the fast-mode MLS solver).
- Skipped: none. FAST: none. VERIFY: none.
- Try it: `docs/DECISIONS.md`.

## M11-T01 — PatchMatch core  (Claude, 2026-09-26)
- Done: `fx_ops::patchmatch::fill(pixels, w, h, hole, sampling, params)` — a pyramid down to where the hole is a few patches wide, NNF upsampled from the coarser level (random at the coarsest, which starts from the mean sample colour), PatchMatch propagation + random search with alternating scan order, EM voting (rayon) per scale; seeded splitmix RNG, so a seed gives the same result. Patch 7 by default.
- Skipped: the card's file split (one file `patchmatch.rs`), colour adaptation, rows-parallel NNF search, Tests (periodic texture fill, same seed ⇒ same result, memory budget on a 20 000² image).
- FAST: search is single-threaded; votes are unweighted (no distance / distance-to-boundary weights); the caller owns the ROI budget (buffer passed in).
- VERIFY: none.
- Try it: through M11-T02 (Edit ▸ Content-Aware Fill).

## M11-T02 — Content-Aware Fill, Spot Healing Content-Aware  (Claude, 2026-09-26)
- Done: `Command::ContentAwareFill { layer, output: Current | NewLayer | Duplicate, seed }` (`fx-core/src/command/m11.rs`): the hole is the selection, the ROI is its tight bounds plus an Auto band (¾ of the hole's size, ≥ 48 px), read tile by tile and box-averaged onto a working grid of ≤ 768² pixels, filled through `PixelOps::patch_fill` (→ `fx_ops::patchmatch`), written back only where the selection covers (mixed by coverage, bilinear from the grid when it was reduced). Edit ▸ Content-Aware Fill… (dialog: Output To, Seed) and Edit ▸ Fill ▸ Use: Content-Aware (engine `engine/m11.rs`). The Spot Healing Brush's Type option (Content-Aware, the default, or Proximity Match): `StrokeTool::SpotHealContentAware` fills the stroke's window from a 1.5-diameter band by PatchMatch, then the D-045 healing blend. The Patch and Content-Aware Move commits (T03, T04) are in the same file.
- Skipped: the workspace (painted sampling area, live preview, Colour / Rotation / Scale adaptation, Mirror), Sample All Layers, grey layers, Tests.
- FAST: a hole larger than the budget is filled at a coarser scale and upsampled (soft); transparent pixels are never sampled; spot heal's window grows with a long stroke.
- VERIFY: Photoshop's Auto sampling area; its default seedless behaviour (Fotox is deterministic per seed).
- Try it: select an object with the Lasso, Edit ▸ Content-Aware Fill… ▸ OK; or Shift+F5 ▸ Use: Content-Aware. J (Spot Healing) over a blemish.

## M11-T03 — Patch tool  (Claude, 2026-09-26)
- Done: `tools/patch.rs` ("patch"): a freehand lasso draws the selection (or use the current one); a drag started inside it moves the ants and commits `Command::Patch { dx, dy, destination, content_aware }` on release. Normal = the D-045 healing blend of the content `(dx, dy)` away into the selection (through `PixelOps::heal_blend`); Content-Aware = PatchMatch sampling only the source box. Destination mode patches the dragged-to place from the selection. Option bar: Patch (Normal / Content-Aware), Source / Destination.
- Skipped: live preview while dragging, Transparent, Structure / Color (Content-Aware), Diffusion, Tests.
- FAST: a Normal patch over 4 × 768² pixels is refused; after a Destination patch the selection stays at the source.
- VERIFY: Photoshop's selection after a patch.
- Try it: J's flyout ▸ Patch Tool, lasso around a blemish, drag it onto clean skin.

## M11-T04 — Content-Aware Move  (Claude, 2026-09-26)
- Done: the same tool as "content-move": the drag commits `Command::ContentAwareMove { dx, dy, extend }` — Move fills the old place by PatchMatch (Auto band) and composites the content at the new place through the moved selection; Extend keeps the old place. The selection follows the content.
- Skipped: Transform on Drop, Structure / Color (edge blending), Sample All Layers, Duplicate mode, Tests.
- FAST: the moved content's edge is not blended (coverage only).
- VERIFY: none.
- Try it: J's flyout ▸ Content-Aware Move Tool, lasso an object, drag it.

## M11-T05 — Content-Aware Scale  (Claude, 2026-09-26)
- Done: `fx_ops::seam::carve` (Avidan–Shamir: gradient energy + 1000 × protection, DP seams, columns then rows on the carved image; enlargement duplicates the first k removal seams, k ≤ half) behind `PixelOps::seam_carve`; `Command::ContentAwareScale { layer, width, height, amount, protect, protect_skin }` reads the layer onto a ≤ 640² working grid, carves to the amount's share of the change, then maps every output pixel (tile by tile, rayon) through the plain scale → carved full-res pixel → working cell → source pixel. Edit ▸ Content-Aware Scale… (dialog: Width / Height %, Amount, Protect channel, Protect Skin Tones).
- Skipped: the transform box UI and its view-level preview, scaling a selection only, Tests.
- FAST: seams are `scale` px wide at full resolution (blocky steps on big images); nearest neighbour for the plain-scale part; the layer mask is not scaled; skin tones by a crude RGB rule; energy recomputed per seam.
- VERIFY: Photoshop's skin detector and how Amount mixes the two.
- Try it: Edit ▸ Content-Aware Scale…, Width 60 %.

## M11-T06 — Liquify  (Claude, 2026-09-26)
- Done: `Mapping::Custom { id, src, dst }` (still `Copy`) naming a geometry in `fx_core::warp_map` (a registry of `TriMesh` / sparse `DispField`, the last 48 kept), supported by `dest_rect`, the sampler's `Transform` (`fx-ops/resample/mapping.rs`: mesh → warp triangles with source points as `uv`; field → `p + d(p)`) and so by `Command::Transform` and the Free Transform live preview. The transform `Session` takes a `CustomWarp` (pointer, keys, options, overlay, status) in place of the box (`start_transform_with`), with its own option bar (`TransformBox.bar`). `fx_ops::liquify::dab`: Forward Warp, Reconstruct, Smooth, Twirl (Alt: counter-clockwise), Pucker / Bloat (Alt swaps), Push Left on a backward field with 4-px nodes in 64² chunks (only touched chunks exist). Filter ▸ Liquify… (Shift+Ctrl+X) starts the session (`tools/liquify.rs`): bar with Tool, Size, Pressure, Rate, ✓ / ✗; `[` `]` size, Delete = Restore All, Enter applies as one resample job.
- Skipped: the modal workspace, Freeze / Thaw Mask, Hand / Zoom inside, Show Mesh / Backdrop, Reconstruct All (partial), stylus pressure, mesh load / save, Face-Aware (D-078), Tests.
- FAST: the history step is "Free Transform"; the output layer grows by the largest displacement all round; source bounds use the field's global maximum; the registry forgets old ids (a stale id maps nothing).
- VERIFY: brush falloff and twirl / pucker speeds against Photoshop.
- Try it: Filter ▸ Liquify…, drag over a face, Enter.

## M11-T07 — Puppet Warp  (Claude, 2026-09-26)
- Done: `fx_ops::puppet` (grid mesh over the content box keeping cells the alpha — dilated by Expansion — covers; Density sets 20 / 36 / 60 cells; MLS deformation, D-079: Rigid, Normal = similarity, Distort = affine); `tools/puppet.rs` session: click adds a pin, drag moves it, Alt+click removes it; Mode and Show Mesh live from the bar; the deformed mesh is registered as `Mapping::Custom` and previewed / committed like Free Transform. Edit ▸ Puppet Warp.
- Skipped: ARAP (D-079 fast default), pin rotation, Pin Depth, Tests.
- FAST: a new pin's source is found from the nearest mesh vertex; Density / Expansion only apply when the session starts; the mesh overlay draws every triangle.
- VERIFY: none.
- Try it: a layer with an object on transparency, Edit ▸ Puppet Warp, click three pins, drag one.

## M11-T08 — Perspective Warp  (Claude, 2026-09-26)
- Done: `tools/perspective_warp.rs` warp session (Edit ▸ Perspective Warp, Alt+Shift+Ctrl+W). Layout: drag draws a quad, corners drag, a corner dropped near another quad's corner snaps to it and the two stay linked. Warp (the bar's Layout / Warp group): corners drag (linked corners move together); each quad maps its layout shape to its warped shape by homographies from the unit square (`Mapping::from_quad`), subdivided 16 × 16 into one `TriMesh` → `Mapping::Custom`, previewed and committed like Free Transform. Straighten (bar button, `warp:straighten`) snaps warped edges within 20° of vertical / horizontal.
- Skipped: Shift+click one edge, deleting a quad, snapping along whole edges, Tests (no crack along a shared edge).
- FAST: shared edges can crack slightly (per-quad homographies); the content outside the quads is dropped by the resample.
- VERIFY: Photoshop keeps the content outside the planes (it does, via the mesh's extension) — Fotox drops it.
- Try it: Edit ▸ Perspective Warp, drag two planes sharing a corner, switch the bar to Warp, drag corners, Enter.

## M11-T09 — Vanishing Point  (Claude, 2026-09-26)
- Skipped: the whole card (a modal workspace with planes, perspective marquee / stamp / brush, paste into a plane). The pieces it needs exist (homographies in `Mapping::from_quad`, `Mapping::Custom` meshes, the Clone Stamp); left for HARDEN or a later milestone after two cards' worth of warp sessions.

## M11-T10 — Acceptance: deferred to HARDEN (S32–S34, Rob's comparison with Photoshop).

## M12-T00 — Decisions  (Claude, 2026-09-26)
- Done: the recommendations recorded as fast defaults D-082 (Smart Objects, reverses D-055), D-083, D-085, D-086, D-087. Point 3 had no recommendation ("Rob's call"): the filter families go in as M12-T03b (D-084), flagged for Rob.
- Skipped: none. FAST: none. VERIFY: none.
- Try it: `docs/DECISIONS.md`.
