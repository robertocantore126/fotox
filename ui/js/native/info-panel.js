// Fotox — the Info and Notes panels in the app (M9-T08).
//
// The engine sends each document's annotations (`annotations`) with the
// colour samplers' current values after every edit; the Ruler's measurement
// arrives as the tool's status line.

import { h, clear } from "../el.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { activeDocument } from "./documents.js";

const byDoc = new Map(); // doc → { annotations, samples }
let infoRoot = null;
let notesRoot = null;
let measure = "";

function current() {
  return byDoc.get(activeDocument()) || { annotations: { notes: [], counts: [], samplers: [] }, samples: [] };
}

export function infoPanel() {
  if (!infoRoot) infoRoot = h("div", { class: "pinfo native" });
  renderInfo();
  return infoRoot;
}

function renderInfo() {
  if (!infoRoot) return;
  clear(infoRoot);
  const { annotations, samples } = current();
  const rows = h("div", { class: "pinfo-rows" });
  (annotations.samplers || []).forEach((s, i) => {
    const v = samples[i] || [0, 0, 0, 0];
    const rgb = v.slice(0, 3).map((c) => Math.round(c / 257));
    rows.append(h("div", { class: "pinfo-row" },
      h("span", { class: "pinfo-k", text: `#${i + 1}` }),
      h("span", { class: "pinfo-v", text: `R ${rgb[0]}  G ${rgb[1]}  B ${rgb[2]}` }),
      h("span", { class: "pinfo-v", style: { background: `rgb(${rgb.join(",")})`, width: "14px", height: "14px", display: "inline-block", borderRadius: "2px" } })));
  });
  const counts = (annotations.counts || []).map((g) => `${g.name}: ${g.points.length}`).join(" · ");
  infoRoot.append(rows,
    measure ? h("div", { class: "pinfo-note", text: measure }) : null,
    counts ? h("div", { class: "pinfo-note", text: counts }) : null,
    !rows.children.length && !measure && !counts ? h("div", { class: "pnote", text: "Color samplers (I), the Ruler and the Count tool report here." }) : null);
}

export function notesPanel() {
  if (!notesRoot) notesRoot = h("div", { class: "pnotes native" });
  renderNotes();
  return notesRoot;
}

function renderNotes() {
  if (!notesRoot) return;
  clear(notesRoot);
  const notes = current().annotations.notes || [];
  if (!notes.length) notesRoot.append(h("div", { class: "pnote", text: "No notes. Click the image with the Note tool." }));
  notes.forEach((n, i) => {
    const area = h("textarea", { class: "dlg-input area", rows: 3, style: { width: "100%" } });
    area.value = n.text;
    area.addEventListener("change", () => bridge.send({ type: UI.ACTION, id: "notes:set", args: { index: i, text: area.value } }));
    notesRoot.append(h("div", { class: "pnote-item" },
      h("div", { class: "pinfo-k", text: `Note ${i + 1}${n.author ? " — " + n.author : ""}` }),
      area,
      h("button", { class: "btn small", type: "button", text: "Delete", onclick: () => bridge.send({ type: UI.ACTION, id: "notes:delete", args: { index: i } }) })));
  });
}

export function initInfo() {
  bridge.on(ENGINE.ANNOTATIONS, (m) => {
    byDoc.set(m.doc, { annotations: m.annotations || {}, samples: m.samples || [] });
    if (m.doc === activeDocument()) {
      renderInfo();
      renderNotes();
    }
  });
  bridge.on(ENGINE.TOOL_INFO, ({ text }) => {
    if (text && text.includes(" A: ")) { measure = text; renderInfo(); }
  });
  bridge.on(ENGINE.ACTIVE_DOCUMENT, () => setTimeout(() => { renderInfo(); renderNotes(); }, 0));
}
