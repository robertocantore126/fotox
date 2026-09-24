// Fotox — motore delle finestre di dialogo: le definizioni arrivano da
// js/data/dialogs.js, qui c'è il rendering e l'interazione.

import { h, icon, clear } from "./el.js";
import { dialogDef } from "./data/dialogs.js";
import { openDropdown, popupLayer } from "./popup.js";
import { state, emit } from "./state.js";
import { getDocCanvas } from "./canvas.js";

let stack = [];

export function openDialog(id, overrides = {}) {
  const def = { ...dialogDef(id), ...overrides };
  const body = h("div", { class: "dlg-body" });
  const cols = (def.fields || []).some((f) => f.type === "col");
  const grid = h("div", { class: "dlg-fields" + (cols ? " cols" : "") + (def.wide ? " wide" : "") });
  for (const field of def.fields || []) grid.append(renderField(field));
  body.append(grid);

  const titleBar = h("div", { class: "dlg-title" },
    def.icon ? icon(def.icon, "ic sm") : null,
    h("span", { class: "dlg-name", text: def.title }),
    h("button", { class: "dlg-x", type: "button", "data-tip": "Close", onclick: () => close() }, icon("i-close", "ic sm")));

  const footer = h("div", { class: "dlg-footer" });
  const cancel = def.cancel === null ? null : h("button", { class: "btn", type: "button", text: def.cancel || "Cancel", onclick: () => close() });
  const ok = def.ok === null ? null : h("button", { class: "btn primary", type: "button", text: def.ok || "OK", onclick: () => { emit("mock", def.title); close(); } });
  if (cancel) footer.append(cancel);
  if (ok) footer.append(ok);

  const dlg = h("div", { class: "dialog" + (def.plain ? " plain" : ""), style: { width: (def.width || 420) + "px" } }, titleBar, body, footer);
  const wrap = h("div", { class: "modal-wrap" },
    h("div", { class: "modal-scrim", onclick: () => close() }),
    dlg);
  popupLayer().append(wrap);
  stack.push({ wrap, id });

  const rect = dlg.getBoundingClientRect();
  dlg.style.marginTop = Math.max(10, (window.innerHeight - rect.height) / 2 - 30) + "px";
  dragify(dlg, titleBar);

  wrap._fotoxClose = close;
  requestAnimationFrame(() => dlg.classList.add("in"));
  return wrap;
}

export function closeTopDialog() {
  const top = stack[stack.length - 1];
  if (top) close(top);
  return !!top;
}

function close(entry) {
  const idx = entry ? stack.findIndex((s) => s === entry) : stack.length - 1;
  if (idx < 0) return;
  const [removed] = stack.splice(idx, 1);
  removed.wrap.classList.remove("in");
  setTimeout(() => removed.wrap.remove(), 120);
}

export function closeAllDialogs() {
  while (stack.length) close(stack[stack.length - 1]);
}

export function isDialogOpen() {
  return stack.length > 0;
}

function dragify(dlg, handle) {
  handle.addEventListener("mousedown", (e) => {
    if (e.target.closest("button")) return;
    const start = { x: e.clientX, y: e.clientY, left: dlg.offsetLeft, top: dlg.offsetTop };
    const move = (ev) => {
      dlg.style.marginLeft = "0px";
      dlg.style.left = start.left + (ev.clientX - start.x) + "px";
      dlg.style.top = start.top + (ev.clientY - start.y) + "px";
      dlg.style.position = "absolute";
    };
    const up = () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
    e.preventDefault();
  });
}

/* ---------------------------------------------------------------- campi */

function renderField(f) {
  if (!f) return h("span");
  switch (f.type) {
    case "sep": return h("div", { class: "dlg-sep" });
    case "label": return h("div", { class: "dlg-label", text: f.text });
    case "row": return h("div", { class: "dlg-row" }, ...(f.fields || []).map(renderField));
    case "col": return h("div", { class: "dlg-col" }, ...(f.fields || []).map(renderField));
    case "group": return h("div", { class: "dlg-group" }, f.label ? h("div", { class: "dlg-group-title", text: f.label }) : null, ...(f.fields || []).map(renderField));
    case "num": return numField(f);
    case "text": return textField(f);
    case "textarea": return h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: f.label }), h("textarea", { class: "dlg-input area", rows: 4, placeholder: f.label }));
    case "select": return selectField(f);
    case "check": return checkField(f);
    case "radio": return radioField(f);
    case "range": return rangeField(f);
    case "color": return colorField(f);
    case "btn": return h("button", { class: "btn small", type: "button", text: f.text, onclick: () => emit("mock", f.text) });
    case "locksize": return h("button", { class: "dlg-lock", type: "button", "data-tip": "Constrain proportions", onclick: (e) => e.currentTarget.classList.toggle("on") }, icon("i-link", "ic sm"));
    case "chain": return h("span", { class: "dlg-chain", text: f.text });
    case "pixels": return h("div", { class: "dlg-pixels" }, h("span", { class: "px-box" }), h("span", { class: "dlg-note", text: "Pixel Dimensions: 1080 × 1080 · 8.4 MB" }));
    case "preview": return previewField();
    case "presets": return presetList(f);
    case "filelist": return fileList(f);
    case "readonly": return readOnly(f);
    case "list": return h("div", { class: "dlg-list" }, ...(f.rows || []).map((r) => h("div", { class: "dlg-list-row", text: r })));
    case "searchbox": return h("input", { class: "dlg-input", placeholder: f.placeholder || "Search", style: { width: "100%" } });
    case "matrix": return matrixField();
    case "histo": return histoField();
    case "curve": return curveField();
    case "colorpicker": return colorPickerField();
    case "gradientbar": return gradientBar();
    case "patternpick": return patternPick();
    case "anchor": return anchorField();
    case "stylelist": return styleList(f);
    case "blend": return h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: "Blend Mode:" }), selectControl(["Normal", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light"], f.mode || "Normal"));
    case "balancebar": return balanceBar(f);
    case "glowtype": return h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: "Technique:" }), radioInline(["Softer", "Precise"], 0), h("span", { class: "dlg-field-label", text: "Source:" }), radioInline(["Edge", "Center"], f.value === "Center" ? 1 : 0));
    case "stroke": return h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: "Fill Type:" }), selectControl(["Colour", "Gradient", "Pattern"], f.value));
    case "channelsrow": return h("div", { class: "dlg-channels" }, ...["R", "G", "B"].map((c) => h("span", { class: "ch-box", text: c })));
    case "blendif": return blendIfField(f);
    case "shortcutgroups": return shortcutGroups();
    case "prefsnav": return h("div", { class: "prefsnav" }, ...(f.items || []).map((t, i) => h("div", { class: "prefsnav-item" + (i === 0 ? " active" : ""), text: t })));
    case "about": return aboutField();
    case "cmykrow": return h("div", { class: "dlg-row" }, ...["Channel 1", "Channel 2", "Channel 3", "Channel 4"].map((c) => numField({ label: c + ":", value: 45, width: 52 })));
    case "pipette": return h("button", { class: "btn small", type: "button", text: f.text, onclick: () => emit("mock", f.text) });
    case "filterlist": return h("div", { class: "dlg-list tall" }, ...(f.items || []).map((t, i) => h("div", { class: "dlg-list-row" + (i === 0 ? " sel" : ""), text: t })));
    case "flarepos": return h("div", { class: "flare-box" }, icon("i-sun", "ic"), h("span", { text: "Drag the crosshair to place the flare centre" }));
    case "focus": return h("div", { class: "focus-box" }, h("div", { class: "focus-in" }), h("span", { text: "In-focus ranges" }));
    case "shearcurve": return shearCurve();
    case "liquify": return liquifyField();
    case "vanishing": return h("div", { class: "liquify-box" }, icon("i-grid", "ic"), h("span", { text: "Drag to create a perspective plane, then press OK" }));
    case "gallery": return galleryField();
    case "openoptions": return h("div", { class: "dlg-note", text: "The Open dialog shows this document's metadata (EXIF, ICC profile and so on)." });
    case "brushgrid": return h("div", { class: "dlg-list" }, h("div", { class: "dlg-list-row", text: "Soft Round 22" }));
    default: return h("div", { class: "dlg-label", text: f.text || f.type });
  }
}

function numField(f) {
  const input = h("input", { class: "dlg-input num", type: "text", value: f.value, style: { width: (f.w || 56) + "px" }, disabled: f.disabled || false });
  return h("span", { class: "dlg-line inline" }, f.label ? h("span", { class: "dlg-field-label", text: f.label }) : null, input, f.unit ? h("span", { class: "dlg-unit", text: f.unit }) : null);
}

function textField(f) {
  return h("div", { class: "dlg-line" }, f.label ? h("span", { class: "dlg-field-label", text: f.label }) : null,
    h("input", { class: "dlg-input", type: f.password ? "password" : "text", value: f.value, style: { width: (f.width || 200) + "px" } }));
}

function selectField(f) {
  const box = selectControl(f.options || [], f.value);
  box.style.minWidth = (f.width || 140) + "px";
  return h("div", { class: "dlg-line" }, f.label ? h("span", { class: "dlg-field-label", text: f.label }) : null, box);
}

function selectControl(options, value) {
  const val = h("span", { class: "pf-value", text: value ?? options[0] });
  const btn = h("button", {
    class: "dlg-select", type: "button",
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({ anchor: btn, items: options, value: val.textContent, width: Math.max(150, btn.offsetWidth), onPick: (v) => { val.textContent = v; emit("mock", v); } });
    },
  }, val, icon("i-chevron-down", "ic xs"));
  return btn;
}

function checkField(f) {
  const box = h("span", { class: "dlg-check" + (f.on ? " on" : ""), onclick: (e) => { e.currentTarget.classList.toggle("on"); } });
  const upd = () => { clear(box); if (box.classList.contains("on")) box.append(icon("i-check", "ic xs")); };
  upd();
  box.addEventListener("click", upd);
  return h("label", { class: "dlg-checkline" }, box, h("span", { text: f.label }));
}

function radioField(f) {
  const group = h("div", { class: "dlg-radio" + (f.inline ? " inline" : "") });
  const name = "r" + Math.random().toString(36).slice(2, 8);
  (f.options || []).forEach((o, i) => {
    const dot = h("input", { type: f.multi ? "checkbox" : "radio", name, checked: i === f.value });
    group.append(h("label", { class: "dlg-checkline" }, dot, h("span", { text: o })));
  });
  return h("div", { class: "dlg-radiowrap" }, f.label ? h("span", { class: "dlg-field-label", text: f.label }) : null, group);
}

function radioInline(options, active) {
  return h("span", { class: "dlg-inline-radio" }, ...options.map((o, i) => h("button", { class: "btn tiny" + (i === active ? " on" : ""), type: "button", text: o, onclick: (e) => { [...e.currentTarget.parentElement.children].forEach((c) => c.classList.remove("on")); e.currentTarget.classList.add("on"); } })));
}

function rangeField(f) {
  const min = f.min ?? 0;
  const max = f.max ?? 100;
  const out = h("input", { class: "dlg-input num", type: "text", value: f.value, style: { width: "46px" } });
  const input = h("input", { class: "dlg-range", type: "range", min, max, value: f.value });
  input.addEventListener("input", () => { out.value = input.value; });
  return h("div", { class: "dlg-line" }, f.label ? h("span", { class: "dlg-field-label", text: f.label }) : null, input, out);
}

function colorField(f) {
  const chip = h("button", { class: "dlg-color", type: "button", style: { background: f.value }, onclick: () => emit("ask-dialog", "color-picker") });
  return h("div", { class: "dlg-line" }, f.label ? h("span", { class: "dlg-field-label", text: f.label }) : null, chip,
    h("input", { class: "dlg-input", type: "text", value: f.value, style: { width: "90px" } }));
}

function previewField() {
  const cv = h("canvas", { class: "dlg-preview", width: 220, height: 150 });
  const ctx = cv.getContext("2d");
  const src = getDocCanvas();
  ctx.fillStyle = "#1b1b1e";
  ctx.fillRect(0, 0, 220, 150);
  if (src) ctx.drawImage(src, 0, 0, 220, 150);
  ctx.strokeStyle = "rgba(255,255,255,.16)";
  ctx.strokeRect(0.5, 0.5, 219, 149);
  return h("div", { class: "dlg-previewwrap" }, cv, h("span", { class: "dlg-note", text: "Preview — the mock shows the document thumbnail" }));
}

function presetList(f) {
  const list = h("div", { class: "preset-list" });
  (f.items || []).forEach(([name, size], i) => {
    list.append(h("div", { class: "preset-row" + (i === 1 ? " sel" : "") },
      h("span", { class: "preset-name", text: name }),
      h("span", { class: "preset-size", text: size })));
  });
  return list;
}

function fileList(f) {
  const list = h("div", { class: "file-list" });
  (f.items || []).forEach((n, i) => {
    list.append(h("div", { class: "file-row" + (i === 0 ? " sel" : "") }, icon("i-image", "ic sm"), h("span", { text: n })));
  });
  return list;
}

function readOnly(f) {
  const box = h("div", { class: "ro-list" });
  // `items` può essere una funzione: i valori che dipendono dall'ambiente
  // (user agent, viewport) vanno letti all'apertura, non al caricamento dei dati.
  const items = typeof f.items === "function" ? f.items() : (f.items || []);
  for (const [k, v] of items) box.append(h("div", { class: "ro-row" }, h("span", { class: "ro-k", text: k }), h("span", { class: "ro-v", text: v })));
  return box;
}

function matrixField() {
  const grid = h("div", { class: "matrix" });
  for (let i = 0; i < 25; i++) {
    grid.append(h("input", { class: "dlg-input num tiny", type: "text", value: i === 12 ? "1" : "0" }));
  }
  return grid;
}

function histoField() {
  const cv = h("canvas", { class: "hist-canvas big", width: 300, height: 110 });
  const ctx = cv.getContext("2d");
  ctx.fillStyle = "#2a2d33";
  for (let x = 0; x < 300; x++) {
    const n = Math.max(2, 100 * Math.exp(-Math.pow((x - 150) / 55, 2)) + 12 * Math.random());
    ctx.fillRect(x, 110 - n, 1, n);
  }
  ctx.strokeStyle = "rgba(255,255,255,.15)";
  [75, 150, 225].forEach((x) => { ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, 110); ctx.stroke(); });
  ctx.fillStyle = "rgba(255,255,255,.5)";
  ctx.fillRect(60, 0, 2, 110);
  ctx.fillRect(230, 0, 2, 110);
  return cv;
}

function curveField() {
  const cv = h("canvas", { class: "curve-canvas", width: 300, height: 220 });
  const ctx = cv.getContext("2d");
  const points = [[0, 220], [100, 140], [200, 80], [300, 0]];
  const draw = () => {
    ctx.clearRect(0, 0, 300, 220);
    ctx.fillStyle = "#23262c";
    ctx.fillRect(0, 0, 300, 220);
    ctx.strokeStyle = "rgba(255,255,255,.12)";
    for (let i = 1; i < 4; i++) {
      ctx.beginPath(); ctx.moveTo(i * 75, 0); ctx.lineTo(i * 75, 220); ctx.stroke();
      ctx.beginPath(); ctx.moveTo(0, i * 55); ctx.lineTo(300, i * 55); ctx.stroke();
    }
    ctx.strokeStyle = "#e8eaf0";
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    ctx.moveTo(points[0][0], points[0][1]);
    for (let i = 1; i < points.length; i++) {
      const [px, py] = points[i - 1];
      const [x, y] = points[i];
      ctx.bezierCurveTo(px + (x - px) / 2, py, px + (x - px) / 2, y, x, y);
    }
    ctx.stroke();
    ctx.fillStyle = "#7cc4ff";
    points.forEach(([x, y]) => ctx.fillRect(x - 2, y - 2, 4, 4));
  };
  cv.addEventListener("mousedown", (e) => {
    const r = cv.getBoundingClientRect();
    points.push([e.clientX - r.left, e.clientY - r.top]);
    points.sort((a, b) => a[0] - b[0]);
    draw();
  });
  draw();
  return cv;
}

function colorPickerField() {
  const wrap = h("div", { class: "colorpicker" });
  const area = h("div", { class: "cp-area", onclick: moveMarker }, h("div", { class: "cp-marker", style: { left: "70%", top: "30%" } }));
  const hue = h("div", { class: "cp-hue", onclick: moveMarker }, h("div", { class: "cp-hue-marker", style: { left: "18%" } }));
  function moveMarker(e) {
    const r = e.currentTarget.getBoundingClientRect();
    const marker = e.currentTarget.querySelector("[class$=marker]");
    if (!marker) return;
    const x = ((e.clientX - r.left) / r.width) * 100;
    const y = ((e.clientY - r.top) / r.height) * 100;
    if (marker.classList.contains("cp-marker")) { marker.style.left = x + "%"; marker.style.top = y + "%"; }
    else marker.style.left = x + "%";
  }
  wrap.append(area, hue,
    h("div", { class: "cp-fields" },
      ...[["R", "30"], ["G", "30"], ["B", "34"], ["H", "230"], ["S", "12"], ["B", "13"]].map(([k, v]) => h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: k }), h("input", { class: "dlg-input num", type: "text", value: v }))),
      h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "#" }), h("input", { class: "dlg-input", type: "text", value: "1E1E22" }))),
    h("div", { class: "cp-swatches" }, ...["#1e1e22", "#ffffff", "#e26060", "#7ac74f", "#5b8df5", "#f5d442", "#8b5cf6", "#3fb6a8"].map((c) => h("span", { class: "swatch", style: { background: c } }))));
  return wrap;
}

function gradientBar() {
  const stops = h("div", { class: "gradient-stops" });
  const barEl = h("div", { class: "gradient-edit" },
    h("span", { class: "g-stop", style: { left: "0%", background: "#0b0b0d" } }),
    h("span", { class: "g-stop mid", style: { left: "50%", background: "#7b7f8a" } }),
    h("span", { class: "g-stop", style: { left: "98%", background: "#ffffff" } }));
  stops.append(barEl, h("div", { class: "g-midpoint", style: { left: "50%" } }));
  return h("div", { class: "gradient-field" }, stops, h("div", { class: "g-actions" },
    h("button", { class: "btn small", type: "button", text: "New stop", onclick: () => emit("mock", "New gradient stop") }),
    h("button", { class: "btn small", type: "button", text: "Delete stop", onclick: () => emit("mock", "Delete gradient stop") }),
    h("span", { class: "dlg-note", text: "Preset: Black, White" })));
}

function patternPick() {
  const grid = h("div", { class: "pattern-grid" });
  for (let i = 0; i < 12; i++) grid.append(h("div", { class: "pattern-tile" + (i === 3 ? " sel" : "") }));
  return h("div", {}, grid, h("div", { class: "dlg-note", text: "Patterns are generated procedurally in this mock." }));
}

function anchorField() {
  const grid = h("div", { class: "anchor-grid" });
  for (let i = 0; i < 9; i++) {
    grid.append(h("button", {
      class: "anchor-cell" + (i === 4 ? " sel" : ""), type: "button",
      onclick: (e) => { [...grid.children].forEach((c) => c.classList.remove("sel")); e.currentTarget.classList.add("sel"); },
    }));
  }
  return h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: "Anchor:" }), grid);
}

function styleList(f) {
  const list = h("div", { class: "style-picker" });
  for (const item of f.items || []) {
    list.append(h("div", {
      class: "style-picker-row" + (item === f.active || (item === "Blending Options: Default" && !f.active) ? " sel" : ""),
      onclick: (e) => { [...list.children].forEach((c) => c.classList.remove("sel")); e.currentTarget.classList.add("sel"); },
    }, item === "Styles" || item === "Blending Options: Default" ? null : h("span", { class: "style-picker-check" }, icon("i-check", "ic xs")), h("span", { text: item })));
  }
  return list;
}

function blendIfField(f) {
  const mk = (label) => h("div", { class: "blendif-row" },
    h("span", { class: "blendif-label", text: label }),
    h("div", { class: "blendif-bar" },
      h("span", { class: "blendif-handle", style: { left: "10%" } }),
      h("span", { class: "blendif-handle", style: { left: "90%" } })));
  return h("div", { class: "blendif" }, mk(f.left), mk(f.right));
}

function balanceBar(f) {
  return h("div", { class: "balance-row" },
    h("span", { class: "balance-side", text: f.left }),
    h("input", { class: "dlg-range", type: "range", min: -100, max: 100, value: 0, style: { width: "220px" } }),
    h("span", { class: "balance-side", text: f.right }));
}

function shortcutGroups() {
  const groups = [
    ["File", [["New...", "Ctrl+N"], ["Open...", "Ctrl+O"], ["Save", "Ctrl+S"], ["Export As...", "Alt+Shift+Ctrl+W"], ["Close", "Ctrl+W"], ["Print...", "Ctrl+P"]]],
    ["Edit", [["Undo", "Ctrl+Z"], ["Redo", "Ctrl+Shift+Z"], ["Copy", "Ctrl+C"], ["Paste", "Ctrl+V"], ["Free Transform", "Ctrl+T"], ["Preferences", "Ctrl+K"]]],
    ["Image", [["Levels...", "Ctrl+L"], ["Curves...", "Ctrl+M"], ["Hue/Saturation...", "Ctrl+U"], ["Colour Balance...", "Ctrl+B"], ["Image Size...", "Alt+Ctrl+I"], ["Canvas Size...", "Alt+Ctrl+C"]]],
    ["Layer", [["New Layer...", "Shift+Ctrl+N"], ["Layer via Copy", "Ctrl+J"], ["Group Layers", "Ctrl+G"], ["Merge Layers", "Ctrl+E"], ["Bring Forward", "Ctrl+]"], ["Send Backward", "Ctrl+["]]],
    ["Select", [["All", "Ctrl+A"], ["Deselect", "Ctrl+D"], ["Reselect", "Shift+Ctrl+D"], ["Inverse", "Shift+Ctrl+I"]]],
    ["View", [["Zoom In", "Ctrl++"], ["Zoom Out", "Ctrl+-"], ["Fit on Screen", "Ctrl+0"], ["Actual Pixels", "Ctrl+1"], ["Rulers", "Ctrl+R"], ["Grid", "Ctrl+'"]]],
    ["Tools", [["Move", "V"], ["Brush", "B"], ["Eraser", "E"], ["Gradient", "G"], ["Pen", "P"], ["Type", "T"], ["Hand", "H"], ["Zoom", "Z"]]],
  ];
  const wrap = h("div", { class: "shortcuts" });
  for (const [group, rows] of groups) {
    const box = h("div", { class: "sc-group" }, h("div", { class: "sc-title", text: group }));
    for (const [label, key] of rows) {
      box.append(h("div", { class: "sc-row" }, h("span", { class: "sc-label", text: label }), h("span", { class: "sc-key", text: key })));
    }
    wrap.append(box);
  }
  return wrap;
}

function aboutField() {
  return h("div", { class: "about-box" },
    h("div", { class: "about-logo" }, icon("i-fx", "ic big")),
    h("div", { class: "about-title", text: "Fotox" }),
    h("div", { class: "about-ver", text: "Interface mock · build 1.0.0" }),
    h("p", { class: "about-text", text: "Fotox is an original, from-scratch reimplementation of the layout and behaviour of a Photoshop-style web editor. Every panel, menu and dialog you see here is our own artwork and code: no assets, icons, stylesheets or branding were taken from any other product." }),
    h("p", { class: "about-text dim", text: "This build is a navigable shell: the interface responds exactly like the real thing, but the image operations are not implemented." }),
    h("div", { class: "about-credits" },
      h("span", { text: "Canvas rendering: native 2D" }),
      h("span", { text: "No external dependencies" }),
      h("span", { text: `${state.doc.w} × ${state.doc.h} px document` })));
}

function shearCurve() {
  const cv = h("canvas", { class: "shear-canvas", width: 240, height: 140 });
  const ctx = cv.getContext("2d");
  ctx.fillStyle = "#23262c";
  ctx.fillRect(0, 0, 240, 140);
  ctx.strokeStyle = "rgba(255,255,255,.12)";
  ctx.beginPath(); ctx.moveTo(120, 0); ctx.lineTo(120, 140); ctx.stroke();
  ctx.strokeStyle = "#e8eaf0";
  ctx.beginPath(); ctx.moveTo(120, 0); ctx.lineTo(120, 140); ctx.stroke();
  ctx.fillStyle = "#7cc4ff";
  ctx.fillRect(118, 4, 4, 4);
  ctx.fillRect(118, 132, 4, 4);
  return cv;
}

function liquifyField() {
  const cv = h("canvas", { class: "liquify-canvas", width: 360, height: 240 });
  const ctx = cv.getContext("2d");
  const src = getDocCanvas();
  ctx.fillStyle = "#1b1b1e";
  ctx.fillRect(0, 0, 360, 240);
  if (src) ctx.drawImage(src, 0, 0, 360, 240);
  ctx.strokeStyle = "rgba(255,255,255,.25)";
  ctx.strokeRect(40, 30, 120, 120);
  return cv;
}

function galleryField() {
  const wrap = h("div", { class: "gallery" });
  const filters = ["Artistic", "Brush Strokes", "Distort", "Sketch", "Stylize", "Texture"];
  const left = h("div", { class: "gallery-col" }, ...filters.map((f, i) => h("div", { class: "gallery-item" + (i === 3 ? " sel" : "") }, icon("i-chevron-right", "ic xs"), h("span", { text: f }))));
  const mid = h("div", { class: "gallery-col" }, ...["Chalk & Charcoal", "Charcoal", "Chrome", "Conté Crayon", "Graphic Pen", "Halftone Pattern", "Note Paper", "Photocopy", "Plaster", "Reticulation", "Stamp", "Torn Edges", "Water Paper"].map((f) => h("div", { class: "gallery-item" + (f === "Chrome" ? " sel" : "") }, h("span", { text: f }))));
  const right = h("div", { class: "gallery-col" }, ...["Graphic Pen", "Halftone Pattern", "Note Paper"].map((f) => h("div", { class: "gallery-thumb" }, h("span", { class: "gallery-thumb-name", text: f }), h("span", { class: "gallery-thumb-img" }))));
  wrap.append(left, mid, right);
  return wrap;
}

export function dialogCount() {
  return stack.length;
}
