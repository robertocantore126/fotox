// Fotox — the Paths panel in the app (M10-T01).
//
// The engine sends the document's Work Path flag, the saved paths' names and
// the selected path; a click selects one (the pen tools then edit it), the
// buttons fill / stroke / load / make / save / delete.

import { h, icon, clear } from "../el.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { activeDocument } from "./documents.js";

const byDoc = new Map(); // doc → { work, paths, active }
let root = null;

function send(id, args = {}) {
  bridge.send({ type: UI.ACTION, id, args });
}

const same = (a, b) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

export function pathsPanel() {
  if (!root) root = h("div", { class: "ppaths native" });
  render();
  return root;
}

function render() {
  if (!root) return;
  clear(root);
  const { work, paths, active } = byDoc.get(activeDocument()) || { work: false, paths: [], active: null };
  const list = h("div", { class: "plist" });
  const row = (label, target, italic) => {
    const name = h("span", { class: "plist-label", text: label, style: italic ? { fontStyle: "italic" } : null });
    if (target !== "work") {
      name.addEventListener("dblclick", (e) => {
        e.stopPropagation();
        const value = prompt("Path name", label);
        if (value) send("path:rename", { target, name: value });
      });
    }
    const r = h("div", { class: "plist-row" + (same(active, target) ? " sel" : ""), onclick: () => send("path:select", { target: same(active, target) ? null : target }) },
      h("span", { class: "pthumb path" }, icon("i-paths", "ic sm")), name);
    r.addEventListener("click", (e) => { if (e.ctrlKey || e.metaKey) { e.stopPropagation(); send("path:to-selection", { target }); } });
    list.append(r);
  };
  if (work) row("Work Path", "work", true);
  paths.forEach((name, i) => row(name, { saved: i }, false));
  if (!work && !paths.length) list.append(h("div", { class: "pnote", text: "No paths. Draw one with the Pen tool (P)." }));
  const btn = (ic, tip, fn) => h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
  root.append(list, h("div", { class: "pbar" },
    btn("i-color", "Fill path with foreground colour", () => send("path:fill")),
    btn("i-shape-line", "Stroke path with brush", () => send("path:stroke", { tool: "brush" })),
    btn("i-marquee", "Load path as a selection", () => send("path:to-selection")),
    btn("i-pen", "Make work path from selection", () => send("path:from-selection", { tolerance: 2 })),
    btn("i-plus", work ? "Save the Work Path" : "Create new path", () => (work ? send("path:save", { name: "" }) : send("path:new"))),
    btn("i-trash", "Delete path", () => send("path:delete")),
  ));
}

export function initPaths() {
  bridge.on(ENGINE.PATHS, (m) => {
    byDoc.set(m.doc, { work: m.work, paths: m.paths || [], active: m.active });
    if (m.doc === activeDocument()) render();
  });
  bridge.on(ENGINE.ACTIVE_DOCUMENT, () => setTimeout(render, 0));
}
