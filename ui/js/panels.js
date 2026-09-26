// Fotox — dock dei pannelli: gruppi di schede, contenuti e menu di gruppo.

import { h, icon, clear } from "./el.js";
import { panelDefs, dockGroups, initialActiveTab, mock } from "./data/panels.js";
import { openDropdown } from "./popup.js";
import { openMenuPopup } from "./menu.js";
import { state, emit, on, setColors } from "./state.js";
import { getDocCanvas } from "./canvas.js";
import * as bridge from "./native/bridge.js";
import * as nativePanels from "./native/layers-panel.js";
import * as nativeBrushes from "./native/brush-settings.js";

const activeTabs = { ...initialActiveTab };
const collapsed = {};
let dockEl = null;

/* ------------------------------------------------------------- struttura */

export function renderDock(container) {
  dockEl = container;
  clear(container);
  container.classList.add("dock");
  const resizer = h("div", { class: "dock-resizer", "data-tip": "Drag to resize the dock" });
  resizer.addEventListener("mousedown", startDockResize);
  container.append(resizer);
  const inner = h("div", { class: "dock-inner" });
  container.append(inner);

  for (const group of dockGroups) inner.append(renderGroup(group));

  on("panels", () => refresh());
  on("panel:group", () => refresh());
  on("zoom", () => renderContentFor("navigator"));
}

function visibleTabs(group) {
  const vis = group.tabs.filter((id) => state.openPanels[id]);
  if (!vis.length) return [];
  if (!vis.includes(activeTabs[group.id])) activeTabs[group.id] = vis[0];
  return vis;
}

function refresh() {
  if (!dockEl) return;
  const inner = dockEl.querySelector(".dock-inner");
  clear(inner);
  for (const group of dockGroups) inner.append(renderGroup(group));
}

function renderGroup(group) {
  const tabs = visibleTabs(group);
  if (!tabs.length) return h("div", { class: "dock-empty-group", hidden: true });
  const active = activeTabs[group.id] || tabs[0];
  const def = panelDefs[active];

  const head = h("div", { class: "panelhead" });
  const tabStrip = h("div", { class: "tabs" });
  for (const id of tabs) {
    const d = panelDefs[id];
    const tab = h("button", {
      class: "tab" + (id === active ? " active" : ""), type: "button",
      "data-tip": d.title, onclick: () => { activeTabs[group.id] = id; refresh(); },
      oncontextmenu: (e) => { e.preventDefault(); panelContext(id, e); },
    }, icon(d.icon, "ic sm"), h("span", { class: "tab-label", text: d.title }));
    tabStrip.append(tab);
  }
  head.append(tabStrip);
  head.append(h("button", {
    class: "panelhead-btn collapse", type: "button", "data-tip": collapsed[group.id] ? "Expand" : "Collapse",
    onclick: (e) => { e.stopPropagation(); collapsed[group.id] = !collapsed[group.id]; refresh(); },
  }, icon(collapsed[group.id] ? "i-chevron-down" : "i-chevron-up", "ic sm")));
  head.append(h("button", {
    class: "panelhead-btn", type: "button", "data-tip": "Panel menu",
    onclick: (e) => { e.stopPropagation(); openGroupMenu(group, e.currentTarget); },
  }, icon("i-menu", "ic sm")));
  head.addEventListener("dblclick", () => { collapsed[group.id] = !collapsed[group.id]; refresh(); });

  const body = h("div", { class: "pbody" }, collapsed[group.id] ? null : content(active));
  body.dataset.panel = active;
  return h("div", { class: "panelblock" + (collapsed[group.id] ? " collapsed" : ""), dataset: { group: group.id } }, head, body);
}

function content(id) {
  const kind = panelDefs[id].kind;
  const renderer = renderers[kind] || renderers.simple;
  return renderer(panelDefs[id]);
}

/** Ridisegna il contenuto di un pannello già montato. */
function renderContentFor(id) {
  if (!dockEl) return;
  const group = dockGroups.find((g) => g.tabs.includes(id));
  if (!group || activeTabs[group.id] !== id) return;
  const block = dockEl.querySelector(`.panelblock[data-group="${group.id}"] .pbody[data-panel="${id}"]`);
  if (!block) return;
  clear(block);
  block.append(content(id));
}

/* ---------------------------------------------------------------- rendering */

function head(extra = []) {
  return h("div", { class: "phead-row" }, ...extra);
}

function fieldRow(label, value, { dropdown = null, width = 74, value2 = null } = {}) {
  const val = h("span", { class: "pf-value", text: value });
  const box = h("button", {
    class: "pf-input", type: "button", style: { width: width + "px" }, "data-tip": label,
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({ anchor: box, items: dropdown || [], value: value, width: width + 40, onPick: (v) => { val.textContent = v; emit("mock", `${label}: ${v}`); } });
    },
  }, val, icon("i-chevron-down", "ic xs"));
  if (!dropdown) box.classList.add("plain");
  return h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: label }), box, value2);
}

function numberRow(label, value, unit = "") {
  return h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: label }),
    h("span", { class: "pf-fieldwrap" }, h("input", { class: "pf-num", type: "text", value }), h("span", { class: "pf-unit", text: unit })));
}

function bar(buttons) {
  return h("div", { class: "pbar" }, ...buttons);
}

function barBtn(ic, tip, fn = null) {
  return h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); if (fn) fn(); else emit("mock", tip); } }, icon(ic, "ic sm"));
}

function listRow({ label, thumb = null, eye = null, cls = "", extra = null, indent = 0 }) {
  const row = h("div", { class: "plist-row " + cls, style: indent ? { paddingLeft: 8 + indent * 14 + "px" } : null },
    eye === null ? h("span", { class: "peye-space" }) : h("button", {
      class: "peye" + (eye ? "" : " off"), type: "button", "data-tip": eye ? "Hide" : "Show",
      onclick: (e) => { e.stopPropagation(); const b = e.currentTarget; b.classList.toggle("off"); b.replaceChildren(icon(b.classList.contains("off") ? "i-eye-off" : "i-eye", "ic sm")); },
    }, icon(eye ? "i-eye" : "i-eye-off", "ic sm")),
    thumb,
    h("span", { class: "plist-label", text: label }),
    extra,
  );
  return row;
}

function layerThumb(kind, color) {
  const style = kind === "fill" ? { background: color } : kind === "bg" ? { background: color } : null;
  const cls = "pthumb " + (kind || "image");
  return h("span", { class: cls, style }, kind === "type" ? h("span", { class: "thumb-type", text: "AG" }) : null);
}

const renderers = {
  color() {
    const wrap = h("div", { class: "pcolor" });
    const swatchStack = h("div", { class: "swatch-stack" });
    const fg = h("button", { class: "big-swatch fg", type: "button", "data-tip": "Foreground colour", style: { background: state.colors.fg }, onclick: () => emit("ask-dialog", "color-picker") });
    const bg = h("button", { class: "big-swatch bg", type: "button", "data-tip": "Background colour", style: { background: state.colors.bg }, onclick: () => emit("ask-dialog", "color-picker") });
    swatchStack.append(bg, fg);
    wrap.append(swatchStack, h("div", { class: "spectrum" }));

    const rgbRow = (label, val) => {
      const input = h("input", { class: "pf-num", type: "text", value: val });
      const slider = h("input", { class: "pminirange", type: "range", min: 0, max: 255, value: val });
      slider.addEventListener("input", () => { input.value = slider.value; });
      return h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: label }), input, slider);
    };
    const hex = h("input", { class: "pf-num hex", type: "text", value: "#1E1E22" });
    wrap.append(
      h("div", { class: "pcolor-fields" },
        rgbRow("R", "30"), rgbRow("G", "30"), rgbRow("B", "34"),
        h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "#" }), hex),
      ),
      bar([
        barBtn("i-link", "Link to current colour layer"),
        barBtn("i-plus", "Add to swatches", () => emit("mock", "Added to swatches")),
        barBtn("i-menu", "Colour panel menu"),
      ]),
    );
    return wrap;
  },

  swatches() {
    const grid = h("div", { class: "swatch-grid" });
    for (const c of mock.swatches) {
      grid.append(h("button", {
        class: "swatch" + (c === "#474747" ? " sel" : ""), type: "button", "data-tip": c, style: { background: c },
        onclick: () => { setColors(c, null); emit("mock", "Foreground colour " + c); },
        oncontextmenu: (e) => { e.preventDefault(); setColors(null, c); },
      }));
    }
    return h("div", {}, h("div", { class: "pblock-title", text: "Default swatches" }), grid,
      bar([barBtn("i-plus", "Create new swatch"), barBtn("i-trash", "Delete swatch"), barBtn("i-presets", "Preset manager"), barBtn("i-menu", "Swatches menu")]));
  },

  styles() {
    const list = h("div", { class: "style-list" });
    for (const s of mock.styles) {
      list.append(h("div", { class: "style-row", "data-tip": s }, h("span", { class: "style-thumb" }), h("span", { class: "plist-label", text: s })));
    }
    return h("div", {}, list, bar([barBtn("i-trash", "Clear style"), barBtn("i-new-layer", "New style"), barBtn("i-menu", "Styles menu")]));
  },

  layers() {
    if (bridge.isNative) return nativePanels.layersPanel();
    const wrap = h("div", { class: "players" });
    const modeBtn = h("button", {
      class: "pf-input grow", type: "button", "data-tip": "Blend mode",
      onclick: (e) => { e.stopPropagation(); openDropdown({ anchor: modeBtn, items: ["Normal", "Dissolve", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light", "Color Dodge", "Color Burn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Color", "Luminosity"], value: "Normal", width: 180, onPick: (v) => { modeBtn.querySelector(".pf-value").textContent = v; } }); },
    }, h("span", { class: "pf-value", text: "Normal" }), icon("i-chevron-down", "ic xs"));
    wrap.append(head([modeBtn,
      h("span", { class: "pf-fieldwrap" }, h("span", { class: "pf-label", text: "Opacity:" }), h("input", { class: "pf-num", type: "text", value: "100", style: { width: "36px" } }), h("span", { class: "pf-unit", text: "%" })),
      h("button", { class: "pf-input plain", "data-tip": "Lock transparent pixels", onclick: () => emit("mock", "Lock transparent pixels") }, icon("i-grid", "ic sm")),
      h("button", { class: "pf-input plain", "data-tip": "Lock image pixels", onclick: () => emit("mock", "Lock image pixels") }, icon("i-lock", "ic sm")),
    ]));

    const lockRow = h("div", { class: "plock-row" },
      h("span", { class: "pf-label", text: "Lock:" }),
      ...[["i-presets", "Lock transparent pixels"], ["i-image", "Lock image pixels"], ["i-layers", "Lock position"], ["i-lock", "Lock all"]].map(([ic, tip]) =>
        h("button", { class: "plock-btn", type: "button", "data-tip": tip, onclick: (e) => { e.currentTarget.classList.toggle("on"); } }, icon(ic, "ic sm"))),
    );
    wrap.append(lockRow);

    const list = h("div", { class: "plist" });
    for (const l of mock.layers) {
      const extra = [h("span", { class: "pmeta", text: l.mode + (l.opacity !== 100 ? " · " + l.opacity + "%" : "") })];
      const row = listRow({ label: l.name + (l.kind === "bg" ? " (locked)" : ""), thumb: layerThumb(l.kind, l.color), eye: l.visible, extra: h("span", { class: "pmets" }, ...extra) });
      if (l.kind === "bg") row.classList.add("bgrow");
      row.addEventListener("click", () => { [...list.children].forEach((c) => c.classList.remove("sel")); row.classList.add("sel"); });
      list.append(row);
    }
    list.children[0].classList.add("sel");
    wrap.append(list);
    wrap.append(bar([
      barBtn("i-link", "Link layers"), barBtn("i-fx", "Add a layer style", () => emit("ask-dialog", "blending-options")),
      barBtn("i-mask", "Add layer mask"), barBtn("i-adjust", "Create new fill or adjustment layer", () => emit("panel:open", "adjustments")),
      barBtn("i-group", "Create a new group"), barBtn("i-new-layer", "Create a new layer"), barBtn("i-trash", "Delete layer"),
      h("span", { class: "pbar-gap" }), barBtn("i-menu", "Layers panel menu"),
    ]));
    return wrap;
  },

  channels() {
    const rows = [["RGB", "#b9b9bd", true], ["Red", "#e26060", true], ["Green", "#7ac74f", true], ["Blue", "#5b8df5", true],
      ["Alpha 1", "#d9d9de", false], ["Alpha 2", "#d9d9de", false]];
    const list = h("div", { class: "plist" });
    rows.forEach(([name, color, on], i) => {
      const row = listRow({ label: name, thumb: h("span", { class: "pthumb chan", style: { background: color } }), eye: on, extra: h("span", { class: "pmeta", text: "Ctrl+" + (i + 2) }) });
      if (i === 0) row.classList.add("sel");
      list.append(row);
    });
    return h("div", {}, list, bar([barBtn("i-image", "Load channel as selection"), barBtn("i-new-layer", "Create new channel"), barBtn("i-trash", "Delete channel"), barBtn("i-menu", "Channels menu")]));
  },

  paths() {
    const list = h("div", { class: "plist" });
    [["Work Path", "i-paths"], ["Shape 1 Vector Mask", "i-mask"], ["Path 2", "i-pen"]].forEach(([name, ic], i) => {
      const row = listRow({ label: name, thumb: h("span", { class: "pthumb path" }, icon(ic, "ic sm")), extra: h("span", { class: "pmeta", text: "" }) });
      if (i === 0) row.classList.add("sel");
      list.append(row);
    });
    return h("div", {}, list, bar([barBtn("i-color", "Fill path with foreground colour"), barBtn("i-shape-line", "Stroke path"), barBtn("i-marquee", "Load path as a selection"), barBtn("i-plus", "Create new path"), barBtn("i-trash", "Delete path"), barBtn("i-menu", "Paths menu")]));
  },

  history() {
    if (bridge.isNative) return nativePanels.historyPanel();
    const list = h("div", { class: "plist" });
    const snap = listRow({ label: "Snapshot 1", thumb: h("span", { class: "pthumb snap" }, icon("i-image", "ic sm")), extra: h("span", { class: "pmeta", text: "" }) });
    list.append(snap);
    mock.history.forEach((name, i) => {
      const row = listRow({ label: name, thumb: h("span", { class: "pthumb hist" }, icon("i-brush", "ic sm")), extra: h("span", { class: "pmeta", text: "" }) });
      if (i === mock.history.length - 1) row.classList.add("sel");
      list.append(row);
    });
    return h("div", {}, list, bar([barBtn("i-image", "Create new snapshot"), barBtn("i-plus", "Create new document from current state"), barBtn("i-trash", "Delete the current state"), barBtn("i-menu", "History menu")]));
  },

  actions() {
    const list = h("div", { class: "plist actions" });
    for (const [set, acts] of mock.actions) {
      list.append(h("div", { class: "action-set" }, h("span", { class: "ptoggle" }, icon("i-check", "ic xs")), icon("i-group", "ic sm"), h("span", { class: "plist-label", text: set })));
      for (const a of acts) {
        list.append(h("div", { class: "plist-row action", style: { paddingLeft: "26px" } },
          h("span", { class: "ptoggle" }, icon("i-check", "ic xs")), h("span", { class: "plist-label", text: a })));
      }
    }
    return h("div", {}, list, bar([barBtn("i-stop", "Stop playing"), barBtn("i-rec", "Begin recording"), barBtn("i-play", "Play the current action"), barBtn("i-new-layer", "Create new set"), barBtn("i-plus", "Create new action"), barBtn("i-trash", "Delete"), barBtn("i-menu", "Actions menu")]));
  },

  adjustments() {
    const grid = h("div", { class: "adj-grid" });
    for (const [ic, label] of mock.adjustments) {
      grid.append(h("button", {
        class: "adj-btn", type: "button", "data-tip": label,
        onclick: () => emit("ask-dialog", label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")),
      }, icon(ic, "ic")));
    }
    return h("div", {}, h("div", { class: "pblock-title", text: "Adjustment layers" }), grid,
      h("div", { class: "pblock-title", text: "Presets" }),
      h("div", { class: "plist" }, ["Brightness/Contrast 1", "Levels 1", "Curves 1"].map((n) => listRow({ label: n, thumb: h("span", { class: "pthumb adj" }, icon("i-adjust", "ic sm")) }))),
      bar([barBtn("i-plus", "Add adjustment"), barBtn("i-trash", "Delete preset"), barBtn("i-menu", "Adjustments menu")]));
  },

  properties() {
    const wrap = h("div", { class: "pprops" });
    wrap.append(
      h("div", { class: "pblock-title", text: "Transform" }),
      numberRow("X", "512 px"), numberRow("Y", "341 px"),
      h("div", { class: "pf-grid2" }, numberRow("W", "1920 px"), numberRow("H", "1280 px")),
      h("div", { class: "pblock-title", text: "Blend" }),
      fieldRow("Blend Mode", "Soft Light", { dropdown: ["Normal", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light"] }),
      numberRow("Opacity", "65 %"), numberRow("Fill", "100 %"),
      h("div", { class: "pblock-title", text: "Masks" }),
      h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: "Density" }), h("input", { class: "pminirange", type: "range", min: 0, max: 100, value: 100 })),
      h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: "Feather" }), h("input", { class: "pminirange", type: "range", min: 0, max: 100, value: 2 })),
      h("div", { class: "pblock-title", text: "Quick Actions" }),
      h("div", { class: "pf-actions" },
        ...[["Remove background", "i-object-select"], ["Select subject", "i-marquee"], ["Crop to square", "i-crop"], ["Enhance colours", "i-sun"]].map(([label, ic]) =>
          h("button", { class: "pf-action", type: "button", onclick: () => emit("mock", label) }, icon(ic, "ic sm"), h("span", { text: label })))),
    );
    return wrap;
  },

  histogram() {
    const cv = h("canvas", { class: "hist-canvas", width: 256, height: 96 });
    const ctx = cv.getContext("2d");
    const g = ctx.createLinearGradient(0, 0, 256, 0);
    g.addColorStop(0, "#2b2f36"); g.addColorStop(0.4, "#8f95a3"); g.addColorStop(1, "#e8eaf0");
    ctx.fillStyle = g;
    for (let x = 0; x < 256; x++) {
      const n = Math.max(2, 90 * Math.exp(-Math.pow((x - 128) / 46, 2)) + 22 * Math.random());
      ctx.fillRect(x, 96 - n, 1, n);
    }
    ctx.strokeStyle = "rgba(255,255,255,.18)";
    for (let i = 1; i < 4; i++) { ctx.beginPath(); ctx.moveTo(i * 64, 0); ctx.lineTo(i * 64, 96); ctx.stroke(); }
    return h("div", { class: "phist" }, cv,
      fieldRow("Channel", "RGB", { dropdown: ["RGB", "Red", "Green", "Blue", "Luminosity"], width: 96 }),
      h("div", { class: "preadouts" },
        h("span", { text: "Mean: 148.32" }), h("span", { text: "Median: 152" }),
        h("span", { text: "Pixels: 2,073,600" }), h("span", { text: "Level: 165" }),
        h("span", { text: "Count: 8,412" }), h("span", { text: "Percentile: 62.4" })),
      bar([barBtn("i-zoom-in", "Zoom in on histogram"), barBtn("i-sun", "Uncached refresh"), barBtn("i-menu", "Histogram menu")]));
  },

  navigator() {
    const cv = h("canvas", { class: "nav-canvas", width: 220, height: 140 });
    const ctx = cv.getContext("2d");
    const src = getDocCanvas();
    if (src) {
      const scale = Math.min(220 / src.width, 140 / src.height);
      ctx.fillStyle = "#1a1a1c";
      ctx.fillRect(0, 0, 220, 140);
      ctx.drawImage(src, (220 - src.width * scale) / 2, (140 - src.height * scale) / 2, src.width * scale, src.height * scale);
    }
    const box = h("div", { class: "nav-view" });
    const holder = h("div", { class: "nav-holder" }, cv, box);
    holder.addEventListener("mousedown", (e) => {
      const r = cv.getBoundingClientRect();
      box.style.left = Math.max(0, Math.min(r.width - box.offsetWidth, e.clientX - r.left - box.offsetWidth / 2)) + "px";
      box.style.top = Math.max(0, Math.min(r.height - box.offsetHeight, e.clientY - r.top - box.offsetHeight / 2)) + "px";
    });
    const out = h("span", { class: "pf-value", text: state.zoom + "%" });
    const slider = h("input", { class: "pminirange", type: "range", min: 3, max: 400, value: Math.min(400, state.zoom) });
    slider.addEventListener("input", () => { out.textContent = slider.value + "%"; emit("zoom:set", Number(slider.value)); });
    on("zoom", (z) => { out.textContent = z + "%"; slider.value = Math.min(400, z); });
    return h("div", { class: "pnav" }, holder,
      h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "Zoom" }), slider, out),
      bar([barBtn("i-minus", "Zoom out", () => emit("zoom:set", state.zoom / 1.25)), barBtn("i-plus", "Zoom in", () => emit("zoom:set", state.zoom * 1.25)), barBtn("i-menu", "Navigator menu")]));
  },

  info() {
    const wrap = h("div", { class: "pinfo" });
    const rows = h("div", { class: "pinfo-rows" });
    for (const [k, v] of mock.infoRows) {
      rows.append(h("div", { class: "pinfo-row" }, h("span", { class: "pinfo-k", text: k }), h("span", { class: "pinfo-v", text: v })));
    }
    wrap.append(rows,
      h("div", { class: "pblock-title", text: "Document" }),
      h("div", { class: "pinfo-note", text: `${state.doc.name} — ${state.doc.w} × ${state.doc.h} px · ${state.doc.mode}/${state.doc.bits} · 72 ppi` }),
      bar([barBtn("i-eyedropper", "Colour sample"), barBtn("i-ruler", "Measurement"), barBtn("i-menu", "Info menu")]));
    return wrap;
  },

  measurements() {
    const table = h("div", { class: "ptable" });
    table.append(h("div", { class: "ptable-head" }, ...["#", "Type", "Length", "Angle", "Area"].map((t) => h("span", { text: t }))));
    for (const r of mock.measurements) table.append(h("div", { class: "ptable-row" }, ...r.map((c) => h("span", { text: c }))));
    return h("div", {}, table, fieldRow("Scale", "1 Pixel = 1 Pixel"), bar([barBtn("i-trash", "Delete measurement"), barBtn("i-ruler", "Record measurement"), barBtn("i-menu", "Measurements menu")]));
  },

  character() {
    const wrap = h("div", { class: "ptext" });
    wrap.append(head([
      h("button", { class: "pf-input grow", "data-tip": "Font family", onclick: (e) => openDropdown({ anchor: e.currentTarget, items: ["Open Sans", "Arimo", "Bitter", "Lato", "Lora", "Merriweather", "Montserrat", "Oswald", "Playfair Display", "Poppins", "Raleway", "Roboto", "Ubuntu"], value: "Open Sans", width: 160 }) },
        h("span", { class: "pf-value", text: "Open Sans" }), icon("i-chevron-down", "ic xs")),
    ]));
    wrap.append(head([
      h("button", { class: "pf-input", style: { width: "96px" }, onclick: (e) => openDropdown({ anchor: e.currentTarget, items: ["Regular", "Italic", "Bold", "Bold Italic"], value: "Regular", width: 120 }) },
        h("span", { class: "pf-value", text: "Regular" }), icon("i-chevron-down", "ic xs")),
      h("span", { class: "pf-fieldwrap" }, h("input", { class: "pf-num", type: "text", value: "24" }), h("span", { class: "pf-unit", text: "pt" })),
    ]));
    const grid = h("div", { class: "ptext-grid" });
    for (const [k, v] of mock.character) {
      if (k === "Font" || k === "Style") continue;
      grid.append(h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: k }), h("input", { class: "pf-num", type: "text", value: v })));
    }
    wrap.append(grid,
      h("div", { class: "pf-actions" },
        ...[["i-quote", "Left"], ["i-props", "Center"], ["i-quote", "Right"], ["i-props", "Justify"]].map(([ic, t]) =>
          h("button", { class: "pf-action", "data-tip": t, onclick: () => emit("mock", "Align " + t) }, icon(ic, "ic sm")))),
      h("div", { class: "swap-colors" },
        h("button", { class: "pf-action", "data-tip": "Text colour", onclick: () => emit("ask-dialog", "color-picker") }, icon("i-color", "ic sm"), h("span", { text: "Text colour" }))),
      bar([barBtn("i-fx", "Toggle OpenType features"), barBtn("i-menu", "Character menu")]));
    return wrap;
  },

  paragraph() {
    const wrap = h("div", { class: "ptext" });
    wrap.append(h("div", { class: "pf-actions" },
      ...[["i-quote", "Left"], ["i-props", "Center"], ["i-quote", "Right"], ["i-props", "Justify last left"], ["i-props", "Justify all"]].map(([ic, t]) =>
        h("button", { class: "pf-action", "data-tip": t, onclick: () => emit("mock", "Align " + t) }, icon(ic, "ic sm")))));
    for (const [k, v] of mock.paragraph) wrap.append(h("div", { class: "pf-row" }, h("span", { class: "pf-label", text: k }), h("input", { class: "pf-num", type: "text", value: v })));
    wrap.append(bar([barBtn("i-presets", "Paragraph presets"), barBtn("i-menu", "Paragraph menu")]));
    return wrap;
  },

  brush() {
    if (bridge.isNative) return nativeBrushes.brushPanel();
    const wrap = h("div", { class: "pbrush" });
    const sizeOut = h("span", { class: "pf-value", text: "22 px" });
    const size = h("input", { class: "pminirange", type: "range", min: 1, max: 200, value: 22 });
    size.addEventListener("input", () => { sizeOut.textContent = size.value + " px"; });
    wrap.append(h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "Size" }), size, sizeOut));
    const hardOut = h("span", { class: "pf-value", text: "75 %" });
    const hard = h("input", { class: "pminirange", type: "range", min: 0, max: 100, value: 75 });
    hard.addEventListener("input", () => { hardOut.textContent = hard.value + " %"; });
    wrap.append(h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "Hardness" }), hard, hardOut));

    const list = h("div", { class: "plist brushlist" });
    for (const [name, size2] of mock.brushPresets) {
      list.append(listRow({
        label: name,
        thumb: h("span", { class: "pthumb brush" }, h("span", { class: "brush-dot", style: { width: Math.min(20, size2) + "px", height: Math.min(20, size2) + "px" } })),
        extra: h("span", { class: "pmeta", text: size2 + " px" }),
      }));
    }
    list.children[0].classList.add("sel");
    wrap.append(list, bar([barBtn("i-plus", "Create new brush"), barBtn("i-trash", "Delete brush"), barBtn("i-presets", "Preset manager"), barBtn("i-menu", "Brush menu")]));
    return wrap;
  },

  "brush-settings"() {
    if (bridge.isNative) return nativeBrushes.brushSettingsPanel();
    const wrap = h("div", { class: "pbrush-set" });
    const items = ["Shape Dynamics", "Scattering", "Texture", "Dual Brush", "Colour Dynamics", "Transfer", "Brush Pose", "Noise", "Wet Edges", "Build-up", "Smoothing", "Protect Texture"];
    for (const [i, name] of items.entries()) {
      wrap.append(h("div", { class: "bset-row" + (i === 0 ? " open" : "") },
        h("button", { class: "ptoggle" + (i < 2 ? " on" : ""), onclick: (e) => e.currentTarget.classList.toggle("on") }, icon("i-check", "ic xs")),
        h("button", { class: "pf-input plain expander", onclick: (e) => { const r = e.currentTarget.closest(".bset-row"); r.classList.toggle("open"); } }, icon("i-chevron-right", "ic xs")),
        h("span", { class: "plist-label", text: name })));
    }
    return wrap;
  },

  toolpresets() {
    const list = h("div", { class: "plist" });
    for (const p of mock.toolPresets) list.append(listRow({ label: p, thumb: h("span", { class: "pthumb preset" }, icon("i-presets", "ic sm")) }));
    list.children[1].classList.add("sel");
    return h("div", {}, h("div", { class: "phead-row" }, h("span", { class: "pf-label", text: "Current Tool Only" }), h("span", { class: "ptoggle on", onclick: (e) => e.currentTarget.classList.toggle("on") }, icon("i-check", "ic xs"))),
      list, bar([barBtn("i-new-layer", "New tool preset"), barBtn("i-trash", "Delete tool preset"), barBtn("i-menu", "Tool presets menu")]));
  },

  libraries() {
    const grid = h("div", { class: "lib-grid" });
    for (let i = 0; i < 6; i++) {
      grid.append(h("div", { class: "lib-tile", "data-tip": "Empty library item" }, icon("i-plus", "ic"), h("span", { text: "New" })));
    }
    return h("div", {}, h("div", { class: "pblock-title", text: "Fotox demo library" }), grid,
      h("div", { class: "pf-actions" },
        h("button", { class: "pf-action", onclick: () => emit("mock", "Create library from document") }, icon("i-doc", "ic sm"), h("span", { text: "Create from document" }))),
      bar([barBtn("i-plus", "Add from file"), barBtn("i-cloud", "Upload to cloud"), barBtn("i-menu", "Libraries menu")]));
  },

  timeline() {
    const rows = h("div", { class: "tl-rows" });
    for (const [name, w] of [["Heading", 62], ["Colour wash", 44], ["Photo", 70]]) {
      rows.append(h("div", { class: "tl-row" }, h("span", { class: "tl-name", text: name }), h("span", { class: "tl-bar", style: { width: w + "%" } })));
    }
    return h("div", { class: "timeline" }, rows, h("div", { class: "tl-playhead", style: { left: "34%" } }),
      h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "Frame" }), h("span", { class: "pf-value", text: "0 / 120" }), h("span", { class: "pf-label", text: "fps 24" })),
      bar([barBtn("i-play", "Play"), barBtn("i-stop", "Stop"), barBtn("i-rec", "Record"), barBtn("i-video", "Render video")]));
  },

  notes() {
    return h("div", { class: "pnotes" }, listRow({ label: "Review note — check the headline kerning", thumb: h("span", { class: "pthumb note" }, icon("i-note", "ic sm")) }), bar([barBtn("i-plus", "New note"), barBtn("i-trash", "Delete note")]));
  },

  simple(def) {
    return h("div", { class: "psimple" }, h("div", { class: "pnote", text: def.note || "Nothing to show here yet." }));
  },
};

/* --------------------------------------------------------------- menu ⟶ */

function panelContext(id, event) {
  const menu = [
    { label: "Close", a: `panel:toggle:${id}` },
    { label: "Close Tab Group", a: `panel:closegroup:${id}` },
    { label: "Panel Options...", a: "dlg:panel-options" },
    { sep: true },
    { label: "Collapse", a: `panel:collapse:${id}` },
    { label: "Expand All", a: "panel:expandall" },
    { sep: true },
    { label: "Move to Left Dock", a: "panel:left" },
    { label: "Dock to Right", a: "panel:right" },
  ];
  openMenuPopup({ left: event.clientX, top: event.clientY, bottom: event.clientY, right: event.clientX, width: 0, height: 0 }, menu, { className: "menu-pop context" });
}

function openGroupMenu(group, anchor) {
  const items = group.tabs.map((id) => ({ label: panelDefs[id].title, chk: `panel:${id}`, a: `panel:toggle:${id}` }));
  items.push({ sep: true });
  items.push({ label: "Collapse", a: `panel:collapse:${group.id}` });
  items.push({ label: "Expand", a: `panel:expand:${group.id}` });
  items.push({ sep: true });
  items.push({ label: "Close Tab Group", a: `panel:closegroup:${group.id}` });
  items.push({ label: "Panel Options...", a: "dlg:panel-options" });
  openMenuPopup(anchor, items, { align: "right" });
}

/* --------------------------------------------------------------- API */

export function togglePanel(id) {
  if (!(id in state.openPanels)) return;
  state.openPanels[id] = !state.openPanels[id];
  if (state.openPanels[id]) focusPanel(id);
  emit("panels");
}

export function focusPanel(id) {
  const group = dockGroups.find((g) => g.tabs.includes(id));
  if (!group) return;
  state.openPanels[id] = true;
  activeTabs[group.id] = id;
  collapsed[group.id] = false;
  emit("panels");
}

export function collapseGroup(groupId, value) {
  if (collapsed[groupId] === value) return;
  collapsed[groupId] = value;
  emit("panels");
}

export function closeGroup(id) {
  const group = dockGroups.find((g) => g.id === id || g.tabs.includes(id));
  if (!group) return;
  group.tabs.forEach((t) => { state.openPanels[t] = false; });
  emit("panels");
}

export function setAllPanels(open) {
  for (const id of Object.keys(state.openPanels)) {
    if (dockGroups.some((g) => g.tabs.includes(id))) state.openPanels[id] = open;
  }
  emit("panels");
}

/** Tutti i pannelli chiusi → riapre il layout predefinito. */
export function resetPanels() {
  const defaults = ["color", "swatches", "styles", "layers", "channels", "paths", "history", "actions", "layerscomps", "adjustments", "properties", "histogram", "navigator", "info", "measurements"];
  for (const id of Object.keys(state.openPanels)) state.openPanels[id] = defaults.includes(id);
  for (const g of dockGroups) collapsed[g.id] = false;
  emit("panels");
}

export function dockWidth() {
  return dockEl ? dockEl.offsetWidth : 260;
}

function startDockResize(e) {
  e.preventDefault();
  const startX = e.clientX;
  const startW = dockEl.offsetWidth;
  const move = (ev) => {
    const w = Math.max(210, Math.min(420, startW - (ev.clientX - startX)));
    dockEl.style.width = w + "px";
  };
  const up = () => {
    document.removeEventListener("mousemove", move);
    document.removeEventListener("mouseup", up);
  };
  document.addEventListener("mousemove", move);
  document.addEventListener("mouseup", up);
}

export { refresh as refreshDock };
