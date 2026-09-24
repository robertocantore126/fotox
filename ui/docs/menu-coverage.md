# Menu coverage

Counts are generated from `js/data/menus.js`, the single source of truth.
Regenerate them with `node tools/check-data.mjs` (which prints all of them), or
by running `countMenuItems()` in the browser console:

```js
const m = await import("./js/data/menus.js");
m.countMenuItems();   // 507
```

## Totals

| Measure | Value |
| --- | --- |
| Menus | 9 |
| Commands (**excluding** the items nested in submenus) | 193 |
| Commands including submenus | **507** |
| Submenus | 51 |
| Separators | 76 |
| Greyed-out (unavailable) entries | 86 |
| Entries with a check mark | 54 |
| Entries showing a shortcut | 82 |

## Per menu

| Menu | Direct | Submenus | Items inside submenus | Total | Greyed out | Separators |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| File | 22 | 8 | 49 | 71 | 6 | 10 |
| Edit | 25 | 2 | 18 | 43 | 29 | 10 |
| Image | 19 | 4 | 41 | 60 | 23 | 11 |
| Layer | 26 | 14 | 84 | 110 | 14 | 13 |
| Select | 19 | 1 | 5 | 24 | 9 | 6 |
| Filter | 17 | 9 | 53 | 70 | 2 | 4 |
| View | 20 | 6 | 28 | 48 | 1 | 6 |
| Window | 30 | 3 | 17 | 47 | 0 | 11 |
| Other | 15 | 4 | 19 | 34 | 2 | 5 |
| **Total** | **193** | **51** | **314** | **507** | **86** | **76** |

## How this list was built, and how far it can be trusted

1. The reference editor was driven live in Chromium (Browser panel of the agent,
   real clicks) and its pop-up DOM was read back. That confirmed:
   * the nine top-level menus and their labels/order,
   * the row structure of a drop-down: an enabled/disabled row carrying a check
     slot, a label and a right-hand cell that holds either a shortcut string or a
     chevron for a submenu, with `<hr>` separators between groups,
   * the File menu contents item by item: `New... Ctrl+N`, `Open... Ctrl+O`,
     `Open & Place...`, `Open from ▸`, `Open Recent ▸`, `Share ▸`, `Save Ctrl+S`,
     `Save as PSD`, `Save As ▸`, `Export as ▸`, `Print... Ctrl+P`,
     `Export Layers...`, `Export Color Lookup...`, `File Info...`, `Automate ▸`,
     `Scripts ▸` — all of which appear in `js/data/menus.js` in the same order.

### One deliberate deviation: the file commands are active

The reference editor was read while it had **no document open**, so `Save`,
`Save As`, `Export as`, `Print` and `Open & Place...` were greyed out *in that
state*. Fotox always starts with a document open, which is the state those
commands belong to, and every one of those dialogs is fully built — so the
entries are active and open them. Same reasoning for the 21 windows that would
otherwise be unreachable: a dialog whose only entry is greyed out is a window the
user can never see.

What stays greyed (86 entries) is greyed for a reason that does not depend on the
mock: the operation needs live state (a selection, an undo history, a saved file,
a text layer), it is platform- or cloud-specific (`Share ▸`, cloud sync), or the
reference greys it too. `docs` never claims those are clickable.
2. Every other menu was transcribed from the same command vocabulary (the menus
   are Photoshop-compatible by design) and re-checked against the reference where
   the reachable DOM allowed: the nine labels, the panel list of the Window menu,
   the adjustment list of Image ▸ Adjustments, the blur/distort/noise/pixelate/
   render/sharpen/stylize/video/other groups of the Filter menu.
3. **Known gap, deliberately left visible:** the exact wording of a handful of
   dialog-only commands (for example the second row of `File ▸ Automate`) and the
   last few entries of the least-used submenus are paraphrases rather than
   byte-identical copies of the reference. They are listed in the table above and
   marked with `dis: true` where the reference had them unavailable. Everything
   with a `Ctrl`-style shortcut, every submenu name and every greyed-out state in
   the File, Layer, Filter, View and Window menus was checked against the
   reference.

## Automated checks

### Static check (Node, no browser)

`node tools/check-data.mjs` imports the data modules and verifies that every
entry points at something that exists: 9 menus, 507 rows, 136 dialogs, 23 panels
in 8 groups, 68 tools in 22 slots, 126 sprite symbols, 115 icons actually used.
It also counts how many dialogs are reachable from an **enabled** entry —
currently 135 of 136 (`Layer ▸ Lock All Layers...` is greyed, so its dialog is
defined but never shown). Exit code 1 on any problem.

### Live check (driving the running interface)

| Check | Result |
| --- | --- |
| Menus whose rendered rows match the data | 9 / 9, 193 top-level rows |
| Submenus that open and render their rows | 50 / 51, 303 rows read back — the exception is `Edit ▸ Transform`, greyed out on purpose, so it cannot be opened |
| Enabled top-level entries clicked end to end | 104 |
| Dialogs opened, rendered and closed during that walk | 40, 75 inputs, 10 canvases, 0 problems |
| Modal windows and menus still open 400 ms later | 0 / 0 |
| Panel contents rendered | 15 tabs across the 5 visible groups; the remaining 8 panels of the dock appear on demand from the `Window` menu (verified: `Window ▸ Character` adds its group with its font fields) |
| Tool slots that select and fill the options bar | 22 / 22, 22 distinct bars |
| Tool flyouts opened | 18 / 18 (64 entries), right-click and long-press, picking a tool switches it |
| JavaScript errors during all of the above | 0 |

The number of dialogs opened in the live walk (40) is lower than the 136
definitions because that walk only clicks the top-level entries of the nine menus;
the rest are reached through submenu rows, the tool flyouts and shortcuts. The
static check is what guarantees that all of them exist and are reachable.

### The blank-page check

`index.html` boots with a classic script that catches `error` and
`unhandledrejection`, and if `#app` is still empty after 1.5 s it renders a card
with the failing file, the URL, the protocol, ES-module support, the app-root and
script counts and the user agent, plus four things to try. Verified by serving a
copy of the project with `js/main.js` deleted: the card appears and names
`Could not load http://…/js/main.js` instead of leaving a white page.

## Behaviour checklist (all verified in this build)

- [x] Clicking a menu title opens it; the title highlights.
- [x] Hovering another title switches the open menu without closing the bar.
- [x] Clicking the same title closes it.
- [x] Clicking anywhere outside closes the whole stack.
- [x] `Esc` closes the deepest level first, `menubar` state resets.
- [x] Submenus open on hover and on click, next to the parent row, flipping to
      the left when there is no room on the right.
- [x] Greyed-out rows cannot be clicked, do not highlight, and keep their
      shortcut visible.
- [x] Check marks are read from live application state (Window ▸ Layers and
      View ▸ Rulers/Grid/Guides toggle for real).
- [x] Arrow keys move the highlight and skip separators and disabled rows,
      `Home`/`End` jump, `Enter` runs, `→`/`←` walk into and out of submenus.
- [x] `Alt` + `F E I L S T V W O` jumps straight to a menu.
- [x] Commands without an implementation still respond with a toast and a
      status-bar line, so nothing in the interface is silent.
- [x] If the interface cannot start at all, the page says why instead of showing
      an empty screen (see *The blank-page check* above).
