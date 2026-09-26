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
