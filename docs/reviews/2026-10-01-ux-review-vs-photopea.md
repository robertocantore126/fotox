# Fotox — Deep UX review vs. Photopea

**Date:** 2026-10-01
**Branch:** `feat/layer-style-overhaul`
**Scope:** the whole editor experience — global input, navigation, panels/dock,
dialogs, options bar, canvas interaction, feedback, and copy/honesty.
**Method:** read-only code review (`ui/js/**`, `ui/css/**`, `ui/index.html`,
`crates/fx-app/src/input.rs` for context). No builds, no tests.
**Baseline:** Photopea (and, where they agree, Photoshop) — the product users
have already learned, so divergence is a cost unless it is better.

This review is deliberately *about improvement*, not a bug list; the two
bug reviews (`2026-10-01-bug-review-layer-styles.md`,
`2026-10-01-workspace-bug-check.md`) cover correctness. The layer-style
comparison to Photopea is already covered in depth in
`docs/reports/STYLES-AND-UX-GAPS-2026-10-01.md`; this document goes broad.

---

## Executive summary

Fotox's core editing UX is in good shape: virtualised Layers panel,
pointer-event reordering, scrubby opacity/fill, drag-to-create guides,
drag-to-move layer styles, real Properties/Character/Histogram panels, honest
greyed-out menus, and a native-routed viewport. What is missing is mostly the
*second layer* of ergonomics that Photopea users rely on daily: keyboard
auto-repeat, scrubby number fields everywhere, zoom fit/fill semantics,
shortcut coverage, non-modal re-editing of adjustments, docks/workspaces, and
file intake by drag-and-drop.

Priority legend: **P0** = actively wrong or broken; **P1** = expected, high
day-to-day cost; **P2** = polish / parity.

| # | Item | Area | Priority |
|---|------|------|----------|
| 1 | `Fill Screen` / `Print Size` zoom are hard-coded (200 % / 72 %) | Zoom | **P0** |
| 2 | Ctrl+F is bound to *Last Filter* but advertised as menu search | Shortcuts | **P0** |
| 3 | "Move to Left Dock" / "Dock to Right" are dead menu items | Panels | **P0** |
| 4 | Dialog copy still says "interface mock, no file is written" in the real app | Copy | **P0** |
| 5 | No `event.repeat` guard — held keys re-fire (toggles, tools, panels) | Input | **P1** |
| 6 | Numeric fields are plain text boxes outside a few scrubby ones | Input | **P1** |
| 7 | No Enter-to-confirm in dialogs / no initial focus | Dialogs | **P1** |
| 8 | Adjustment layers re-edit only through a modal dialog | Dialogs | **P1** |
| 9 | No OS drag-and-drop / paste-to-open file intake | Canvas | **P1** |
| 10 | No named workspaces (only "reset"); `ws:` is a toast | Panels | **P1** |
| 11 | Long jobs report progress as a status string, with no cancel | Feedback | **P1** |
| 12 | Missing common order/cycle shortcuts (Ctrl+], Ctrl+[, Ctrl+Tab, new layer) | Shortcuts | **P2** |
| 13 | No Layers search/filter (matters with the 1 000-layer virtualization) | Panels | **P2** |
| 14 | Color picker lacks eyedropper, Lab/CMYK, alpha, add-to-swatches | Color | **P2** |
| 15 | Options bar silently overflows on narrow windows | Layout | **P2** |

---

## A. Global input & keyboard

### A1. No auto-repeat guard (P1)
`ui/js/shortcuts.js` registers its `keydown` listener without checking
`e.repeat`. Every repeat re-runs the bound action:

- holding `Tab` (toggle panels) flickers the dock on/off;
- holding `F` (screen mode) cycles standard → menubar → full repeatedly;
- holding a view toggle (`Ctrl+;`, `Ctrl+'`, `Ctrl+R`) thrashes the flag;
- holding `[`/`]` is fine (intended), but the digit opacity strides also re-run.

The only `e.repeat` handling in the whole product is Space-pan in
`crates/fx-app/src/input.rs` (~line 198). **Fix:** early-return on `e.repeat`
for the *toggle/menu* branches at minimum, keep it for the size/opacity keys.

### A2. Modifier+key conventions not covered (P2)
Photopea/PS muscle memory that has no binding here:

| Shortcut | Photoshop/Photopea | Fotox |
|---|---|---|
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | next / previous document | absent |
| `Ctrl+]` / `Ctrl+[` | bring forward / send backward | absent |
| `Ctrl+Shift+N` | new layer | absent (only `Ctrl+Shift+N` doc) |
| `Ctrl+Alt+Z` etc. | step undo | `Ctrl+Alt+Z` is *Toggle Last State* (minor divergence) |
| `Shift+Tab` | hide panels but keep the toolbar | absent (plain `Tab` only) |
| `D` | default colours | absent (button only) |
| `X` | swap colours | absent (button only) |

The tool letter keys, group cycling (`Shift`+letter), `[`/`]`, number opacity,
flyout on right-click/long-press, and `Alt`+menu-letter navigation are all
already correct and match Photoshop — good.

### A3. Missing wheel/scrub gestures (P2)
- Plain wheel scrolls; zoom needs `Ctrl+wheel` (`ui/js/canvas.js:378`).
  In native mode the engine owns the wheel, so this is a *configuration*
  question, but Photopea's default is wheel-zoom — worth a preference.
- `Alt+wheel` to resize the brush (a Photopea/PS staple) is not present.
- No `Ctrl+drag` / `Alt+drag` on numeric fields (see C).

---

## B. Zoom, pan and navigation

### B1. "Fill Screen" and "Print Size" are constants (P0)
`ui/js/actions.js` (~247-248):
```js
if (a === "zoom:fill")  { zoomTo(200); status("Fill screen"); return; }
if (a === "zoom:print") { zoomTo(72);  status("Print size");  return; }
```
and the status-bar zoom dropdown's "Fill Screen" also calls `zoomTo(200)`
(`ui/js/main.js`). These are passed straight to the engine as `SET_ZOOM`, so a
4 000 px document "fills the screen" at 200 % and a 200 px icon does too.
Photopea computes these from viewport size and document PPI. **Fix:** send a
fit/fill hint (or compute against viewport bounds + PPI) instead of a literal.

### B2. Faster zoom in/out stepping (P2)
`zoomIn`/`zoomOut` use a flat ×1.25 in browser mode; native zoom is an engine
action. Photopea steps through the canonical ladder (…33.3, 50, 66.7, 100, 200
…) and supports `Alt+scroll` and double-click-to-zoom consistently. Worth
aligning the step ladder so repeated zoom lands on friendly numbers.

### B3. No "Fit on screen on open / on new document" toggle (P2)
`fitInitial()` runs only in browser mode; native relies on the engine. No
preference to choose initial zoom (fit vs 100 %). Photopea remembers it.

---

## C. Numeric input & scrubby controls

Fotox already has *some* scrubbing — the Layers panel opacity/fill label drag
(`ui/js/native/layers-panel.js`) and the live Properties/Character fields
(`ui/js/native/overview-panels.js:245` `scrubby()` with Shift = ×10, Alt = ×0.1).
That is exactly Photopea's model. The problem is it is inconsistent:

### C1. Options-bar fields do not scrub and do not step (P1)
`optionsbar.js` `num()` / `sizeField()` / `percentField()` build plain
`<input type="text">` with no label-scrub, no ↑/↓ increment, no Shift = ×10,
no unit switching (px↔%/pt). Photopea lets you hover *any* number field, drag to
scrub, and arrow-key to step. **Fix:** lift `scrubby()` into a shared module and
apply it to `ob-num`, dialog `num`, and adjustment fields uniformly.

### C2. Typed values are not normalised everywhere (P2)
`range` in `optionsbar.js` clamps on `change`, but plain `num()` does not clamp
to a range and does not reject non-numeric text before it reaches the engine
(`number()` returns `null` and the field keeps the string). Photopea clamps and
reverts visibly. **Fix:** clamp + visual revert on blur for all numeric fields.

### C3. No arithmetic in fields (P2)
Photopea/PS evaluate `100/2`, `+10`, `*1.5` in numeric boxes. Not supported
here; cheap to add in a shared field wrapper.

---

## D. Menus, shortcuts and discoverability

### D1. Ctrl+F collision (P0)
`ui/js/shortcuts.js` maps `ctrl+f` → *Last Filter*. `ui/js/main.js` advertises
the top-bar search button with `"data-tip-key": "Ctrl+F"`. So the tooltip lies
and search has no working shortcut. **Fix:** give search a non-colliding
shortcut (Photopea uses `Ctrl+F` for *find/last filter* too but exposes search
elsewhere) — e.g. `Ctrl+Shift+F` or `/`, and keep `Ctrl+F` = Last Filter;
update the tooltip to match.

### D2. Search exists but is under-marketed (P2)
There *is* a `search` dialog, but nothing surfaces it on first run and it isn't
in the mental model. Photopea's search is a first-class way to reach any tool
and command. **Fix:** make the search affordance visible (a small field in the
top bar) and index tool/dialog names, which already live in `data/*.js`.

### D3. Honest grey-out is good — keep it, but re-audit `implemented.js` (P2)
`ui/js/menu.js` greys unimplemented actions and labels them "Planned for M…",
and `IMPLEMENTED` is generated. That is a genuine strength over Photopea (which
shows features it then refuses). Keep it, but it means the generated list must
track reality; `ui/js/actions.js` still contains many `toast("… mock")`
fallbacks — verify none of those paths are reachable in the native app.

---

## E. Panels, docking and workspaces

### E1. Dock context menu has dead items (P0)
`ui/js/panels.js:571-572` adds **"Move to Left Dock" (`panel:left`)** and
**"Dock to Right" (`panel:right`)**. `actions.js` routes any `panel:` prefix to
`panels.togglePanel(a.slice(6))`, i.e. it tries to open a panel literally named
`left`/`right`, which is not in any group — the click does nothing. Either
implement left docking/float, or remove the items so they are not silent lies.
(Photopea supports tab docking and floating panels; a single right dock is a
real simplification, but then the menu must not promise more.)

### E2. No named workspaces (P1)
`actions.js` handles `ws:` with `toast("Workspace: …")`; only
`panels:reset` ("Workspace reset") is real. Photopea ships preset workspaces
(Essentials / Graphics / Painting…) and saves custom ones. **Fix:** define a
handful of workspace presets over `openPanels` + dock geometry and persist the
current layout under a name. The state model (`state.openPanels`, `dockGroups`)
already supports this.

### E3. No Layers search/filter (P2)
The Layers list is virtualised for 1 000 layers (excellent), but finding a layer
is pure scrolling. Photopea/PS put a filter field ("kind", "name") above the
list. **Fix:** add a filter input that narrows `visibleRows()` by name/kind.

### E4. No panel drag-to-reorder / tab tear-off (P2)
Groups and tabs are fixed by `data/panels.js`. Reordering tabs and moving a tab
between groups is expected. Lower priority than E1/E2 but related.

### E5. Options bar silently clips (P2)
`.optionsbar` is `overflow-x: auto` with `scrollbar-width: none` — on a narrow
window, controls disappear with no affordance. Photopea wraps/clamps to a "»
more" flyout. **Fix:** at least show a faded edge or a "»" overflow button.

---

## F. Dialogs

### F1. No Enter-to-confirm, no initial focus (P1)
`ui/js/dialogs.js` builds OK/Cancel as plain buttons with click handlers.
Pressing Enter in a dialog does nothing unless a text field has its own handler;
focus is not moved to the first field on open. Photopea/PS: Enter = OK,
Escape = Cancel, first field focused. **Fix:** bind Enter→OK and focus the first
focusable field in `openDialog`; trap focus within the top dialog.

### F2. Adjustment re-editing is modal (P1)
Editing an adjustment layer always opens the (scrimmed, modal) dialog, even
though there is now a live Properties panel. Photopea lets you tweak an
adjustment non-modally while seeing the canvas. **Fix:** put the most-used
adjustment sliders inline in the Properties panel for the selected adjustment
layer (the mapping already exists in `ADJUSTMENT_DIALOGS`), with the dialog as
the "more options" path. This mirrors how Photopea's Properties/Settings panes
work and removes an open/apply/cancel round-trip per tweak.

### F3. Dialog niceties (P2)
- No position memory / no "don't recenter" (always centered via `marginTop`).
- No `min-width`-aware resize; long Layer Style pages can exceed short windows.
- No unsaved-guard when Escape closes a live dialog (it calls `onCancel`;
  fine, but there's no "you changed something" cue).

---

## G. Canvas interaction & file intake

### G1. No OS drag-and-drop file open (P1)
Grep for `dragover`/`DataTransfer`/`drop` in `ui/js` finds only layer-row and
guide drags (both pointer-event based). Photopea's signature move is dropping a
file (PNG/JPG/PSD/SVG) onto the window to open/place it, and paste-from-OS.
**Fix:** add a window-level `dragover`/`drop` handler that forwards the file to
the engine's import path, plus `paste` handling for images.

### G2. Right-click on canvas (verify) (P2)
Right-click is the expected place for tool-contextual choices (brush
size/hardness popup, layer blend for Move). In native mode the engine owns the
pointer, so this must be decided engine-side; worth confirming it does not fall
through to the browser context menu.

### G3. Quick Mask, screen mode, swap/reset colours (P2)
These live in the toolbar's `toolextras` block and are hidden below 900 px
(`app.css`). Photopea keeps `Q`, `F`, `X`, `D` on the keyboard. Add `X`/`D`
bindings (see A2) so they survive a narrow window.

---

## H. Progress, feedback and performance

### H1. Progress is a status string with no cancel (P1)
`ui/js/native/documents.js` maps `ENGINE.PROGRESS` to
`status("${label}… ${n} %")` and `PROGRESS_DONE` to `status("Ready")`. Long
imports/saves/exports therefore have **no** progress dialog, no determinate bar
in the UI, and **no cancel**. Photopea shows a progress overlay with a Cancel
button. **Fix:** a small non-modal progress strip (or the status bar's own bar)
plus a Cancel that sends the engine's cancel action.

### H2. No busy affordance for blocking operations (P2)
The engine is async and the UI stays responsive, which is good — but there is no
visual "working" state on the affected document tab or command. A subtle
in-tab indicator would remove the "did it take?" uncertainty.

### H3. Frame-time overlay is app-only and hidden (P2)
`Ctrl+Alt+F` toggles an fps/percentile overlay (`canvas.js` `showStatus`), which
is excellent for perf work — but it is app-only and there is no discoverable
entry point. Fine to keep pro-only; consider a debug menu entry.

---

## I. Copy and honesty (real app vs. mock)

### I1. The app still calls itself a mock (P0)
In native mode the engine really imports, saves and exports, yet:
- the status bar's ⓘ button toasts *"Fotox is a navigable interface mock — no
  file is written to disk"* (`ui/js/main.js`, `buildStatusbar`);
- the boot status line reads *"Fotox 1.0 — interface mock …"*;
- the About box (`ui/js/dialogs.js` `aboutField`) says *"This build is a
  navigable shell … the image operations are not implemented."*

These are false once `bridge.isNative` is true and will erode trust. **Fix:**
branch the copy on `bridge.isNative` (app: real engine, real files; browser:
mock). This is the single cheapest credibility win in the UI.

### I2. "Status" language in a browser vs. app (P2)
Several fallback toasts say "needs the app (not available in a browser)"
(honest and useful) — keep that pattern and make sure the app path never
reaches a mock toast (ties to D3).

---

## J. Layout, density and styling

### J1. Single right dock, resizable (good baseline) (P2)
Dock resizing and per-panel list heights (persisted in `localStorage`) are nice
touches that Photopea lacks in places. The main gap is E1/E2 (docking/workspaces).

### J2. Toolbar extras hidden under 900 px (P2)
See G3. Consider a "»" overflow for the toolbar too, rather than hiding.

### J3. Theme is dark-only (P2)
`theme-dark.css` only. Photopea also defaults dark, but a light theme is a
common ask; variables are already centralised, so a second palette is cheap.

---

## What already matches or beats Photopea (keep it)

- **Virtualised Layers list** with pointer-event drag (above/below/into a
  group), edge auto-scroll, Alt-drag-to-copy layer styles — solid.
- **Honest menus**: unimplemented actions are greyed with a planned milestone,
  never a dead click (`ui/js/menu.js`).
- **Native viewport routing**: pointer input is handed to the engine only when
  no popup/dialog is open (`main.js` `overlays`); rulers, guides, zoom and pan
  follow engine messages. Clean separation.
- **Live panel metrics**: RAM with hot/warm/scratch/GPU breakdown in the status
  bar (`canvas.js` `showStatus`) beats Photopea's single number.
- **Guides by dragging off the ruler** (`native/guides.js`) and **scrubby
  opacity/fill** are exactly the PS expectations.
- **Layer-style drag between layers** and the fx fold-out rows match Photoshop's
  model closely (further detail in the styles gap doc).

---

## Prioritised recommendations

**Sprint 1 (correctness + trust, all P0)**
1. Compute `zoom:fill` / `zoom:print` from viewport + PPI; remove the 200/72
   constants and the status-dropdown special case.
2. Free `Ctrl+F` for search or re-label it; wire the search entry point.
3. Implement or delete "Move to Left Dock" / "Dock to Right".
4. Branch all "interface mock / no file written" copy on `bridge.isNative`.

**Sprint 2 (ergonomics)**
5. Add the `event.repeat` guard to toggle/menu branches in `shortcuts.js`.
6. Promote `scrubby()` to a shared control and use it in the options bar,
   dialogs and adjustment fields (with arrow steps and arithmetic).
7. Enter-to-OK + initial focus + focus trap in `dialogs.js`.
8. Inline adjustment sliders in the Properties panel for the active adjustment.
9. OS drag-and-drop / paste file intake to the engine import path.
10. Named workspaces + a real `ws:` implementation over `openPanels`.

**Sprint 3 (parity / polish)**
11. Progress overlay with Cancel for long jobs.
12. Layers filter/search field.
13. Order/cycle/new-layer shortcuts (`Ctrl+]`, `Ctrl+[`, `Ctrl+Tab`,
    `Ctrl+Shift+N`, `Shift+Tab`, `X`, `D`).
14. Color-picker upgrade (eyedropper, Lab/CMYK, alpha, add to swatches).
15. Options-bar overflow affordance; `Alt+wheel` brush resize; zoom-step ladder.

---

## Appendix — quick parity checklist (Photopea ⇄ Fotox)

| Capability | Photopea | Fotox |
|---|---|---|
| Wheel zoom (default) | yes | Ctrl+wheel (config) |
| Alt+wheel brush size | yes | no |
| Scrub any number field | yes | partial (Layers/Properties only) |
| Arrow-key field stepping / arithmetic | yes | no |
| Enter = OK in dialogs | yes | no |
| Non-modal adjustment editing | yes | modal only |
| OS drag-drop / paste to open | yes | no |
| Named workspaces | yes | no (reset only) |
| Dock/float panels | yes | fixed dock (dead menu items) |
| Layers search/filter | yes | no |
| Progress + cancel | yes | status text only |
| Ctrl+Tab document cycling | yes | no |
| Ctrl+] / Ctrl+[ reorder | yes | no |
| Search every command | yes | dialog exists, no shortcut |
| Honest disabled menus | partial | **better** |
| Memory breakdown in status bar | partial | **better** |
| Virtualised layer list | yes | yes |
