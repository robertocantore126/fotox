// Fotox — the Layer Comps panel in the app (M12-T06).
//
// The engine sends the document's comps (names, the applied one). A click
// applies a comp; double-click renames it; the buttons record a new comp,
// update the selected one, step to the previous / next, delete.

import { h, icon, clear } from "../el.js";
import { openDialog } from "../dialogs.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { activeDocument } from "./documents.js";

const byDoc = new Map(); // doc → { names, active }
let root = null;

function send(id, args = {}) {
  bridge.send({ type: UI.ACTION, id, args });
}

export function compsPanel() {
  if (!root) root = h("div", { class: "pcomps native" });
  render();
  send("comps:refresh");
  return root;
}

function newComp() {
  openDialog("new-layer-comp", {
    onOk: (v) => send("comps:new", { name: v["Name:"] || "", visibility: !!v.Visibility, position: !!v.Position, appearance: !!v["Appearance (Layer Style)"] }),
  });
}

function render() {
  if (!root) return;
  clear(root);
  const { names, active } = byDoc.get(activeDocument()) || { names: [], active: null };
  const list = h("div", { class: "plist" });
  names.forEach((name, i) => {
    const label = h("span", { class: "plist-label", text: name });
    label.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      const value = prompt("Layer comp name", name);
      if (value) send("comps:rename", { index: i, name: value });
    });
    list.append(h("div", { class: "plist-row" + (i === active ? " sel" : ""), onclick: () => send("comps:apply", { index: i }) },
      h("span", { class: "pthumb" }, icon(i === active ? "i-check" : "i-presets", "ic sm")), label));
  });
  if (!names.length) list.append(h("div", { class: "pnote", text: "No layer comps yet. Record one with +." }));
  const btn = (ic, tip, fn) => h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
  root.append(list, h("div", { class: "pbar" },
    btn("i-chevron-left", "Apply previous layer comp", () => send("comps:prev")),
    btn("i-chevron-right", "Apply next layer comp", () => send("comps:next")),
    btn("i-redo", "Update the layer comp", () => send("comps:update")),
    btn("i-plus", "Create new layer comp", newComp),
    btn("i-trash", "Delete layer comp", () => send("comps:delete")),
  ));
}

export function initComps() {
  bridge.on(ENGINE.COMPS, (m) => {
    byDoc.set(m.doc, { names: m.names || [], active: m.active ?? null });
    if (m.doc === activeDocument()) render();
  });
  bridge.on(ENGINE.ACTIVE_DOCUMENT, () => setTimeout(render, 0));
}
