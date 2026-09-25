// Fotox — the live Layers and History panels in the app (M2-T06, M2-T04 step 3).
//
// Driven entirely by the engine: `layers` (the flat list, top → bottom, tree
// via `depth`), `history`, binary `thumbnail` frames. Every change the user
// makes is sent as a document `command` (docs/PROTOCOL.md §4); nothing is
// changed locally, the next `layers` message shows the result.
//
// The list is virtualised (only the visible rows exist), so 1 000 layers
// scroll and update as fast as 10.

import { h, icon, clear, add } from "../el.js";
import { openDropdown } from "../popup.js";
import { openDialog } from "../dialogs.js";
import { state } from "../state.js";
import { toast } from "../tooltip.js";
import * as bridge from "./bridge.js";
import { UI, ENGINE } from "./protocol.js";

const ROW_H = 30;
const THUMB_SIZE = 64; // px requested from the engine (drawn at 26 px, sharp on HiDPI)

const BLENDS = [
  ["pass_through", "Pass Through"],
  ["normal", "Normal"], ["dissolve", "Dissolve"],
  ["darken", "Darken"], ["multiply", "Multiply"], ["color_burn", "Color Burn"], ["linear_burn", "Linear Burn"], ["darker_color", "Darker Color"],
  ["lighten", "Lighten"], ["screen", "Screen"], ["color_dodge", "Color Dodge"], ["linear_dodge", "Linear Dodge (Add)"], ["lighter_color", "Lighter Color"],
  ["overlay", "Overlay"], ["soft_light", "Soft Light"], ["hard_light", "Hard Light"], ["vivid_light", "Vivid Light"], ["linear_light", "Linear Light"], ["pin_light", "Pin Light"], ["hard_mix", "Hard Mix"],
  ["difference", "Difference"], ["exclusion", "Exclusion"], ["subtract", "Subtract"], ["divide", "Divide"],
  ["hue", "Hue"], ["saturation", "Saturation"], ["color", "Color"], ["luminosity", "Luminosity"],
];
const blendName = (id) => (BLENDS.find(([k]) => k === id) || [id, id])[1];

// New fill/adjustment layers: label → the `NewLayer` JSON the engine expects.
const NEW_ADJUSTMENTS = [
  ["Solid Color...", () => ({ solid_fill: { rgba: hexToRgba16(state.colors.fg) } })],
  ["Brightness/Contrast...", () => ({ adjustment: { kind: "brightness_contrast", brightness: 0, contrast: 0, legacy: false } })],
  ["Levels...", () => ({ adjustment: { kind: "levels", channels: [0, 1, 2, 3].map(() => ({ in_black: 0, in_white: 1, gamma: 1, out_black: 0, out_white: 1 })) } })],
  ["Curves...", () => ({ adjustment: { kind: "curves", channels: [[], [], [], []] } })],
  ["Exposure...", () => ({ adjustment: { kind: "exposure", exposure: 0, offset: 0, gamma: 1 } })],
  ["Hue/Saturation...", () => ({ adjustment: { kind: "hue_saturation", hue: 0, saturation: 0, lightness: 0, colorize: false } })],
  ["Invert", () => ({ adjustment: { kind: "invert" } })],
];

let doc = null;          // active document id
let layers = [];         // LayerInfo[] of the active document, top → bottom
let tree = [];           // per row: { parent, index (bottom = 0), count }
const collapsed = new Map(); // "doc:layer" → true when a group is collapsed in the panel
const thumbs = new Map();    // "doc:layer" → data URL
const requested = new Set(); // "doc:layer" thumbnails already asked for
let anchor = null;       // shift-click range anchor (layer id)
let history = null;      // last `history` message of the active document
const histories = new Map(); // doc → history message

let layersRoot = null;
let historyRoot = null;
let listEl = null;
let spacerEl = null;
let dragId = null;
let editNew = null;      // ids before a new adjustment layer: its dialog opens when it arrives

/** Start listening to the engine. Call once, in native mode. */
export function initNativePanels() {
  bridge.on(ENGINE.ACTIVE_DOCUMENT, ({ doc: id }) => {
    doc = id;
    layers = [];
    tree = [];
    history = id == null ? null : histories.get(id) || null;
    editNew = null;
    renderLayers();
    renderHistory();
  });
  bridge.on(ENGINE.LAYERS, (msg) => {
    if (msg.doc !== doc) return;
    layers = msg.layers;
    tree = buildTree(layers);
    for (const l of layers) {
      const key = `${doc}:${l.id}`;
      if (l.kind === "group" && !collapsed.has(key)) collapsed.set(key, !l.expanded);
    }
    requestThumbnails();
    renderLayers();
    // Like Photoshop, a new adjustment layer opens its settings.
    const added = editNew && layers.find((l) => l.adjustment && !editNew.has(l.id));
    if (added) {
      editNew = null;
      if (added.adjustment.kind !== "invert") editAdjustment(added);
    }
  });
  bridge.on(ENGINE.HISTORY, (msg) => {
    histories.set(msg.doc, msg);
    if (msg.doc === doc) {
      history = msg;
      renderHistory();
    }
  });
  bridge.on(ENGINE.DOCUMENT_CLOSED, ({ doc: id }) => histories.delete(id));
  bridge.on(ENGINE.THUMBNAIL, (msg, payload) => {
    const canvas = document.createElement("canvas");
    canvas.width = msg.width;
    canvas.height = msg.height;
    const pixels = new Uint8ClampedArray(payload.buffer, payload.byteOffset, payload.length);
    canvas.getContext("2d").putImageData(new ImageData(pixels, msg.width, msg.height), 0, 0);
    // toDataURL, not toBlob: toBlob is throttled in CEF.
    thumbs.set(`${msg.doc}:${msg.layer}`, canvas.toDataURL());
    if (msg.doc === doc) renderRows();
  });
}

/* ------------------------------------------------------------------ commands */

function send(command) {
  if (doc == null) return;
  bridge.send({ type: UI.COMMAND, doc, command });
}
const ref = (id) => ({ id });
const setProps = (id, props) => send({ op: "set_layer_props", layer: ref(id), props });
const selectedIds = () => layers.filter((l) => l.selected).map((l) => l.id);
/** The id of the active layer of the active document, or null (M4 filters). */
export function activeLayerId() {
  const a = active();
  return a ? a.id : null;
}

/** The kind of the active layer ("pixel", "group", …), or null. */
export function activeLayerKind() {
  const a = active();
  return a ? a.kind : null;
}

const active = () => {
  // The active layer is the last one selected; `layers` does not carry the
  // selection order, so fall back to the topmost selected row.
  const sel = layers.filter((l) => l.selected);
  return sel.find((l) => l.id === lastClicked) || sel[0] || null;
};
let lastClicked = null;

/* ------------------------------------------------------------------ tree */

/** Parent id and bottom-based sibling index of every row (the flat list is top → bottom). */
function buildTree(rows) {
  const out = rows.map(() => ({ parent: null, index: 0, count: 0 }));
  const stack = []; // [{ row, depth }] of open groups
  const siblings = new Map(); // parent id (or "root") → row indices, top → bottom
  rows.forEach((l, i) => {
    while (stack.length && stack[stack.length - 1].depth >= l.depth) stack.pop();
    const parent = stack.length ? rows[stack[stack.length - 1].row].id : null;
    out[i].parent = parent;
    const key = parent ?? "root";
    if (!siblings.has(key)) siblings.set(key, []);
    siblings.get(key).push(i);
    if (l.kind === "group") stack.push({ row: i, depth: l.depth });
  });
  for (const list of siblings.values()) {
    list.forEach((row, pos) => {
      out[row].index = list.length - 1 - pos;
      out[row].count = list.length;
    });
  }
  return out;
}

/** Rows shown in the panel (children of collapsed groups are hidden). */
function visibleRows() {
  const out = [];
  let hideBelow = null; // depth of a collapsed group we are inside
  layers.forEach((l, i) => {
    if (hideBelow !== null) {
      if (l.depth > hideBelow) return;
      hideBelow = null;
    }
    out.push(i);
    if (l.kind === "group" && collapsed.get(`${doc}:${l.id}`)) hideBelow = l.depth;
  });
  return out;
}

function childCount(groupId) {
  return tree.filter((t) => t.parent === groupId).length;
}

/* ------------------------------------------------------------------ Layers panel */

/** The Layers panel content (a persistent element, updated in place). */
export function layersPanel() {
  if (!layersRoot) {
    layersRoot = h("div", { class: "players native" });
    renderLayers();
  }
  return layersRoot;
}

function renderLayers() {
  if (!layersRoot) return;
  clear(layersRoot);
  if (doc == null) {
    layersRoot.append(h("div", { class: "pempty", text: "Open a document to see its layers." }));
    return;
  }
  const a = active();
  const locks = !!a;

  // Blend mode, opacity, fill of the active layer.
  const modeBtn = h("button", {
    class: "pf-input grow", type: "button", "data-tip": "Blend mode", disabled: !a,
    onclick: (e) => {
      e.stopPropagation();
      if (!a) return;
      const items = BLENDS.filter(([k]) => k !== "pass_through" || a.kind === "group").map(([, name]) => name);
      openDropdown({
        anchor: modeBtn, items, value: blendName(a.blend), width: 190,
        onPick: (name) => {
          const id = (BLENDS.find(([, n]) => n === name) || [])[0];
          if (id) setProps(a.id, { blend: id });
        },
      });
    },
  }, h("span", { class: "pf-value", text: a ? blendName(a.blend) : "Normal" }), icon("i-chevron-down", "ic xs"));

  layersRoot.append(h("div", { class: "phead-row" }, modeBtn, percentField("Opacity:", a, "opacity")));
  layersRoot.append(h("div", { class: "phead-row" },
    h("span", { class: "plock-row" },
      h("span", { class: "pf-label", text: "Lock:" }),
      lockBtn("i-image", "Lock image pixels", a && a.locked_pixels, locks, () => setProps(a.id, { locked_pixels: !a.locked_pixels })),
      lockBtn("i-layers", "Lock position", a && a.locked_position, locks, () => setProps(a.id, { locked_position: !a.locked_position })),
      lockBtn("i-lock", "Lock all", a && a.locked_pixels && a.locked_position, locks, () => {
        const on = !(a.locked_pixels && a.locked_position);
        setProps(a.id, { locked_pixels: on, locked_position: on });
      })),
    h("span", { class: "pbar-gap" }),
    percentField("Fill:", a, "fill")));

  // The virtualised list.
  listEl = h("div", { class: "plist nlist" });
  spacerEl = h("div", { class: "nlist-space" });
  listEl.append(spacerEl);
  listEl.addEventListener("scroll", () => renderRows());
  listEl.addEventListener("dragover", (e) => { if (dragId != null) e.preventDefault(); });
  listEl.addEventListener("drop", (e) => dropOnList(e));
  layersRoot.append(listEl);

  layersRoot.append(h("div", { class: "pbar" },
    barBtn("i-mask", "Add layer mask", () => {
      if (!a) return;
      if (a.has_mask) toast("The layer already has a mask");
      else send({ op: "add_mask", layer: ref(a.id), fill: "reveal_all" });
    }),
    barBtn("i-adjust", "Create new fill or adjustment layer", (btn) => openDropdown({
      anchor: btn, items: NEW_ADJUSTMENTS.map(([n]) => n), value: "", width: 200,
      onPick: (name) => {
        const make = NEW_ADJUSTMENTS.find(([n]) => n === name);
        if (!make) return;
        const layer = make[1]();
        if (layer.adjustment) editNew = new Set(layers.map((l) => l.id));
        send({ op: "add_layer", layer, name: null });
      },
    })),
    barBtn("i-group", "Create a new group (with the selected layers: Ctrl+G)", () => {
      const sel = selectedIds();
      if (sel.length > 1) send({ op: "group_layers", layers: sel.map(ref), name: null });
      else send({ op: "add_layer", layer: "group", name: null });
    }),
    barBtn("i-new-layer", "Create a new layer", () => send({ op: "add_layer", layer: "pixel", name: null })),
    barBtn("i-trash", "Delete layer", () => {
      const sel = selectedIds();
      if (sel.length) send({ op: "delete_layers", layers: sel.map(ref) });
    }),
  ));

  // Rows need the list's height, known after layout.
  requestAnimationFrame(() => renderRows());
}

function renderRows() {
  if (!listEl || !listEl.isConnected) return;
  const rows = visibleRows();
  spacerEl.style.height = rows.length * ROW_H + "px";
  for (const old of [...listEl.querySelectorAll(".nrow")]) old.remove();
  const top = listEl.scrollTop;
  const height = listEl.clientHeight || 300;
  const first = Math.max(0, Math.floor(top / ROW_H) - 3);
  const last = Math.min(rows.length, Math.ceil((top + height) / ROW_H) + 3);
  for (let v = first; v < last; v++) listEl.append(row(rows[v], v));
}

function row(i, v) {
  const l = layers[i];
  const key = `${doc}:${l.id}`;
  const el = h("div", {
    class: "plist-row nrow" + (l.selected ? " sel" : "") + (l.visible ? "" : " hidden-layer"),
    style: { top: v * ROW_H + "px", height: ROW_H + "px", paddingLeft: 4 + l.depth * 14 + "px" },
    draggable: "true", "data-id": String(l.id),
  });

  const eye = h("button", {
    class: "peye" + (l.visible ? "" : " off"), type: "button", "data-tip": l.visible ? "Hide" : "Show",
    onclick: (e) => { e.stopPropagation(); setProps(l.id, { visible: !l.visible }); },
  }, icon(l.visible ? "i-eye" : "i-eye-off", "ic sm"));

  const expander = l.kind === "group"
    ? h("button", {
      class: "nexpand", type: "button", "data-tip": collapsed.get(key) ? "Expand" : "Collapse",
      onclick: (e) => { e.stopPropagation(); collapsed.set(key, !collapsed.get(key)); renderRows(); },
    }, icon(collapsed.get(key) ? "i-chevron-right" : "i-chevron-down", "ic xs"))
    : h("span", { class: "nexpand" });

  let thumb;
  if (l.kind === "group") thumb = h("span", { class: "pthumb adj" }, icon("i-group", "ic sm"));
  else if (l.kind === "adjustment") {
    thumb = h("span", {
      class: "pthumb adj", "data-tip": "Double-click to edit the adjustment",
      ondblclick: (e) => { e.stopPropagation(); editAdjustment(l); },
    }, icon("i-adjust", "ic sm"));
  } else {
    thumb = h("span", { class: "pthumb nthumb" });
    const url = thumbs.get(key);
    if (url) thumb.append(h("img", { src: url, alt: "" }));
    else if (l.fill_color) thumb.style.background = rgba16ToCss(l.fill_color);
  }

  const name = h("span", { class: "plist-label", text: (l.clipped ? "↳ " : "") + l.name });
  name.addEventListener("dblclick", (e) => { e.stopPropagation(); rename(l, name); });

  const meta = [];
  if (l.blend !== "normal" && l.blend !== "pass_through") meta.push(blendName(l.blend));
  if (l.opacity < 1) meta.push(Math.round(l.opacity * 100) + "%");
  // The DOM's own append() would print a null child as "null": use add().
  add(el, [eye, expander, thumb,
    l.has_mask ? h("span", { class: "pthumb nmask", "data-tip": "Layer mask" }, icon("i-mask", "ic xs")) : null,
    name,
    meta.length ? h("span", { class: "pmeta", text: meta.join(" · ") }) : null,
    l.locked ? h("span", { class: "nlock", "data-tip": "Locked" }, icon("i-lock", "ic xs")) : null]);

  el.addEventListener("click", (e) => select(l, e));
  el.addEventListener("dragstart", (e) => { dragId = l.id; e.dataTransfer.effectAllowed = "move"; e.dataTransfer.setData("text/plain", String(l.id)); });
  el.addEventListener("dragend", () => { dragId = null; clearDropMarks(); });
  el.addEventListener("dragover", (e) => {
    if (dragId == null) return;
    e.preventDefault();
    e.stopPropagation();
    clearDropMarks();
    el.classList.add("drop-" + dropZone(e, el, l));
  });
  el.addEventListener("drop", (e) => {
    e.preventDefault();
    e.stopPropagation();
    const zone = dropZone(e, el, l);
    clearDropMarks();
    moveTo(dragId, i, zone);
    dragId = null;
  });
  return el;
}

function select(l, e) {
  const visible = visibleRows().map((i) => layers[i].id);
  let ids;
  if (e.shiftKey && anchor != null && visible.includes(anchor)) {
    const [a, b] = [visible.indexOf(anchor), visible.indexOf(l.id)].sort((x, y) => x - y);
    ids = visible.slice(a, b + 1).filter((id) => id !== l.id);
    ids.push(l.id);
  } else if (e.ctrlKey || e.metaKey) {
    ids = selectedIds().filter((id) => id !== l.id);
    if (!l.selected) ids.push(l.id);
    anchor = l.id;
  } else {
    ids = [l.id];
    anchor = l.id;
  }
  lastClicked = l.id;
  send({ op: "select_layers", layers: ids.map(ref) });
}

function rename(l, label) {
  const input = h("input", { class: "pf-num nrename", type: "text", value: l.name });
  label.replaceWith(input);
  input.focus();
  input.select();
  let done = false;
  const finish = (commit) => {
    if (done) return;
    done = true;
    const value = input.value.trim();
    if (commit && value && value !== l.name) setProps(l.id, { name: value });
    input.replaceWith(label);
  };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") finish(true);
    if (e.key === "Escape") finish(false);
  });
  input.addEventListener("blur", () => finish(true));
}

/* drag and drop: above / below a row, or into a group (middle of the row) */

function dropZone(e, el, l) {
  const y = e.offsetY / el.offsetHeight;
  if (l.kind === "group" && y > 0.3 && y < 0.7) return "into";
  return y < 0.5 ? "above" : "below";
}

function clearDropMarks() {
  if (!listEl) return;
  listEl.querySelectorAll(".drop-above, .drop-below, .drop-into").forEach((r) => r.classList.remove("drop-above", "drop-below", "drop-into"));
}

function moveTo(id, targetRow, zone) {
  if (id == null) return;
  const from = layers.findIndex((l) => l.id === id);
  if (from < 0 || from === targetRow) return;
  const target = layers[targetRow];
  const src = tree[from];
  let parent;
  let index;
  if (zone === "into") {
    parent = target.id;
    index = childCount(target.id) - (src.parent === target.id ? 1 : 0); // on top
  } else {
    parent = tree[targetRow].parent;
    // `index` counts the target list *without* the moved layer (bottom = 0).
    index = tree[targetRow].index + (zone === "above" ? 1 : 0);
    if (src.parent === parent && src.index < tree[targetRow].index) index -= 1;
  }
  send({ op: "move_layer", layer: ref(id), parent: parent == null ? null : ref(parent), index });
}

function dropOnList(e) {
  // Dropped below the last row: bottom of the root list.
  e.preventDefault();
  if (dragId == null) return;
  send({ op: "move_layer", layer: ref(dragId), parent: null, index: 0 });
  dragId = null;
  clearDropMarks();
}

/* opacity / fill: typed value, or scrub by dragging the label */

function percentField(label, a, prop) {
  const value = a ? Math.round(a[prop] * 100) : 100;
  const input = h("input", { class: "pf-num", type: "text", value: String(value), style: { width: "36px" }, disabled: !a });
  const apply = (v) => { if (a) setProps(a.id, { [prop]: Math.min(100, Math.max(0, v)) / 100 }); };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") { apply(Number(input.value) || 0); input.blur(); }
  });
  input.addEventListener("change", () => apply(Number(input.value) || 0));
  const lab = h("span", { class: "pf-label nscrub", text: label, "data-tip": "Drag to change" });
  lab.addEventListener("mousedown", (e) => {
    if (!a) return;
    e.preventDefault();
    const startX = e.clientX;
    const start = value;
    let pending = null;
    const move = (ev) => {
      const v = Math.min(100, Math.max(0, Math.round(start + (ev.clientX - startX) / 2)));
      input.value = String(v);
      // One command per frame at most; the engine merges them into one step.
      if (pending === null) pending = requestAnimationFrame(() => { pending = null; apply(Number(input.value)); });
    };
    const up = () => { document.removeEventListener("mousemove", move); document.removeEventListener("mouseup", up); };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  });
  return h("span", { class: "pf-fieldwrap" }, lab, input, h("span", { class: "pf-unit", text: "%" }));
}

function lockBtn(ic, tip, on, enabled, fn) {
  return h("button", {
    class: "plock-btn" + (on ? " on" : ""), type: "button", "data-tip": tip, disabled: !enabled,
    onclick: (e) => { e.stopPropagation(); if (enabled) fn(); },
  }, icon(ic, "ic sm"));
}

function barBtn(ic, tip, fn) {
  return h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(e.currentTarget); } }, icon(ic, "ic sm"));
}

function requestThumbnails() {
  const ids = layers
    .filter((l) => (l.kind === "pixel" || l.kind === "solid_fill") && !requested.has(`${doc}:${l.id}`))
    .map((l) => l.id);
  if (!ids.length) return;
  for (const id of ids) requested.add(`${doc}:${id}`);
  bridge.send({ type: UI.REQUEST_THUMBNAILS, doc, layers: ids, size: THUMB_SIZE });
}

/* ------------------------------------------------------------------ adjustments (M2-T04 step 3) */

// Dialog ↔ `Adjustment` mapping for the adjustments with a slider dialog.
// Every change is sent live as `set_adjustment`; Cancel restores the original.
const ADJUSTMENT_DIALOGS = {
  brightness_contrast: {
    dialog: "brightness-contrast",
    toValues: (a) => ({ "Brightness:": a.brightness, "Contrast:": a.contrast, "Use Legacy": a.legacy }),
    fromValues: (v) => ({ kind: "brightness_contrast", brightness: v["Brightness:"], contrast: v["Contrast:"], legacy: !!v["Use Legacy"] }),
  },
  hue_saturation: {
    dialog: "hue-saturation",
    toValues: (a) => ({ "Hue:": a.hue, "Saturation:": a.saturation, "Lightness:": a.lightness, Colorize: a.colorize }),
    fromValues: (v) => ({ kind: "hue_saturation", hue: v["Hue:"], saturation: v["Saturation:"], lightness: v["Lightness:"], colorize: !!v.Colorize }),
  },
  exposure: {
    dialog: "exposure",
    // The dialog's sliders are integers: offset in hundredths, gamma in hundredths.
    toValues: (a) => ({ "Exposure:": a.exposure, "Offset:": Math.round(a.offset * 100), "Gamma Correction:": Math.round(a.gamma * 100) }),
    fromValues: (v) => ({ kind: "exposure", exposure: v["Exposure:"], offset: v["Offset:"] / 100, gamma: Math.max(0.01, v["Gamma Correction:"] / 100) }),
  },
};

// Levels and Curves hold one setting per channel (0 = composite, 1..3 = R, G, B);
// the dialog's Channel menu picks the one its fields show.
const CHANNELS = ["RGB", "Red", "Green", "Blue"];
const PER_CHANNEL_DIALOGS = {
  levels: {
    dialog: "levels",
    toValues: (c) => ({
      "Input Black:": Math.round(c.in_black * 255), "Gamma (x100):": Math.round(c.gamma * 100), "Input White:": Math.round(c.in_white * 255),
      "Output Black:": Math.round(c.out_black * 255), "Output White:": Math.round(c.out_white * 255),
    }),
    fromValues: (v) => {
      const inBlack = v["Input Black:"];
      const inWhite = Math.max(inBlack + 2, v["Input White:"]);
      return { in_black: inBlack / 255, in_white: inWhite / 255, gamma: Math.max(0.01, v["Gamma (x100):"] / 100), out_black: v["Output Black:"] / 255, out_white: v["Output White:"] / 255 };
    },
  },
  curves: {
    dialog: "curves",
    toValues: (points) => ({ curve: points.length < 2 ? [[0, 0], [1, 1]] : points }),
    fromValues: (v) => v.curve,
  },
};

function editAdjustment(l) {
  const adj = l.adjustment;
  if (!adj) return;
  if (PER_CHANNEL_DIALOGS[adj.kind]) { editPerChannel(l, PER_CHANNEL_DIALOGS[adj.kind]); return; }
  const spec = ADJUSTMENT_DIALOGS[adj.kind];
  if (!spec) {
    toast(adj.kind === "invert" ? "Invert has no settings" : "This adjustment has no dialog yet");
    return;
  }
  const original = adj;
  const set = (adjustment) => send({ op: "set_adjustment", layer: ref(l.id), adjustment });
  // "Preview" off shows the layer as it was; OK still applies the dialog's values.
  const preview = (values) => set(values.Preview === false ? original : spec.fromValues(values));
  openDialog(spec.dialog, {
    title: `${l.name}`,
    values: spec.toValues(adj),
    onChange: preview,
    onOk: (values) => set(spec.fromValues(values)),
    onCancel: () => set(original),
  });
}

function editPerChannel(l, spec) {
  const original = l.adjustment;
  const channels = original.channels.map((c) => JSON.parse(JSON.stringify(c)));
  let shown = 0;
  const set = (adjustment) => send({ op: "set_adjustment", layer: ref(l.id), adjustment });
  const current = () => ({ kind: original.kind, channels });
  openDialog(spec.dialog, {
    title: `${l.name}`,
    values: { "Channel:": CHANNELS[0], ...spec.toValues(channels[0]) },
    onChange: (values, dialog) => {
      const picked = CHANNELS.indexOf(values["Channel:"]);
      if (picked >= 0 && picked !== shown) {
        shown = picked;
        dialog.set(spec.toValues(channels[shown]));
        return;
      }
      channels[shown] = spec.fromValues(values);
      set(values.Preview === false ? original : current());
    },
    onOk: () => set(current()),
    onCancel: () => set(original),
  });
}

/* ------------------------------------------------------------------ History panel */

/** The History panel content (persistent, updated in place). */
export function historyPanel() {
  if (!historyRoot) {
    historyRoot = h("div", { class: "phistory native" });
    renderHistory();
  }
  return historyRoot;
}

function renderHistory() {
  if (!historyRoot) return;
  clear(historyRoot);
  const list = h("div", { class: "plist" });
  if (doc == null) {
    list.append(h("div", { class: "pempty", text: "No document." }));
  } else {
    const labels = history ? history.labels : [];
    const current = history ? history.current : 0;
    ["Open", ...labels].forEach((label, i) => {
      const r = h("div", {
        class: "plist-row" + (i === current ? " sel" : "") + (i > current ? " undone" : ""),
        onclick: () => jump(i - current),
      },
      h("span", { class: "pthumb hist" }, icon(i === 0 ? "i-image" : "i-brush", "ic sm")),
      h("span", { class: "plist-label", text: label }));
      list.append(r);
    });
  }
  historyRoot.append(list, h("div", { class: "pbar" },
    barBtn("i-undo", "Step backward (Ctrl+Z)", () => jump(-1)),
    barBtn("i-redo", "Step forward (Ctrl+Shift+Z)", () => jump(1)),
  ));
  // Keep the current state in view.
  requestAnimationFrame(() => list.querySelector(".sel")?.scrollIntoView({ block: "nearest" }));
}

function jump(steps) {
  if (doc == null || !steps) return;
  const type = steps < 0 ? UI.UNDO : UI.REDO;
  for (let n = 0; n < Math.abs(steps); n++) bridge.send({ type, doc });
}

/* ------------------------------------------------------------------ colours */

function hexToRgba16(hex) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(hex || "");
  if (!m) return [0, 0, 0, 65535];
  return [parseInt(m[1], 16) * 257, parseInt(m[2], 16) * 257, parseInt(m[3], 16) * 257, 65535];
}

function rgba16ToCss([r, g, b, a]) {
  return `rgba(${Math.round(r / 257)}, ${Math.round(g / 257)}, ${Math.round(b / 257)}, ${(a / 65535).toFixed(3)})`;
}
