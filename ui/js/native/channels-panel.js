// Fotox — the Channels panel, Save / Load Selection and Quick Mask in the
// app (M9-T01, D-068).
//
// The engine sends each document's alpha channels (`channels`) with 48 × 48
// thumbnails. RGB and its three channels are listed like Photoshop's; FAST:
// clicking one does not show it as grey yet.

import { h, icon, clear } from "../el.js";
import { emit } from "../state.js";
import { openDialog } from "../dialogs.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { activeDocument } from "./documents.js";

const lists = new Map(); // doc → { channels, quick_mask }
let root = null;
let selected = -1;

function channels() {
  return lists.get(activeDocument()) || { channels: [], quick_mask: false };
}

function command(command) {
  const doc = activeDocument();
  if (doc == null) return;
  bridge.send({ type: UI.COMMAND, doc, command });
}

function grayThumb(b64) {
  const canvas = h("canvas", { class: "pthumb chan", width: 48, height: 48, style: { width: "28px", height: "28px" } });
  if (!b64) return canvas;
  const bytes = atob(b64);
  const ctx = canvas.getContext("2d");
  const img = ctx.createImageData(48, 48);
  for (let i = 0; i < 48 * 48 && i < bytes.length; i++) {
    const v = bytes.charCodeAt(i);
    img.data[i * 4] = img.data[i * 4 + 1] = img.data[i * 4 + 2] = v;
    img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  return canvas;
}

const MODES = { "New Selection": "replace", "Add to Selection": "add", "Subtract from Selection": "subtract", "Intersect with Selection": "intersect" };

export function channelsPanel() {
  if (!root) root = h("div", { class: "pchannels native" });
  render();
  return root;
}

function render() {
  if (!root) return;
  clear(root);
  const { channels: list, quick_mask } = channels();
  const rows = h("div", { class: "plist" });
  [["RGB", "#b9b9bd", "Ctrl+2"], ["Red", "#e26060", "Ctrl+3"], ["Green", "#7ac74f", "Ctrl+4"], ["Blue", "#5b8df5", "Ctrl+5"]].forEach(([name, color, key], i) => {
    rows.append(h("div", { class: "plist-row" + (i === 0 && selected < 0 ? " sel" : "") },
      h("span", { class: "pthumb chan", style: { background: color } }),
      h("span", { class: "plist-label", text: name }),
      h("span", { class: "pmeta", text: key })));
  });
  if (quick_mask) {
    rows.append(h("div", { class: "plist-row sel" }, h("span", { class: "pthumb chan", style: { background: "#c33" } }), h("span", { class: "plist-label", text: "Quick Mask", style: { fontStyle: "italic" } })));
  }
  list.forEach((c, i) => {
    const thumb = grayThumb(c.thumb);
    thumb.addEventListener("click", (e) => {
      // Ctrl+click the thumbnail: load it as the selection (Photoshop).
      if (e.ctrlKey || e.metaKey) { e.stopPropagation(); command({ op: "load_selection", channel: i, invert: false, mode: e.shiftKey ? "add" : e.altKey ? "subtract" : "replace" }); }
    });
    const name = h("span", { class: "plist-label", text: c.name });
    name.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      const value = prompt("Channel name", c.name);
      if (value) command({ op: "set_channel", channel: i, name: value });
    });
    rows.append(h("div", { class: "plist-row" + (i === selected ? " sel" : ""), onclick: () => { selected = i; render(); } },
      thumb, name, h("span", { class: "pmeta", text: "Ctrl+" + (i + 6) })));
  });
  const btn = (ic, tip, fn) => h("button", { class: "pbar-btn", type: "button", "data-tip": tip, onclick: (e) => { e.stopPropagation(); fn(); } }, icon(ic, "ic sm"));
  root.append(rows, h("div", { class: "pbar" },
    btn("i-marquee", "Load channel as selection", () => { if (selected >= 0) command({ op: "load_selection", channel: selected, invert: false, mode: "replace" }); }),
    btn("i-image", "Save selection as channel", () => command({ op: "save_selection", channel: null, name: null, mode: "replace" })),
    btn("i-new-layer", "Duplicate channel", () => { if (selected >= 0) command({ op: "duplicate_channel", channel: selected }); }),
    btn("i-trash", "Delete channel", () => { if (selected >= 0) { command({ op: "delete_channel", channel: selected }); selected = -1; } }),
  ));
}

/* ---------------------------------------------------------------- dialogs */

export function isChannelDialog(id) {
  return id === "save-selection" || id === "load-selection";
}

export function openChannelDialog(id) {
  const names = channels().channels.map((c) => c.name);
  if (id === "save-selection") {
    openDialog("save-selection", {
      fields: [
        { type: "select", label: "Channel:", options: ["New", ...names], value: "New" },
        { type: "text", label: "Name:", value: `Alpha ${names.length + 1}` },
        { type: "select", label: "Operation:", options: ["Replace Channel", "Add to Channel", "Subtract from Channel", "Intersect with Channel"], value: "Replace Channel" },
      ],
      onOk: (v) => {
        const index = names.indexOf(v["Channel:"]);
        const mode = { "Replace Channel": "replace", "Add to Channel": "add", "Subtract from Channel": "subtract", "Intersect with Channel": "intersect" }[v["Operation:"]] || "replace";
        const name = document.querySelector(".dialog .dlg-input:not(.num)")?.value || null;
        command({ op: "save_selection", channel: index >= 0 ? index : null, name, mode });
      },
    });
    return;
  }
  if (!names.length) { emit("mock", "There are no saved selections (alpha channels)"); return; }
  openDialog("load-selection", {
    fields: [
      { type: "select", label: "Channel:", options: names, value: names[0] },
      { type: "check", label: "Invert", value: false },
      { type: "select", label: "Operation:", options: Object.keys(MODES), value: "New Selection" },
    ],
    onOk: (v) => {
      const index = names.indexOf(v["Channel:"]);
      if (index >= 0) command({ op: "load_selection", channel: index, invert: !!v.Invert, mode: MODES[v["Operation:"]] || "replace" });
    },
  });
}

export function initChannels() {
  bridge.on(ENGINE.CHANNELS, (m) => {
    lists.set(m.doc, { channels: m.channels || [], quick_mask: !!m.quick_mask });
    if (m.doc === activeDocument()) render();
  });
  // After documents.js has switched (listeners run in order).
  bridge.on(ENGINE.ACTIVE_DOCUMENT, () => setTimeout(render, 0));
}
