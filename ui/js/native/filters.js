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
  // M12-T03b (D-084).
  "box-blur": (v) => ({ kind: "box_blur", radius: clamp(v["Radius:"], 1, 2000) }),
  "motion-blur": (v) => ({ kind: "motion_blur", angle: clamp(v["Angle:"], -360, 360), distance: clamp(v["Distance:"], 1, 2000) }),
  "radial-blur": (v) => ({ kind: "radial_blur", amount: clamp(v["Amount:"], 1, 100), zoom: v["Blur Method:"] === "Zoom" || v["Blur Method:"] === 1 }),
  "surface-blur": (v) => ({ kind: "surface_blur", radius: clamp(v["Radius:"], 1, 100), threshold: clamp(v["Threshold:"], 2, 255) }),
  "add-noise": (v) => ({
    kind: "add_noise", amount: clamp(v["Amount:"], 0.1, 400),
    gaussian: v["Distribution:"] === "Gaussian" || v["Distribution:"] === 1, monochromatic: !!v.Monochromatic, seed: 1,
  }),
  median: (v) => ({ kind: "median", radius: clamp(v["Radius:"], 1, 500) }),
  "dust-scratches": (v) => ({ kind: "dust_scratches", radius: clamp(v["Radius:"], 1, 500), threshold: clamp(v["Threshold:"], 0, 255) }),
  emboss: (v) => ({ kind: "emboss", angle: clamp(v["Angle:"], -360, 360), height: clamp(v["Height:"], 1, 100), amount: clamp(v["Amount:"], 1, 500) }),
  "high-pass": (v) => ({ kind: "high_pass", radius: clamp(v["Radius:"], 0.1, 1000) }),
  maximum: (v) => ({ kind: "maximum", radius: clamp(v["Radius:"], 1, 500) }),
  minimum: (v) => ({ kind: "minimum", radius: clamp(v["Radius:"], 1, 500) }),
  offset: (v) => {
    const mode = v["Undefined Areas:"];
    const m = typeof mode === "number" ? mode : ["Set to Transparent", "Repeat Edge Pixels", "Wrap Around"].indexOf(mode);
    return { kind: "offset", dx: Number(v["Horizontal:"]) || 0, dy: Number(v["Vertical:"]) || 0, mode: Math.max(0, m) };
  },
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
  // A Smart Object takes the filter as a Smart Filter (M12-T03).
  if (activeLayerKind() !== "pixel" && activeLayerKind() !== "smart") { toast("Filters work on pixel layers: select one"); return; }
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
