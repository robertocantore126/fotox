// Fotox — Edit ▸ Generative Fill in the app (M13-T06, D-091).
//
// The prompt (it may be empty, as in Photoshop) goes to the engine as
// `ai:generative-fill`; the engine reads the selection and the area around
// it, runs the bundled inpaint workflow on the local ComfyUI and lands the
// variations as a group of masked layers (one history step). Esc cancels.

import { openDialog } from "../dialogs.js";
import { h } from "../el.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";

let last = "";

export function isGenerativeDialog(id) {
  return id === "generative-fill";
}

export function openGenerativeFill() {
  const prompt = h("textarea", { class: "dlg-input area", rows: 3, placeholder: "Describe what to add — or leave empty to fill from the surroundings", style: { width: "100%" } });
  prompt.value = last;
  openDialog("generative-fill", {
    title: "Generative Fill",
    width: 520,
    fields: [
      { type: "element", el: prompt },
      { type: "label", text: "Runs on your local ComfyUI (Preferences ▸ AI Models & ComfyUI). The result is a group of variation layers masked by the selection." },
    ],
    ok: "Generate",
    cancel: "Cancel",
    onOk: () => {
      last = prompt.value;
      bridge.send({ type: UI.ACTION, id: "ai:generative-fill", args: { prompt: prompt.value } });
    },
  });
  setTimeout(() => prompt.focus(), 0);
}
