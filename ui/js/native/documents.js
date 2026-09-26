// Fotox — document tabs in native mode (M1-T08).
//
// The engine owns the documents: tabs are created, updated and removed from
// its `document_opened` / `document_changed` / `document_closed` /
// `active_document` / `view` messages. A tab click sends
// `activate_document`, its close button `close_document`.

import { h, icon } from "../el.js";
import { state } from "../state.js";
import { status } from "../tooltip.js";
import { openDialog } from "../dialogs.js";
import * as bridge from "./bridge.js";
import { UI, ENGINE } from "./protocol.js";

const docs = new Map(); // doc id → { info, zoom, tab }
let active = null;
let strip = null;
let addButton = null;

/** Take over the tab strip `tabs`; `add` is the "+" button kept at its end. */
export function initDocumentTabs(tabs, add) {
  strip = tabs;
  addButton = add;
  strip.replaceChildren(addButton);

  bridge.on(ENGINE.DOCUMENT_OPENED, ({ info }) => {
    const tab = h("div", { class: "doctab", "data-doc": String(info.doc), onclick: () => bridge.send({ type: UI.ACTIVATE_DOCUMENT, doc: info.doc }) },
      icon("i-image", "ic sm"),
      h("span", { class: "doctab-label" }),
      h("button", {
        class: "doctab-x", type: "button", "data-tip": "Close document",
        onclick: (e) => { e.stopPropagation(); bridge.send({ type: UI.CLOSE_DOCUMENT, doc: info.doc }); },
      }, icon("i-close", "ic xs")));
    strip.insertBefore(tab, addButton);
    docs.set(info.doc, { info, zoom: null, tab });
    refresh(info.doc);
  });

  bridge.on(ENGINE.DOCUMENT_CHANGED, ({ info }) => {
    const d = docs.get(info.doc);
    if (!d) return;
    d.info = info;
    refresh(info.doc);
  });

  bridge.on(ENGINE.DOCUMENT_CLOSED, ({ doc }) => {
    const d = docs.get(doc);
    if (!d) return;
    d.tab.remove();
    docs.delete(doc);
  });

  bridge.on(ENGINE.ACTIVE_DOCUMENT, ({ doc }) => {
    active = doc;
    for (const [id, d] of docs) d.tab.classList.toggle("active", id === doc);
    const d = doc == null ? null : docs.get(doc);
    const size = document.getElementById("statusdocsize");
    if (size) size.textContent = d ? `${d.info.width} × ${d.info.height} px` : "";
  });

  bridge.on(ENGINE.VIEW, (view) => {
    const d = docs.get(view.doc);
    if (!d) return;
    d.zoom = view.zoom * 100;
    refresh(view.doc);
  });

  // Closing an unsaved document: Save / Don't Save / Cancel (M3-T06).
  bridge.on(ENGINE.CLOSE_DIRTY_DOCUMENT, ({ doc, name }) => {
    const answer = (value) => bridge.send({ type: UI.CLOSE_DOCUMENT_ANSWER, doc, answer: value });
    openDialog("save-changes", {
      fields: [{ type: "label", text: `Save changes to “${name}” before closing?` }],
      buttons: [
        { text: "Save", primary: true, onClick: () => answer("save") },
        { text: "Don't Save", onClick: () => answer("dont_save") },
        { text: "Cancel", onClick: () => answer("cancel") },
      ],
      onCancel: () => answer("cancel"),
    });
  });

  // Long jobs (import, save, export): progress in the status bar.
  bridge.on(ENGINE.PROGRESS, ({ label, fraction }) => status(`${label}… ${Math.round(fraction * 100)} %`));
  bridge.on(ENGINE.PROGRESS_DONE, () => status("Ready"));
}

/** The id of the active document, or null. */
export function activeDocument() {
  return active;
}

/** What the engine last reported about the active document (size, ppi, …), or null. */
export function activeDocumentInfo() {
  const d = active == null ? null : docs.get(active);
  return d ? d.info : null;
}

function refresh(id) {
  const d = docs.get(id);
  if (!d) return;
  const { info } = d;
  const zoom = d.zoom == null ? "" : ` @ ${formatZoom(d.zoom)}%`;
  const bits = info.depth === "u16" || info.depth === "U16" ? 16 : 8;
  const label = `${info.name}${zoom} (RGB/${bits})${info.dirty ? "*" : ""}`;
  d.tab.querySelector(".doctab-label").textContent = label;
  d.tab.title = `${info.name} — ${info.width} × ${info.height} px, ${info.profile_name}, ${Math.round(info.ppi)} ppi`;
  if (id === active) state.doc = { ...state.doc, name: info.name, w: info.width, h: info.height, bits };
}

function formatZoom(z) {
  return z < 10 ? String(Math.round(z * 100) / 100) : String(Math.round(z));
}
