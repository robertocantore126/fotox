// Fotox — the Layer Style window (M6-T08/T09, rebuilt 2026-10-01 after
// Photoshop and Photopea; docs/reports/STYLES-AND-UX-GAPS-2026-10-01.md).
//
// One window, as in Photoshop: the effects on the left (a box turns each on
// or off, a click shows its page, + adds another Drop Shadow / Inner
// Shadow / Color Overlay / Gradient Overlay / Stroke), the page in the
// middle, OK / Cancel / New Style / Preview on the right (the canvas is the
// preview). Every change sends the whole `LayerStyles` (`set_layer_style`); the
// engine merges the repeated edits into one history step, and Cancel sends
// the style (and the layer's opacity, fill and mode) back as they were.
//
// While a page is open, a drag on the image moves what it places: a shadow's
// or satin's angle and distance, a gradient, a pattern, a bevel's texture.
//
// Style presets and each effect's Make Default live in the preferences file
// (`style_presets`, `style_defaults`), not in the browser.

import { h, icon, clear } from "../el.js";
import { openDialog, openColorPopover, askText, BLEND_MODES, isDialogOpen } from "../dialogs.js";
import { openDropdown } from "../popup.js";
import { toast } from "../tooltip.js";
import { activeLayerInfo, sendCommand } from "./layers-panel.js";
import { hexToRgba16, rgba16ToHex } from "./tools.js";
import { currentGradientResolved, gradientEditor, gradientCss, PRESETS as GRADIENT_PRESETS, allGradients, resolveSwatches } from "./gradients.js";
import { currentPattern, patternList } from "./patterns.js";
import { activeDocumentInfo } from "./documents.js";
import { viewZoom } from "./overview-panels.js";
import { prefValue, setPrefs } from "./prefs.js";

/* ------------------------------------------------------------ the effects */

const c16 = (r, g, b) => [r * 257, g * 257, b * 257, 65535];
const BLACK = c16(0, 0, 0);
const WHITE = c16(255, 255, 255);
const GLOW = [65535, 65535, 48830, 65535];

function defaultGradientLayer() {
  const g = currentGradientResolved() || structuredClone(GRADIENT_PRESETS["Black, White"]);
  resolveSwatches(g);
  return { gradient: g, kind: "linear", angle: 90, scale: 100, reverse: false, dither: true, offset: [0, 0] };
}

/** Photoshop's factory settings of each effect. */
const FACTORY = {
  drop_shadow: () => ({ enabled: true, blend: "multiply", color: BLACK, opacity: 0.75, angle: 120, use_global_light: true, distance: 5, spread: 0, size: 5, noise: 0, contour: "linear", anti_aliased: false, knocks_out: true }),
  inner_shadow: () => ({ enabled: true, blend: "multiply", color: BLACK, opacity: 0.75, angle: 120, use_global_light: true, distance: 5, choke: 0, size: 5, noise: 0, contour: "linear", anti_aliased: false }),
  outer_glow: () => ({ enabled: true, blend: "screen", opacity: 0.75, color: GLOW, spread: 0, size: 5, noise: 0, fill: { type: "color" }, contour: "linear", anti_aliased: false, technique: "softer", range: 50, jitter: 0 }),
  inner_glow: () => ({ enabled: true, blend: "screen", opacity: 0.75, color: GLOW, choke: 0, size: 5, noise: 0, source: "edge", contour: "linear", fill: { type: "color" }, anti_aliased: false, technique: "softer", range: 50, jitter: 0 }),
  bevel: () => ({
    enabled: true, style: "inner_bevel", depth: 100, up: true, size: 5, soften: 0, angle: 120, use_global_light: true, altitude: 30,
    highlight_blend: "screen", highlight_color: WHITE, highlight_opacity: 0.75, shadow_blend: "multiply", shadow_color: BLACK, shadow_opacity: 0.75,
    technique: "smooth", contour: "linear", contour_range: 100, contour_anti_aliased: false, gloss_contour: "linear", anti_aliased: false,
  }),
  satin: () => ({ enabled: true, blend: "multiply", color: BLACK, opacity: 0.5, angle: 19, distance: 11, size: 14, invert: true, contour: "linear", anti_aliased: false }),
  color_overlay: () => ({ enabled: true, blend: "normal", color: c16(255, 0, 0), opacity: 1 }),
  gradient_overlay: () => ({ enabled: true, blend: "normal", opacity: 1, gradient: defaultGradientLayer(), align: true }),
  pattern_overlay: () => ({ enabled: true, blend: "normal", opacity: 1, pattern: currentPattern() ?? null, scale: 100, angle: 0, phase: [0, 0], align: true }),
  stroke: () => ({ enabled: true, size: 3, position: "outside", blend: "normal", opacity: 1, color: BLACK, fill: { type: "color" } }),
};

/** The dialog's list, top to bottom (Photoshop's order). */
const EFFECTS = [
  { key: "bevel", title: "Bevel & Emboss" },
  { key: "stroke", title: "Stroke", stack: true },
  { key: "inner_shadow", title: "Inner Shadow", stack: true },
  { key: "inner_glow", title: "Inner Glow" },
  { key: "satin", title: "Satin" },
  { key: "color_overlay", title: "Color Overlay", stack: true },
  { key: "gradient_overlay", title: "Gradient Overlay", stack: true },
  { key: "pattern_overlay", title: "Pattern Overlay" },
  { key: "outer_glow", title: "Outer Glow" },
  { key: "drop_shadow", title: "Drop Shadow", stack: true },
];
const TITLE = Object.fromEntries(EFFECTS.map((e) => [e.key, e.title]));
/** Photoshop's limit on the instances of one effect. */
const MAX_INSTANCES = 10;

/** Menu ids (`dlg:style-…`) → effect keys. */
const DIALOG_KEYS = {
  "style-drop-shadow": "drop_shadow", "style-inner-shadow": "inner_shadow", "style-outer-glow": "outer_glow", "style-inner-glow": "inner_glow",
  "style-bevel": "bevel", "style-satin": "satin", "style-color-overlay": "color_overlay", "style-gradient-overlay": "gradient_overlay",
  "style-pattern-overlay": "pattern_overlay", "style-stroke": "stroke",
};

/** A new effect: the user's default (Make Default) over the factory's. */
function newEffect(key) {
  const saved = (prefValue("style_defaults") || {})[key];
  return { ...FACTORY[key](), ...(saved ? structuredClone(saved) : {}), enabled: true };
}

/** A style as the dialog edits it: every effect a list (older ones were single). */
function normalize(styles) {
  const s = styles ? structuredClone(styles) : {};
  for (const { key } of EFFECTS) {
    const v = s[key];
    s[key] = Array.isArray(v) ? v : v ? [v] : [];
  }
  return s;
}

/** The style the engine gets: empty lists dropped, `null` when nothing is left. */
function compact(styles) {
  const out = {};
  styles = styles && { ...styles, pattern_overlay: (styles.pattern_overlay || []).filter((e) => e.pattern != null) };
  for (const [k, v] of Object.entries(styles || {})) {
    if (Array.isArray(v) ? v.length : v !== undefined && v !== null) out[k] = v;
  }
  const blendingOnly = ["channels", "interior_as_group", "layer_mask_hides", "vector_mask_hides", "effects_visible"];
  const meaningful = Object.keys(out).some((k) => !blendingOnly.includes(k))
    || (out.channels && out.channels.some((c) => !c)) || out.interior_as_group || out.layer_mask_hides || out.vector_mask_hides || out.effects_visible === false;
  return meaningful ? out : null;
}

/* ----------------------------------------------------------- small utils */

const blendId = (name) => String(name || "Normal").toLowerCase().replace(/[^a-z]+/g, "_").replace(/_add_?$/, "").replace(/_$/, "");
const blendLabel = (id) => BLEND_MODES.find((m) => blendId(m) === id) || "Normal";
const pct = (v) => Math.round((v ?? 0) * 100);
const num = (v, fallback = 0) => (Number.isFinite(Number(v)) ? Number(v) : fallback);
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

// The document's Global Light, from its `DocumentInfo`; the value just sent
// wins until the engine's answer arrives.
let lightSent = null;
function globalLight() {
  const info = activeDocumentInfo();
  const angle = info && Number.isFinite(info.global_light) ? info.global_light : 120;
  const altitude = info && Number.isFinite(info.global_altitude) ? info.global_altitude : 30;
  if (lightSent && lightSent.doc !== info?.doc) lightSent = null;
  if (lightSent && Math.abs(angle - lightSent.angle) < 1e-9 && Math.abs(altitude - lightSent.altitude) < 1e-9) lightSent = null;
  return lightSent || { angle, altitude };
}
function setGlobalLight(angle, altitude = globalLight().altitude) {
  lightSent = { doc: activeDocumentInfo()?.doc, angle, altitude };
  sendCommand({ op: "set_global_light", angle, altitude });
}

/* -------------------------------------------------------------- controls */

const row = (label, ...kids) => h("div", { class: "ls-row" }, label != null ? h("span", { class: "ls-label", text: label }) : null, ...kids);
const group = (title, ...kids) => h("div", { class: "ls-group" }, title ? h("div", { class: "ls-group-title", text: title }) : null, ...kids);

/**
 * A slider with its number box: arrow keys step the box (Shift × 10), and
 * the label scrubs (drag it sideways), as in Photoshop.
 */
function slider(label, value, { min = 0, max = 100, unit = "", step = 1, onInput, box = 48 }) {
  const range = h("input", { class: "ls-range", type: "range", min, max, step, value });
  const out = h("input", { class: "dlg-input num", type: "text", value: fmt(value, step), style: { width: box + "px" } });
  const set = (v, fire = true) => {
    // Typed values are clamped too: the box, the slider and the effect agree.
    v = clamp(Math.round(v / step) * step, min, max);
    range.value = String(clamp(v, min, max));
    out.value = fmt(v, step);
    if (fire) onInput(v);
  };
  range.addEventListener("input", () => set(Number(range.value)));
  out.addEventListener("change", () => set(num(out.value, value)));
  out.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const d = (e.key === "ArrowUp" ? 1 : -1) * step * (e.shiftKey ? 10 : 1);
    set(clamp(num(out.value) + d, min, max));
  });
  const name = h("span", { class: "ls-label scrub", text: label, "data-tip": "Drag sideways to change" });
  name.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const x0 = e.clientX;
    const v0 = num(out.value);
    const move = (m) => set(clamp(v0 + Math.round((m.clientX - x0) / 2) * step * (m.shiftKey ? 10 : 1), min, max));
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  const el = h("div", { class: "ls-row" }, name, range, out, unit ? h("span", { class: "dlg-unit", text: unit }) : null);
  el.setValue = (v) => set(v, false);
  return el;
}
function fmt(v, step) {
  return step < 1 ? String(Math.round(v * 100) / 100) : String(Math.round(v));
}

/** A colour well (straight RGBA16 in, out). */
function colorWell(rgba, onInput, tip = "Pick a colour") {
  const chip = h("button", { class: "ls-color", type: "button", "data-tip": tip, style: { background: rgba16ToHex(rgba) } });
  chip.addEventListener("click", (e) => {
    e.stopPropagation();
    openColorPopover(chip, rgba16ToHex(rgba), (hex) => {
      chip.style.background = hex;
      rgba = hexToRgba16(hex);
      onInput(rgba);
    });
  });
  return chip;
}

/** A drop-down of `options` (`[value, label]` pairs or labels). */
function select(options, value, onPick, width = 130) {
  const pairs = options.map((o) => (Array.isArray(o) ? o : [o, o]));
  const labelOf = (v) => (pairs.find(([k]) => k === v) || pairs[0])[1];
  const val = h("span", { class: "pf-value", text: labelOf(value) });
  const btn = h("button", { class: "dlg-select", type: "button", style: { minWidth: width + "px" } }, val, icon("i-chevron-down", "ic xs"));
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    openDropdown({
      anchor: btn, items: pairs.map(([, l]) => l), value: val.textContent, width: Math.max(150, btn.offsetWidth),
      onPick: (label) => {
        val.textContent = label;
        onPick((pairs.find(([, l]) => l === label) || pairs[0])[0]);
      },
    });
  });
  return btn;
}

function check(label, on, onChange, tip) {
  const box = h("span", { class: "dlg-check" + (on ? " on" : "") });
  const paint = () => { clear(box); if (box.classList.contains("on")) box.append(icon("i-check", "ic xs")); };
  paint();
  const line = h("label", { class: "dlg-checkline", "data-tip": tip || null }, box, h("span", { text: label }));
  line.addEventListener("click", (e) => {
    e.preventDefault();
    box.classList.toggle("on");
    paint();
    onChange(box.classList.contains("on"));
  });
  return line;
}

/** Two or three exclusive buttons (Direction, Source, Technique…). */
function segmented(options, value, onPick) {
  const wrap = h("span", { class: "dlg-inline-radio" });
  for (const [v, label] of options) {
    wrap.append(h("button", {
      class: "btn tiny" + (v === value ? " on" : ""), type: "button", text: label,
      onclick: (e) => { [...wrap.children].forEach((c) => c.classList.remove("on")); e.currentTarget.classList.add("on"); onPick(v); },
    }));
  }
  return wrap;
}

/** Blend Mode: the menu, and the effect's colour beside it (Photoshop's layout). */
function blendRow(label, mode, onMode, color, onColor) {
  return row(label, select(BLEND_MODES, blendLabel(mode), (m) => onMode(blendId(m)), 150), color ? colorWell(color, onColor) : null);
}

/**
 * The Angle dial (drag it, or type), with Use Global Light: while that is
 * ticked, turning the dial turns the document's Global Light, so every effect
 * that uses it turns together.
 */
function angleRow(label, e, set, { global = true } = {}) {
  const current = () => (global && e.use_global_light ? globalLight().angle : num(e.angle, 120));
  const size = 34;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", size);
  svg.setAttribute("height", size);
  svg.setAttribute("class", "ls-dial");
  svg.innerHTML = `<circle cx="17" cy="17" r="15" /><line x1="17" y1="17" x2="17" y2="4" /><circle class="knob" cx="17" cy="4" r="2" />`;
  const box = h("input", { class: "dlg-input num", type: "text", style: { width: "42px" } });
  const draw = (a) => {
    const r = (a * Math.PI) / 180;
    const x = 17 + Math.cos(r) * 13, y = 17 - Math.sin(r) * 13;
    const line = svg.querySelector("line"), knob = svg.querySelector(".knob");
    line.setAttribute("x2", x); line.setAttribute("y2", y);
    knob.setAttribute("cx", x); knob.setAttribute("cy", y);
    box.value = String(Math.round(a));
  };
  const apply = (a) => {
    a = ((((a + 180) % 360) + 360) % 360) - 180;
    draw(a);
    if (global && e.use_global_light) setGlobalLight(a);
    else set({ angle: a });
  };
  svg.addEventListener("mousedown", (ev) => {
    ev.preventDefault();
    const rect = svg.getBoundingClientRect();
    const move = (m) => {
      let a = (Math.atan2(-(m.clientY - rect.top - size / 2), m.clientX - rect.left - size / 2) * 180) / Math.PI;
      if (m.shiftKey) a = Math.round(a / 15) * 15;
      apply(Math.round(a));
    };
    move(ev);
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  box.addEventListener("change", () => apply(num(box.value, current())));
  draw(current());
  const kids = [svg, box, h("span", { class: "dlg-unit", text: "°" })];
  if (global) {
    kids.push(check("Use Global Light", !!e.use_global_light, (on) => {
      // Ticking it takes the document's light; unticking keeps the angle shown.
      set(on ? { use_global_light: true } : { use_global_light: false, angle: globalLight().angle });
      draw(current());
    }));
  }
  const el = row(label, ...kids);
  el.redraw = () => draw(current());
  return el;
}

/* -------------------------------------------------------------- contours */

const CONTOURS = [
  ["linear", "Linear"], ["cone", "Cone"], ["cone_inverted", "Cone - Inverted"], ["cove_deep", "Cove - Deep"], ["cove_shallow", "Cove - Shallow"],
  ["gaussian", "Gaussian"], ["half_round", "Half Round"], ["ring", "Ring"], ["ring_double", "Ring - Double"],
  ["rolling_slope", "Rolling Slope - Descending"], ["rounded_steps", "Rounded Steps"], ["sawtooth", "Sawtooth 1"],
];
const contourName = (c) => (typeof c === "string" ? (CONTOURS.find(([k]) => k === c) || CONTOURS[0])[1] : "Custom");

/** The curve at `t`, as `fx_core::styles::Contour::apply` computes it. */
function contourAt(c, t) {
  t = clamp(t, 0, 1);
  const smooth = (x) => x * x * (3 - 2 * x);
  const peak = (m, w) => Math.exp(-(((t - m) / w) ** 2));
  let v;
  if (c && typeof c === "object" && c.custom) {
    v = customAt(c.custom.points.slice(0, clamp(c.custom.len, 2, 16)), t);
  } else {
    switch (c) {
      case "cone": v = 1 - Math.abs(2 * t - 1); break;
      case "cone_inverted": v = Math.abs(2 * t - 1); break;
      case "cove_deep": v = t ** 3; break;
      case "cove_shallow": v = t ** 1.6; break;
      case "gaussian": v = smooth(t); break;
      case "half_round": v = Math.sqrt(1 - (1 - t) ** 2); break;
      case "ring": v = peak(0.5, 0.18); break;
      case "ring_double": v = Math.max(peak(0.3, 0.1), peak(0.75, 0.1)); break;
      case "rolling_slope": v = clamp(1 - t + 0.2 * Math.sin(2 * Math.PI * t), 0, 1); break;
      case "rounded_steps": v = (Math.floor(t * 4) + smooth((t * 4) % 1)) / 4; break;
      case "sawtooth": v = (t * 3) % 1; break;
      default: v = t;
    }
  }
  return clamp(v, 0, 1);
}
function customAt(points, t) {
  const p = points.map(([x, y]) => [x / 255, y / 255]);
  const n = p.length;
  if (t <= p[0][0]) return p[0][1];
  if (t >= p[n - 1][0]) return p[n - 1][1];
  let i = 0;
  while (i < n - 2 && t > p[i + 1][0]) i++;
  const [p1, p2] = [p[i], p[i + 1]];
  const p0 = i > 0 ? p[i - 1] : [2 * p1[0] - p2[0], 2 * p1[1] - p2[1]];
  const p3 = i + 2 < n ? p[i + 2] : [2 * p2[0] - p1[0], 2 * p2[1] - p1[1]];
  const span = Math.max(p2[0] - p1[0], 1e-9);
  const u = (t - p1[0]) / span;
  const m1 = ((p2[1] - p0[1]) / Math.max(p2[0] - p0[0], 1e-9)) * span;
  const m2 = ((p3[1] - p1[1]) / Math.max(p3[0] - p1[0], 1e-9)) * span;
  const u2 = u * u, u3 = u2 * u;
  return (2 * u3 - 3 * u2 + 1) * p1[1] + (u3 - 2 * u2 + u) * m1 + (-2 * u3 + 3 * u2) * p2[1] + (u3 - u2) * m2;
}
/** A custom contour's JSON from `[x, y]` points (0..255). */
function customContour(points) {
  // Rounded first, then sorted and one point per input: two points that
  // round to the same input would otherwise both reach the engine.
  const sorted = points.map(([x, y]) => [Math.round(clamp(x, 0, 255)), Math.round(clamp(y, 0, 255))])
    .sort((a, b) => a[0] - b[0]).filter((p, i, a) => i === 0 || p[0] !== a[i - 1][0]).slice(0, 16);
  if (sorted.length < 2) return "linear";
  // Unused entries past `len` are padding the engine never reads.
  const padded = sorted.map((p) => [...p]);
  while (padded.length < 16) padded.push([0, 0]);
  return { custom: { points: padded, len: sorted.length } };
}
function contourPoints(c) {
  if (c && typeof c === "object" && c.custom) return c.custom.points.slice(0, c.custom.len).map((p) => [...p]);
  // A preset as editable points: sampled where it bends.
  const xs = [0, 32, 64, 96, 128, 160, 192, 224, 255];
  return xs.map((x) => [x, Math.round(contourAt(c, x / 255) * 255)]);
}

function drawContour(canvas, c, { grid = false, points = null, selected = -1 } = {}) {
  const ctx = canvas.getContext("2d");
  const { width: w, height: hgt } = canvas;
  ctx.fillStyle = "#f2f2f2";
  ctx.fillRect(0, 0, w, hgt);
  if (grid) {
    ctx.strokeStyle = "#cfcfcf";
    ctx.lineWidth = 1;
    for (let k = 1; k < 4; k++) {
      ctx.beginPath(); ctx.moveTo((w * k) / 4, 0); ctx.lineTo((w * k) / 4, hgt); ctx.stroke();
      ctx.beginPath(); ctx.moveTo(0, (hgt * k) / 4); ctx.lineTo(w, (hgt * k) / 4); ctx.stroke();
    }
  }
  ctx.fillStyle = "rgba(40,40,40,0.85)";
  ctx.beginPath();
  ctx.moveTo(0, hgt);
  for (let x = 0; x <= w; x++) ctx.lineTo(x, hgt - contourAt(c, x / w) * hgt);
  ctx.lineTo(w, hgt);
  ctx.closePath();
  ctx.fill();
  if (points) {
    points.forEach(([px, py], i) => {
      ctx.fillStyle = i === selected ? "#4c8dff" : "#fff";
      ctx.strokeStyle = "#000";
      ctx.beginPath();
      ctx.rect((px / 255) * w - 3, hgt - (py / 255) * hgt - 3, 6, 6);
      ctx.fill();
      ctx.stroke();
    });
  }
}

/**
 * Contour: its thumbnail (a click opens the Contour Editor), the preset
 * picker beside it, and Anti-aliased.
 */
function contourRow(label, contour, aa, onChange) {
  const canvas = h("canvas", { class: "ls-contour", width: 40, height: 40, "data-tip": "Edit the contour" });
  const name = h("span", { class: "dlg-note", text: contourName(contour) });
  const redraw = () => { drawContour(canvas, contour); name.textContent = contourName(contour); };
  redraw();
  canvas.addEventListener("click", () => openContourEditor(contour, (c) => { contour = c; redraw(); onChange(contour, aa); }));
  const pick = h("button", { class: "ls-mini", type: "button", "data-tip": "Contour presets" }, icon("i-chevron-down", "ic xs"));
  pick.addEventListener("click", (e) => {
    e.stopPropagation();
    openDropdown({
      anchor: pick, items: CONTOURS.map(([, n]) => n), value: contourName(contour), width: 200,
      onPick: (n) => { contour = (CONTOURS.find(([, l]) => l === n) || CONTOURS[0])[0]; redraw(); onChange(contour, aa); },
    });
  });
  return row(label, canvas, pick, name, check("Anti-aliased", aa, (on) => { aa = on; onChange(contour, aa); }));
}

/** The Contour Editor: click adds a point, drag moves it, drag it out removes it. */
function openContourEditor(contour, onOk) {
  let points = contourPoints(contour);
  let selected = -1;
  const S = 220;
  const canvas = h("canvas", { width: S, height: S, class: "ls-contour-editor" });
  const input = h("input", { class: "dlg-input num", type: "text", style: { width: "44px" } });
  const output = h("input", { class: "dlg-input num", type: "text", style: { width: "44px" } });
  const current = () => customContour(points);
  const paint = () => {
    drawContour(canvas, current(), { grid: true, points, selected });
    const p = points[selected];
    input.value = p ? String(p[0]) : "";
    output.value = p ? String(p[1]) : "";
  };
  const at = (e) => {
    const r = canvas.getBoundingClientRect();
    return [clamp(((e.clientX - r.left) / r.width) * 255, 0, 255), clamp((1 - (e.clientY - r.top) / r.height) * 255, 0, 255)];
  };
  canvas.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const [x, y] = at(e);
    selected = points.findIndex(([px, py]) => Math.abs(px - x) < 8 && Math.abs(py - y) < 8);
    if (selected < 0) {
      points.push([Math.round(x), Math.round(contourAt(current(), x / 255) * 255)]);
      points.sort((a, b) => a[0] - b[0]);
      selected = points.findIndex(([px]) => px === Math.round(x));
    }
    const r = canvas.getBoundingClientRect();
    const move = (m) => {
      const out = m.clientX < r.left - 20 || m.clientX > r.right + 20 || m.clientY < r.top - 20 || m.clientY > r.bottom + 20;
      if (out && points.length > 2 && selected > 0 && selected < points.length - 1) {
        points.splice(selected, 1);
        selected = -1;
        up();
        paint();
        return;
      }
      const [nx, ny] = at(m);
      const lo = selected > 0 ? points[selected - 1][0] + 1 : 0;
      const hi = selected < points.length - 1 ? points[selected + 1][0] - 1 : 255;
      points[selected] = [Math.round(clamp(nx, lo, hi)), Math.round(ny)];
      paint();
    };
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    paint();
  });
  const typed = () => {
    if (!points[selected]) return;
    points[selected] = [Math.round(clamp(num(input.value), 0, 255)), Math.round(clamp(num(output.value), 0, 255))];
    points.sort((a, b) => a[0] - b[0]);
    paint();
  };
  input.addEventListener("change", typed);
  output.addEventListener("change", typed);
  const presets = select(CONTOURS, "linear", (k) => { points = contourPoints(k); selected = -1; paint(); }, 170);
  const body = h("div", { class: "ls-contour-body" },
    canvas,
    h("div", { class: "ls-col" },
      row("Preset:", presets),
      row("Input:", input, h("span", { class: "dlg-unit", text: "%" })),
      row("Output:", output, h("span", { class: "dlg-unit", text: "%" })),
      h("div", { class: "dlg-note", text: "Click to add a point, drag to move it, drag it out of the box to remove it." })));
  paint();
  openDialog("contour-editor", {
    title: "Contour Editor", width: 470, plain: true,
    fields: [{ type: "element", el: body }],
    onOk: () => onOk(current()),
  });
}

/* ------------------------------------------------- gradients and patterns */

/** A gradient swatch: a click edits it (Gradient Editor), the arrow picks a preset. */
function gradientRow(label, g, onChange) {
  const bar = h("button", { class: "ls-gradient", type: "button", "data-tip": "Edit the gradient", style: { background: gradientCss(g) } });
  const refresh = () => { bar.style.background = gradientCss(g); };
  // Each edit shows on the canvas at once; Cancel puts the gradient back.
  const replace = (next) => {
    for (const k of Object.keys(g)) delete g[k];
    Object.assign(g, structuredClone(next));
  };
  bar.addEventListener("click", () => {
    const before = structuredClone(g);
    const work = structuredClone(g);
    const live = () => {
      const next = structuredClone(work);
      resolveSwatches(next);
      replace(next);
      refresh();
      onChange(g);
    };
    openDialog("gradient-editor-style", {
      title: "Gradient Editor", width: 520, plain: true,
      fields: [{ type: "element", el: gradientEditor(work, live) }],
      onOk: live,
      onCancel: () => { replace(before); refresh(); onChange(g); },
    });
  });
  const pick = h("button", { class: "ls-mini", type: "button", "data-tip": "Gradient presets" }, icon("i-chevron-down", "ic xs"));
  pick.addEventListener("click", (e) => {
    e.stopPropagation();
    openDropdown({
      anchor: pick, items: Object.keys(allGradients()), width: 220,
      onPick: (name) => {
        const preset = allGradients()[name];
        if (!preset) return;
        const next = structuredClone(preset);
        resolveSwatches(next);
        for (const k of Object.keys(g)) delete g[k];
        Object.assign(g, next);
        refresh();
        onChange(g);
      },
    });
  });
  return row(label, bar, pick);
}

function patternRow(id, onPick) {
  const selected = { id };
  return h("div", { class: "ls-row top" }, h("span", { class: "ls-label", text: "Pattern:" }), h("div", { class: "ls-patterns" }, patternList(selected, (pid) => onPick(pid))));
}

const GRADIENT_STYLES = [["linear", "Linear"], ["radial", "Radial"], ["angle", "Angle"], ["reflected", "Reflected"], ["diamond", "Diamond"]];
const METHODS = [["perceptual", "Perceptual"], ["linear", "Linear"], ["classic", "Classic"]];

/** The controls of a placed gradient (`GradientLayer`): Gradient Overlay, a stroke's gradient. */
function gradientLayerControls(gl, set, { align, onAlign, drag } = {}) {
  const kids = [
    gradientRow("Gradient:", gl.gradient, () => set({})),
    row(null, check("Reverse", !!gl.reverse, (on) => { gl.reverse = on; set({}); }), check("Dither", gl.dither !== false, (on) => { gl.dither = on; set({}); })),
    row("Style:", select(GRADIENT_STYLES, gl.kind || "linear", (k) => { gl.kind = k; set({}); }), onAlign ? check("Align with Layer", !!align, onAlign) : null),
    row("Method:", select(METHODS, gl.gradient.method || "perceptual", (m) => { gl.gradient.method = m; set({}); })),
    angleRow("Angle:", gl, (p) => { gl.angle = p.angle; set({}); }, { global: false }),
    slider("Scale:", num(gl.scale, 100), { min: 10, max: 150, unit: "%", onInput: (v) => { gl.scale = v; set({}); } }),
  ];
  if (drag) kids.push(row(null, h("button", { class: "btn small", type: "button", text: "Reset Alignment", onclick: () => { gl.offset = [0, 0]; set({}); } }), h("span", { class: "dlg-note", text: "Drag on the image to move it." })));
  return kids;
}

/* ------------------------------------------------------------ the window */

/**
 * The open window's state: the layer, the style and properties it had
 * (Cancel goes back to them), the style being edited, and the page shown
 * (`{type: "effect", key, i}`, `"blending"`, `"styles"`, or the bevel's
 * `"contour"` / `"texture"` sub-pages).
 */
let S = null;

export function isStyleDialog(id) {
  return id in DIALOG_KEYS || id === "blending-options" || id === "global-light" || id === "scale-effects";
}

/** Open the Layer Style window (or Global Light / Scale Effects) for the active layer. */
export function openStyleDialog(id) {
  if (id === "global-light") { openGlobalLight(); return; }
  if (id === "scale-effects") { openScaleEffects(); return; }
  const layer = activeLayerInfo();
  if (!layer) { toast("Select a layer first"); return; }
  if (layer.kind === "adjustment") { toast("An adjustment layer cannot have a layer style"); return; }
  if (S && S.wrap && S.wrap.isConnected) {
    // Already open (a menu while the window is up): just show that page.
    showKey(DIALOG_KEYS[id]);
    return;
  }
  S = {
    layer,
    light: { angle: globalLight().angle, altitude: globalLight().altitude },
    start: layer.styles ? structuredClone(layer.styles) : null,
    props: { opacity: layer.opacity, fill: layer.fill, blend: layer.blend },
    styles: normalize(layer.styles),
    page: { type: "blending" },
    preview: true,
    showAll: true,
  };
  const key = DIALOG_KEYS[id];
  if (key) showKey(key, { build: false });
  build();
}

/**
 * Open the window on effect `key`'s instance `i` (the Layers panel's effect
 * rows), or on Blending Options with `key` null (a double-click on a layer).
 */
export function openStyleDialogAt(key, i = 0) {
  if (!(S && S.wrap && S.wrap.isConnected)) openStyleDialog("blending-options");
  if (!S) return;
  if (key && S.styles[key]?.[i]) S.page = { type: "effect", key, i };
  redraw();
}

/** Show effect `key`'s page, turning it on (adding it) as clicking its name does in Photoshop. */
function showKey(key, { build: rebuild = true } = {}) {
  if (!key) return;
  const list = S.styles[key];
  if (!list.length) list.push(newEffect(key));
  list[0].enabled = true;
  S.page = { type: "effect", key, i: 0 };
  commit();
  if (rebuild) redraw();
}

function build() {
  const sidebar = h("div", { class: "ls-sidebar" });
  const pane = h("div", { class: "ls-pane" });
  const previewBox = check("Preview", S.preview, (on) => { S.preview = on; commit({ props: true }); });
  const side = h("div", { class: "ls-side" },
    h("button", { class: "btn primary", type: "button", text: "OK", onclick: () => finish(true) }),
    h("button", { class: "btn", type: "button", text: "Cancel", onclick: () => finish(false) }),
    h("button", { class: "btn", type: "button", text: "New Style...", onclick: newStyle }),
    previewBox);
  const root = h("div", { class: "ls-window" }, sidebar, pane, side);
  S.sidebar = sidebar;
  S.pane = pane;
  S.wrap = openDialog("layer-style", {
    title: `Layer Style — ${S.layer.name}`, width: 900, plain: true, icon: "i-fx",
    fields: [{ type: "element", el: root }],
    ok: null, cancel: null,
    onCancel: () => revert(),
  });
  // Clicks outside the window do not close it (Photoshop's is modal); a
  // drag on the image moves what the page places.
  S.wrap.addEventListener("click", (e) => { if (e.target.classList?.contains("modal-scrim")) e.stopPropagation(); }, true);
  canvasDrag(S.wrap, (dx, dy) => S.drag && S.drag(dx, dy));
  redraw();
}

function redraw() {
  drawSidebar();
  drawPane();
}

/** OK keeps what is on the layer; Cancel (and ×, Escape) puts it back. */
function finish(ok) {
  const wrap = S.wrap;
  if (ok) {
    if (!S.preview) { S.preview = true; commit({ props: true }); }
    S = null;
    wrap._fotoxClose(undefined, true);
  } else {
    wrap._fotoxClose(undefined, false);
  }
}

function revert() {
  if (!S) return;
  sendCommand({ op: "set_layer_style", layer: { id: S.layer.id }, styles: S.start });
  sendCommand({ op: "set_layer_props", layer: { id: S.layer.id }, props: S.props });
  setGlobalLight(S.light.angle, S.light.altitude);
  S = null;
}

/**
 * Send the style (or, with Preview off, the one the layer had). The
 * Blending Options' opacity, fill and mode follow Preview the same way
 * (`props`: when Preview changes; while it is on they are sent as edited).
 */
function commit({ props = false } = {}) {
  if (!S) return;
  // A Pattern Overlay needs a pattern: without one it stays off, and says so
  // (as Stroke and Bevel Texture do).
  for (const e of S.styles.pattern_overlay || []) {
    if (e.pattern == null && e.enabled !== false) {
      e.enabled = false;
      toast("Define a pattern first (Edit ▸ Define Pattern)");
      if (S.sidebar) queueMicrotask(() => S && drawSidebar());
    }
  }
  const styles = S.preview ? compact(S.styles) : S.start;
  sendCommand({ op: "set_layer_style", layer: { id: S.layer.id }, styles });
  if (props && S.liveProps) {
    sendCommand({ op: "set_layer_props", layer: { id: S.layer.id }, props: S.preview ? S.liveProps : S.props });
  }
}

/* -------------------------------------------------------------- sidebar */

function drawSidebar() {
  const bar = S.sidebar;
  clear(bar);
  const list = h("div", { class: "ls-list" });
  const item = (label, page, { box, active, plus, indent, dim } = {}) => {
    const r = h("div", { class: "ls-item" + (active ? " sel" : "") + (indent ? " sub" : "") + (dim ? " dim" : "") });
    if (box) {
      const b = h("span", { class: "style-picker-check live" + (box.on ? " on" : ""), "data-tip": box.on ? "Turn off" : "Turn on" });
      if (box.on) b.append(icon("i-check", "ic xs"));
      b.addEventListener("click", (e) => { e.stopPropagation(); box.toggle(); });
      r.append(b);
    } else {
      r.append(h("span", { class: "ls-nobox" }));
    }
    r.append(h("span", { class: "ls-item-name", text: label }));
    if (plus) {
      r.append(h("button", { class: "ls-plus", type: "button", "data-tip": `Add another ${label}`, onclick: (e) => { e.stopPropagation(); plus(); } }, icon("i-plus", "ic xs")));
    }
    r.addEventListener("click", page);
    list.append(r);
  };
  const is = (t) => S.page.type === t;
  item("Styles", () => { S.page = { type: "styles" }; redraw(); }, { active: is("styles") });
  item("Blending Options", () => { S.page = { type: "blending" }; redraw(); }, { active: is("blending") });
  for (const { key, title, stack } of EFFECTS) {
    const inst = S.styles[key];
    if (!S.showAll && !inst.length) continue;
    const rows = inst.length ? inst : [null];
    rows.forEach((e, i) => {
      const active = is("effect") && S.page.key === key && S.page.i === i;
      item(title, () => {
        if (!e) { S.styles[key].push(newEffect(key)); commit(); }
        S.page = { type: "effect", key, i };
        redraw();
      }, {
        active,
        box: {
          on: !!e && e.enabled !== false,
          toggle: () => {
            if (!e) S.styles[key].push(newEffect(key));
            else e.enabled = e.enabled === false;
            commit();
            drawSidebar();
          },
        },
        plus: stack && inst.length < MAX_INSTANCES ? () => {
          const copy = e ? structuredClone(e) : newEffect(key);
          copy.enabled = true;
          S.styles[key].splice(e ? i : 0, 0, copy);
          if (!e) S.styles[key].length = 1;
          S.page = { type: "effect", key, i: e ? i : 0 };
          commit();
          redraw();
        } : null,
      });
      if (key === "bevel" && e) {
        item("Contour", () => { S.page = { type: "contour", key, i }; redraw(); }, {
          indent: true, active: is("contour"),
          box: { on: e.contour !== "linear" || num(e.contour_range, 100) !== 100, toggle: () => { e.contour = e.contour !== "linear" ? "linear" : "half_round"; commit(); redraw(); } },
        });
        item("Texture", () => { ensureTexture(e); S.page = { type: "texture", key, i }; commit(); redraw(); }, {
          indent: true, active: is("texture"),
          box: { on: !!e.texture, toggle: () => { if (e.texture) delete e.texture; else ensureTexture(e); commit(); redraw(); } },
        });
      }
    });
  }
  // The bottom bar: fx menu, move the selected effect up / down, delete it.
  const sel = is("effect") ? S.page : null;
  const fx = h("button", { class: "ls-mini wide", type: "button", "data-tip": "Effects list" }, icon("i-fx", "ic sm"), icon("i-chevron-down", "ic xs"));
  fx.addEventListener("click", (e) => {
    e.stopPropagation();
    const items = [S.showAll ? "Show Applied Effects Only" : "Show All Effects", "Reset to Default List", "Clear All Effects"];
    openDropdown({
      anchor: fx, items, width: 220,
      onPick: (what) => {
        if (what === "Show All Effects" || what === "Show Applied Effects Only") S.showAll = !S.showAll;
        if (what === "Reset to Default List") for (const { key } of EFFECTS) S.styles[key] = S.styles[key].slice(0, 1);
        if (what === "Clear All Effects") { for (const { key } of EFFECTS) S.styles[key] = []; S.page = { type: "blending" }; }
        commit();
        redraw();
      },
    });
  });
  const move = (d) => {
    const listOf = S.styles[sel.key];
    const j = sel.i + d;
    if (j < 0 || j >= listOf.length) return;
    [listOf[sel.i], listOf[j]] = [listOf[j], listOf[sel.i]];
    S.page = { ...sel, i: j };
    commit();
    redraw();
  };
  const stackable = sel && EFFECTS.find((x) => x.key === sel.key)?.stack && S.styles[sel.key].length > 1;
  bar.append(list, h("div", { class: "ls-tools" },
    fx,
    h("span", { style: { flex: "1" } }),
    h("button", { class: "ls-mini", type: "button", "data-tip": "Move effect up", disabled: !stackable, onclick: () => move(-1) }, icon("i-chevron-up", "ic xs")),
    h("button", { class: "ls-mini", type: "button", "data-tip": "Move effect down", disabled: !stackable, onclick: () => move(1) }, icon("i-chevron-down", "ic xs")),
    h("button", {
      class: "ls-mini", type: "button", "data-tip": "Delete effect", disabled: !sel,
      onclick: () => { if (!sel) return; S.styles[sel.key].splice(sel.i, 1); S.page = { type: "blending" }; commit(); redraw(); },
    }, icon("i-trash", "ic xs"))));
}

function ensureTexture(e) {
  if (e.texture) return;
  const pattern = currentPattern();
  if (pattern == null) { toast("Define a pattern first (Edit ▸ Define Pattern)"); return; }
  e.texture = { pattern, scale: 100, depth: 100, invert: false, align: true, phase: [0, 0] };
}

/* ----------------------------------------------------------------- pages */

function drawPane() {
  const pane = S.pane;
  clear(pane);
  S.drag = null;
  const p = S.page;
  if (p.type === "styles") pane.append(stylesPage());
  else if (p.type === "blending") pane.append(blendingPage());
  else {
    const e = S.styles[p.key]?.[p.i];
    if (!e) { S.page = { type: "blending" }; pane.append(blendingPage()); return; }
    const set = (patch) => { Object.assign(e, patch); commit(); if (patch.enabled !== undefined) drawSidebar(); };
    const title = p.type === "contour" ? "Contour" : p.type === "texture" ? "Texture" : TITLE[p.key] + (S.styles[p.key].length > 1 ? ` (${p.i + 1})` : "");
    pane.append(h("div", { class: "ls-title", text: title }));
    const page = p.type === "contour" ? bevelContourPage(e, set) : p.type === "texture" ? texturePage(e, set) : PAGES[p.key](e, set);
    pane.append(...page);
    if (p.type === "effect") pane.append(defaultsBar(p.key, e));
  }
}

/** Make Default / Reset to Default (kept in the preferences). */
function defaultsBar(key, e) {
  return h("div", { class: "ls-defaults" },
    h("button", {
      class: "btn small", type: "button", text: "Make Default",
      onclick: () => {
        const all = { ...(prefValue("style_defaults") || {}) };
        const copy = structuredClone(e);
        delete copy.enabled;
        all[key] = copy;
        setPrefs({ style_defaults: all });
        toast(`${TITLE[key]}: these settings are now the default`);
      },
    }),
    h("button", {
      class: "btn small", type: "button", text: "Reset to Default",
      onclick: () => {
        const fresh = newEffect(key);
        for (const k of Object.keys(e)) delete e[k];
        Object.assign(e, fresh);
        commit();
        drawPane();
      },
    }));
}

const PX = { min: 0, max: 250, unit: "px" };
const PCT = { min: 0, max: 100, unit: "%" };

const PAGES = {
  drop_shadow: (e, set) => {
    const angle = angleRow("Angle:", e, set);
    const distance = slider("Distance:", num(e.distance, 5), { ...PX, max: 500, onInput: (v) => set({ distance: v }) });
    S.drag = shadowDrag(e, set, angle, distance, -1);
    return [
      group("Structure",
        blendRow("Blend Mode:", e.blend, (m) => set({ blend: m }), e.color, (c) => set({ color: c })),
        slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) }),
        angle, distance,
        slider("Spread:", num(e.spread), { ...PCT, onInput: (v) => set({ spread: v }) }),
        slider("Size:", num(e.size, 5), { ...PX, onInput: (v) => set({ size: v }) })),
      group("Quality",
        contourRow("Contour:", e.contour || "linear", !!e.anti_aliased, (c, aa) => set({ contour: c, anti_aliased: aa })),
        slider("Noise:", num(e.noise), { ...PCT, onInput: (v) => set({ noise: v }) }),
        check("Layer Knocks Out Drop Shadow", e.knocks_out !== false, (on) => set({ knocks_out: on }))),
      h("div", { class: "dlg-note", text: "Drag on the image to move the shadow." }),
    ];
  },
  inner_shadow: (e, set) => {
    const angle = angleRow("Angle:", e, set);
    const distance = slider("Distance:", num(e.distance, 5), { ...PX, max: 500, onInput: (v) => set({ distance: v }) });
    S.drag = shadowDrag(e, set, angle, distance, -1);
    return [
      group("Structure",
        blendRow("Blend Mode:", e.blend, (m) => set({ blend: m }), e.color, (c) => set({ color: c })),
        slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) }),
        angle, distance,
        slider("Choke:", num(e.choke), { ...PCT, onInput: (v) => set({ choke: v }) }),
        slider("Size:", num(e.size, 5), { ...PX, onInput: (v) => set({ size: v }) })),
      group("Quality",
        contourRow("Contour:", e.contour || "linear", !!e.anti_aliased, (c, aa) => set({ contour: c, anti_aliased: aa })),
        slider("Noise:", num(e.noise), { ...PCT, onInput: (v) => set({ noise: v }) })),
      h("div", { class: "dlg-note", text: "Drag on the image to move the shadow." }),
    ];
  },
  outer_glow: (e, set) => glowPage(e, set, false),
  inner_glow: (e, set) => glowPage(e, set, true),
  bevel: (e, set) => {
    const altitude = slider("Altitude:", e.use_global_light ? globalLight().altitude : num(e.altitude, 30), {
      min: 0, max: 90, unit: "°",
      onInput: (v) => (e.use_global_light ? setGlobalLight(globalLight().angle, v) : set({ altitude: v })),
    });
    return [
      group("Structure",
        row("Style:", select([["inner_bevel", "Inner Bevel"], ["outer_bevel", "Outer Bevel"], ["emboss", "Emboss"], ["pillow_emboss", "Pillow Emboss"], ["stroke_emboss", "Stroke Emboss"]], e.style, (v) => {
          set({ style: v });
          if (v === "stroke_emboss" && !S.styles.stroke.some((s) => s.enabled !== false)) toast("Stroke Emboss embosses the Stroke effect: turn on a Stroke");
        })),
        row("Technique:", select([["smooth", "Smooth"], ["chisel_hard", "Chisel Hard"], ["chisel_soft", "Chisel Soft"]], e.technique || "smooth", (v) => set({ technique: v }))),
        slider("Depth:", num(e.depth, 100), { min: 1, max: 1000, unit: "%", onInput: (v) => set({ depth: v }) }),
        row("Direction:", segmented([[true, "Up"], [false, "Down"]], e.up !== false, (v) => set({ up: v }))),
        slider("Size:", num(e.size, 5), { ...PX, onInput: (v) => set({ size: v }) }),
        slider("Soften:", num(e.soften), { min: 0, max: 16, unit: "px", onInput: (v) => set({ soften: v }) })),
      group("Shading",
        angleRow("Angle:", e, (p) => { set(p); altitude.setValue(p.use_global_light ? globalLight().altitude : num(e.altitude, 30)); }),
        altitude,
        contourRow("Gloss Contour:", e.gloss_contour || "linear", !!e.anti_aliased, (c, aa) => set({ gloss_contour: c, anti_aliased: aa })),
        blendRow("Highlight Mode:", e.highlight_blend, (m) => set({ highlight_blend: m }), e.highlight_color, (c) => set({ highlight_color: c })),
        slider("Opacity:", pct(e.highlight_opacity), { ...PCT, onInput: (v) => set({ highlight_opacity: v / 100 }) }),
        blendRow("Shadow Mode:", e.shadow_blend, (m) => set({ shadow_blend: m }), e.shadow_color, (c) => set({ shadow_color: c })),
        slider("Opacity:", pct(e.shadow_opacity), { ...PCT, onInput: (v) => set({ shadow_opacity: v / 100 }) })),
    ];
  },
  satin: (e, set) => {
    const angle = angleRow("Angle:", e, set, { global: false });
    const distance = slider("Distance:", num(e.distance, 11), { ...PX, onInput: (v) => set({ distance: v }) });
    S.drag = shadowDrag(e, set, angle, distance, 1);
    return [
      group("Structure",
        blendRow("Blend Mode:", e.blend, (m) => set({ blend: m }), e.color, (c) => set({ color: c })),
        slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) }),
        angle, distance,
        slider("Size:", num(e.size, 14), { ...PX, onInput: (v) => set({ size: v }) }),
        contourRow("Contour:", e.contour || "linear", !!e.anti_aliased, (c, aa) => set({ contour: c, anti_aliased: aa })),
        check("Invert", !!e.invert, (on) => set({ invert: on }))),
    ];
  },
  color_overlay: (e, set) => [
    group("Color",
      blendRow("Blend Mode:", e.blend, (m) => set({ blend: m }), e.color, (c) => set({ color: c })),
      slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) })),
  ],
  gradient_overlay: (e, set) => {
    const gl = e.gradient;
    S.drag = (dx, dy) => { gl.offset = [num(gl.offset?.[0]) + dx, num(gl.offset?.[1]) + dy]; set({}); };
    return [
      group("Gradient",
        blendRow("Blend Mode:", e.blend, (m) => set({ blend: m })),
        slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) }),
        ...gradientLayerControls(gl, set, { align: e.align, onAlign: (on) => set({ align: on }), drag: true })),
    ];
  },
  pattern_overlay: (e, set) => {
    S.drag = (dx, dy) => { e.phase = [num(e.phase?.[0]) + dx, num(e.phase?.[1]) + dy]; set({}); };
    return [
      group("Pattern",
        blendRow("Blend Mode:", e.blend, (m) => set({ blend: m })),
        slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) }),
        e.pattern == null ? h("div", { class: "dlg-note", text: "No pattern: define one first (Edit ▸ Define Pattern), then pick it here." }) : null,
        patternRow(e.pattern, (id) => set({ pattern: id, enabled: true })),
        row(null, h("button", { class: "btn small", type: "button", text: "Snap to Origin", onclick: () => set({ phase: [0, 0] }) }), check("Link with Layer", !!e.align, (on) => set({ align: on }))),
        angleRow("Angle:", e, (p) => set({ angle: p.angle }), { global: false }),
        slider("Scale:", num(e.scale, 100), { min: 1, max: 1000, unit: "%", onInput: (v) => set({ scale: v }) }),
        h("div", { class: "dlg-note", text: "Drag on the image to move the pattern." })),
    ];
  },
  stroke: (e, set) => {
    const fill = e.fill || { type: "color" };
    const fillType = row("Fill Type:", select([["color", "Color"], ["gradient", "Gradient"], ["pattern", "Pattern"]], fill.type, (t) => {
      if (t === fill.type) return;
      if (t === "gradient") set({ fill: { type: "gradient", align: true, gradient: defaultGradientLayer() } });
      else if (t === "pattern") {
        const id = currentPattern();
        if (id == null) { toast("Define a pattern first (Edit ▸ Define Pattern)"); drawPane(); return; }
        set({ fill: { type: "pattern", pattern: id, scale: 100, angle: 0, phase: [0, 0], align: true } });
      } else set({ fill: { type: "color" } });
      drawPane();
    }));
    const fillControls = [];
    if (fill.type === "gradient") {
      S.drag = (dx, dy) => { fill.gradient.offset = [num(fill.gradient.offset?.[0]) + dx, num(fill.gradient.offset?.[1]) + dy]; set({}); };
      fillControls.push(...gradientLayerControls(fill.gradient, set, { align: fill.align, onAlign: (on) => { fill.align = on; set({}); }, drag: true }));
    } else if (fill.type === "pattern") {
      S.drag = (dx, dy) => { fill.phase = [num(fill.phase?.[0]) + dx, num(fill.phase?.[1]) + dy]; set({}); };
      fillControls.push(
        patternRow(fill.pattern, (id) => { fill.pattern = id; set({}); }),
        row(null, h("button", { class: "btn small", type: "button", text: "Snap to Origin", onclick: () => { fill.phase = [0, 0]; set({}); } }), check("Link with Layer", !!fill.align, (on) => { fill.align = on; set({}); })),
        slider("Scale:", num(fill.scale, 100), { min: 1, max: 1000, unit: "%", onInput: (v) => { fill.scale = v; set({}); } }));
    } else {
      fillControls.push(row("Color:", colorWell(e.color, (c) => set({ color: c }))));
    }
    return [
      group("Structure",
        slider("Size:", num(e.size, 3), { min: 1, max: 250, unit: "px", onInput: (v) => set({ size: v }) }),
        row("Position:", select([["outside", "Outside"], ["inside", "Inside"], ["center", "Center"]], e.position || "outside", (v) => set({ position: v }))),
        blendRow("Blend Mode:", e.blend, (m) => set({ blend: m })),
        slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) })),
      group("Fill", fillType, ...fillControls),
    ];
  },
};

function glowPage(e, set, inner) {
  const fill = e.fill || { type: "color" };
  const which = fill.type === "gradient" ? "gradient" : "color";
  const colorPick = h("div", { class: "ls-row" },
    segmented([["color", "Color"], ["gradient", "Gradient"]], which, (v) => {
      if (v === "gradient") set({ fill: { type: "gradient", align: false, gradient: defaultGradientLayer() } });
      else set({ fill: { type: "color" } });
      drawPane();
    }),
    which === "color" ? colorWell(e.color, (c) => set({ color: c })) : null);
  const gradientBits = which === "gradient"
    ? [gradientRow("Gradient:", fill.gradient.gradient, () => set({})), check("Reverse", !!fill.gradient.reverse, (on) => { fill.gradient.reverse = on; set({}); })]
    : [];
  return [
    group("Structure",
      blendRow("Blend Mode:", e.blend, (m) => set({ blend: m })),
      slider("Opacity:", pct(e.opacity), { ...PCT, onInput: (v) => set({ opacity: v / 100 }) }),
      slider("Noise:", num(e.noise), { ...PCT, onInput: (v) => set({ noise: v }) }),
      colorPick, ...gradientBits),
    group("Elements",
      row("Technique:", segmented([["softer", "Softer"], ["precise", "Precise"]], e.technique || "softer", (v) => set({ technique: v }))),
      inner ? row("Source:", segmented([["center", "Center"], ["edge", "Edge"]], e.source || "edge", (v) => set({ source: v }))) : null,
      inner
        ? slider("Choke:", num(e.choke), { ...PCT, onInput: (v) => set({ choke: v }) })
        : slider("Spread:", num(e.spread), { ...PCT, onInput: (v) => set({ spread: v }) }),
      slider("Size:", num(e.size, 5), { ...PX, onInput: (v) => set({ size: v }) })),
    group("Quality",
      contourRow("Contour:", e.contour || "linear", !!e.anti_aliased, (c, aa) => set({ contour: c, anti_aliased: aa })),
      slider("Range:", num(e.range, 50), { min: 1, max: 100, unit: "%", onInput: (v) => set({ range: v }) }),
      slider("Jitter:", num(e.jitter), { ...PCT, onInput: (v) => set({ jitter: v }) })),
  ];
}

function bevelContourPage(e, set) {
  return [
    group("Elements",
      contourRow("Contour:", e.contour || "linear", !!e.contour_anti_aliased, (c, aa) => set({ contour: c, contour_anti_aliased: aa })),
      slider("Range:", num(e.contour_range, 100), { min: 1, max: 100, unit: "%", onInput: (v) => set({ contour_range: v }) })),
  ];
}

function texturePage(e, set) {
  const t = e.texture;
  if (!t) return [h("div", { class: "dlg-note", text: "Define a pattern first (Edit ▸ Define Pattern), then tick Texture." })];
  const tset = (patch) => { Object.assign(t, patch); set({}); };
  S.drag = (dx, dy) => tset({ phase: [num(t.phase?.[0]) + dx, num(t.phase?.[1]) + dy] });
  return [
    group("Elements",
      patternRow(t.pattern, (id) => tset({ pattern: id })),
      row(null, h("button", { class: "btn small", type: "button", text: "Snap to Origin", onclick: () => tset({ phase: [0, 0] }) }), check("Link with Layer", t.align !== false, (on) => tset({ align: on }))),
      slider("Scale:", num(t.scale, 100), { min: 1, max: 1000, unit: "%", onInput: (v) => tset({ scale: v }) }),
      slider("Depth:", num(t.depth, 100), { min: -1000, max: 1000, unit: "%", onInput: (v) => tset({ depth: v }) }),
      check("Invert", !!t.invert, (on) => tset({ invert: on })),
      h("div", { class: "dlg-note", text: "Drag on the image to move the texture." })),
  ];
}

/**
 * A drag on the image moves a shadow (`sign` −1: cast away from the light)
 * or a satin (`sign` 1): the offset follows the pointer, and the angle and
 * distance follow the offset (the Global Light with Use Global Light).
 */
function shadowDrag(e, set, angleEl, distanceEl, sign) {
  const global = () => sign < 0 && e.use_global_light;
  return (dx, dy) => {
    const a0 = ((global() ? globalLight().angle : num(e.angle)) * Math.PI) / 180;
    const d0 = num(e.distance);
    // The offset: a shadow's (−cos a, sin a)·d, a satin's (cos a, −sin a)·d
    // (`fx_core::styles::LayerStyles::effect_at`, y down).
    const ox = sign * Math.cos(a0) * d0 + dx;
    const oy = -sign * Math.sin(a0) * d0 + dy;
    const d = Math.round(Math.hypot(ox, oy));
    const a = Math.round((Math.atan2(-sign * oy, sign * ox) * 180) / Math.PI);
    if (global()) setGlobalLight(a);
    set(global() ? { distance: d } : { distance: d, angle: a });
    angleEl.redraw();
    distanceEl.setValue(d);
  };
}

/* ------------------------------------------------------- blending options */

function blendingPage() {
  const layer = S.layer;
  const props = { opacity: layer.opacity, fill: layer.fill, blend: layer.blend, ...(S.liveProps || {}) };
  const setProps = (patch) => {
    S.liveProps = { ...props, ...patch };
    Object.assign(props, patch);
    // Preview off: kept for OK (or for Preview on), the layer is untouched.
    if (S.preview) sendCommand({ op: "set_layer_props", layer: { id: layer.id }, props: patch });
  };
  const st = S.styles;
  const channels = st.channels || [true, true, true];
  const setStyle = (patch) => { Object.assign(st, patch); commit(); };
  const start = st.blend_if || { this_layer: [0, 0, 255, 255], underlying: [0, 0, 255, 255] };
  const blendIf = { this_layer: [...start.this_layer], underlying: [...start.underlying] };
  const sendBlendIf = () => {
    const open = (r) => r[0] === 0 && r[1] === 0 && r[2] === 255 && r[3] === 255;
    if (open(blendIf.this_layer) && open(blendIf.underlying)) delete st.blend_if;
    else st.blend_if = { this_layer: [...blendIf.this_layer], underlying: [...blendIf.underlying] };
    commit();
  };
  const group_ = layer.kind === "group";
  return h("div", { class: "ls-col" },
    h("div", { class: "ls-title", text: "Blending Options" }),
    group("General Blending",
      blendRow("Blend Mode:", props.blend, (m) => setProps({ blend: m })),
      slider("Opacity:", pct(props.opacity), { ...PCT, onInput: (v) => setProps({ opacity: v / 100 }) })),
    group("Advanced Blending",
      group_ ? null : slider("Fill Opacity:", pct(props.fill), { ...PCT, onInput: (v) => setProps({ fill: v / 100 }) }),
      row("Channels:", ...["R", "G", "B"].map((c, i) => check(c, channels[i], (on) => { const next = [...channels]; next[i] = on; channels[i] = on; setStyle({ channels: next }); }))),
      group_ ? null : check("Blend Interior Effects as Group", !!st.interior_as_group, (on) => setStyle({ interior_as_group: on }), "The overlays, satin and inner glow blend with the content first; the layer's mode applies to the result"),
      check("Layer Mask Hides Effects", !!st.layer_mask_hides, (on) => setStyle({ layer_mask_hides: on }), "The mask cuts the effects, instead of the effects following the masked shape"),
      check("Vector Mask Hides Effects", !!st.vector_mask_hides, (on) => setStyle({ vector_mask_hides: on }))),
    blendIfEditor(blendIf, sendBlendIf));
}

/**
 * Blend If (Gray): two black-to-white bars, each with a black and a white
 * handle. Alt-drag splits a handle into its two halves (the fade), as in
 * Photoshop; a plain drag moves both halves.
 */
function blendIfEditor(model, onChange) {
  const W = 256;
  const box = h("div", { class: "ls-group blendif" }, h("div", { class: "ls-group-title", text: "Blend If: Gray (Alt-drag splits a handle)" }));
  const bar = (label, key) => {
    const r = h("div", { class: "blendif-row", style: { margin: "6px 0 14px" } });
    const out = h("span", { style: { float: "right", opacity: ".8" } });
    const name = h("div", { class: "dlg-label" }, label, out);
    const track = h("div", { style: { position: "relative", width: W + "px", height: "12px", background: "linear-gradient(90deg,#000,#fff)", border: "1px solid var(--border, #555)" } });
    const handles = [0, 1, 2, 3].map((i) => {
      const hnd = h("div", {
        style: {
          position: "absolute", top: "12px", width: "0", height: "0", borderLeft: "5px solid transparent", borderRight: "5px solid transparent",
          borderBottom: `8px solid ${i < 2 ? "#111" : "#eee"}`, marginLeft: "-5px", cursor: "ew-resize", filter: "drop-shadow(0 0 1px #888)",
        },
      });
      track.append(hnd);
      return hnd;
    });
    const draw = () => {
      const rr = model[key];
      handles.forEach((hnd, i) => { hnd.style.left = `${(rr[i] / 255) * W}px`; });
      out.textContent = `${rr[0] === rr[1] ? rr[0] : `${rr[0]}/${rr[1]}`}   ${rr[2] === rr[3] ? rr[2] : `${rr[2]}/${rr[3]}`}`;
    };
    handles.forEach((hnd, index) => hnd.addEventListener("mousedown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      let i = index;
      const split = e.altKey;
      const rect = track.getBoundingClientRect();
      const pair = i < 2 ? [0, 1] : [2, 3];
      let pick = null;
      const move = (m) => {
        const v = Math.round(clamp(((m.clientX - rect.left) / rect.width) * 255, 0, 255));
        const rr = model[key];
        if (split && pick === null) pick = rr[pair[0]] === rr[pair[1]] ? (v < rr[i] ? pair[0] : pair[1]) : i;
        if (split) i = pick;
        if (split || rr[pair[0]] !== rr[pair[1]]) rr[i] = v; else { rr[pair[0]] = v; rr[pair[1]] = v; }
        if (i === 0) rr[1] = Math.max(rr[1], rr[0]);
        if (i === 1) rr[0] = Math.min(rr[0], rr[1]);
        if (i === 2) rr[3] = Math.max(rr[3], rr[2]);
        if (i === 3) rr[2] = Math.min(rr[2], rr[3]);
        if (rr[1] > rr[2]) { if (i < 2) { rr[2] = rr[1]; rr[3] = Math.max(rr[3], rr[2]); } else { rr[1] = rr[2]; rr[0] = Math.min(rr[0], rr[1]); } }
        draw();
        onChange();
      };
      const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    }));
    draw();
    r.append(name, track);
    return r;
  };
  const reset = h("button", { class: "btn small", type: "button", text: "Reset" });
  box.append(bar("This Layer:", "this_layer"), bar("Underlying Layer:", "underlying"), reset);
  reset.addEventListener("click", () => {
    model.this_layer = [0, 0, 255, 255];
    model.underlying = [0, 0, 255, 255];
    box.replaceWith(blendIfEditor(model, onChange));
    onChange();
  });
  return box;
}

/* ---------------------------------------------------- presets and swatches */

/** The saved style presets (`[{name, styles}]`), from the preferences. */
export function stylePresets() {
  const list = prefValue("style_presets");
  return Array.isArray(list) ? list.filter((p) => p && typeof p.name === "string" && p.styles) : [];
}

export function saveStylePresets(list) {
  setPrefs({ style_presets: list });
}

function stylesPage() {
  const grid = h("div", { class: "ls-presets" });
  const list = stylePresets();
  if (!list.length) grid.append(h("div", { class: "dlg-note", text: "No saved styles yet: build one here, then press New Style..." }));
  list.forEach((p, i) => {
    grid.append(h("button", {
      class: "ls-preset", type: "button", "data-tip": `${p.name} — click to use it`,
      onclick: () => { S.styles = normalize(p.styles); commit(); drawSidebar(); },
      oncontextmenu: (e) => {
        e.preventDefault();
        openDropdown({
          anchor: e.currentTarget, items: ["Rename Style...", "Delete Style"], width: 160,
          onPick: async (what) => {
            const next = stylePresets();
            if (what === "Delete Style") next.splice(i, 1);
            else {
              const name = await askText("Rename Style", "Name:", p.name);
              if (!name) return;
              next[i] = { ...next[i], name };
            }
            saveStylePresets(next);
            if (S) drawPane();
          },
        });
      },
    }, styleSwatch(p.styles, 44)));
  });
  return h("div", { class: "ls-col" }, h("div", { class: "ls-title", text: "Styles" }), grid, h("div", { class: "dlg-note", text: "Right-click a style to rename or delete it." }));
}

async function newStyle() {
  const name = await askText("New Style", "Name:", `${S.layer.name} Style`);
  if (!name || !S) return;
  const styles = compact(S.styles);
  if (!styles) { toast("The style is empty: turn an effect on first"); return; }
  saveStylePresets([...stylePresets(), { name, styles }]);
  toast(`Style "${name}" saved`);
  if (S.page.type === "styles") drawPane();
}

const cssColor = (c, alpha = 1) => `rgba(${Math.round(c[0] / 257)},${Math.round(c[1] / 257)},${Math.round(c[2] / 257)},${((c[3] ?? 65535) / 65535) * alpha})`;
const on = (list) => (Array.isArray(list) ? list : list ? [list] : []).filter((e) => e && e.enabled !== false);

/**
 * A square showing a style roughly (CSS: shadows as box-shadows, the stroke
 * as a border, the overlays as the fill): an icon for the saved styles, not
 * a preview. It cannot show blend modes, the backdrop, Pattern Overlay or
 * Satin, so the Layer Style window has none: the canvas is its preview.
 */
export function styleSwatch(styles, size = 36) {
  const k = size / 64;
  const shape = h("div", { class: "ls-swatch" });
  const s = { width: size * 0.62 + "px", height: size * 0.62 + "px", background: "#cfcfcf", borderRadius: Math.round(size * 0.08) + "px" };
  const shadows = [];
  if (styles && styles.effects_visible !== false) {
    const light = globalLight().angle;
    for (const e of on(styles.drop_shadow)) {
      const a = ((e.use_global_light ? light : e.angle) * Math.PI) / 180;
      shadows.push(`${-Math.cos(a) * e.distance * k}px ${Math.sin(a) * e.distance * k}px ${e.size * k}px ${cssColor(e.color, e.opacity)}`);
    }
    for (const e of on(styles.outer_glow)) shadows.push(`0 0 ${Math.max(2, e.size * k * 1.5)}px ${cssColor(e.color, e.opacity)}`);
    for (const e of on(styles.inner_shadow)) {
      const a = ((e.use_global_light ? light : e.angle) * Math.PI) / 180;
      shadows.push(`inset ${-Math.cos(a) * e.distance * k}px ${Math.sin(a) * e.distance * k}px ${e.size * k}px ${cssColor(e.color, e.opacity)}`);
    }
    for (const e of on(styles.inner_glow)) shadows.push(`inset 0 0 ${Math.max(2, e.size * k * 1.5)}px ${cssColor(e.color, e.opacity)}`);
    for (const e of on(styles.bevel)) {
      const d = Math.max(1, e.size * k * 0.5);
      shadows.push(`inset ${d}px ${d}px ${d}px ${cssColor(e.highlight_color, e.highlight_opacity)}`, `inset ${-d}px ${-d}px ${d}px ${cssColor(e.shadow_color, e.shadow_opacity)}`);
    }
    const color = on(styles.color_overlay)[0];
    const gradient = on(styles.gradient_overlay)[0];
    if (gradient) s.background = gradientCss(gradient.gradient.gradient);
    if (color) s.background = cssColor(color.color, color.opacity);
    const stroke = on(styles.stroke)[0];
    if (stroke) {
      const w = Math.max(1, Math.round(stroke.size * k));
      s.outline = `${w}px solid ${stroke.fill?.type === "color" || !stroke.fill ? cssColor(stroke.color, stroke.opacity) : "#888"}`;
      s.outlineOffset = stroke.position === "inside" ? `${-w}px` : stroke.position === "center" ? `${-w / 2}px` : "0";
    }
  }
  s.boxShadow = shadows.join(", ");
  Object.assign(shape.style, s);
  return h("div", { class: "ls-swatch-box", style: { width: size + "px", height: size + "px" } }, shape);
}

/* ------------------------------------------- Global Light, Scale Effects */

function openGlobalLight() {
  const light = globalLight();
  const e = { angle: light.angle, use_global_light: false };
  let altitude = light.altitude;
  const dial = angleRow("Angle:", e, (p) => { e.angle = p.angle; setGlobalLight(e.angle, altitude); }, { global: false });
  const alt = slider("Altitude:", altitude, { min: 0, max: 90, unit: "°", onInput: (v) => { altitude = v; setGlobalLight(e.angle, altitude); } });
  openDialog("global-light", {
    title: "Global Light", width: 360, plain: true,
    fields: [{ type: "element", el: h("div", { class: "ls-col" }, dial, alt, h("div", { class: "dlg-note", text: "Every effect with Use Global Light follows this light." })) }],
    onOk: () => setGlobalLight(e.angle, altitude),
    onCancel: () => setGlobalLight(light.angle, light.altitude),
  });
}

function openScaleEffects() {
  const layer = activeLayerInfo();
  if (!layer || !layer.styles) { toast("The layer has no layer style"); return; }
  let percent = 100;
  openDialog("scale-effects", {
    title: "Scale Layer Effects", width: 360, plain: true,
    fields: [{ type: "element", el: slider("Scale:", 100, { min: 1, max: 1000, unit: "%", box: 56, onInput: (v) => { percent = v; } }) }],
    onOk: () => {
      if (percent !== 100) sendCommand({ op: "scale_effects", layers: [{ id: layer.id }], percent });
    },
  });
}

/* -------------------------------------------------------- canvas dragging */

/**
 * While the window is open, a drag on the image (outside the window) moves
 * what the page places: `onDelta(dx, dy)` in document pixels. The click
 * that ends the drag does not reach the backdrop.
 */
function canvasDrag(wrap, onDelta) {
  const vp = document.getElementById("viewport");
  if (!vp || !wrap) return;
  const inside = (e) => {
    const r = vp.getBoundingClientRect();
    return e.clientX >= r.left && e.clientX < r.right && e.clientY >= r.top && e.clientY < r.bottom;
  };
  const down = (e) => {
    if (!wrap.isConnected) { document.removeEventListener("mousedown", down, true); return; }
    if (e.button !== 0 || !e.target.classList || !e.target.classList.contains("modal-scrim") || !inside(e)) return;
    e.preventDefault();
    e.stopPropagation();
    const k = (window.devicePixelRatio || 1) / viewZoom();
    let last = { x: e.clientX, y: e.clientY };
    const move = (m) => {
      const dx = (m.clientX - last.x) * k, dy = (m.clientY - last.y) * k;
      last = { x: m.clientX, y: m.clientY };
      if (dx || dy) onDelta(dx, dy);
    };
    const up = () => {
      window.removeEventListener("mousemove", move, true);
      window.removeEventListener("mouseup", up, true);
    };
    window.addEventListener("mousemove", move, true);
    window.addEventListener("mouseup", up, true);
  };
  document.addEventListener("mousedown", down, true);
}

/** Whether the Layer Style window is open (the Layers panel waits for it). */
export function styleWindowOpen() {
  return !!(S && S.wrap && S.wrap.isConnected && isDialogOpen());
}
