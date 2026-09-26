// Fotox — guides, grid and snapping in the app (M7-T06).
//
// The View flags (Extras, Guides, Grid, Snap, Snap To) stay UI state; the
// engine gets them as the `_view` options and draws the guides and the grid
// itself. A guide is dragged out of a ruler: while the button is down the
// pointer belongs to the UI (direct input off), and the drop sends
// `guide:add` with the viewport pixel, which the engine maps to the document.

import { openDialog } from "../dialogs.js";
import { on, state } from "../state.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";

const VIEW_FLAGS = ["extras", "guides", "grid", "snap", "snap-guides", "snap-grid", "snap-layers", "snap-bounds", "lockguides"];

export function sendViewFlags() {
  const options = {};
  for (const key of VIEW_FLAGS) options[key] = !!state.flags[key];
  bridge.send({ type: UI.TOOL_OPTIONS, tool: "_view", options });
}

/** Wire the rulers of the native workspace. */
export function initGuides(rulerTop, rulerLeft, viewportEl) {
  on("flag", (key) => { if (VIEW_FLAGS.includes(key)) sendViewFlags(); });
  sendViewFlags();
  for (const [ruler, vertical] of [[rulerTop, false], [rulerLeft, true]]) {
    ruler.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      bridge.send({ type: UI.DIRECT_INPUT, enabled: false });
      const up = (ev) => {
        window.removeEventListener("pointerup", up, true);
        bridge.send({ type: UI.DIRECT_INPUT, enabled: true });
        const rect = viewportEl.getBoundingClientRect();
        const inside = ev.clientX >= rect.left && ev.clientX <= rect.right && ev.clientY >= rect.top && ev.clientY <= rect.bottom;
        if (!inside) return; // dropped back on a ruler: no guide
        const dpr = window.devicePixelRatio || 1;
        const screen = vertical ? (ev.clientX - rect.left) * dpr : (ev.clientY - rect.top) * dpr;
        bridge.send({ type: UI.ACTION, id: "guide:add", args: { vertical, screen } });
      };
      window.addEventListener("pointerup", up, true);
    });
  }
}

/** View ▸ New Guide… and New Guide Layout… (engine-backed). */
export function isGuideDialog(id) {
  return id === "new-guide" || id === "guide-layout";
}

export function openGuideDialog(id) {
  if (id === "new-guide") {
    openDialog(id, {
      onOk: (v) => bridge.send({
        type: UI.ACTION, id: "guide:add",
        args: { vertical: v["Orientation:"] !== "Horizontal", position: Number(v["Position:"]) || 0, percent: v[""] === "%" },
      }),
    });
    return;
  }
  openDialog(id, {
    onOk: (v) => bridge.send({
      type: UI.ACTION, id: "guide:layout",
      args: { columns: Number(v["Columns:"]) || 0, rows: Number(v["Rows:"]) || 0, gutter: Number(v["Gutter:"]) || 0, margin: Number(v["Margin:"]) || 0 },
    }),
  });
}
