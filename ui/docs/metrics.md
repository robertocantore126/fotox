# Metrics

This file records what was measured on the reference editor (a Photoshop-style
web editor running in Chromium, inspected live through DevTools-style probes)
and what Fotox does instead. Measurements were taken from computed styles and
`getBoundingClientRect()`, not from the reference stylesheet: our CSS is written
from scratch, only the *target sizes* are shared.

## Measured on the reference

| Property | Reference value | How it was measured |
| --- | --- | --- |
| Application background | `rgb(71, 71, 71)` = `#474747` | `getComputedStyle(document.body).backgroundColor` |
| Menu-bar button box | `22.6 px` tall, padding `2px 5px 3px` | computed style of a top-bar button |
| Menu-bar font | `13px "Open Sans", "Segoe UI", sans-serif` | computed style |
| Menu bar | 9 titles: File, Edit, Image, Layer, Select, Filter, View, Window, Other (+ Account) | DOM |
| Drop-down host | a pop-up container holding one floating panel per menu | DOM |
| Drop-down row | enabled/disabled class, plus a check slot, a label and a right cell | DOM |
| Row, right cell | either a shortcut string or a `6×10` chevron `<svg>` for submenus | DOM |
| Group separator | `<hr>` between logical groups | DOM |
| Panel skeleton | block → header + body; header carries the tab strip | DOM |
| Tool column | one column of tool buttons, groups exposed through flyouts | screenshots + DOM |
| Chrome vs document | chrome is DOM, the document itself is drawn on a canvas | canvas count in the DOM |

## Fotox values

| Property | Fotox | Note |
| --- | --- | --- |
| Application background | `#474747` | `--bg-app`, matches the reference |
| Chrome background | `#3f3f3f`, second level `#383838` | panels are a shade darker than the app |
| Panel header | `#3a3a3a`, `23 px` tall | same row rhythm as the menus |
| Menu row height | `23 px` | `--row-h` |
| Menu text | `13 px` `"Open Sans", "Segoe UI", system-ui, sans-serif` | same stack as the reference, with local fallbacks so the app also looks right offline |
| Menu-bar button | `22.6 px` tall, padding `2px 5px 3px` | identical to the measured box |
| Menu pop-up | `216 px` minimum width, `3 px` padding, `1 px` border, 3 px radius | our own skin, same proportions |
| Tool button | `30 × 25 px` in a `44 px` column | chosen so 22 slots + colour block fit a 900 px window without scrolling |
| Dock | `262 px`, resizable `210 … 420 px` | |
| Status bar | `24 px`, right-aligned readouts | |
| Rulers | `18 px` thick, ticks every 1/2/5/10/25/50/100/250/500 document px depending on zoom | labels rotate on the vertical ruler |
| Zoom range | `3 % … 3200 %`, wheel with `Ctrl`, fit on screen, actual pixels | |
| Demo document | `1080 × 1080`, RGB/8, `72 ppi` | drawn procedurally, no image asset |

## Decisions worth knowing

* **Fonts.** The reference loads Open Sans from Google Fonts. Fotox asks for the
  same family first but always keeps local fallbacks, so nothing is fetched from
  the network and the app runs offline.
* **Icons.** 130 strokes-based symbols live in `assets/icons.svg` on a 20×20
  grid. They are inlined at startup with presentation attributes directly on the
  shapes rather than referenced with `<use>`: referencing a `<symbol>` means the
  computed values set on the symbol do not travel into the shadow tree, which
  makes sprites fragile. Inlining also keeps them colourable with `currentColor`.
* **Colour.** The chrome is intentionally neutral (`#474747` family) with one
  accent (`#2f6df6`) used consistently for selection, active tools, active tabs
  and the Account button, plus the reference's cyan guide colour.
* **No external requests.** Not even a favicon: the only network traffic is the
  app's own files.
