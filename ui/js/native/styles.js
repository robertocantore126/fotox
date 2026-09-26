// Fotox — live Layer Style dialogs (M6-T08/T09).
//
// Layer ▸ Layer Style ▸ Drop Shadow / Inner Shadow / Outer Glow / Color
// Overlay / Stroke and Blending Options edit the active layer. Every change
// sends the whole `LayerStyles` (`set_layer_style`); the engine merges the
// repeated edits into one history step. Cancel sends the styles back as they
// were.

import { openDialog } from "../dialogs.js";
import { toast } from "../tooltip.js";
import { activeLayerInfo, sendCommand } from "./layers-panel.js";
import { hexToRgba16, rgba16ToHex } from "./tools.js";

const blendId = (name) => String(name || "Normal").toLowerCase().replace(/[^a-z]+/g, "_").replace(/_add_?$/, "");
const blendLabel = (id) => String(id || "normal").split("_").map((w) => w[0].toUpperCase() + w.slice(1)).join(" ");

// dialog id → [styles field, defaults, to dialog values, from dialog values]
const EFFECTS = {
  "style-drop-shadow": {
    field: "drop_shadow",
    defaults: { enabled: true, blend: "multiply", color: [0, 0, 0, 65535], opacity: 0.75, angle: 120, use_global_light: true, distance: 5, spread: 0, size: 5 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": Math.round(e.opacity * 100), "Angle:": e.angle, "Use Global Light": e.use_global_light, "Distance:": e.distance, "Spread:": e.spread, "Size:": e.size }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, angle: v["Angle:"], use_global_light: !!v["Use Global Light"], distance: v["Distance:"], spread: v["Spread:"], size: v["Size:"] }),
  },
  "style-inner-shadow": {
    field: "inner_shadow",
    defaults: { enabled: true, blend: "multiply", color: [0, 0, 0, 65535], opacity: 0.75, angle: 120, use_global_light: false, distance: 5, choke: 0, size: 5 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": Math.round(e.opacity * 100), "Angle:": e.angle, "Distance:": e.distance, "Choke:": e.choke, "Size:": e.size }),
    // FAST: the dialog has no Use Global Light box; its angle always applies.
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, angle: v["Angle:"], use_global_light: false, distance: v["Distance:"], choke: v["Choke:"], size: v["Size:"] }),
  },
  "style-outer-glow": {
    field: "outer_glow",
    defaults: { enabled: true, blend: "screen", opacity: 0.75, color: [65535, 65535, 48830, 65535], spread: 0, size: 5 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": Math.round(e.opacity * 100), "Spread:": e.spread, "Size:": e.size }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, spread: v["Spread:"], size: v["Size:"] }),
  },
  "style-color-overlay": {
    field: "color_overlay",
    defaults: { enabled: true, blend: "normal", color: [65535, 0, 0, 65535], opacity: 1 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": Math.round(e.opacity * 100) }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100 }),
  },
  "style-stroke": {
    field: "stroke",
    defaults: { enabled: true, size: 3, position: "outside", blend: "normal", opacity: 1, color: [0, 0, 0, 65535] },
    toValues: (e) => ({ Colour: rgba16ToHex(e.color), "Size:": e.size, "Position:": { outside: "Outside", inside: "Inside", center: "Centre" }[e.position], "Blend Mode:": blendLabel(e.blend), "Opacity:": Math.round(e.opacity * 100) }),
    fromValues: (v, e) => ({ ...e, color: hexToRgba16(v.Colour), size: Math.max(0, v["Size:"]), position: { Outside: "outside", Inside: "inside", Centre: "center" }[v["Position:"]] || "outside", blend: blendId(v["Blend Mode:"]), opacity: v["Opacity:"] / 100 }),
  },
};

/** Whether the engine implements this dialog id (without the `dlg:`). */
export function isStyleDialog(id) {
  return id in EFFECTS || id === "blending-options";
}

/** Open a live style dialog for the active layer. */
export function openStyleDialog(id) {
  const layer = activeLayerInfo();
  if (!layer) { toast("Select a layer first"); return; }
  if (layer.kind === "group" || layer.kind === "adjustment") { toast("Layer styles need a pixel, shape, text or fill layer"); return; }
  if (id === "blending-options") { blendingOptions(layer); return; }
  const spec = EFFECTS[id];
  const original = layer.styles || null;
  const effect = (original && original[spec.field]) || spec.defaults;
  const set = (styles) => sendCommand({ op: "set_layer_style", layer: { id: layer.id }, styles });
  const withEffect = (values) => ({ ...(original || {}), [spec.field]: spec.fromValues(values, effect) });
  openDialog(id, {
    title: `Layer Style — ${layer.name}`,
    values: spec.toValues(effect),
    onChange: (values) => set(withEffect(values)),
    onOk: (values) => set(withEffect(values)),
    onCancel: () => set(original),
  });
  // Show the effect at once, as Photoshop does when the dialog opens.
  set(withEffect(spec.toValues(effect)));
}

function blendingOptions(layer) {
  const original = { opacity: layer.opacity, fill: layer.fill, blend: layer.blend };
  const set = (props) => sendCommand({ op: "set_layer_props", layer: { id: layer.id }, props });
  const from = (v) => ({ opacity: Math.min(1, Math.max(0, v["Opacity:"] / 100)), fill: Math.min(1, Math.max(0, v["Fill Opacity:"] / 100)), blend: blendId(v["Blend Mode:"]) });
  openDialog("blending-options", {
    title: `Layer Style — ${layer.name}`,
    values: { "Blend Mode:": blendLabel(layer.blend), "Opacity:": Math.round(layer.opacity * 100), "Fill Opacity:": Math.round(layer.fill * 100) },
    onChange: (v) => set(from(v)),
    onOk: (v) => set(from(v)),
    onCancel: () => set(original),
  });
}
