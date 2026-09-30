// Fotox — the Brushes and Brush Settings panels (M8-T01) and the option
// bar's brush picker.
//
// The Brush Settings panel edits the part of a brush the option bar does not
// show (tip, roundness, angle, spacing, the dynamics). It is one brush for
// every painting tool (FAST: Photoshop keeps one per tool), sent to the
// engine with each tool's options as `_brush` (see main.js sendToolOptions).
// The Brushes panel lists the engine's presets (brushes.json) in their
// folders with their thumbnail strokes; `.abr` files are imported through a
// file input. The option bar's brush button opens a Photoshop / Photopea
// style picker: Size and Hardness sliders over the presets' tips, by folder.

import { h, icon, clear } from "../el.js";
import { emit } from "../state.js";
import { setOption, optionValue, registerControl, sizeToSlider, sliderToSize } from "../optionsbar.js";
import { openPopup } from "../popup.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";

/** The Brush Settings values (engine names: `BrushParams` + `Dynamics`). */
const brush = {
  tip: 0,
  roundness: 1,
  angle: 0,
  spacing: 0.25,
  // The round tip's fall-off: "gaussian" = Photoshop's soft round (measured
  // from Photopea), "classic" = Fotox's first smoothstep tip.
  profile: "gaussian",
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
const NO_DYNAMICS = structuredClone(brush.dynamics);

/** Browser-only presets (the mock engine has no library). */
let presets = [
  { name: "Soft Round", group: "General Brushes", size: 45, hardness: 0, spacing: 0.25, roundness: 1, angle: 0, tip: 0, profile: "gaussian", dynamics: {} },
  { name: "Hard Round", group: "General Brushes", size: 30, hardness: 1, spacing: 0.25, roundness: 1, angle: 0, tip: 0, profile: "gaussian", dynamics: {} },
  { name: "Soft Round (classic)", group: "Fotox Classic", size: 30, hardness: 0, spacing: 0.25, roundness: 1, angle: 0, tip: 0, profile: "classic", dynamics: {} },
];
let selected = -1;
const roots = { list: null, settings: null };
/** Folders the user closed (by name), shared by the panel and the picker. */
const closed = new Set();
/** Redraw hooks of the option-bar brush buttons on screen. */
const buttons = new Set();

/** What goes to the engine as `_brush`. */
export function brushExtras() {
  return { tip: brush.tip, roundness: brush.roundness, angle: brush.angle, spacing: brush.spacing, profile: brush.profile, dynamics: { ...brush.dynamics } };
}

function changed() {
  emit("brush:changed");
}

/** Wire the engine's preset list. */
export function initBrushes() {
  bridge.on(ENGINE.BRUSHES, (m) => {
    presets = m.presets || [];
    renderList();
    redrawButtons();
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

/** A `w × h` coverage image (base64, one byte a pixel) as a canvas. */
function coverageCanvas(b64, w, h_, cls) {
  const canvas = h("canvas", { class: cls, width: w, height: h_ });
  if (!b64) return canvas;
  const bytes = atob(b64);
  const ctx = canvas.getContext("2d");
  const img = ctx.createImageData(w, h_);
  for (let i = 0; i < w * h_ && i < bytes.length; i++) {
    img.data[i * 4] = img.data[i * 4 + 1] = img.data[i * 4 + 2] = 230;
    img.data[i * 4 + 3] = bytes.charCodeAt(i);
  }
  ctx.putImageData(img, 0, 0);
  return canvas;
}

/** The 96 × 32 stroke thumbnail. */
const thumbCanvas = (b64) => coverageCanvas(b64, 96, 32, "brush-stroke-thumb");

/** The 40 × 40 tip thumbnail; a drawn dot in the browser mock. */
function tipCanvas(p, cls = "brush-tip-thumb") {
  if (p && p.tip_thumb) return coverageCanvas(p.tip_thumb, 40, 40, cls);
  const canvas = h("canvas", { class: cls, width: 40, height: 40 });
  const ctx = canvas.getContext("2d");
  const hard = p ? p.hardness ?? 1 : 1;
  const g = ctx.createRadialGradient(20, 20, 0, 20, 20, 16);
  g.addColorStop(0, "rgba(230,230,230,1)");
  g.addColorStop(Math.min(0.99, hard), "rgba(230,230,230,1)");
  g.addColorStop(1, "rgba(230,230,230,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 40, 40);
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
  brush.profile = p.profile || "classic";
  // A preset is the whole brush: dynamics it does not set are off.
  brush.dynamics = { ...NO_DYNAMICS, ...(p.dynamics || {}) };
  setOption("Size", Math.round(p.size));
  setOption("Hardness", Math.round((p.hardness ?? 1) * 100));
  renderList();
  renderSettings();
  redrawButtons();
  changed();
}

function send(id, args = {}) {
  if (bridge.isNative) bridge.send({ type: UI.ACTION, id, args });
}

/** The presets by folder, in first-seen order: `[[name, [[preset, index]…]]…]`. */
function groups() {
  const map = new Map();
  presets.forEach((p, i) => {
    const g = p.group || "My Brushes";
    if (!map.has(g)) map.set(g, []);
    map.get(g).push([p, i]);
  });
  return [...map];
}

/** A collapsible folder header; `redraw` re-renders the owner. */
function folder(name, count, redraw) {
  const open = !closed.has(name);
  return h("div", {
    class: "brush-folder" + (open ? " open" : ""),
    onclick: (e) => {
      e.stopPropagation();
      if (closed.has(name)) closed.delete(name); else closed.add(name);
      redraw();
    },
  }, icon(open ? "i-chevron-down" : "i-chevron-right", "ic xs"), icon("i-folder", "ic sm"), h("span", { class: "brush-folder-name", text: name }), h("span", { class: "pmeta", text: String(count) }));
}

/* ---------------------------------------------------------------- Brushes */

/** The Brushes panel (persistent root, re-rendered in place). */
export function brushPanel() {
  if (!roots.list) roots.list = h("div", { class: "pbrush native" });
  renderList();
  return roots.list;
}

function newPresetFromCurrent() {
  const name = prompt("Brush name", "Brush " + (presets.length + 1));
  if (!name) return;
  const preset = {
    name,
    group: "My Brushes",
    size: Number(optionValue("Size")) || 30,
    hardness: (optionValue("Hardness") ?? 100) / 100,
    spacing: brush.spacing, roundness: brush.roundness, angle: brush.angle, profile: brush.profile, dynamics: brush.dynamics,
  };
  send("brush:save-preset", { preset, tip: brush.tip });
}

async function importAbr() {
  const file = await pickFile(".abr");
  if (file) send("brush:import-abr", { data: file.data, name: file.name });
}

function renderList() {
  const root = roots.list;
  if (!root) return;
  const scroll = root.querySelector(".brushlist")?.scrollTop || 0;
  clear(root);
  const list = h("div", { class: "plist brushlist" });
  for (const [name, items] of groups()) {
    list.append(folder(name, items.length, renderList));
    if (closed.has(name)) continue;
    for (const [p, i] of items) {
      list.append(h("div", {
        class: "plist-row brush-row" + (i === selected ? " sel" : ""),
        onclick: () => applyPreset(i),
        ondblclick: () => {
          const next = prompt("Brush name", p.name);
          if (next) send("brush:rename-preset", { index: i, name: next });
        },
      },
      tipCanvas(p, "brush-tip-mini"),
      thumbCanvas(p.thumb),
      h("span", { class: "plist-label", text: p.name }),
      h("span", { class: "pmeta", text: Math.round(p.size) + " px" })));
    }
  }
  const btn = (ic, tip, fn) => h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
  root.append(list, h("div", { class: "pbar" },
    btn("i-plus", "Create new brush from the current settings", newPresetFromCurrent),
    btn("i-trash", "Delete brush", () => { if (selected >= 0) send("brush:delete-preset", { index: selected }); selected = -1; }),
    btn("i-folder", "Import Brushes (.abr)…", importAbr),
  ));
  list.scrollTop = scroll;
}

/* -------------------------------------------------------- the brush picker */

function redrawButtons() {
  for (const redraw of buttons) redraw();
}

/**
 * The option bar's brush button: the current tip, and on click the picker
 * (Photoshop's "Brush Preset picker", Photopea's brush drop-down): Size and
 * Hardness sliders, then every preset's tip in its folder.
 */
function brushButton() {
  const thumb = h("span", { class: "brush-thumb" });
  const el = h("button", {
    class: "ob-brush", type: "button", "data-tip": "Brush preset picker",
    onclick: (e) => { e.stopPropagation(); openPicker(el); },
  }, thumb, icon("i-chevron-down", "ic xs"));
  const redraw = () => {
    if (!el.isConnected && thumb.childElementCount) { buttons.delete(redraw); return; }
    clear(thumb);
    const p = presets[selected];
    thumb.append(p ? tipCanvas(p, "brush-thumb-canvas") : h("span", { class: "brush-dot" }));
  };
  buttons.add(redraw);
  redraw();
  return { el, read: null };
}

registerControl("brushpreset", brushButton);

/** A labelled slider + box row of the picker bound to option `key`. */
function pickerSlider(label, key, { min, max, toSlider, fromSlider, unit }) {
  const value = Number(optionValue(key));
  const box = h("input", { class: "ob-num", type: "text", inputmode: "decimal", value: Number.isFinite(value) ? String(value) : "", style: { width: "46px" } });
  const slider = h("input", { class: "ob-range picker-range", type: "range", min, max, value: toSlider(Number.isFinite(value) ? value : 0) });
  slider.addEventListener("input", () => {
    const v = fromSlider(Number(slider.value));
    box.value = String(v);
    setOption(key, v);
  });
  box.addEventListener("change", () => {
    const v = Number(box.value);
    if (!Number.isFinite(v)) return;
    slider.value = String(toSlider(v));
    setOption(key, v);
  });
  return h("div", { class: "picker-row" }, h("span", { class: "picker-label", text: label }), slider, box, h("span", { class: "ob-unit", text: unit }));
}

function openPicker(anchor) {
  const content = h("div", { class: "brush-picker" });
  const draw = () => {
    const scroll = content.querySelector(".picker-grid-wrap")?.scrollTop || 0;
    clear(content);
    content.append(pickerSlider("Size:", "Size", { min: 0, max: 1000, toSlider: sizeToSlider, fromSlider: sliderToSize, unit: "px" }));
    // Hardness shapes the round tip only (a sampled tip is its image).
    if (optionValue("Hardness") != null) {
      const row = pickerSlider("Hardness:", "Hardness", { min: 0, max: 100, toSlider: (v) => Math.round(v), fromSlider: (v) => v, unit: "%" });
      if (brush.tip) row.classList.add("off");
      content.append(row);
    }
    const wrap = h("div", { class: "picker-grid-wrap" });
    for (const [name, items] of groups()) {
      wrap.append(folder(name, items.length, draw));
      if (closed.has(name)) continue;
      const grid = h("div", { class: "picker-grid" });
      for (const [p, i] of items) {
        grid.append(h("button", {
          class: "picker-cell" + (i === selected ? " sel" : ""), type: "button", "data-tip": `${p.name} — ${Math.round(p.size)} px`,
          onclick: (e) => { e.stopPropagation(); applyPreset(i); draw(); },
        }, tipCanvas(p), h("span", { class: "picker-size", text: String(Math.round(p.size)) })));
      }
      wrap.append(grid);
    }
    content.append(wrap);
    content.append(h("div", { class: "picker-foot" },
      h("button", { class: "ob-btn", type: "button", text: "New brush…", onclick: (e) => { e.stopPropagation(); newPresetFromCurrent(); } }),
      h("button", { class: "ob-btn", type: "button", text: "Import .abr…", onclick: (e) => { e.stopPropagation(); importAbr(); } }),
      h("span", { class: "picker-name", text: presets[selected]?.name || "" })));
    wrap.scrollTop = scroll;
  };
  draw();
  openPopup({ anchor, content, className: "brush-picker-pop", width: 300 });
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
  const profile = () => {
    const sel = h("select", { class: "pf-select" },
      h("option", { value: "gaussian", text: "Photoshop soft (measured)", selected: brush.profile === "gaussian" }),
      h("option", { value: "classic", text: "Fotox classic", selected: brush.profile !== "gaussian" }));
    sel.addEventListener("change", () => { brush.profile = sel.value; changed(); });
    return h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "Fall-off" }), sel);
  };
  const section = (title, ...rows) => h("details", { class: "bset-section", open: true }, h("summary", { class: "pblock-title", text: title }), ...rows);
  root.append(
    section("Brush Tip Shape",
      h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: brush.tip ? "Sampled tip" : "Round tip" }),
        brush.tip ? h("button", { class: "ob-btn", type: "button", text: "Use round", onclick: () => { brush.tip = 0; renderSettings(); changed(); } }) : null),
      brush.tip ? null : profile(),
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
