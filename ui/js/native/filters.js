// Fotox — filter dialogs in native mode (M4-T05).
//
// While a filter dialog is open, every change sends `filter_preview`: the
// engine shows the active layer filtered on the visible area, live. OK sends
// the `apply_filter` command (the engine runs it as a job, with progress);
// Cancel, or the dialog's Preview box off, sends `filter_preview_cancel`.

import { dialogValues, openDialog } from "../dialogs.js";
import { dialogDef } from "../data/dialogs.js";
import { toast } from "../tooltip.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";
import { activeDocument } from "./documents.js";
import { activeLayerId, activeLayerKind } from "./layers-panel.js";

// Dialog id → the engine's `FilterParams` from the dialog's values (by label).
const FILTERS = {
  "gaussian-blur": (v) => ({ kind: "gaussian_blur", radius: clamp(v["Radius:"], 0.1, 1000) }),
  "unsharp-mask": (v) => ({
    kind: "unsharp_mask",
    amount: clamp(v["Amount:"], 1, 500),
    radius: clamp(v["Radius:"], 0.1, 1000),
    threshold: Math.round(clamp(v["Threshold:"], 0, 255)),
  }),
};

const clamp = (value, lo, hi) => Math.min(hi, Math.max(lo, Number.isFinite(value) ? value : lo));

/** True when `dialogId` is a filter the engine implements. */
export function isEngineFilter(dialogId) {
  return Object.prototype.hasOwnProperty.call(FILTERS, dialogId);
}

/** Open a filter dialog wired to the engine's live preview. */
export function openFilterDialog(dialogId) {
  const doc = activeDocument();
  const layer = activeLayerId();
  if (doc == null || layer == null) { toast("Open a document and select a layer first"); return; }
  if (activeLayerKind() !== "pixel") { toast("Filters work on pixel layers: select one"); return; }
  const params = FILTERS[dialogId];
  const preview = (values) => {
    if (values.Preview === false) bridge.send({ type: UI.FILTER_PREVIEW_CANCEL, doc });
    else bridge.send({ type: UI.FILTER_PREVIEW, doc, layer, filter: params(values) });
  };
  const dialog = openDialog(dialogId, {
    // The mock's thumbnail box: the preview is the document itself here.
    fields: (dialogDef(dialogId).fields || []).filter((f) => f.type !== "preview"),
    onChange: preview,
    onOk: (values) => bridge.send({ type: UI.COMMAND, doc, command: { op: "apply_filter", layer: { id: layer }, filter: params(values) } }),
    onCancel: () => bridge.send({ type: UI.FILTER_PREVIEW_CANCEL, doc }),
  });
  // Show the starting values filtered right away.
  preview(dialogValues(dialog));
}
