// Fotox — the document-backed Navigator, Histogram, Properties, Character
// and Paragraph panels (they were mock-ups with invented values).
//
// The engine answers `overview:request` with a binary `overview` frame: a
// small picture of the composite, its histogram, the active layer's content
// box and, for a text layer, its formatting (engine/overview.rs). The panels
// ask again after every edit, debounced; the newest answer wins.

import { h, clear, icon } from "../el.js";
import { openColorPopover, askText } from "../dialogs.js";
import { selectButton } from "../popup.js";
import { state, on, emit } from "../state.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { activeLayerInfo, sendCommand, BLENDS, editAdjustment } from "./layers-panel.js";
import { stylePresets, saveStylePresets, styleSwatch } from "./styles.js";
import { onPrefs } from "./prefs.js";

let seq = 0;
let latest = null;        // the newest overview header
let picture = null;       // its picture, as a canvas
let view = null;          // the engine's last `view` message
let fonts = [];           // system font families (the Type tool's list)
let timer = 0;
const roots = { navigator: null, histogram: null, properties: null, character: null, paragraph: null };
let histChannel = "RGB";

const PANELS = Object.keys(roots);
const wanted = () => PANELS.filter((p) => state.openPanels[p]);

/** The view's zoom (1 = 100 %), for tools that turn screen drags into document pixels. */
export function viewZoom() {
  return view && view.zoom > 0 ? view.zoom : 1;
}

/** Ask for a fresh overview soon (edits come in bursts). */
export function requestOverview(delay = 200) {
  clearTimeout(timer);
  timer = setTimeout(() => {
    const open = wanted();
    if (!open.length) return;
    const layer = activeLayerInfo();
    bridge.send({
      type: UI.ACTION, id: "overview:request",
      args: {
        request: ++seq, size: 256, layer: layer ? layer.id : null,
        bounds_only: !open.includes("navigator") && !open.includes("histogram"),
      },
    });
  }, delay);
}

export function initOverview() {
  if (!bridge.isNative) return;
  for (const type of [ENGINE.LAYERS, ENGINE.LAYERS_PATCH, ENGINE.LAYERS_STRUCTURE_PATCH, ENGINE.HISTORY, ENGINE.ACTIVE_DOCUMENT]) {
    bridge.on(type, () => requestOverview());
  }
  bridge.on(ENGINE.ACTIVE_DOCUMENT, ({ doc }) => {
    if (doc == null) { latest = null; picture = null; renderAll(); }
  });
  bridge.on(ENGINE.FONTS, ({ families }) => { fonts = (families || []).map((f) => f.name); render("character"); });
  bridge.on(ENGINE.VIEW, (v) => { view = v; render("navigator"); });
  bridge.on(ENGINE.OVERVIEW, (msg, payload) => {
    if (msg.request < (latest ? latest.request : 0)) return;
    latest = msg;
    if (msg.width && msg.height && payload && payload.length === msg.width * msg.height * 4) {
      const cv = document.createElement("canvas");
      cv.width = msg.width;
      cv.height = msg.height;
      cv.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(payload.buffer, payload.byteOffset, payload.length), msg.width, msg.height), 0, 0);
      picture = cv;
    }
    renderAll();
  });
  on("panels", () => requestOverview(0));
}

function renderAll() {
  for (const p of PANELS) render(p);
}

function render(name) {
  const root = roots[name];
  if (!root || !root.isConnected) return;
  clear(root);
  ({ navigator: drawNavigator, histogram: drawHistogram, properties: drawProperties, character: drawCharacter, paragraph: drawParagraph })[name](root);
}

function mount(name, cls) {
  roots[name] = h("div", { class: cls });
  render(name);
  requestOverview(0);
  return roots[name];
}

const note = (text) => h("div", { class: "pnote", text });
const title = (text) => h("div", { class: "pblock-title", text });
const act = (id, args) => bridge.send({ type: UI.ACTION, id, args: args || {} });

/* -------------------------------------------------------------- Navigator */

export function navigatorPanel() {
  return mount("navigator", "pnav native");
}

function drawNavigator(root) {
  const W = 220, H = 140;
  const cv = h("canvas", { class: "nav-canvas", width: W, height: H });
  const ctx = cv.getContext("2d");
  ctx.fillStyle = "#1a1a1c";
  ctx.fillRect(0, 0, W, H);
  if (!latest || !picture) {
    root.append(cv, note(latest === null ? "Open a document." : "Reading the document…"));
    return;
  }
  const dw = latest.doc_width, dh = latest.doc_height;
  const s = Math.min(W / dw, H / dh);
  const ox = (W - dw * s) / 2, oy = (H - dh * s) / 2;
  // Checkerboard under transparent parts.
  ctx.fillStyle = "#3a3a3e";
  ctx.fillRect(ox, oy, dw * s, dh * s);
  ctx.drawImage(picture, ox, oy, dw * s, dh * s);
  if (view && view.zoom > 0) {
    const vp = document.getElementById("viewport");
    const r = vp ? vp.getBoundingClientRect() : { width: 800, height: 600 };
    const dpr = window.devicePixelRatio || 1;
    const vw = (r.width * dpr) / view.zoom, vh = (r.height * dpr) / view.zoom;
    ctx.strokeStyle = "#e5484d";
    ctx.lineWidth = 2;
    ctx.strokeRect(ox + (view.center_x - vw / 2) * s, oy + (view.center_y - vh / 2) * s, vw * s, vh * s);
  }
  const toDoc = (e) => {
    const b = cv.getBoundingClientRect();
    const x = ((e.clientX - b.left) * (W / b.width) - ox) / s;
    const y = ((e.clientY - b.top) * (H / b.height) - oy) / s;
    return { x: Math.max(0, Math.min(dw, x)), y: Math.max(0, Math.min(dh, y)) };
  };
  cv.addEventListener("mousedown", (e) => {
    e.preventDefault();
    act("view:center", toDoc(e));
    const move = (m) => act("view:center", toDoc(m));
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  const zoom = view ? view.zoom * 100 : 100;
  // A log slider: 1 % … 3200 %.
  const slider = h("input", { class: "pminirange", type: "range", min: 0, max: 1000, value: Math.round((Math.log(zoom) / Math.log(3200)) * 1000) });
  const out = h("span", { class: "pf-value", text: `${Math.round(zoom * 10) / 10}%` });
  const setZoom = (z) => view && bridge.send({ type: UI.SET_ZOOM, doc: view.doc, zoom: z / 100 });
  slider.addEventListener("input", () => {
    const z = Math.pow(3200, slider.value / 1000);
    out.textContent = `${Math.round(z * 10) / 10}%`;
    setZoom(z);
  });
  root.append(cv,
    h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "Zoom" }), slider, out),
    h("div", { class: "pbar" },
      barBtn("i-minus", "Zoom out", () => act("zoom:out")),
      barBtn("i-plus", "Zoom in", () => act("zoom:in")),
      barBtn("i-zoom-in", "Fit on screen", () => act("zoom:fit"))));
}

function barBtn(ic, tip, fn) {
  return h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
}

/* -------------------------------------------------------------- Histogram */

export function histogramPanel() {
  return mount("histogram", "phist native");
}

const CHANNELS = { RGB: [0, 1, 2], Red: [0], Green: [1], Blue: [2], Luminosity: [3] };
const TINT = ["rgba(229,72,77,.75)", "rgba(70,190,90,.75)", "rgba(80,130,240,.75)", "rgba(220,220,225,.85)"];

function drawHistogram(root) {
  const W = 256, H = 100;
  const cv = h("canvas", { class: "hist-canvas", width: W, height: H });
  const sel = selectButton(Object.keys(CHANNELS).map((c) => [c, c]), histChannel, (v) => { histChannel = v; render("histogram"); }, { style: { width: "110px" } });
  if (!latest || !latest.histogram || !latest.histogram.length) {
    root.append(cv, note(latest === null ? "Open a document." : "Reading the document…"));
    return;
  }
  const ctx = cv.getContext("2d");
  ctx.globalCompositeOperation = histChannel === "RGB" ? "lighter" : "source-over";
  const chans = CHANNELS[histChannel];
  let peak = 1;
  for (const c of chans) for (const v of latest.histogram[c]) peak = Math.max(peak, v);
  for (const c of chans) {
    ctx.fillStyle = chans.length === 1 && c !== 3 ? TINT[c].replace(".75", "1") : TINT[c];
    latest.histogram[c].forEach((v, x) => {
      const n = Math.round((v / peak) * H);
      if (n) ctx.fillRect(x, H - n, 1, n);
    });
  }
  // Statistics of the channel shown (RGB: luminosity, as Photoshop).
  const hist = latest.histogram[histChannel === "RGB" ? 3 : chans[0]];
  let count = 0, sum = 0, sq = 0;
  hist.forEach((v, i) => { count += v; sum += v * i; sq += v * i * i; });
  const mean = count ? sum / count : 0;
  const dev = count ? Math.sqrt(Math.max(0, sq / count - mean * mean)) : 0;
  let acc = 0, median = 0;
  for (let i = 0; i < 256; i++) { acc += hist[i]; if (acc >= count / 2) { median = i; break; } }
  const level = h("span", { text: "Level: —" }), cnt = h("span", { text: "Count: —" }), pct = h("span", { text: "Percentile: —" });
  cv.addEventListener("mousemove", (e) => {
    const b = cv.getBoundingClientRect();
    const i = Math.max(0, Math.min(255, Math.floor(((e.clientX - b.left) / b.width) * 256)));
    let below = 0;
    for (let k = 0; k <= i; k++) below += hist[k];
    level.textContent = `Level: ${i}`;
    cnt.textContent = `Count: ${hist[i].toLocaleString()}`;
    pct.textContent = `Percentile: ${count ? ((below / count) * 100).toFixed(1) : 0}`;
  });
  root.append(cv,
    h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: "Channel" }), sel),
    h("div", { class: "preadouts" },
      h("span", { text: `Mean: ${mean.toFixed(2)}` }), h("span", { text: `Std Dev: ${dev.toFixed(2)}` }),
      h("span", { text: `Median: ${median}` }), h("span", { text: `Pixels: ${count.toLocaleString()}` }),
      level, cnt, pct),
    note(`Sampled from a ${latest.width} × ${latest.height} preview of the composite.`),
    h("div", { class: "pbar" }, barBtn("i-sun", "Refresh", () => requestOverview(0))));
}

/* ------------------------------------------------------------- Properties */

export function propertiesPanel() {
  return mount("properties", "pprops native");
}

const QUICK = [["Remove background", "i-object-select", "ai:remove-bg"], ["Select subject", "i-marquee", "ai:subject"]];

function numField(label, value, unit, onCommit, opts = {}) {
  const input = h("input", { class: "pf-num", type: "text", value: value == null ? "" : String(value), disabled: !!opts.readonly, "data-tip": opts.tip || "" });
  const commit = () => {
    const v = Number(input.value);
    if (input.value.trim() === "" || !Number.isFinite(v) || String(v) === String(value)) return;
    onCommit(v);
  };
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") { commit(); input.blur(); } if (e.key === "Escape") { input.value = value; input.blur(); } });
  input.addEventListener("change", commit);
  scrubby(input, (v) => onCommit(v), opts);
  return h("div", { class: "pf-row" }, h("span", { class: "pf-label scrub", text: label }), h("span", { class: "pf-fieldwrap" }, input, h("span", { class: "pf-unit", text: unit || "" })));
}

/** Drag on a number field's label to change it (Photoshop's scrubby slider). */
function scrubby(input, onCommit, opts) {
  if (opts.readonly) return;
  queueMicrotask(() => {
    const label = input.closest(".pf-row")?.querySelector(".pf-label");
    if (!label) return;
    label.style.cursor = "ew-resize";
    label.addEventListener("mousedown", (e) => {
      e.preventDefault();
      const start = Number(input.value) || 0, x0 = e.clientX;
      let last = start;
      const move = (m) => {
        const step = m.shiftKey ? 10 : m.altKey ? 0.1 : 1;
        let v = start + Math.round((m.clientX - x0) / 2) * step;
        if (opts.min != null) v = Math.max(opts.min, v);
        if (opts.max != null) v = Math.min(opts.max, v);
        v = Math.round(v * 10) / 10;
        if (v === last) return;
        last = v;
        input.value = v;
        if (opts.live) onCommit(v);
      };
      const up = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
        if (!opts.live && last !== start) onCommit(last);
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    });
  });
}

const KIND_NAMES = { pixel: "Pixel layer", group: "Group", adjustment: "Adjustment layer", solid_fill: "Solid color fill", fill: "Fill layer", shape: "Shape layer", text: "Type layer", smart: "Smart Object" };

function drawProperties(root) {
  const l = activeLayerInfo();
  if (!l) {
    root.append(note("Select a layer to see its properties."));
    return;
  }
  const b = latest && latest.layer === l.id ? latest.bounds : null;
  const set = (props) => sendCommand({ op: "set_layer_props", layer: { id: l.id }, props });
  const move = (dx, dy) => (dx || dy) && sendCommand({ op: "move_each", moves: [[{ id: l.id }, Math.round(dx), Math.round(dy)]], label: "Move" });
  root.append(h("div", { class: "pblock-title", text: `${KIND_NAMES[l.kind] || l.kind} — ${l.name}` }));
  if (l.kind !== "adjustment") {
    root.append(title("Transform"));
    if (b) {
      const [x0, y0, x1, y1] = b;
      const locked = l.locked_position || l.locked_by_group;
      root.append(
        h("div", { class: "pf-grid2" },
          numField("X", x0, "px", (v) => move(v - x0, 0), { readonly: locked, tip: locked ? "Position is locked" : "" }),
          numField("Y", y0, "px", (v) => move(0, v - y0), { readonly: locked, tip: locked ? "Position is locked" : "" })),
        h("div", { class: "pf-grid2" },
          numField("W", x1 - x0, "px", () => {}, { readonly: true, tip: "Resize with Edit ▸ Free Transform (Ctrl+T)" }),
          numField("H", y1 - y0, "px", () => {}, { readonly: true, tip: "Resize with Edit ▸ Free Transform (Ctrl+T)" })),
        h("div", { class: "pf-actions" },
          h("button", { class: "pf-action", type: "button", onclick: () => emit("action", "xf:free") }, icon("i-move", "ic sm"), h("span", { text: "Free Transform" }))));
    } else {
      root.append(note(latest && latest.layer === l.id ? "The layer is empty." : "Measuring…"));
    }
  }
  root.append(title("Blend"));
  const blend = selectButton(BLENDS.filter(([id]) => id !== "pass_through" || l.kind === "group").map(([id, name]) => [id, name]), l.blend,
    (v) => set({ blend: v }), { style: { width: "140px" } });
  root.append(h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: "Mode" }), blend),
    numField("Opacity", Math.round(l.opacity * 100), "%", (v) => set({ opacity: Math.min(100, Math.max(0, v)) / 100 }), { min: 0, max: 100 }),
    numField("Fill", Math.round(l.fill * 100), "%", (v) => set({ fill: Math.min(100, Math.max(0, v)) / 100 }), { min: 0, max: 100 }));
  if (l.adjustment) {
    root.append(title("Adjustment"), h("div", { class: "pf-actions" },
      h("button", { class: "pf-action", type: "button", onclick: () => editAdjustment(l) }, icon("i-adjust", "ic sm"), h("span", { text: "Edit settings…" }))));
  }
  root.append(title("Masks"));
  const maskBtns = [];
  if (l.has_mask) {
    maskBtns.push(
      ["Apply mask", "i-check", () => sendCommand({ op: "delete_mask", layer: { id: l.id }, apply: true })],
      ["Delete mask", "i-trash", () => sendCommand({ op: "delete_mask", layer: { id: l.id }, apply: false })]);
  } else {
    maskBtns.push(["Add layer mask", "i-plus", () => sendCommand({ op: "add_mask", layer: { id: l.id }, fill: "reveal_all" })]);
  }
  root.append(note(l.has_mask ? `Layer mask${l.edit_mask ? " (painting goes to the mask)" : ""}` : "No layer mask"),
    l.vector_mask != null ? note(`Vector mask${l.vector_mask ? "" : " (disabled)"}`) : null,
    h("div", { class: "pf-actions" }, ...maskBtns.map(([t, ic, fn]) => h("button", { class: "pf-action", type: "button", onclick: fn }, icon(ic, "ic sm"), h("span", { text: t })))));
  if (latest && latest.text && latest.layer === l.id) {
    const t = latest.text;
    root.append(title("Type"), note(`${t.family} ${t.style.replace("_", " ")} · ${Math.round(t.size_pt * 10) / 10} pt · ${t.align}${t.runs > 1 ? " · mixed formatting" : ""}`),
      h("div", { class: "pf-actions" },
        h("button", { class: "pf-action", type: "button", onclick: () => { if (!state.openPanels.character) emit("action", "panel:character"); } }, icon("i-type", "ic sm"), h("span", { text: "Character panel" }))));
  }
  root.append(title("Quick Actions"), h("div", { class: "pf-actions" },
    ...QUICK.map(([label, ic, id]) => h("button", { class: "pf-action", type: "button", onclick: () => emit("action", id) }, icon(ic, "ic sm"), h("span", { text: label })))));
}

/* ------------------------------------------------------ Character, Paragraph */

export function characterPanel() {
  return mount("character", "ptext native");
}

export function paragraphPanel() {
  return mount("paragraph", "ptext native");
}

const hex2 = (v) => Math.round(v / 257).toString(16).padStart(2, "0");
const toHex = (c) => `#${hex2(c[0])}${hex2(c[1])}${hex2(c[2])}`;
const fromHex = (s) => [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16) * 257).concat([65535]);
const STYLES = [["regular", "Regular"], ["italic", "Italic"], ["bold", "Bold"], ["bold_italic", "Bold Italic"]];

function textOf() {
  const l = activeLayerInfo();
  if (!l || l.kind !== "text") return { l, t: null };
  return { l, t: latest && latest.layer === l.id ? latest.text : null };
}

const style = (args) => act("text:style", args);

function drawCharacter(root) {
  const { l, t } = textOf();
  if (!l || l.kind !== "text") {
    root.append(note("Select a type layer to edit its characters. New text uses the Type tool's option bar."));
    return;
  }
  if (!t) { root.append(note("Reading the text…")); return; }
  const families = fonts.length ? [...new Set([t.family, ...fonts])] : [t.family];
  const family = selectButton(families.map((f) => [f, f]), t.family, (v) => style({ family: v }), { style: { width: "100%" } });
  const st = selectButton(STYLES.map(([v, n]) => [v, n]), t.style, (v) => style({ style: v }), { style: { width: "110px" } });
  // The app's colour popover (off-screen CEF draws no native colour chooser).
  const color = h("button", { class: "dlg-color", type: "button", "data-tip": "Text colour", style: { background: toHex(t.color) } });
  color.addEventListener("click", (e) => {
    e.stopPropagation();
    openColorPopover(color, toHex(t.color), (hex) => { color.style.background = hex; style({ color: fromHex(hex) }); });
  });
  root.append(
    h("div", { class: "phead-row" }, family),
    h("div", { class: "phead-row" }, st, color),
    numField("Size", Math.round(t.size_pt * 10) / 10, "pt", (v) => style({ size_pt: Math.max(0.1, v) }), { min: 0.1, max: 1296 }),
    numField("Tracking", Math.round(t.tracking * 10) / 10, "px", (v) => style({ tracking: v })),
    numField("Leading", t.leading == null ? "" : Math.round(t.leading * 10) / 10, "px", (v) => style({ leading: v > 0 ? v : null }), { min: 0, tip: "Empty = Auto" }),
    t.runs > 1 ? note("The text has mixed formatting: a change here applies to all of it.") : null);
  if (!fonts.length) root.append(note("Pick the Type tool once to load the system font list."));
}

function drawParagraph(root) {
  const { l, t } = textOf();
  if (!l || l.kind !== "text") {
    root.append(note("Select a type layer to set its alignment."));
    return;
  }
  if (!t) { root.append(note("Reading the text…")); return; }
  const aligns = [["left", "i-quote", "Left align text"], ["center", "i-props", "Center text"], ["right", "i-quote", "Right align text"], ["justify", "i-props", "Justify"]];
  root.append(h("div", { class: "pf-actions" }, ...aligns.map(([v, ic, tip]) =>
    h("button", { class: "pf-action" + (t.align === v ? " on" : ""), type: "button", "data-tip": tip, onclick: () => style({ align: v }) }, icon(ic, "ic sm"), h("span", { text: v[0].toUpperCase() + v.slice(1) })))));
}

/* ---------------------------------------------------------- honest mock-ups */

/** A panel whose feature is not built yet: say so, with no fake controls. */
export function plannedPanel(name, what) {
  return h("div", { class: "pplanned" }, note(`${name} is not available yet.`), what ? note(what) : null);
}

/* ------------------------------------------------------------------ Styles */

// Style presets: a name and a whole `LayerStyles`, kept in the preferences
// file (the Layer Style window's Styles page shows the same list). Click
// applies to the active layer (one history step); + saves the active layer's
// style; the bin deletes the selected preset. Presets an older build kept in
// this browser profile are moved to the preferences once.
const OLD_STYLE_KEY = "fotox.style-presets";
let styleSel = -1;
let stylesRoot = null;
let watching = false;

function migrateStyles() {
  try {
    const old = JSON.parse(localStorage.getItem(OLD_STYLE_KEY) || "[]");
    if (Array.isArray(old) && old.length && !stylePresets().length) {
      saveStylePresets(old.filter((p) => p && typeof p.name === "string" && p.styles));
    }
    localStorage.removeItem(OLD_STYLE_KEY);
  } catch { /* nothing to move */ }
}

export function stylesPanel() {
  stylesRoot = h("div", { class: "pstyles native" });
  if (!watching) {
    watching = true;
    onPrefs(() => drawStyles());
  }
  migrateStyles();
  drawStyles();
  return stylesRoot;
}

function drawStyles() {
  if (!stylesRoot) return;
  clear(stylesRoot);
  const list = stylePresets();
  const layer = activeLayerInfo();
  const apply = (p) => {
    if (!layer) return;
    sendCommand({ op: "set_layer_style", layer: { id: layer.id }, styles: JSON.parse(JSON.stringify(p.styles)) });
  };
  const rows = h("div", { class: "style-list" });
  list.forEach((p, i) => {
    rows.append(h("div", {
      class: "style-row" + (i === styleSel ? " sel" : ""), "data-tip": `${p.name} — click to apply to the active layer`,
      onclick: () => { styleSel = i; apply(p); drawStyles(); },
    }, styleSwatch(p.styles, 26), h("span", { class: "plist-label", text: p.name })));
  });
  if (!list.length) rows.append(note("No saved styles. Give a layer a style (Layer ▸ Layer Style), then press + to keep it here."));
  stylesRoot.append(rows, h("div", { class: "pbar" },
    barBtn("i-trash", "Clear the active layer's style", () => layer && sendCommand({ op: "set_layer_style", layer: { id: layer.id }, styles: null })),
    barBtn("i-plus", "New style from the active layer", async () => {
      const l = activeLayerInfo();
      if (!l || !l.styles) { emit("mock", "The active layer has no style to save"); return; }
      const name = await askText("New Style", "Name:", `${l.name} style`);
      if (!name) return;
      const next = [...stylePresets(), { name, styles: l.styles }];
      styleSel = next.length - 1;
      saveStylePresets(next);
    }),
    barBtn("i-minus", "Delete the selected style preset", () => {
      const next = stylePresets();
      if (styleSel < 0 || styleSel >= next.length) return;
      next.splice(styleSel, 1);
      styleSel = -1;
      saveStylePresets(next);
    })));
}
