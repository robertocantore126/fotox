# Fotox

An original, from-scratch recreation of the **interface** of a Photoshop-style
web editor: menu bar with full drop-down trees, context-sensitive options bar,
tool column with flyouts, resizable panel dock and the whole family of dialogs.

Fotox is a **navigable interface mock**. Every menu entry, drop-down, panel,
flyout and dialog responds exactly like the real thing; the image operations
behind them are deliberately not implemented. Nothing is read from disk, nothing
is written to disk, no file is uploaded anywhere.

No asset of any other product is used in this project: the code, the layout,
the colour set, the ~130-icon SVG set and the name are original. Structure and
command names follow the conventions of Photoshop-style editors (which are
functional and not protectable); the icons, the stylesheet and the branding are
ours.

---

## Running it

The app is a set of static files with **no dependencies and no build step**, but
it uses ES modules and `fetch()`, so it needs to be served over HTTP rather than
opened with `file://`.

```bash
cd fotox/ui
python tools/serve.py               # port 5500 by default
```

The server prints the address and opens your browser on it:

```
  Fotox è servito qui:   http://127.0.0.1:5500/
                         http://localhost:5500/
  radice:                …\fotox
  fermare con Ctrl+C.
```

`tools/serve.py` wraps Python's static server and adds what that server gets
wrong for development:

* `Cache-Control: no-store`, so a reload really picks up your edits
  (`python -m http.server` lets the browser use a heuristic cache that swallows
  changes);
* **a free port when the requested one is busy** — it probes with a real
  connection, prints `!! porta 5500 già occupata …` and moves to 5501, instead of
  dying with a `WinError 10048` traceback;
* it listens on `127.0.0.1` **and** on `::1`, so `http://localhost:5500/` works on
  machines where `localhost` resolves to IPv6 first;
* it refuses to start on a folder without `index.html`, which is how you catch
  "I started the server in the wrong directory";
* `--no-open` skips opening the browser. An optional second argument serves a
  different folder.

There is no package.json, no npm install, no bundler. Any static server works
(`npx serve`, `php -S`, nginx, …), but the browser must reach the page over
**HTTP**: ES modules are blocked on `file://`, so double-clicking `index.html`
gives an empty page.

### If you see a blank page

The page explains itself instead of staying silent: `index.html` carries a small
classic-script bootstrap that catches load and runtime errors and, if nothing has
rendered after 1.5 s, prints a card with the failing file, the URL, the protocol,
whether the browser supports ES modules, the script count and the user agent.

In practice a blank page means one of these:

| Cause | Fix |
| --- | --- |
| `index.html` opened from disk (`file://`) | start `tools/serve.py` and use the printed `http://` URL |
| the server was started in the wrong folder | start it from the `fotox` folder (the script now says so) |
| an old server instance still held the port | the script detects it and prints the port it moved to |
| JavaScript disabled, or an extension blocking `127.0.0.1` | check the console with `F12` |

---

## What is implemented

| Area | State |
| --- | --- |
| Menu bar (9 menus, **507 commands**, 51 submenus) | complete: labels, shortcuts, greyed-out entries, check marks, separators |
| Menu behaviour | click to open, hover to switch, submenu flyouts with edge flipping, click-away and `Esc` to close, full arrow-key navigation, `Alt`+letter accelerators |
| Tool column | **22 slots / 68 tools**, long-press and right-click flyouts, active state, tooltips with shortcut letters, foreground/background swatches with swap and reset |
| Options bar | a bespoke control set per tool: selects, toggles, numeric fields, sliders, icon groups, brush and gradient previews — all interactive |
| Panel dock | **23 panels** in 8 collapsible tab groups, drag handle to resize, per-group menus, list/grid/timeline/histogram/navigator contents, all built from mock data |
| Dialogs | **136 dialog definitions, 135 reachable from a menu entry**: presets lists, image-size and canvas-size with anchor grid, levels with histogram, curves you can click to add points, layers styles, filter gallery, colour picker, preferences, keyboard shortcuts, About… draggable, modal, `Esc` to dismiss |
| Diagnostics dialog | reads back the real renderer, user agent, DPR and viewport, each time it is opened |
| File commands | `Open`, `Save`, `Save As` (14 formats), `Export as` (7), `Print`, `Export Layers`, `File Info` are active because their dialogs exist; the entries that stay greyed depend on live state (a selection, an undo history, a saved file) |
| Workspace | procedurally drawn demo document, zoom (3 %–3200 %) with fit/actual-pixels, rulers that follow zoom and scroll, grid, guides, scrollbars, wheel-zoom with `Ctrl`, pan with the hand tool / middle button / `Alt`-drag, document tab strip |
| Status bar | live message area, zoom drop-down, document size, mode, memory readout |
| Keyboard | `Ctrl+N/O/S/P/L/M/U/B/K/T/0/1/R/H/;`, `F5`–`F11`, `Tab` for the panels, `F` cycles screen modes, single letters select tools, arrows drive the menus |
| Screen modes | standard, full screen with menu bar, full screen |

Every command that has no implementation still answers: it shows a toast and a
status-bar message, so no entry in the interface is mute.

## What is deliberately missing

* Pixels are never edited: the tools select, they do not paint.
* No file I/O, no PSD parsing, no export: the dialogs are complete but inert.
* No account, cloud, plugins, ads or telemetry; the Account/Cloud entries are
  part of the mock.

---

## Layout of the project

```
index.html               shell + bootstrap diagnostic (explains a blank page instead of showing one)
css/theme-dark.css       design tokens, reset, typography, generic buttons
css/app.css              application shell and workspace
css/menus.css            popups: menus, drop-downs, tooltips, toasts
css/panels.css           dock and every panel content kind
css/dialogs.css          modal windows and every field type
assets/icons.svg         130 original SVG symbols, one file, stroke-based
js/main.js               startup: builds the chrome and wires the engines together
js/state.js              application state + tiny event emitter
js/el.js                 DOM helpers and the icon factory
js/icons.js              sprite loading and inlining
js/popup.js              popup stack: menus, submenus, drop-downs, positioning
js/menu.js               menu bar, menu trees, keyboard navigation, context menus
js/panels.js             dock, tab groups, panel content renderers
js/optionsbar.js         options bar control renderers
js/dialogs.js            modal engine and all field renderers
js/canvas.js             workspace: document drawing, zoom, pan, rulers, guides
js/tooltip.js            delegated tooltips, toasts, status messages
js/shortcuts.js          global keyboard map
js/actions.js            action dispatcher for every menu entry
js/data/menus.js         the 507 commands
js/data/tools.js         the 68 tools and their groups
js/data/options.js       one options-bar schema per tool
js/data/panels.js        dock layout, panel definitions, mock content
js/data/dialogs.js       all dialog definitions (explicit + compact filter table)
tools/serve.py           development static server (free port, IPv4+IPv6, opens the browser)
tools/check-data.mjs     data consistency check: menus → dialogs, panels, tools, icons, reachability
docs/menu-coverage.md    per-menu coverage and how it was checked
docs/metrics.md          measured metrics and the decisions behind them
```

### Checking it

```bash
node tools/check-data.mjs
```

Imports the data modules in Node (no browser, no test framework, no
dependencies) and fails if a menu entry points at a dialog, panel, tool or icon
that does not exist. It also reports how many dialogs are actually reachable from
an *enabled* entry — a dialog whose only entry is greyed out is a window the user
will never see. Current output ends with
`✓ nessun problema: ogni azione, pannello, strumento e icona esiste.`

### Adding a command

1. add an entry in `js/data/menus.js` — `{ label, short, a, chk, dis, sub }`
   (`a` is the action id),
2. if it needs a window, describe it in `js/data/dialogs.js`,
3. if it needs behaviour rather than a "not implemented" toast, handle its
   prefix in `js/actions.js`.

Nothing else has to change: the menu engine, the keyboard navigation and the
check-mark state all come for free.

---

## Notes on fidelity

Metrics were measured on a live Photoshop-style editor running in Chromium and
then rebuilt in our own stylesheet (see `docs/metrics.md`): 23 px menu rows,
13 px menu text, 22.6 px menu-bar buttons, `#474747` application background,
the wording of greyed-out entries on an untitled document, and so on. The drop-down vocabulary
(the order of the nine menus, the shape of each group, where separators sit) was
transcribed by driving that editor's DOM.
