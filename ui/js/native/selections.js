// Fotox — M9's selection dialogs in the app: Color Range (T03), Focus Area
// (T04), Select and Mask (T05). Each OK sends one `select_by` command; the
// engine computes the coverage as a job.
//
// FAST: no live preview in the dialogs yet (the result shows as marching
// ants after OK); Color Range samples the foreground (and background) swatch
// instead of an eyedropper on the image.

import { openDialog } from "../dialogs.js";
import { state } from "../state.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";
import { activeDocument } from "./documents.js";

const hex = (s) => [1, 3, 5].map((i) => parseInt(String(s).slice(i, i + 2), 16) / 255);

function selectBy(select, mode = "replace") {
  const doc = activeDocument();
  if (doc == null) return;
  bridge.send({ type: UI.COMMAND, doc, command: { op: "select_by", select, mode } });
}

const RANGES = {
  "Sampled Colors": "sampled", Reds: "reds", Yellows: "yellows", Greens: "greens", Cyans: "cyans", Blues: "blues", Magentas: "magentas",
  Highlights: "highlights", Midtones: "midtones", Shadows: "shadows", "Skin Tones": "skin_tones",
};

let lastRange = { "Select:": "Sampled Colors", "Sample:": "Foreground Colour", "Fuzziness:": 40, "Shadows below:": 65, "Highlights above:": 190, Invert: false };

function openColorRange() {
  openDialog("color-range", {
    width: 420,
    wide: false,
    fields: [
      { type: "select", label: "Select:", options: Object.keys(RANGES), value: lastRange["Select:"] },
      { type: "select", label: "Sample:", options: ["Foreground Colour", "Foreground and Background"], value: lastRange["Sample:"] },
      { type: "range", label: "Fuzziness:", value: lastRange["Fuzziness:"], min: 0, max: 200 },
      { type: "range", label: "Shadows below:", value: lastRange["Shadows below:"], min: 0, max: 255 },
      { type: "range", label: "Highlights above:", value: lastRange["Highlights above:"], min: 0, max: 255 },
      { type: "check", label: "Invert", value: lastRange.Invert },
    ],
    onOk: (v) => {
      lastRange = v;
      const samples = [hex(state.colors.fg)];
      if (v["Sample:"] === "Foreground and Background") samples.push(hex(state.colors.bg));
      selectBy({
        kind: "color_range",
        range: RANGES[v["Select:"]] || "sampled",
        samples,
        fuzziness: Number(v["Fuzziness:"]) || 40,
        localized: null,
        invert: !!v.Invert,
        low: Number(v["Shadows below:"]) || 65,
        high: Number(v["Highlights above:"]) || 190,
      });
    },
  });
}

let lastFocus = { "In-Focus Range:": 60, "Image Noise Level:": 20, "Soften Edge": true };

function openFocusArea() {
  openDialog("focus-area", {
    width: 400,
    wide: false,
    fields: [
      { type: "range", label: "In-Focus Range:", value: lastFocus["In-Focus Range:"], min: 0, max: 100 },
      { type: "range", label: "Image Noise Level:", value: lastFocus["Image Noise Level:"], min: 0, max: 100 },
      { type: "check", label: "Soften Edge", value: lastFocus["Soften Edge"] },
    ],
    onOk: (v) => {
      lastFocus = v;
      selectBy({ kind: "focus_area", in_focus: (Number(v["In-Focus Range:"]) || 0) / 100, noise: (Number(v["Image Noise Level:"]) || 0) / 100, soften: !!v["Soften Edge"] });
    },
  });
}

let lastRefine = { "Radius:": 10, "Smart Radius": true, "Smooth:": 0, "Feather:": 0, "Contrast:": 0, "Shift Edge:": 0, "Output To:": "Selection" };

/** Select ▸ Select and Mask (T05). FAST: a dialog, not Photoshop's workspace. */
function openSelectMask() {
  openDialog("select-mask", {
    title: "Select and Mask",
    width: 420,
    wide: false,
    fields: [
      { type: "label", text: "Edge Detection" },
      { type: "range", label: "Radius:", value: lastRefine["Radius:"], min: 0, max: 64 },
      { type: "check", label: "Smart Radius", value: lastRefine["Smart Radius"] },
      { type: "label", text: "Global Refinements" },
      { type: "range", label: "Smooth:", value: lastRefine["Smooth:"], min: 0, max: 100 },
      { type: "range", label: "Feather:", value: lastRefine["Feather:"], min: 0, max: 50 },
      { type: "range", label: "Contrast:", value: lastRefine["Contrast:"], min: 0, max: 100 },
      { type: "range", label: "Shift Edge:", value: lastRefine["Shift Edge:"], min: -100, max: 100 },
      { type: "select", label: "Output To:", options: ["Selection", "Layer Mask", "New Layer with Layer Mask"], value: lastRefine["Output To:"] },
    ],
    onOk: (v) => {
      lastRefine = v;
      // The other outputs are made from the refined selection once the job is
      // done: queued first, then the job.
      const out = v["Output To:"];
      if (out !== "Selection") bridge.send({ type: UI.ACTION, id: "select-mask:output", args: { output: out === "Layer Mask" ? "mask" : "layer" } });
      selectBy({
        kind: "refine",
        radius: Number(v["Radius:"]) || 0,
        smart_radius: !!v["Smart Radius"],
        smooth: Number(v["Smooth:"]) || 0,
        feather: Number(v["Feather:"]) || 0,
        contrast: Number(v["Contrast:"]) || 0,
        shift_edge: Number(v["Shift Edge:"]) || 0,
      });
    },
  });
}

export function isSelectionDialog(id) {
  return id === "color-range" || id === "focus-area" || id === "select-mask";
}

export function openSelectionDialog(id) {
  if (id === "color-range") openColorRange();
  else if (id === "focus-area") openFocusArea();
  else openSelectMask();
}
