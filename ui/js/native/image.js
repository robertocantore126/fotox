// Fotox — the image geometry dialogs in native mode (M6-T02).
//
// Image ▸ Image Size (Alt+Ctrl+I), Image ▸ Canvas Size (Alt+Ctrl+C) and
// Image ▸ Image Rotation ▸ Arbitrary… collect their values here and send **one**
// command each; the engine does the work as a job (recipe R2) and reports
// progress in the status bar.
//
// The dialogs show exactly what the engine takes: pixels and ppi on the two
// size dialogs, an angle in degrees plus a direction on Rotate. The mock's
// derived rows are not shown — inches and millimetres, the unit menu — until
// T09 makes them live (Canvas Size rewrites no pixel at all, D-015, so an
// extension colour has nothing to fill either).

import { openDialog } from "../dialogs.js";
import { toast } from "../tooltip.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";
import { activeDocument, activeDocumentInfo } from "./documents.js";

/** The interpolation menu's names → the engine's `Filter` (D-052). */
const FILTERS = {
  Automatic: "bicubic_automatic",
  "Preserve Details 2.0": "bicubic_automatic",
  "Bicubic Sharper (reduction)": "bicubic_sharper",
  "Bicubic Smoother (enlargement)": "bicubic_smoother",
  "Bicubic (smooth gradients)": "bicubic",
  "Nearest Neighbor (hard edges)": "nearest",
  Bilinear: "bilinear",
};

/** The Canvas Size anchor grid, in the order the dialog draws it (4 = centre). */
const ANCHORS = ["top_left", "top", "top_right", "left", "center", "right", "bottom_left", "bottom", "bottom_right"];

const whole = (value, fallback) => (Number.isFinite(Number(value)) ? Math.round(Number(value)) : fallback);
const positive = (value, fallback) => (Number(value) > 0 ? Number(value) : fallback);

/** True for the dialogs this module runs. */
export function isImageDialog(id) {
  return id === "image-size" || id === "canvas-size" || id === "rotate-arbitrary";
}

/** Open Image Size, Canvas Size or Rotate Arbitrary, wired to the engine. */
export function openImageDialog(id) {
  const doc = activeDocument();
  if (doc == null) { toast("Open a document first"); return; }
  const info = activeDocumentInfo();
  if (!info) { toast("The document size is not known yet"); return; }
  const command = (command) => bridge.send({ type: UI.COMMAND, doc, command });
  const sizeRow = (width, height) => ({
    type: "row",
    fields: [
      { type: "num", label: "Width:", value: width, unit: "px", w: 90 },
      { type: "locksize" },
      { type: "num", label: "Height:", value: height, unit: "px", w: 90 },
      { type: "chain", text: "px" },
    ],
  });

  if (id === "image-size") {
    openDialog(id, {
      fields: [
        { type: "label", text: `Pixel Dimensions: ${info.width} × ${info.height} px` },
        sizeRow(info.width, info.height),
        { type: "row", fields: [
          { type: "num", label: "Resolution:", value: Math.round(info.ppi), w: 90 },
          { type: "chain", text: "pixels/inch" },
        ] },
        { type: "check", label: "Resample", on: true },
        // The interpolation menu, unlabelled as in Photoshop: `values[""]`.
        { type: "select", label: "", options: Object.keys(FILTERS), value: "Bicubic (smooth gradients)" },
      ],
      onOk: (v) => {
        const ppi = positive(v["Resolution:"], info.ppi);
        // "Resample" off: the print size changes and the pixels do not, so the
        // engine gets the current pixel dimensions (it refuses anything else).
        const resample = v["Resample"] ? FILTERS[v[""]] || "bicubic_automatic" : null;
        command({
          op: "image_size",
          width: resample ? whole(v["Width:"], info.width) : info.width,
          height: resample ? whole(v["Height:"], info.height) : info.height,
          ppi,
          resample,
        });
      },
    });
    return;
  }

  if (id === "canvas-size") {
    openDialog(id, {
      fields: [
        { type: "label", text: `Current Size: ${info.width} × ${info.height} px` },
        { type: "group", label: "New Size", fields: [sizeRow(info.width, info.height)] },
        { type: "anchor", label: "Anchor:" },
      ],
      onOk: (v) => command({
        op: "canvas_size",
        width: whole(v["Width:"], info.width),
        height: whole(v["Height:"], info.height),
        anchor: ANCHORS[v["Anchor:"]] || "center",
      }),
    });
    return;
  }

  // Rotate Arbitrary: the engine takes degrees clockwise, so the direction
  // flips the sign. 0° — and any whole turn — would leave the canvas as it is,
  // exactly as Photoshop's OK button does there.
  openDialog(id, {
    onOk: (v) => {
      const angle = Number(v["Angle:"]);
      if (!Number.isFinite(angle) || angle % 360 === 0) return;
      const direction = v["Rotate:"] === "Counter Clockwise" ? -1 : 1;
      command({
        op: "rotate_canvas_arbitrary",
        angle_deg: direction * angle,
        filter: "bicubic",
      });
    },
  });
}
