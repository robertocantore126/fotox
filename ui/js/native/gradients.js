// Fotox — gradients in the app (M8-T03): the option bar's gradient picker,
// the Gradient Editor, and Layer ▸ New Fill Layer ▸ Gradient.
//
// A gradient is the engine's `fx_core::gradient::Gradient` as JSON; a stop
// colour may be "fg" / "bg", which the engine replaces with the swatches when
// it paints (so "Foreground to Background" follows the colours).

import { h, icon, clear } from "../el.js";
import { state, emit } from "../state.js";
import { openDialog } from "../dialogs.js";
import { openDropdown } from "../popup.js";
import { registerControl } from "../optionsbar.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";
import { activeLayerInfo, sendCommand, activeLayerId } from "./layers-panel.js";

const stop = (location, color) => ({ location, midpoint: 0.5, color });
const op = (location, opacity) => ({ location, midpoint: 0.5, opacity });
const hex = (s) => [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16) / 255);

/** Photoshop's basic presets. */
export const PRESETS = {
  "Foreground to Background": { colors: [stop(0, "fg"), stop(1, "bg")], opacities: [], method: "perceptual" },
  "Foreground to Transparent": { colors: [stop(0, "fg"), stop(1, "fg")], opacities: [op(0, 1), op(1, 0)], method: "perceptual" },
  "Black, White": { colors: [stop(0, [0, 0, 0]), stop(1, [1, 1, 1])], opacities: [], method: "perceptual" },
  "Red, Green": { colors: [stop(0, hex("#e11e1e")), stop(1, hex("#1ea01e"))], opacities: [], method: "perceptual" },
  "Violet, Orange": { colors: [stop(0, hex("#29166f")), stop(1, hex("#f39200"))], opacities: [], method: "perceptual" },
  "Blue, Red, Yellow": { colors: [stop(0, hex("#0a00b2")), stop(0.5, hex("#ff0000")), stop(1, hex("#fffc00"))], opacities: [], method: "perceptual" },
  "Copper": { colors: [stop(0, hex("#97461a")), stop(0.3, hex("#fbd8c5")), stop(0.83, hex("#6c2e16")), stop(1, hex("#efdbcd"))], opacities: [], method: "perceptual" },
  "Chrome": { colors: [stop(0, hex("#29868b")), stop(0.5, hex("#ffffff")), stop(0.52, hex("#1c1c1c")), stop(0.64, hex("#8b5a2b")), stop(1, hex("#ffffff"))], opacities: [], method: "perceptual" },
  "Transparent Rainbow": { colors: [stop(0, [1, 0, 0]), stop(0.2, [1, 1, 0]), stop(0.4, [0, 1, 0]), stop(0.6, [0, 1, 1]), stop(0.8, [0, 0, 1]), stop(1, [1, 0, 1])], opacities: [op(0, 0), op(0.1, 1), op(0.9, 1), op(1, 0)], method: "perceptual" },
};

let current = structuredClone(PRESETS["Foreground to Background"]);
let currentName = "Foreground to Background";

function toCss(c) {
  if (c === "fg") return state.colors.fg;
  if (c === "bg") return state.colors.bg;
  return `rgb(${c.map((v) => Math.round(v * 255)).join(",")})`;
}

/** A CSS preview of a gradient (FAST: midpoints and methods not shown). */
export function gradientCss(g) {
  const stops = g.colors.map((s) => `${toCss(s.color)} ${Math.round(s.location * 100)}%`);
  return `linear-gradient(90deg, ${stops.join(", ")})`;
}

function toHex(c) {
  if (c === "fg") return state.colors.fg;
  if (c === "bg") return state.colors.bg;
  return "#" + c.map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("");
}

/**
 * The Gradient Editor as a DOM node editing `g` in place; `changed()` after
 * every edit.
 */
export function gradientEditor(g, changed = () => {}) {
  const root = h("div", { class: "grad-editor" });
  const render = () => {
    clear(root);
    const bar = h("div", { class: "gradient-preview big", style: { background: gradientCss(g), height: "28px", borderRadius: "3px", margin: "4px 0 8px" } });
    const presetBtn = h("button", { class: "btn small", type: "button", text: "Presets…", onclick: (e) => openDropdown({
      anchor: e.currentTarget, items: Object.keys(PRESETS), value: "", width: 200,
      onPick: (name) => { Object.assign(g, structuredClone(PRESETS[name])); render(); changed(); },
    }) });
    const method = h("select", { class: "pf-select" }, ...["perceptual", "linear", "classic"].map((m) => h("option", { value: m, text: m[0].toUpperCase() + m.slice(1), selected: g.method === m })));
    method.addEventListener("change", () => { g.method = method.value; changed(); });
    root.append(h("div", { class: "dlg-line" }, presetBtn, h("span", { class: "dlg-field-label", text: "Method:" }), method), bar);
    // Colour stops.
    const rows = h("div", { class: "grad-stops" }, h("div", { class: "dlg-label", text: "Colour stops (location %, midpoint %)" }));
    g.colors.forEach((s, i) => {
      const color = h("input", { type: "color", value: toHex(s.color) });
      color.addEventListener("input", () => { s.color = hex(color.value); bar.style.background = gradientCss(g); changed(); });
      const kind = h("select", { class: "pf-select" }, ...[["rgb", "Colour"], ["fg", "Foreground"], ["bg", "Background"]].map(([v, t]) => h("option", { value: v, text: t, selected: (typeof s.color === "string" ? s.color : "rgb") === v })));
      kind.addEventListener("change", () => { s.color = kind.value === "rgb" ? hex(color.value) : kind.value; render(); changed(); });
      const loc = numBox(s.location * 100, (v) => { s.location = v / 100; g.colors.sort((a, b) => a.location - b.location); changed(); });
      const mid = numBox(s.midpoint * 100, (v) => { s.midpoint = Math.min(95, Math.max(5, v)) / 100; changed(); });
      const del = h("button", { class: "pbar-btn", type: "button", "data-tip": "Delete stop", onclick: () => { if (g.colors.length > 1) { g.colors.splice(i, 1); render(); changed(); } } }, icon("i-trash", "ic sm"));
      rows.append(h("div", { class: "dlg-line" }, kind, color, loc, mid, del));
    });
    rows.append(h("button", { class: "btn small", type: "button", text: "Add colour stop", onclick: () => { g.colors.push(stop(0.5, [0.5, 0.5, 0.5])); g.colors.sort((a, b) => a.location - b.location); render(); changed(); } }));
    // Opacity stops.
    const ops = h("div", { class: "grad-stops" }, h("div", { class: "dlg-label", text: "Opacity stops (opacity %, location %, midpoint %)" }));
    g.opacities.forEach((s, i) => {
      const o = numBox(s.opacity * 100, (v) => { s.opacity = Math.min(100, Math.max(0, v)) / 100; changed(); });
      const loc = numBox(s.location * 100, (v) => { s.location = v / 100; g.opacities.sort((a, b) => a.location - b.location); changed(); });
      const mid = numBox(s.midpoint * 100, (v) => { s.midpoint = Math.min(95, Math.max(5, v)) / 100; changed(); });
      const del = h("button", { class: "pbar-btn", type: "button", "data-tip": "Delete stop", onclick: () => { g.opacities.splice(i, 1); render(); changed(); } }, icon("i-trash", "ic sm"));
      ops.append(h("div", { class: "dlg-line" }, o, loc, mid, del));
    });
    ops.append(h("button", { class: "btn small", type: "button", text: "Add opacity stop", onclick: () => { if (!g.opacities.length) g.opacities.push(op(0, 1)); g.opacities.push(op(1, 1)); render(); changed(); } }));
    root.append(rows, ops);
  };
  render();
  return root;
}

function numBox(value, set) {
  const input = h("input", { class: "dlg-input num", type: "text", value: Math.round(value), style: { width: "44px" } });
  input.addEventListener("change", () => set(Math.min(100, Math.max(0, Number(input.value) || 0))));
  return input;
}

/** Edit ▸ the option bar gradient in the Gradient Editor. */
export function openGradientEditor(onDone) {
  const work = structuredClone(current);
  openDialog("gradient-editor", {
    title: "Gradient Editor",
    width: 460,
    fields: [{ type: "element", el: gradientEditor(work) }],
    onOk: () => { current = work; currentName = "Custom"; onDone?.(); },
  });
}

/** The option bar's gradient picker (`key: "Gradient"`). */
function pickerControl() {
  const preview = h("span", { class: "gradient-preview", style: { background: gradientCss(current) } });
  const refresh = () => { preview.style.background = gradientCss(current); el.dataset.tip = currentName; };
  const notify = () => { refresh(); emit("brush:changed"); };
  const el = h("button", {
    class: "ob-gradient", type: "button", "data-tip": currentName,
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({
        anchor: el, items: [...Object.keys(PRESETS), "Edit…"], value: currentName, width: 200,
        onPick: (name) => {
          if (name === "Edit…") { openGradientEditor(notify); return; }
          current = structuredClone(PRESETS[name]);
          currentName = name;
          notify();
        },
      });
    },
  }, preview, icon("i-chevron-down", "ic xs"));
  return { el, read: () => structuredClone(current) };
}

/* ------------------------------------------------------ fill layer dialog */

const STYLES = ["Linear", "Radial", "Angle", "Reflected", "Diamond"];

/** Layer ▸ New Fill Layer ▸ Gradient (or edit the active one's). */
export function openGradientFillDialog(edit = false) {
  const info = edit ? activeLayerInfo() : null;
  const old = info?.fill_layer?.fill === "gradient" ? info.fill_layer : null;
  const work = structuredClone(old ? old.gradient : current);
  openDialog("fill-gradient", {
    title: old ? "Gradient Fill" : "New Layer — Gradient Fill",
    width: 460,
    fields: [
      { type: "element", el: gradientEditor(work) },
      { type: "select", label: "Style:", options: STYLES, value: old ? old.kind[0].toUpperCase() + old.kind.slice(1) : "Linear" },
      { type: "num", label: "Angle:", value: old ? old.angle : 90, unit: "°", w: 60 },
      { type: "num", label: "Scale:", value: old ? old.scale : 100, unit: "%", w: 60 },
      { type: "check", label: "Reverse", value: old ? old.reverse : false },
      { type: "check", label: "Dither", value: old ? old.dither : true },
    ],
    onOk: (v) => {
      const content = {
        fill: "gradient",
        gradient: work,
        kind: String(v["Style:"] || "Linear").toLowerCase(),
        angle: Number(v["Angle:"]) || 0,
        scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)),
        reverse: !!v.Reverse,
        dither: v.Dither !== false,
        offset: [0, 0],
      };
      resolveSwatches(content.gradient);
      if (old) sendCommand({ op: "set_fill_layer", layer: { id: activeLayerId() }, content });
      else sendCommand({ op: "add_layer", layer: { fill: { content } }, name: null });
    },
  });
}

/** A fill layer keeps fixed colours: "fg"/"bg" become the current swatches. */
function resolveSwatches(g) {
  for (const s of g.colors) if (typeof s.color === "string") s.color = hex(s.color === "fg" ? state.colors.fg : state.colors.bg);
}

export function initGradients() {
  registerControl("gradient", pickerControl);
}

export function isGradientDialog(id) {
  return id === "fill-gradient" || id === "gradient-editor";
}

export function openGradientDialog(id) {
  if (id === "fill-gradient") openGradientFillDialog(false);
  else openGradientEditor(() => emit("brush:changed"));
}

export { bridge, UI };

/** The current gradient, its swatch stops resolved (Gradient Overlay, M12-T04). */
export function currentGradientResolved() {
  const g = structuredClone(current);
  resolveSwatches(g);
  return g;
}

export { resolveSwatches };
