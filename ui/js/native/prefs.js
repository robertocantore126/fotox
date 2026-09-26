// Fotox — Preferences and Open Recent in the app (M7-T09, D-062).
//
// The engine owns `preferences.json` and sends it as `preferences`; the
// dialogs send `prefs:set` with the changed keys. File ▸ Open Recent is
// rebuilt from the `recent` list.

import { openDialog } from "../dialogs.js";
import { menus } from "../data/menus.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";

let prefs = {};

export function initPrefs() {
  bridge.on(ENGINE.PREFERENCES, (m) => {
    prefs = m.prefs || {};
    rebuildRecent(prefs.recent || []);
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
  return id === "prefs" || id === "prefs-performance" || id === "prefs-guides";
}

const set = (patch) => bridge.send({ type: UI.ACTION, id: "prefs:set", args: patch });

export function openPrefsDialog(id) {
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
