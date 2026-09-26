// Fotox — File ▸ New in the app (M7-T01, D-058).
//
// The dialog collects size, resolution, depth and background and sends one
// `doc:new` action; the engine builds a document of Solid tiles, so any size
// costs no pixel memory.

import { openDialog } from "../dialogs.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";

// FAST: no Clipboard preset yet (the shell does not report the clipboard's size).
const PRESETS = {
  "Last Used": null,
  "Web 1920 × 1080 (72 ppi)": [1920, 1080, 72],
  "A4 (300 ppi)": [2480, 3508, 300],
  "4K UHD (72 ppi)": [3840, 2160, 72],
  "30 000² (B3 test)": [30000, 30000, 72],
};
const BACKGROUNDS = { White: "white", Black: "black", "Background Color": "background", Transparent: "transparent" };

let last = { "Width:": 1920, "Height:": 1080, "Resolution:": 72, "Bit Depth:": "8 bit", "Background Contents:": "White" };

export function openNewDocument() {
  openDialog("new-doc", {
    width: 440,
    wide: false,
    fields: [
      { type: "select", label: "Preset:", options: Object.keys(PRESETS), value: "Last Used" },
      { type: "num", label: "Width:", value: last["Width:"], unit: "px", w: 90 },
      { type: "num", label: "Height:", value: last["Height:"], unit: "px", w: 90 },
      { type: "num", label: "Resolution:", value: last["Resolution:"], unit: "ppi", w: 90 },
      { type: "select", label: "Bit Depth:", options: ["8 bit", "16 bit"], value: last["Bit Depth:"] },
      { type: "select", label: "Background Contents:", options: Object.keys(BACKGROUNDS), value: last["Background Contents:"] },
    ],
    onChange: (values, dialog) => {
      const preset = PRESETS[values["Preset:"]];
      if (preset && (values["Width:"] !== preset[0] || values["Height:"] !== preset[1] || values["Resolution:"] !== preset[2])) {
        dialog.set({ "Width:": preset[0], "Height:": preset[1], "Resolution:": preset[2] });
        // FAST: writeValues only fills sliders/menus; set the number boxes directly.
        document.querySelectorAll(".dialog .dlg-line.inline").forEach((line) => {
          const label = line.querySelector(".dlg-field-label")?.textContent;
          const input = line.querySelector(".dlg-input.num");
          const index = { "Width:": 0, "Height:": 1, "Resolution:": 2 }[label];
          if (input && index !== undefined) input.value = preset[index];
        });
      }
    },
    onOk: (v) => {
      last = v;
      bridge.send({
        type: UI.ACTION,
        id: "doc:new",
        args: {
          width: Math.max(1, Math.round(Number(v["Width:"]) || 1920)),
          height: Math.max(1, Math.round(Number(v["Height:"]) || 1080)),
          ppi: Number(v["Resolution:"]) || 72,
          depth: v["Bit Depth:"] === "16 bit" ? 16 : 8,
          background: BACKGROUNDS[v["Background Contents:"]] || "white",
        },
      });
    },
  });
}
