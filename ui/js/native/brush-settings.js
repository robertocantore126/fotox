// Fotox — the Brushes and Brush Settings panels (M8-T01).
//
// The Brush Settings panel edits the part of a brush the option bar does not
// show (tip, roundness, angle, spacing, the dynamics). It is one brush for
// every painting tool (FAST: Photoshop keeps one per tool), sent to the
// engine with each tool's options as `_brush` (see main.js sendToolOptions).
// The Brushes panel lists the engine's presets (brushes.json) with their
// thumbnail strokes; `.abr` files are imported through a file input.

import { h, icon, clear } from "../el.js";
import { emit } from "../state.js";
import { setOption, optionValue } from "../optionsbar.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";

/** The Brush Settings values (engine names: `BrushParams` + `Dynamics`). */
const brush = {
  tip: 0,
  roundness: 1,
  angle: 0,
  spacing: 0.25,
  dynamics: {
    size_jitter: 0, size_control: "off", min_diameter: 0,
    angle_jitter: 0, angle_control: "off",
    roundness_jitter: 0, roundness_control: "off", min_roundness: 0.25,
    fade_steps: 25,
    scatter: 0, scatter_both_axes: false, count: 1, count_jitter: 0,
    opacity_jitter: 0, opacity_control: "off", flow_jitter: 0, flow_control: "off",
    fg_bg_jitter: 0, hue_jitter: 0, saturation_jitter: 0, brightness_jitter: 0, per_tip: false,
  },
};

/** Browser-only presets (the mock engine has no library). */
let presets = [
  { name: "Soft Round", size: 30, hardness: 0, spacing: 0.25, roundness: 1, angle: 0, tip: 0, dynamics: {} },
  { name: "Hard Round", size: 30, hardness: 1, spacing: 0.25, roundness: 1, angle: 0, tip: 0, dynamics: {} },
];
let selected = -1;
const roots = { list: null, settings: null };

/** What goes to the engine as `_brush`. */
export function brushExtras() {
  return { tip: brush.tip, roundness: brush.roundness, angle: brush.angle, spacing: brush.spacing, dynamics: { ...brush.dynamics } };
}

function changed() {
  emit("brush:changed");
}

/** Wire the engine's preset list. */
export function initBrushes() {
  bridge.on(ENGINE.BRUSHES, (m) => {
    presets = m.presets || [];
    renderList();
  });
}

/** Ask the user for a file; resolves to a `data:` URL (base64). */
export function pickFile(accept) {
  return new Promise((resolve) => {
    const input = h("input", { type: "file", accept, style: { display: "none" } });
    input.addEventListener("change", () => {
      const file = input.files && input.files[0];
      input.remove();
      if (!file) return resolve(null);
      const reader = new FileReader();
      reader.onload = () => resolve({ name: file.name, data: reader.result });
      reader.readAsDataURL(file);
    });
    document.body.append(input);
    input.click();
  });
}

/** A 96 × 32 coverage thumbnail (base64) as a canvas. */
function thumbCanvas(b64) {
  const canvas = h("canvas", { class: "brush-stroke-thumb", width: 96, height: 32 });
  if (!b64) return canvas;
  const bytes = atob(b64);
  const ctx = canvas.getContext("2d");
  const img = ctx.createImageData(96, 32);
  for (let i = 0; i < 96 * 32 && i < bytes.length; i++) {
    img.data[i * 4] = img.data[i * 4 + 1] = img.data[i * 4 + 2] = 230;
    img.data[i * 4 + 3] = bytes.charCodeAt(i);
  }
  ctx.putImageData(img, 0, 0);
  return canvas;
}

function applyPreset(i) {
  const p = presets[i];
  if (!p) return;
  selected = i;
  brush.tip = p.tip || 0;
  brush.roundness = p.roundness ?? 1;
  brush.angle = p.angle ?? 0;
  brush.spacing = p.spacing ?? 0.25;
  brush.dynamics = { ...brush.dynamics, ...(p.dynamics || {}) };
  setOption("Size", Math.round(p.size));
  setOption("Hardness", Math.round((p.hardness ?? 1) * 100));
  renderList();
  renderSettings();
  changed();
}

function send(id, args = {}) {
  if (bridge.isNative) bridge.send({ type: UI.ACTION, id, args });
}

/* ---------------------------------------------------------------- Brushes */

/** The Brushes panel (persistent root, re-rendered in place). */
export function brushPanel() {
  if (!roots.list) roots.list = h("div", { class: "pbrush native" });
  renderList();
  return roots.list;
}

function renderList() {
  const root = roots.list;
  if (!root) return;
  clear(root);
  const list = h("div", { class: "plist brushlist" });
  presets.forEach((p, i) => {
    list.append(h("div", {
      class: "plist-row" + (i === selected ? " sel" : ""),
      onclick: () => applyPreset(i),
      ondblclick: () => {
        const name = prompt("Brush name", p.name);
        if (name) send("brush:rename-preset", { index: i, name });
      },
    },
    thumbCanvas(p.thumb),
    h("span", { class: "plist-label", text: p.name }),
    h("span", { class: "pmeta", text: Math.round(p.size) + " px" })));
  });
  const btn = (ic, tip, fn) => h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
  root.append(list, h("div", { class: "pbar" },
    btn("i-plus", "Create new brush from the current settings", () => {
      const name = prompt("Brush name", "Brush " + (presets.length + 1));
      if (!name) return;
      const preset = {
        name,
        size: Number(optionValue("Size")) || 30,
        hardness: (Number(optionValue("Hardness")) || 100) / 100,
        spacing: brush.spacing, roundness: brush.roundness, angle: brush.angle, dynamics: brush.dynamics,
      };
      send("brush:save-preset", { preset, tip: brush.tip });
    }),
    btn("i-trash", "Delete brush", () => { if (selected >= 0) send("brush:delete-preset", { index: selected }); selected = -1; }),
    btn("i-folder", "Import Brushes (.abr)…", async () => {
      const file = await pickFile(".abr");
      if (file) send("brush:import-abr", { data: file.data, name: file.name });
    }),
  ));
}

/* ------------------------------------------------------------ Brush Settings */

const CONTROLS = [["off", "Off"], ["fade", "Fade"], ["pen_pressure", "Pen Pressure"], ["pen_tilt", "Pen Tilt"]];

/** The Brush Settings panel. */
export function brushSettingsPanel() {
  if (!roots.settings) roots.settings = h("div", { class: "pbrush-set native" });
  renderSettings();
  return roots.settings;
}

function renderSettings() {
  const root = roots.settings;
  if (!root) return;
  clear(root);
  const d = brush.dynamics;
  // A percent slider bound to `obj[key]` (0..1, or 0..max).
  const pct = (label, obj, key, max = 1) => {
    const out = h("span", { class: "pf-value", text: Math.round((obj[key] / max) * 100) + " %" });
    const input = h("input", { class: "pminirange", type: "range", min: 0, max: 100, value: Math.round((obj[key] / max) * 100) });
    input.addEventListener("input", () => { obj[key] = (Number(input.value) / 100) * max; out.textContent = input.value + " %"; changed(); });
    return h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: label }), input, out);
  };
  const num = (label, obj, key, min, max, unit = "") => {
    const input = h("input", { class: "pf-num", type: "number", min, max, value: obj[key] });
    input.addEventListener("change", () => { obj[key] = Math.min(max, Math.max(min, Number(input.value) || 0)); changed(); });
    return h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: label }), input, h("span", { class: "pf-unit", text: unit }));
  };
  const control = (label, key) => {
    const sel = h("select", { class: "pf-select" }, ...CONTROLS.map(([v, t]) => h("option", { value: v, text: t, selected: d[key] === v })));
    sel.addEventListener("change", () => { d[key] = sel.value; changed(); });
    return h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: label }), sel);
  };
  const check = (label, obj, key) => {
    const box = h("input", { type: "checkbox", checked: !!obj[key] });
    box.addEventListener("change", () => { obj[key] = box.checked; changed(); });
    return h("label", { class: "pf-row narrow" }, box, h("span", { class: "pf-label", text: label }));
  };
  const section = (title, ...rows) => h("details", { class: "bset-section", open: true }, h("summary", { class: "pblock-title", text: title }), ...rows);
  root.append(
    section("Brush Tip Shape",
      h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: brush.tip ? "Sampled tip" : "Round tip" }),
        brush.tip ? h("button", { class: "ob-btn", type: "button", text: "Use round", onclick: () => { brush.tip = 0; renderSettings(); changed(); } }) : null),
      num("Angle", brush, "angle", -180, 180, "°"),
      pct("Roundness", brush, "roundness"),
      pct("Spacing", brush, "spacing", 10)),
    section("Shape Dynamics",
      pct("Size Jitter", d, "size_jitter"), control("Control", "size_control"), pct("Minimum Diameter", d, "min_diameter"),
      pct("Angle Jitter", d, "angle_jitter"), control("Control", "angle_control"),
      pct("Roundness Jitter", d, "roundness_jitter"), control("Control", "roundness_control"), pct("Minimum Roundness", d, "min_roundness"),
      num("Fade steps", d, "fade_steps", 1, 9999)),
    section("Scattering",
      pct("Scatter", d, "scatter", 10), check("Both Axes", d, "scatter_both_axes"),
      num("Count", d, "count", 1, 16), pct("Count Jitter", d, "count_jitter")),
    section("Transfer",
      pct("Opacity Jitter", d, "opacity_jitter"), control("Control", "opacity_control"),
      pct("Flow Jitter", d, "flow_jitter"), control("Control", "flow_control")),
    section("Color Dynamics",
      check("Apply Per Tip", d, "per_tip"),
      pct("Foreground/Background Jitter", d, "fg_bg_jitter"),
      pct("Hue Jitter", d, "hue_jitter"), pct("Saturation Jitter", d, "saturation_jitter"), pct("Brightness Jitter", d, "brightness_jitter")),
  );
}
