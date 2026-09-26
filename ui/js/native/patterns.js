// Fotox — patterns in the app (M8-T06): the Patterns panel, the option
// bar's pattern picker (Pattern Stamp, Paint Bucket), Layer ▸ New Fill Layer
// ▸ Pattern and its Layer Content Options.
//
// The engine owns the library (`%APPDATA%\Fotox\patterns.json`) and sends
// it as `patterns`; the current pattern is the one the tools and Edit ▸ Fill
// ▸ Pattern use.

import { h, icon, clear } from "../el.js";
import { emit, on } from "../state.js";
import { openDialog } from "../dialogs.js";
import { openDropdown } from "../popup.js";
import { registerControl } from "../optionsbar.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { activeLayerInfo, sendCommand, activeLayerId } from "./layers-panel.js";
import { pickFile } from "./brush-settings.js";

let patterns = [];
let current = null;
let root = null;
const pickers = new Set();

function send(id, args = {}) {
  if (bridge.isNative) bridge.send({ type: UI.ACTION, id, args });
}

/** A 48 × 48 RGBA8 thumbnail (base64) as a canvas. */
function thumb(p, size = 32) {
  const canvas = h("canvas", { class: "pattern-thumb", width: 48, height: 48, style: { width: size + "px", height: size + "px" } });
  if (!p?.thumb) return canvas;
  const bytes = atob(p.thumb);
  const ctx = canvas.getContext("2d");
  const img = ctx.createImageData(48, 48);
  for (let i = 0; i < img.data.length && i < bytes.length; i++) img.data[i] = bytes.charCodeAt(i);
  ctx.putImageData(img, 0, 0);
  return canvas;
}

function use(id) {
  current = id;
  send("pattern:use", { id });
  render();
  for (const refresh of pickers) refresh();
  emit("brush:changed");
}

/* ------------------------------------------------------------------ panel */

export function patternsPanel() {
  if (!root) root = h("div", { class: "ppatterns native" });
  render();
  return root;
}

function render() {
  if (!root) return;
  clear(root);
  const grid = h("div", { class: "pattern-grid", style: { display: "flex", flexWrap: "wrap", gap: "4px", padding: "6px" } });
  for (const p of patterns) {
    grid.append(h("button", {
      class: "pattern-cell" + (p.id === current ? " sel" : ""), type: "button", "data-tip": `${p.name} (${p.width} × ${p.height})`,
      style: { outline: p.id === current ? "2px solid var(--accent, #4c8dff)" : "none", padding: "0", border: "0", background: "none" },
      onclick: () => use(p.id),
      ondblclick: () => { const name = prompt("Pattern name", p.name); if (name) send("pattern:rename", { id: p.id, name }); },
    }, thumb(p, 40)));
  }
  const btn = (ic, tip, fn) => h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
  root.append(grid, h("div", { class: "pbar" },
    btn("i-plus", "Define Pattern from the selection", () => send("misc:define-pattern-sel")),
    btn("i-folder", "Import a PNG as a pattern…", async () => {
      const file = await pickFile("image/png");
      if (file) send("pattern:import", { data: file.data, name: file.name });
    }),
    btn("i-trash", "Delete pattern", () => { if (current != null) send("pattern:delete", { id: current }); }),
  ));
}

/* --------------------------------------------------------- option picker */

function pickerControl() {
  const box = h("span", { class: "ob-pattern-thumb" });
  const refresh = () => {
    clear(box);
    const p = patterns.find((q) => q.id === current);
    box.append(thumb(p, 20));
    el.dataset.tip = p ? p.name : "Pattern";
  };
  const el = h("button", {
    class: "ob-gradient", type: "button", "data-tip": "Pattern",
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({
        anchor: el, items: patterns.map((p) => p.name), value: patterns.find((p) => p.id === current)?.name || "", width: 180,
        onPick: (name) => { const p = patterns.find((q) => q.name === name); if (p) use(p.id); },
      });
    },
  }, box, icon("i-chevron-down", "ic xs"));
  pickers.add(refresh);
  refresh();
  return { el, read: () => current };
}

/* ------------------------------------------------------ fill layer dialog */

export function patternList(selected, onPick) {
  const list = h("div", { class: "pattern-grid", style: { display: "flex", flexWrap: "wrap", gap: "4px", maxHeight: "140px", overflow: "auto" } });
  const draw = () => {
    clear(list);
    for (const p of patterns) {
      list.append(h("button", {
        type: "button", "data-tip": p.name, style: { outline: p.id === selected.id ? "2px solid var(--accent, #4c8dff)" : "none", padding: "0", border: "0", background: "none" },
        onclick: () => { selected.id = p.id; onPick?.(p.id); draw(); },
      }, thumb(p, 36)));
    }
  };
  draw();
  return list;
}

/** Layer ▸ New Fill Layer ▸ Pattern (or edit the active one's). */
export function openPatternFillDialog(edit = false) {
  const info = edit ? activeLayerInfo() : null;
  const old = info?.fill_layer?.fill === "pattern" ? info.fill_layer : null;
  const selected = { id: old ? old.pattern : current };
  openDialog("fill-pattern", {
    title: old ? "Pattern Fill" : "New Layer — Pattern Fill",
    fields: [
      { type: "element", el: patternList(selected) },
      { type: "num", label: "Scale:", value: old ? old.scale : 100, unit: "%", w: 60 },
      { type: "num", label: "Angle:", value: old ? old.angle : 0, unit: "°", w: 60 },
    ],
    onOk: (v) => {
      if (selected.id == null) return;
      const content = { fill: "pattern", pattern: selected.id, scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)), angle: Number(v["Angle:"]) || 0 };
      if (old) sendCommand({ op: "set_fill_layer", layer: { id: activeLayerId() }, content });
      else sendCommand({ op: "add_layer", layer: { fill: { content } }, name: null });
    },
  });
}

export function isPatternDialog(id) {
  return id === "fill-pattern";
}

export function initPatterns() {
  registerControl("pattern", pickerControl);
  bridge.on(ENGINE.PATTERNS, (m) => {
    patterns = m.patterns || [];
    current = m.current ?? current;
    render();
    for (const refresh of pickers) refresh();
  });
  on("pattern:content-options", () => openPatternFillDialog(true));
}

/** The picked pattern's id (Pattern Overlay, M12-T04). */
export function currentPattern() {
  return current;
}
