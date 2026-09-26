// Fotox — Preferences and Open Recent in the app (M7-T09, D-062).
//
// The engine owns `preferences.json` and sends it as `preferences`; the
// dialogs send `prefs:set` with the changed keys. File ▸ Open Recent is
// rebuilt from the `recent` list.
//
// Preferences ▸ AI (M13-T01/T06): ONNX Runtime and the models (download
// with the size shown first, delete), and the ComfyUI bridge (address,
// checkpoint, workflow files, Test). The engine adds `_ai` to the message.

import { openDialog } from "../dialogs.js";
import { h } from "../el.js";
import { menus } from "../data/menus.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";

let prefs = {};
/** The AI page's models block while the dialog is open (re-rendered on news). */
let aiModels = null;

export function initPrefs() {
  bridge.on(ENGINE.PREFERENCES, (m) => {
    prefs = m.prefs || {};
    rebuildRecent(prefs.recent || []);
    if (aiModels && aiModels.isConnected) renderModels(aiModels);
  });
}

function rebuildRecent(recent) {
  const file = menus.find((m) => m.label === "File");
  const item = file && file.items.find((i) => i.label === "Open Recent");
  if (!item) return;
  const name = (p) => p.split(/[\\/]/).pop();
  item.sub = recent.length
    ? [...recent.map((p, i) => ({ label: name(p), a: `doc:open-recent:${i}`, tip: p })), { sep: true }, { label: "Clear Recent", a: "misc:clear-recent" }]
    : [{ label: "No recent files", a: "", dis: true }];
}

export function isPrefsDialog(id) {
  return id === "prefs" || id === "prefs-performance" || id === "prefs-guides" || id === "prefs-ai";
}

const set = (patch) => bridge.send({ type: UI.ACTION, id: "prefs:set", args: patch });
const act = (id, args) => bridge.send({ type: UI.ACTION, id, args: args || {} });

function renderModels(box) {
  const ai = prefs._ai || {};
  box.replaceChildren(
    h("div", { class: "dlg-label", text: ai.runtime ? `ONNX Runtime: ${ai.runtime}` : "ONNX Runtime not found — put onnxruntime.dll next to Fotox or set FOTOX_ORT_DYLIB" }),
    h("div", { class: "dlg-label", text: `Models folder: ${ai.folder || "?"}` }),
    ...(ai.models || []).map((m) =>
      h("div", { class: "dlg-line" },
        h("span", { class: "dlg-field-label", text: m.name }),
        h("span", { class: "dlg-note", text: `${m.purpose} · ${m.licence} · ${m.mb} MB · ${m.installed ? "installed" : "not installed"}` }),
        m.installed
          ? h("button", { class: "btn small", type: "button", text: "Delete", onclick: () => act("ai:delete", { id: m.id }) })
          : h("button", { class: "btn small", type: "button", text: `Download ${m.mb} MB`, onclick: () => act("ai:download", { id: m.id }) }))),
  );
}

function input(value, width, placeholder) {
  return h("input", { class: "dlg-input", type: "text", value: value ?? "", placeholder: placeholder || "", style: { width: (width || 260) + "px" } });
}

function row(label, el) {
  return h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: label }), el);
}

function openAiPrefs(id) {
  aiModels = h("div", { class: "dlg-group" });
  renderModels(aiModels);
  const address = input(prefs.comfy_address, 220, "auto: 127.0.0.1:8000, then :8188");
  const checkpoint = input(prefs.comfy_checkpoint, 260, "auto: an inpainting or SDXL checkpoint");
  const negative = input(prefs.comfy_negative, 260, "blurry, low quality, watermark, text");
  const steps = input(prefs.comfy_steps ?? 25, 60);
  const cfg = input(prefs.comfy_cfg ?? 6, 60);
  const variations = input(prefs.comfy_variations ?? 3, 60);
  const fill = input(prefs.comfy_workflow_fill, 260, "bundled inpaint workflow");
  const expand = input(prefs.comfy_workflow_expand, 260, "bundled outpaint workflow");
  const comfy = h("div", { class: "dlg-group" },
    h("div", { class: "dlg-group-title", text: "Generative Fill / Expand — local ComfyUI" }),
    row("Address:", address),
    h("div", { class: "dlg-line" }, h("span", { class: "dlg-field-label", text: "" }),
      h("button", { class: "btn small", type: "button", text: "Test connection", onclick: () => act("comfy:test", { address: address.value }) })),
    row("Checkpoint:", checkpoint),
    row("Negative prompt:", negative),
    row("Steps:", steps), row("CFG:", cfg), row("Variations:", variations),
    row("Fill workflow (API JSON):", fill),
    row("Expand workflow (API JSON):", expand),
    h("div", { class: "dlg-label", text: "Nothing leaves this machine unless the address points elsewhere." }));
  openDialog(id, {
    title: "Preferences — AI Models & ComfyUI",
    width: 640,
    fields: [
      { type: "element", el: h("div", { class: "dlg-group" }, h("div", { class: "dlg-group-title", text: "Models (local, ONNX Runtime)" }), aiModels) },
      { type: "element", el: comfy },
    ],
    onOk: () => {
      const num = (el, lo, hi, def) => Math.min(hi, Math.max(lo, Number(el.value) || def));
      set({
        comfy_address: address.value.trim(),
        comfy_checkpoint: checkpoint.value.trim(),
        comfy_negative: negative.value.trim() || undefined,
        comfy_steps: num(steps, 1, 150, 25),
        comfy_cfg: num(cfg, 1, 30, 6),
        comfy_variations: Math.round(num(variations, 1, 8, 3)),
        comfy_workflow_fill: fill.value.trim(),
        comfy_workflow_expand: expand.value.trim(),
      });
    },
  });
  // The runtime and models as they are now.
  act("ai:status");
}

export function openPrefsDialog(id) {
  if (id === "prefs-ai") {
    openAiPrefs(id);
    return;
  }
  if (id === "prefs-guides") {
    openDialog(id, {
      title: "Preferences — Guides & Grid",
      fields: [
        { type: "num", label: "Gridline Every:", value: prefs.grid_every ?? 100, unit: "px", w: 70 },
        { type: "num", label: "Subdivisions:", value: prefs.subdivisions ?? 4, w: 60 },
      ],
      onOk: (v) => set({ grid_every: Math.max(1, Number(v["Gridline Every:"]) || 100), subdivisions: Math.max(1, Math.round(Number(v["Subdivisions:"]) || 4)) }),
    });
    return;
  }
  // General / Performance (FAST: one page; the scratch folder is shown, not edited).
  openDialog(id, {
    title: "Preferences — Performance",
    fields: [
      { type: "num", label: "Memory Budget (MB):", value: prefs.memory_budget_mb ?? 4096, w: 80 },
      { type: "label", text: `Scratch folder: ${prefs.scratch_dir || "default"} — applies at the next start` },
    ],
    onOk: (v) => set({ memory_budget_mb: Math.max(256, Math.round(Number(v["Memory Budget (MB):"]) || 4096)) }),
  });
}
