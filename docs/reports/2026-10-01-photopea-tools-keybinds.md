# Photopea — tools, keybinds and workflow extraction

**Date:** 2026-10-01
**Question answered:** can we pull Photopea's tool list, its keyboard shortcuts,
and a record of a workflow (a JSON of "what happened") out of Photopea?

**Short answer:** tools and keybinds — yes, reliably (data file:
`docs/reports/2026-10-01-photopea-tools-keybinds.json`). Workflow — yes, but with
a choice: Photopea's built-in **Actions recorder** gives a *semantic* list (export
as `.ATN`), while a **Live-Messaging harness** or **browser input capture** gives a
*JSON log*. There is no public "command stream" of UI actions, so a recorder has
to hook one of those three surfaces.

---

## 1. What was extracted, and from where

| Part | Source | Reliability |
|---|---|---|
| Tool IDs | Official API → https://www.photopea.com/api/environment | Authoritative (Photopea publishes it) |
| Key letters | https://defkey.com/photopea-5-4-shortcuts **and** a 2025 gist | Two independent lists, cross-checked |
| Scripting model | https://www.photopea.com/learn/scripts | Authoritative |
| Plugin / message channel | https://www.photopea.com/api/plugins, /api/live | Authoritative |
| Workflow recording | https://www.photopea.com/learn/actions | Authoritative |
| In-app ground truth | `Edit → Keyboard Shortcuts…` (shortcut `?`) | Preferred, but not scraped here |

> Note on this environment: the live preview reported **"Ad blocking detected"**
> and the editor's top-level `window.app` was **not reachable** from the page
> console, so DOM-scraping Photopea's own Keyboard-Shortcuts window was not
> reliable from here. The table below therefore comes from the API docs plus the
> two corroborating listings. In a normal browser the in-app window is the
> ground truth and should be preferred (`?`, or **More → Keyboard Shortcuts**).

---

## 2. Tools (with Photopea tool IDs)

Tool IDs are from the Environment API — these numeric IDs are what you use in
`environment.showtools`, `environment.panels`, `topt`/`tmnu` config, etc.
A letter in brackets means several tools share that key (a flyout group).

| ID | Tool | Key | Group |
|---:|------|-----|-------|
| 0  | Move Tool | V | move |
| 70 | Artboard Tool | V | move |
| 1  | Rectangle Select | M | marquee |
| 2  | Ellipse Select | M | marquee |
| 5  | Lasso Select | L | lasso |
| 6  | Polygonal Lasso Select | L | lasso |
| 7  | Magnetic Lasso Select | L | lasso |
| 9  | Magic Wand | W | select |
| 8  | Quick Selection | W | select |
| 3  | Object Selection | W | select |
| 10 | Crop Tool | C | crop |
| 11 | Perspective Crop | C | crop |
| 12 | Slice Tool | C | crop |
| 13 | Slice Select Tool | C | crop |
| 14 | Eyedropper | I | sample |
| 15 | Color Sampler | I | sample |
| 16 | Ruler | I | sample |
| 18 | Spot Healing Brush Tool | J | heal |
| 19 | Healing Brush Tool | J | heal |
| 90 | Magic Replace | J | heal |
| 20 | Patch Tool | J | heal |
| 21 | Content-Aware Move Tool | J | heal |
| 22 | Red Eye Tool | J | heal |
| 23 | Brush Tool | B | paint |
| 24 | Pencil Tool | B | paint |
| 25 | Color Replacement | B | paint |
| 27 | Clone Tool | S | clone |
| 31 | Eraser Tool | E | eraser |
| 32 | Background Eraser | E | eraser |
| 33 | Magic Eraser | E | eraser |
| 34 | Gradient Tool | G | fill |
| 35 | Paint Bucket Tool | G | fill |
| 36 | Blur Tool | *(unconfirmed)* | focus |
| 37 | Sharpen Tool | *(unconfirmed)* | focus |
| 38 | Smudge Tool | *(unconfirmed)* | focus |
| 39 | Dodge Tool | O | tone |
| 40 | Burn Tool | O | tone |
| 41 | Sponge Tool | O | tone |
| 47 | Type Tool | T | type |
| 48 | Vertical Type Tool | T | type |
| 42 | Pen | P | pen |
| 43 | Free Pen | P | pen |
| 44 | Curvature Pen | P | pen |
| 45 | Add Anchor Point | P | pen |
| 46 | Delete Anchor Point | P | pen |
| 72 | Convert Point | P | pen |
| 51 | Path Select | A | path |
| 52 | Direct Select | A | path |
| 54 | Rectangle (shape) | U | shape |
| 55 | Ellipse (shape) | U | shape |
| 57 | Line (shape) | U | shape |
| 56 | Parametric Shape | U | shape |
| 58 | Custom Shape | U | shape |
| 59 | Hand Tool | H | view |
| 60 | Rotate View | H | view |
| 61 | Zoom Tool | Z | view |

The documented **tool-option formats** (useful for reading Photopea's option-bar
state):

- **Move**: `Options [autoSelect, transformControls, distances]`;
  menu flags `[Auto-Select, Transformation controls, Distances, Quick Save (Get
  PNG…), Vertical Align, Horizontal Align]`.
- **Magic Wand**: `Options [combiningOperation, feather, [tolerance, antiAlias, contiguous]]`.
- **Crop**: `Options [constraintMode, width, height]`.

---

## 3. Keybinds

### Menus

| Command | Keys |
|---|---|
| Open | Ctrl+O |
| Save | Ctrl+S |
| Save as PSD | Shift+Ctrl+S |
| Export as | Alt+Shift+Ctrl+S |
| New Project | Alt+Ctrl+N *(unconfirmed)* |
| Step Forward | Shift+Ctrl+Z |
| Step Backward | Ctrl+Z |
| Cut / Copy / Paste | Ctrl+X / Ctrl+C / Ctrl+V |
| Clear | Delete |
| Fill | Alt+Backspace |
| Free Transform | **Alt+Ctrl+T** |
| Preferences | Ctrl+K |
| Find | Ctrl+F |
| Levels / Curves | Ctrl+L / Ctrl+M |
| Hue/Saturation | Ctrl+U |
| Invert | Ctrl+I |
| New Layer | Shift+Ctrl+N |
| Layer via Copy | Ctrl+J |
| Clipping Mask | Alt+Ctrl+G |
| Group Layers | Ctrl+G |
| Merge Down | Ctrl+E |
| Select All / Deselect / Inverse | Ctrl+A / Ctrl+D / Shift+Ctrl+I |
| Zoom In / Out | Ctrl++ / Ctrl+- |
| Rulers / Guides / Grid | Ctrl+R / Ctrl+; / Ctrl+' |
| Show Keyboard Shortcuts | ? |

### Brush & colour

| Command | Keys |
|---|---|
| Decrease / Increase Brush Size | `[` / `]` |
| Decrease / Increase Hardness | `{` / `}` |
| Default Colours (white/black) | D |
| Swap Colours | X |
| Quick Mask Mode | Q |

### Navigation & held keys

| Action | Input |
|---|---|
| Vertical scroll | Wheel |
| Horizontal scroll | Ctrl+Wheel |
| Zoom | Alt+Wheel |
| Move tool (held) | Ctrl |
| Hand tool (held) | Space |
| Zoom tool (held) | Ctrl+Space |

Full tool→key map is in the JSON (`tools[].keys`).

---

## 4. Extracting a workflow → JSON

Photopea does **not** expose a public event stream of UI commands. Three viable
routes:

### A — Actions panel (semantic, replayable)
`Window → Actions` → New Action Set → New Action → **Record** → do the work →
stop → **Export** as `.ATN`.
- Output: `.ATN` (Adobe action format, one action set, ordered Steps).
- Convert to JSON by parsing the `.ATN` (action step records carry the operation
  name and parameters).
- Pro: semantic names, replayable in Photopea; no code injection.
- Con: not every interaction is recordable (freehand strokes and some tool
  gestures are not action steps); binary needs a parser.

### B — Live-Messaging harness (JSON log, scriptable)
Embed Photopea in your own page and use the same channel its plugins use:

```html
<!-- host page -->
<iframe id="pp" src="https://www.photopea.com/#%7B%22environment%22%3A%7B%7D%7D"></iframe>
<script>
  const pp = document.getElementById("pp");
  const log = [];
  // Photopea -> us: app.echoToOE(...) arrives as a message event
  window.addEventListener("message", (e) => {
    if (e.source !== pp.contentWindow) return;
    log.push(e.data);
  });
  // us -> Photopea: run a script inside the app
  function run(script) { pp.contentWindow.postMessage(script, "*"); }
  run(`
    (function(){
      if (window.__rec) return; window.__rec = 1;
      const send = (o) => { try { App.echoToOE(JSON.stringify(o)); } catch(e){} };
      // 1) key events -> command guess from the keybind map
      addEventListener("keydown", (e) => send({ t: Date.now(), type: "key",
        key: e.key, ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, meta: e.metaKey }), true);
      // 2) state snapshots, diffed later
      setInterval(() => { try {
        const d = app.activeDocument; if (!d) return;
        send({ t: Date.now(), type: "state", w: d.width, h: d.height,
               layers: d.layers.length, active: d.activeLayer && d.activeLayer.name });
      } catch(e){} }, 1000);
      send({ type: "ready" });
    })();
  `);
</script>
```

- The recorder script runs **inside** the Photopea iframe, where `app` / `App`
  live; results come back through `App.echoToOE`.
- Pro: one place to hook the public API, DOM events, and state polling.
- Con: only public-API calls and DOM events are visible — the internal UI emits
  no public command event, so menu actions are inferred from keys/clicks.

### C — Browser input capture + state diffing
Record `keydown`/menu clicks in an open Photopea tab and reconstruct commands
from the keybind map in §3; snapshot `app.activeDocument` on a timer and diff.
Captures everything the user pressed, but at input level, not semantic.

**Recommended:** A when you want a clean replayable operation list; B (or B+C)
when you want a continuous JSON of a live session.

---

## 5. Fotox alignment (where Fotox's shortcuts differ from Photopea)

From `ui/js/shortcuts.js` vs the table above — the notable divergences:

| Action | Photopea | Fotox | Note |
|---|---|---|---|
| Free Transform | Alt+Ctrl+T | Ctrl+T | Fotox follows Photoshop; Photopea differs. Pick one and document it. |
| Ctrl+F | Find | Last Filter | Fotox's top-bar search button advertises Ctrl+F — real conflict (see the global UX review). |
| Horizontal scroll | Ctrl+Wheel | Wheel scrolls | Fotox browser mode: plain wheel scrolls, Ctrl+wheel zooms. |
| Zoom | Alt+Wheel | Ctrl+Wheel | Opposite modifier. |
| Brush hardness | `{` / `}` | Shift+`[` / `]` | Divergence. |
| Default / Swap colours | D / X | buttons only | Fotox has no D/X keys. |
| Show shortcuts | ? | (dialog exists, no key) | `dlg:shortcuts` is unbound. |
| Quick Mask | Q | Q | match (Fotox treats Q as a mode toggle). |
| Save as PSD | Shift+Ctrl+S | Ctrl+Shift+S = Save As | Fotox's Save As is generic. |
| Copy Merged / paste-in-place / merge-visible | — | Ctrl+Shift+C, Ctrl+Shift+V, Ctrl+Shift+E | Fotox extras beyond Photopea's list. |

Fotox already matches on: tool letters, Shift+letter flyout cycling, `[`/`]`
size, number-key opacity, Space pan, Ctrl+Z / Ctrl+Shift+Z, Ctrl+A/D/Shift+I,
Ctrl+G/E/J, Ctrl+L/M/U, Ctrl+R/;/', Ctrl++/-/0/1.

---

## 6. Refreshing this extraction

1. **Preferred:** open `https://www.photopea.com` in a normal browser, press `?`
   (or **More → Keyboard Shortcuts**), and copy the table — that is the current
   ground truth.
2. Tool IDs: they are stable and documented at
   https://www.photopea.com/api/environment — re-check after major releases.
3. Workflow: use method A for a replayable list, or run the harness in method B
   for a live JSON log.

Raw data: `docs/reports/2026-10-01-photopea-tools-keybinds.json`.
