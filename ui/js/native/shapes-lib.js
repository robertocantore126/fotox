// Fotox — the Custom Shape tool's shape picker (M10-T07): the engine's
// library (built-in + Edit ▸ Define Custom Shape) sent as `shapes`.

import { h, icon } from "../el.js";
import { emit } from "../state.js";
import { openDropdown } from "../popup.js";
import { registerControl } from "../optionsbar.js";
import * as bridge from "./bridge.js";
import { ENGINE } from "./protocol.js";

let names = ["Star", "Arrow", "Check", "Lightning", "Heart", "Cloud", "Leaf", "Speech Bubble"];
let current = "Star";

function picker() {
  const label = h("span", { class: "ob-value", text: current });
  const el = h("button", {
    class: "ob-select", type: "button", "data-tip": "Custom shape",
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({
        anchor: el, items: names, value: current, width: 180,
        onPick: (name) => { current = name; label.textContent = name; emit("brush:changed"); },
      });
    },
  }, h("span", { class: "ob-text", text: "Shape:" }), label, icon("i-chevron-down", "ic xs"));
  return { el, read: () => current };
}

export function initShapes() {
  registerControl("customshape", picker);
  bridge.on(ENGINE.SHAPES, (m) => { names = m.names || names; });
}
