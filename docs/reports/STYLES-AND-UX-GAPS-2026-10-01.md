# Layer styles and UX: what Fotox lacks (2026-10-01)

Rob asked for three things: Object Selection on the layer's whole surface,
a Layer Style overhaul after reading Photoshop / Photopea and Compositor, and
the UX differences with Compositor (the macOS editor vendored in
`XuanZhi9/compositor-windows/reference/Compositor`, 24 k lines of Swift, with
an 887-row behaviour ledger in `compositor-windows/docs/ledger`).

Sources read: Adobe's help pages under *Apply layer effects* (add styles,
preset styles, options overview, contours, Global Light, Scale Effects,
remove, Make Default, Create Layers, Knockout, opacity/fill); Photopea's
*Layer Styles* page; Compositor's source and ledger (338 rows on selections,
input, tools, transform, layers and UI); Fotox's `fx-core/src/styles.rs`,
`fx-engine/src/effects.rs`, `ui/js/native/styles.js`, `ui/js/dialogs.js`,
`ui/js/data/dialogs.js`, `ui/js/native/layers-panel.js`,
`ui/js/shortcuts.js`, the tools and `engine.rs`. Every "missing" below was
checked in the code, not recalled.

## 1. Object Selection — done

The tool read the composite, so a covered object could not be selected.
It now reads the active layer's own content (`ai::layer_alone`: the layer
visible, opaque, Normal, without masks or styles, over neutral grey so a
dark object on transparency does not vanish into the black a straight read
gives). **Sample All Layers** (option bar, off by default, as in Photoshop)
brings back the composite. An adjustment layer, which has no content of its
own, falls back to the composite. The EfficientSAM embedding cache is keyed
on the layer too.

Tests: `fx-engine/tests/object_layer.rs` — the covered layer reaches the
model over grey (always runs); BiRefNet finds a square an opaque layer hides
completely, IoU 1.000 (`#[ignore]`, real model).

## 2. Layer styles

**Compositor has no layer styles at all** — no shadow, glow, bevel or
effect code anywhere in its source (its only "shadow" is the one under the
document rectangle). So the reference here is Photoshop, with Photopea as
the second opinion.

**What Fotox already has** is more than the dialog lets on. The model and
renderer cover all ten effects; contours (12 presets) on shadows, glows,
satin and bevel; bevel Technique, Gloss Contour, Anti-aliased and Texture;
Color / Gradient / Pattern fill types for Stroke; a gradient for Outer
Glow; Noise; Blend If (Gray); Global Light (the command); Copy / Paste /
Clear Layer Style; style presets in the Styles panel. The comment above
`bevel` in `effects.rs` ("Smooth technique only, no Contour / Texture") is
stale — all three are wired.

The weakness is mostly the **dialog**, then a handful of missing options.

### 2a. The dialog (why it feels wrong)

| # | Photoshop / Photopea | Fotox now |
|---|---|---|
| D1 | One window. Clicking a name in the sidebar swaps the right pane in place. | Every page is a separate dialog. A sidebar click closes the dialog and opens another (`openPage` → `openDialog`): it jumps, loses scroll, redraws the frame. |
| D2 | Right column: OK, Cancel, New Style…, Preview checkbox, a live thumbnail of the style. | OK and Cancel only. No Preview toggle, no thumbnail, no New Style. |
| D3 | Styles page at the top of the sidebar (the presets). | Removed from the sidebar (`wireStyleList`: "style presets: not in the app"), although the Styles panel exists. |
| D4 | **Make Default / Reset to Default** under every effect. | Missing; defaults are hard-coded in `styles.js`. |
| D5 | **+** beside Drop Shadow, Inner Shadow, Color Overlay, Gradient Overlay and Stroke: up to 10 of each, reorderable with arrows, deletable with a trash icon. | One of each: `LayerStyles` holds `Option<DropShadow>` etc. |
| D6 | fx menu at the bottom of the sidebar: Show All Effects / Show Default Effects / Reset to Default List. | Missing. |
| D7 | Angle is a dial you drag; Distance and Angle can be set by dragging the shadow on the canvas while its page is open. | Angle is a slider. Canvas drag only for Gradient and Pattern Overlay. |
| D8 | Contour is a thumbnail: a click opens the Contour Editor (a curve with points, New / Save / Load); the arrow opens the preset picker with previews. | A plain drop-down of 12 names; no curve, no preview, no custom contour. |
| D9 | Gradient and Pattern pickers on the page itself (Stroke fill, Outer / Inner Glow gradient, Bevel Texture). | Choosing "Gradient" or "Pattern", or ticking Texture, silently takes the Gradient tool's current gradient or the current pattern; there is no picker on these pages, so changing it means leaving the dialog. |
| D10 | Numeric fields: arrow keys step (×10 with Shift), scrubby labels. | Slider plus box; arrows not verified in dialogs. |

### 2b. Options that are missing

| Effect | Missing |
|---|---|
| Blending Options | **Knockout** (None / Shallow / Deep); **Channels** R / G / B; **Blend Interior Effects as Group**; **Blend Clipped Layers as Group**; **Transparency Shapes Layer**; **Layer Mask Hides Effects**; **Vector Mask Hides Effects**; Blend If per channel (only Gray now). |
| Drop Shadow | **Layer Knocks Out Drop Shadow**; contour Anti-aliased. |
| Inner Shadow | Noise; contour Anti-aliased. |
| Outer Glow | Technique Softer / Precise; **Range**; **Jitter**; contour Anti-aliased. |
| Inner Glow | Gradient fill; Technique; Range; Jitter. |
| Bevel & Emboss | Style **Stroke Emboss**; Contour page's Range; texture pattern picker and Snap to Origin. |
| Satin | contour Anti-aliased. |
| Gradient Overlay | Method (Perceptual / Linear / Classic); Reset Alignment. |
| Pattern Overlay | Snap to Origin. |
| Stroke | Overprint; the gradient's Shape Burst style. |

### 2c. Around the dialog

| # | Photoshop / Photopea | Fotox now |
|---|---|---|
| A1 | Effects listed **under the layer** in the Layers panel, collapsible, each with an eye; an "Effects" row with its own eye. | Only an "fx" tag on the row. |
| A2 | Double-click the layer row (not the name or thumbnail) opens Blending Options. | Double-click does nothing there (only the name renames, thumbnails edit). |
| A3 | Alt-drag the fx icon to another layer copies the style; drag it to the trash clears it. | Missing. |
| A4 | Layer ▸ Layer Style ▸ **Global Light…** (Angle + Altitude). | The command exists; no menu item or dialog. |
| A5 | **Scale Effects…** | Missing. |
| A6 | **Create Layer(s)** (effects to pixel layers, clipped). | Missing. |
| A7 | **Hide All Effects / Show All Effects**. | Missing. |
| A8 | Paste Layer Style onto several selected layers. | The engine takes the selection (`engine.rs` ~3276); fine. |
| A9 | Presets stored with the app, importable/exportable as .asl. | Presets live in the CEF page's `localStorage` (`overview-panels.js`): they vanish if the browser profile is cleared. No .asl. |

### 2d. Proposed overhaul, in order

1. **One dialog** (D1–D4, D6, D10): a persistent Layer Style window whose
   right pane is rebuilt in place; OK / Cancel / New Style / Preview /
   thumbnail column; Styles page back; Make Default / Reset to Default per
   effect (stored in preferences, not localStorage); fx menu. Pure UI.
2. **On-page pickers and widgets** (D7–D9): angle dial, canvas drag for
   shadow / inner shadow / satin distance-angle, contour thumbnail + picker
   + curve editor (custom contours become a `Contour::Custom(points)`),
   gradient / pattern pickers on Stroke, glows and Texture.
3. **Multiple instances** (D5): `LayerStyles` gains `Vec`s for the five
   stackable effects (serde keeps reading the old single form), effect
   indices become (kind, instance), the renderer loops. The largest change.
4. **Missing options** (2b), cheapest first: Layer Knocks Out Drop Shadow,
   Layer/Vector Mask Hides Effects, Channels, Stroke Emboss, Range / Jitter /
   Technique for glows, Blend If per channel, Knockout and the two "as
   Group" flags last (they change how the compositor walks groups).
5. **Around the dialog** (2c): effects in the Layers panel with eyes,
   double-click row, fx drag, Global Light dialog, Scale Effects, Create
   Layers, Hide All Effects; presets to the preferences folder (+ .asl
   import later, with PSD).

## 3. UX commands: Fotox against Compositor

Compositor is a much smaller editor (15 tools, 13 blend modes, no pen
input), but it is careful about the rules of each gesture. Where it has a
rule, Fotox was checked for the same one.

### Where Fotox already matches or goes further

- Selection modifiers: Shift adds, Alt subtracts — and Fotox has
  **Shift+Alt = Intersect**, which Compositor lacks (Option simply wins).
- Dragging inside a selection moves the outline (`OutlineDrag`); arrow keys
  nudge it 1 px, Shift 10 px.
- Marquee: Space repositions while drawing (Compositor has none).
- Polygonal lasso: Backspace drops the last point, Escape cancels,
  double-click / Enter / first point close (Compositor: Delete and Escape).
- Magic Wand: Sample All Layers off by default (same); Paste in Place
  (Shift+Ctrl+V; Compositor pastes in place only while its own clipboard is
  current).
- Brush keys: `[` `]` size, Shift+`[` `]` hardness in 25 % steps, digit keys
  for opacity with the two-digit combination within 0.6 s — the same rules.
- Mode buttons show the mode Shift/Alt would select while held.

### Where Compositor does something Fotox does not

| # | Compositor (and Photoshop) | Fotox now |
|---|---|---|
| U1 | **Delete with no selection deletes the active layer** (or its mask when the mask is targeted). | `clip:clear` without a selection does nothing (`engine.rs` 3618). |
| U2 | **Eye swipe**: press an eye and drag over others; they all take the same state, one undo step. | One click per eye. |
| U3 | **Alt-click an eye** shows only that layer (Photoshop; Compositor lacks it too, listed for completeness). | Missing. |
| U4 | **Ctrl-click a layer thumbnail loads its pixels as a selection** (the thumbnail shows its own cursor while Ctrl is held). | Missing from the panel (the menu has Select ▸ Load Selection). |
| U5 | **Alt-click between two layers** creates / releases a clipping mask, with a clipping cursor while Alt is held. | Only the context menu. |
| U6 | **Alt-drag a mask thumbnail** to another layer copies the mask. | Missing. |
| U7 | **Blend-mode hover preview**: hovering a mode in the menu shows it on the canvas without committing; **Shift + / Shift −** cycles the mode. | Missing. |
| U8 | **Move snaps to other layers' edges and centres**, 10 screen points, with guide lines. | `snap.rs`: "Layers snaps to the canvas, not to each layer's bounds" (FAST). |
| U9 | **Status-bar hint per tool**: one line with the tool's whole gesture model ("Drag a rectangle · Shift add · Option subtract · …"). | No hints. |
| U10 | Busy indicator only after 250 ms, so quick edits never flash the UI. | Not checked in Fotox; worth a look. |
| U11 | A draft outline with fewer than 3 points in New mode deselects; subtracting from no selection does nothing; an unchanged selection makes no undo step. | Not verified; deserves a test each. |
| U12 | First Undo cancels a pending gradient / transform instead of stepping history. | Not verified. |

U1–U8 are real gaps; U9–U12 are rules worth a test.
