# Fotox — UX review: Selection, Move, Transform, Layer selection (vs Photopea)

**Date:** 2026-10-01
**Branch:** `feat/layer-style-overhaul`
**Scope:** marching-ants selection tools, the Move tool, Free Transform /
Edit ▸ Transform, and selecting/reordering layers in the Layers panel.
**Method:** read-only code review — engine tools (`crates/fx-engine/src/tools/*`),
the command layer (`crates/fx-core`), and the UI (`ui/js/**`). No builds, no
tests. `FAST:` comments in the source are treated as deliberate shortcuts and
called out as such.
**Baseline:** Photopea (with Photoshop where they agree), the tool Photopea
users learned first.

This complements `2026-10-01-ux-review-vs-photopea.md` (global UX) and
`docs/reports/STYLES-AND-UX-GAPS-2026-10-01.md` (layer styles). It does not
re-cover those.

---

## Executive summary

The selection/move/transform core is **surprisingly faithful to Photopea already**.
Marquee, lasso (all three), magic wand, Move with Auto-Select + duplicate +
nudge, Free Transform with every modifier gesture, Transform Selection, and the
full Select menu (Modify, Grow, Similar, Save/Load, Color Range, Select and
Mask) are all present and match the conventions. The differences are a short
list of live-feedback gaps, a few missing "quality of life" gestures, and two
discoverability problems.

Priority legend: **P0** wrong/broken, **P1** expected / high day-to-day cost,
**P2** polish.

| # | Item | Area | Priority |
|---|------|------|----------|
| 1 | No Ctrl+click layer thumbnail → load layer as selection | Layer sel. | **P1** |
| 2 | Moving the chosen pixels of a selection shows no live preview | Move | **P1** |
| 3 | Quick Selection grows its outline only on release | Selection | **P1** |
| 4 | Transform numeric fields throw away skew/perspective | Transform | **P1** |
| 5 | "Deselect Layers" / "Select Similar Layers" dead in the Select menu | Layer sel. | **P1** |
| 6 | Color Range samples the fg/bg swatch, not the image; no preview | Selection | **P1** |
| 7 | No layer linking (move/transform linked layers together) | Layer sel. | **P1** |
| 8 | No "Repeat Transform" (Ctrl+Shift+T) / Transform Again | Transform | **P1** |
| 9 | Transform/Select-and-Mask have no live preview (FAST markers) | Transform | **P2** |
| 10 | No W/H proportional link in the transform bar | Transform | **P2** |
| 11 | No layer colour labels / filter by label | Layer sel. | **P2** |
| 12 | Object Selection has no hover "Object Finder" | Selection | **P2** |
| 13 | Move tool can't drag a layer into another document | Move | **P2** |

---

## 1. Selection tools

### What already matches Photopea (keep it)
- **Marquee** (`tools/marquee.rs`): Rect / Ellipse / Row / Column; option-bar
  Style (Normal, Fixed Ratio, Fixed Size), Feather, Anti-alias; Shift = square,
  Alt = from centre; **Space while dragging moves the marquee**; a short drag is
  a click and clears the selection in Replace; the click slop is a *screen*
  distance so it behaves the same at every zoom. This is a better-than-average
  match.
- **Lasso** (`tools/lasso.rs`): freehand (samples every 0.5 screen px) and
  polygonal (click-per-point, Enter / double-click / click-first-point to
  close, Backspace to drop, Escape to cancel). **Alt in freehand draws a
  straight segment** (PS's lasso↔polygon switch); **right-click pauses** an open
  path so you can pan/zoom. Magnetic Lasso (`magnetic.rs`) is separate.
- **Magic Wand** (`tools/wand.rs`): Tolerance / Anti-alias / Contiguous /
  Sample All Layers; click outside the canvas deselects in Replace.
- **Modifier semantics** (`tools/mod.rs` `mode_at_press`): Shift = Add,
  Alt = Subtract, Shift+Alt = Intersect, otherwise the option bar's Mode button
  group. Exactly Photoshop/Photopea.
- **Dragging the selection outline** (`OutlineDrag`): a New-mode press inside
  the ants with no movement selects/acts, with movement offsets the whole
  selection (`Command::OffsetSelection`) — the ants follow live.
- **Arrow-key nudge** of the outline, 1 px / Shift 10 px (`nudge_outline`).

### G1. Quick Selection previews only on release (P1)
`tools/quick_select.rs` collects dabs while dragging and emits one
`Command::SelectBy` at release; the module's own header says **"FAST: the outline
updates at release, not live (the brush circles and the path are drawn
meanwhile)."** Photopea grows the ants under the brush as you drag, so the user
sees the region they are capturing and can correct mid-stroke. **Fix:** stream a
coarse coverage update (the engine already runs selections as jobs) so the ants
follow the stroke, then commit the full-resolution one on release.

Also note `cursor()` returns `CursorShape::None` for Quick Selection — the brush
circle overlay substitutes, but if the overlay is not drawn the pointer is
invisible. Verify the circle is always on.

### G2. Object Selection: no hover, lasso centre is approximate (P2)
`tools/object_select.rs` is a real AI path (BiRefNet / EfficientSAM via
`AiRequest::Object`), which is *ahead* of Photopea's older object tools. But its
header records **"FAST: no Object Finder (the hover highlight); the lasso's
centre is its box's centre, even when that falls outside a concave outline."**
Photopea highlights the object under the cursor before you click. **Fix:**
lower-priority, but the hover preview is what makes the tool feel smart.

### G3. Color Range and Select and Mask are not interactive (P1/P2)
`ui/js/native/selections.js` header: **"FAST: no live preview in the dialogs
yet (the result shows … after OK); Color Range samples the foreground (and
background) swatch instead of an eyedropper on the image."**
- Color Range is the one selection dialog where the eyedropper *is* the
  interaction. Sampling the current fg swatch makes the common "click the
  colour I want to select" flow impossible. **P1.**
- Focus Area and Select and Mask have no coverage preview. Photopea shows the
  candidate mask live. **P2** (Select and Mask particularly — it is a
  selection-shaping tool, so dialing it blind costs several OK/undo cycles).

### G4. "Select Subject / Sky" (good) and refine edge
`Select ▸ Subject` and `Select ▸ Sky` are wired to the AI actions
(`ai:subject`, `ai:sky`) — this matches Photopea's own one-click subject/sky and
beats it on the sky case. No "Refine Edge" item, which is correct (Photopea and
current Photoshop fold it into Select and Mask).

---

## 2. Selection commands & the Select menu

Coverage (`ui/js/data/menus.js` Select menu, `fx-core` commands):

| Command | Photopea | Fotox |
|---|---|---|
| All / Deselect / Reselect / Inverse | yes | yes (`sel:all`, `sel:none`, `sel:reselect`, `sel:inverse`) |
| Modify: Border / Smooth / Expand / Contract / Feather | yes | yes (`dlg:sel-*`) |
| Grow / Similar | yes | yes (`sel:grow`, `sel:similar`) |
| Transform Selection | yes | yes (`sel:transform`, live box; **FAST** no ant preview) |
| Save / Load Selection | yes | yes (channels-backed) |
| Color Range / Focus Area / Select and Mask | yes | yes (see G3) |
| Subject / Sky | yes | yes |
| All Layers | yes | yes (`sel:all-layers`) |
| **Deselect Layers** | yes | **disabled** (`sel:none-layers`, `dis: true`) |
| **Select Similar Layers** | yes | **disabled** (`sel:similar-layers`, `dis: true`) |

### G5. Select ▸ All Layers / Deselect Layers / Select Similar Layers (P1)
"Deselect Layers" and "Select Similar Layers" are hard-disabled in the menu
(`dis: true`), so selecting by layer *kind* (all type layers, all adjustment
layers) is impossible. Photopea exposes this from the Layers right-click too.
**Fix:** implement `select_layers` variants by name pattern / kind / link
group and enable the two items; add them to the Layers context menu.

---

## 3. Move tool

### What already matches Photopea (keep it)
`tools/move_tool.rs` is carefully built:
- **Auto-Select off by default**, Ctrl inverts it while held (documented as the
  Photopea choice);
- Shift+click adds / removes from the selection; a press on a layer already in a
  multi-selection keeps the selection; Shift constrains a drag to 0/45/90°;
- **Alt+drag duplicates** layers (or, with a pixel selection, the selected
  pixels) in **one** history step (`"Duplicate and Move"` / `"Duplicate
  Pixels"`);
- Auto-Select clicks *through* hidden, fully-locked and position-locked layers,
  ignores adjustment layers, and uses an alpha threshold (`PICK_ALPHA`) so a
  faint haze does not steal the click — this is the exact behaviour Photopea
  users expect;
- Arrow nudges (1 px / Shift 10 px) merge into one history step per burst;
- clear refusal toasts ("… has no pixels to move: rasterize it, or deselect…").

### G6. No live preview when moving selected pixels (P1)
The pixel branch of `MoveTool::preview` is empty:
```rust
if drag.pixels {
    // FAST: no live preview of a pixel move; the ants follow instead.
    return;
}
```
So dragging a selection with the Move tool shows the ants moving but **the
pixels stay put until release**. Photopea (and the transform preview it already
uses elsewhere) shows the pixels moving live. The engine already has the
machinery — `transform_preview.rs` lifts the selected pixels, leaves a hole and
floats the result — so the Move tool could reuse that preview path. **Fix:**
preview a `MovePixels` the way a transform previews, then commit on release.

### G7. Show Transform Controls works (verify the default)
`engine.rs::transform_controls` draws the active layer's box + handles under the
Move tool and a press on a handle starts Free Transform (engine.rs ~686 and
~4298). This does match Photopea's Move tool. Two notes:
- `ui/js/data/options.js` ships `Show Transform Controls` **on** for the Move
  tool. Photoshop's default is off; confirm this is the intended default, since
  an on-by-default box means a click within 8 screen px of a handle starts a
  scale instead of a move.
- Because handle-grab precedes the move, the interaction is fine — just make
  sure the handle slop scales with zoom (it does: `reach = 8.0 / zoom`).

### G8. No cross-document layer drag (P2)
You cannot drag a layer (or layer group) from one document tab onto another to
copy/move it. Photopea supports this; it is one of the most-used ways to combine
images. **Fix:** pointer drag over the tab strip → `DuplicateLayers` into the
target document.

### G9. Auto-Select / Select coupling (P2)
The option bar shows `Auto-Select` and `Select: Layer | Group` as independent
controls; Photoshop disables the Layer/Group menu while Auto-Select is off. The
bar's `enables` mechanism (`optionsbar.js` `sync()`) already exists for exactly
this — wire it.

---

## 4. Free Transform / Edit ▸ Transform

### What already matches (and sometimes beats) Photopea
`tools/transform.rs` is the strongest part of this review:

- **Mode set**: Free, Scale, Rotate, Skew, Distort, Perspective, Warp (the whole
  Edit ▸ Transform submenu, `xf:*`).
- **Gestures** (Photoshop 2019+): corner scales about the opposite corner, Shift
  keeps proportions, Alt scales about the reference point; side scales one axis;
  inside moves; just outside rotates (Shift snaps 15°); Ctrl-corner distorts,
  Ctrl+side moves the side, Ctrl+Shift+side skews, Ctrl+Alt+Shift-corner is
  perspective; a folding box is refused (`quad_is_convex`).
- **Reference point** is hidden and taken with Alt (PS 2019+), rather than a
  permanent pivot.
- **Instant turns** (Rotate 90/180, Flip H/V) compose a pixel-centre-exact
  mapping (`turn_mapping`), so a quarter turn copies pixels without resampling.
- **Numeric bar** X / Y / W / H / Angle with "empty = keep", and the status line
  shows live W/H/Angle while dragging.
- **Selection-aware**: `start_rect` uses the selection's bounds and
  `transform_preview.rs` lifts the selected pixels, leaves the hole and floats
  the moved result — i.e. Ctrl+T on a selection transforms *the selection*, not
  the whole layer. Matches Photopea.
- **Transform Selection** (`sel:transform`) drives the same box and commits
  `Command::TransformSelection`.

### G10. Typing in the numeric fields discards skew/perspective (P1)
`Session::set_numeric` is documented **"FAST: skew and perspective are lost"** —
it rebuilds the box as a plain rotated rectangle around the reference point.
So a user who has Ctrl-distorted a box and then types a W value silently loses
the distortion. Photopea keeps the parallelogram while you edit a number. **Fix:**
apply the numeric edit to the existing `quad` (or refuse with a message) instead
of rebuilding it.

### G11. No proportional W/H link (P2)
The transform bar has W and H as independent fields with no chain icon between
them. Photopea/Photoshop have a link that keeps the aspect ratio when you type.
**Fix:** add a lock between W and H in `ui/js/data/options.js` (`_transform`) and
respect it in `set_numeric`.

### G12. No Repeat Transform / Transform Again (P1)
`Ctrl+Shift+T` (repeat last transform) and `Ctrl+Alt+Shift+T` (duplicate +
transform) are absent from both `ui/js/shortcuts.js` and the engine. Photopea
users lean on Repeat Transform constantly for repetitive layouts. **Fix:** store
the last committed `Command::Transform` mapping and add the two actions.

### G13. Reference-point presets (P2)
Fotox's reference point is free-dragged with Alt; Photopea/Photoshop also offer a
9-point locator in the bar to snap the pivot to a corner or edge. Cheap to add
to the `_transform` bar and `Session::set_numeric` (which already centralises on
a reference point).

### G14. Commit-on-click-away can surprise (P2)
`Grab::Away` commits the transform on any press outside the box (as PS 2019+
does). Photopea keeps the box until you press Enter / ✓. If the intent is
Photopea familiarity, consider making click-away a no-op (or requiring Enter),
since an accidental click currently bakes a transform into history.

### G15. Shape/type + selection is refused (P2)
`start_transform` refuses a shape or type layer when a selection is active
("Deselect first: a shape or type layer transforms as a whole"). Photoshop
transforms the whole shape/type layer without complaint. The refusal is honest
but adds a step; consider simply ignoring the selection for those kinds.

---

## 5. Layer selection & the Layers panel

### What already matches (keep it)
`ui/js/native/layers-panel.js` is a strong Photoshop-style panel: virtualised
rows, Shift range (anchored on the last click), Ctrl/Cmd toggle, rename on
double-click, drag to reorder with above/below/into-group drop zones and edge
auto-scroll, a full context menu (Blend Options, Layer Style, mask, clipping,
Smart Object, merge, flatten), lock buttons, mask/vector-mask thumbnails that
switch the paint target, and double-click to open blend/style/adjustment.

### G16. No Ctrl+click thumbnail to load the layer as a selection (P1)
The regime everyone uses ("hold Ctrl and click a layer's thumbnail to select its
shape") is missing: clicking a pixel thumbnail sends `layer:edit-mask`
(paint target), and there is no Ctrl/Cmd branch. The **Channels** panel already
implements exactly this gesture (`ui/js/native/channels-panel.js` — Ctrl+click
loads a channel with Add/Subtract/Replace), so the pattern exists. **Fix:**
Ctrl/Cmd+click a layer or mask thumbnail → `LoadSelection` from its alpha.

### G17. No layer linking (P1)
The native Layers panel's action bar (`i-fx`, `i-mask`, `i-adjust`, `i-group`,
`i-new-layer`, `i-trash`) has **no link button**, and the mock renderer's link
button (`ui/js/panels.js`) toggles a class only. So there is no way to move or
transform several layers together except by selecting them all — which is close,
but Photopea's link survives deselecting and re-selecting, and link is how people
keep a mask + its content moving together. **Fix:** add a link bit on the layer,
a link button + link indicator in the panel, and make Move/Transform treat a
linked group as one unit.

### G18. Layer colour labels / filter (P2)
No colour-label assignment (Layers right-click → colour) and no filtering by
label or kind. Photopea has both, and they matter once a document has dozens of
layers. Ties into the missing Layers search field noted in the global review.

### G19. Selection-order fidelity (P2)
`active()` falls back to "topmost selected row" when the last-clicked layer is
not in the selection. The engine carries a selection *list*; the panel's guess
is usually right but can disagree with the engine after an undo. Minor; worth an
explicit `A` ordering field in the `layers` message if drift is ever observed.

---

## 6. Cross-cutting & keyboard

- Arrow keys, Escape, Enter and Backspace/Delete are forwarded to the engine
  when no dialog/popup is open (`ui/js/shortcuts.js` `VIEWPORT_KEYS`). Good:
  the polygonal lasso's Enter/Backspace and the outline nudge all work. Note the
  earlier global review's finding that there is **no `event.repeat` guard** —
  holding an arrow nudges as fast as the OS repeats (acceptable) but it also
  means a held `[` / `]` and digit works (intended).
- Mirroring/`Ctrl+click` conventions: Photopea uses Ctrl (Cmd on macOS) for
  load-selection, add-to-selection and transform shortcuts. Fotox's Mac support
  should use `metaKey`; `OutlineDrag` and the marquee modifiers are engine-side
  and receive `Modifiers` — verify macOS Cmd maps to `ctrl` in the input layer
  so Modifier behaviour is identical on both platforms.
- No shortcut for "Show Transform Controls" or for toggling the transform bar's
  Interpolation; low priority.

---

## 7. What already matches or beats Photopea

- **Every marquee/lasso/wand modifier and option** matches, including two
  details Photopea gets wrong or omits in places: Space-to-move-while-drawing
  and a **zoom-independent click slop**.
- **Show Transform Controls** on the Move tool (box + handle drag that starts
  Free Transform) — this is implemented, not just planned; the older
  `docs/reports/HARDEN-H2.md` note marking it open is now stale.
- **Free Transform gesture set** and the pixel-centre-exact instant turns.
- **Selection-aware Free Transform** (lifts the selection, leaves the hole).
- **AI Select Subject / Sky / Object Selection** — ahead of Photopea's classic
  tools.
- **Honest control state**: locked/hidden layers are clicked through with a
  toast rather than an unexplained no-op.

---

## Prioritised recommendations

**Sprint 1 — close the live-feedback and selection gaps**
1. Ctrl/Cmd+click a layer/mask thumbnail to load it as a selection (mirror the
   Channels panel gesture).
2. Live preview for a pixel-selection move (reuse `transform_preview`).
3. Enable "Deselect Layers" and "Select Similar Layers" (by kind / name / link).
4. Add Repeat Transform (`Ctrl+Shift+T`) and Duplicate-and-Transform
   (`Ctrl+Alt+Shift+T`).
5. Make Transform's numeric edit preserve skew/perspective.

**Sprint 2 — parity polish**
6. Live Quick Selection outline (coarse streamed coverage).
7. Color Range eyedropper on the image + coverage preview.
8. Layer linking (button, indicator, Move/Transform treat link groups as one).
9. Cross-document layer drag onto a tab.
10. Select and Mask / Focus Area live preview.

**Sprint 3 — small wins**
11. W/H proportional link and a 9-point reference locator in the transform bar.
12. Auto-Select disables the Layer/Group menu when off.
13. Layer colour labels + filter; Layers search field.
14. Object Selection hover (Object Finder).
15. Decide the Move tool's "Show Transform Controls" default and the
    click-away-commits behaviour explicitly, and document both.

---

## Appendix — parity checklist (this review's areas)

| Capability | Photopea | Fotox |
|---|---|---|
| Marquee modifiers / Style / Feather / AA | yes | yes |
| Space-move marquee, zoom-independent slop | partial | **yes** |
| Freehand + polygonal + magnetic lasso | yes | yes |
| Magic Wand Tolerance / Contiguous / Sample All | yes | yes |
| Quick Selection live outline | yes | release-only (FAST) |
| Object Selection | yes (older) | yes (AI; no hover) |
| Modify / Grow / Similar / Transform Selection | yes | yes |
| Save / Load Selection | yes | yes |
| Color Range eyedropper + preview | yes | swatch, no preview |
| Select and Mask live refine | partial | dialog only |
| Move: Auto-Select / duplicate / align | yes | yes |
| Move: live pixel-move preview | yes | no (FAST) |
| Move: Show Transform Controls | yes | **yes** |
| Move: drag layer to another document | yes | no |
| Free Transform: all gestures + modes | yes | yes |
| Free Transform: skew/perspective survive numeric edit | yes | no (FAST) |
| Repeat / Duplicate Transform | yes | no |
| W/H proportional link | yes | no |
| Ctrl+click layer → load selection | yes | no |
| Layer linking for move/transform | yes | no |
| Layer colour labels / filter | yes | no |
| Select similar / all-layers / deselect-layers | yes | partial |
| Honest locked/hidden click-through | partial | **yes** |
| AI Subject / Sky / Object | yes | **yes** |
