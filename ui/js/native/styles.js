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
import { currentGradientResolved, gradientEditor, resolveSwatches } from "./gradients.js";
import { currentPattern, patternList } from "./patterns.js";

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
  // M12-T04.
  "style-inner-glow": {
    field: "inner_glow",
    defaults: { enabled: true, blend: "screen", opacity: 0.75, color: [65535, 65535, 48830, 65535], choke: 0, size: 5 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": Math.round(e.opacity * 100), "Choke:": e.choke, "Size:": e.size }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, choke: v["Choke:"], size: v["Size:"] }),
  },
  "style-satin": {
    field: "satin",
    defaults: { enabled: true, blend: "multiply", color: [0, 0, 0, 65535], opacity: 0.5, angle: 19, distance: 11, size: 14, invert: true },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": Math.round(e.opacity * 100), "Angle:": e.angle, "Distance:": e.distance, "Size:": e.size, Invert: e.invert }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, angle: v["Angle:"], distance: v["Distance:"], size: v["Size:"], invert: !!v.Invert }),
  },
  "style-bevel": {
    field: "bevel",
    defaults: {
      enabled: true, style: "inner_bevel", depth: 100, up: true, size: 5, soften: 0, angle: 120, use_global_light: true, altitude: 30,
      highlight_blend: "screen", highlight_color: [65535, 65535, 65535, 65535], highlight_opacity: 0.75,
      shadow_blend: "multiply", shadow_color: [0, 0, 0, 65535], shadow_opacity: 0.75,
    },
    toValues: (e) => ({
      "Style:": { inner_bevel: "Inner Bevel", outer_bevel: "Outer Bevel", emboss: "Emboss", pillow_emboss: "Pillow Emboss" }[e.style] || "Inner Bevel",
      "Depth:": e.depth, "Direction:": e.up ? "Up" : "Down", "Size:": e.size, "Soften:": e.soften, "Angle:": e.angle, "Altitude:": e.altitude,
      "Highlight Mode:": blendLabel(e.highlight_blend), Highlight: rgba16ToHex(e.highlight_color), "Highlight Opacity:": Math.round(e.highlight_opacity * 100),
      "Shadow Mode:": blendLabel(e.shadow_blend), Shadow: rgba16ToHex(e.shadow_color), "Shadow Opacity:": Math.round(e.shadow_opacity * 100),
    }),
    fromValues: (v, e) => ({
      ...e,
      // FAST: Stroke Emboss acts as Emboss.
      style: { "Inner Bevel": "inner_bevel", "Outer Bevel": "outer_bevel", Emboss: "emboss", "Pillow Emboss": "pillow_emboss", "Stroke Emboss": "emboss" }[v["Style:"]] || "inner_bevel",
      depth: v["Depth:"], up: v["Direction:"] !== "Down" && v["Direction:"] !== 1, size: v["Size:"], soften: v["Soften:"],
      angle: v["Angle:"], use_global_light: false, altitude: v["Altitude:"],
      highlight_blend: blendId(v["Highlight Mode:"]), highlight_color: hexToRgba16(v.Highlight), highlight_opacity: v["Highlight Opacity:"] / 100,
      shadow_blend: blendId(v["Shadow Mode:"]), shadow_color: hexToRgba16(v.Shadow), shadow_opacity: v["Shadow Opacity:"] / 100,
    }),
  },
};

/** Whether the engine implements this dialog id (without the `dlg:`). */
export function isStyleDialog(id) {
  return id in EFFECTS || id === "blending-options" || id === "style-gradient-overlay" || id === "style-pattern-overlay";
}

/** Open a live style dialog for the active layer. */
export function openStyleDialog(id) {
  const layer = activeLayerInfo();
  if (!layer) { toast("Select a layer first"); return; }
  if (layer.kind === "group" || layer.kind === "adjustment") { toast("Layer styles need a pixel, shape, text or fill layer"); return; }
  if (id === "blending-options") { blendingOptions(layer); return; }
  if (id === "style-gradient-overlay") { gradientOverlay(layer); return; }
  if (id === "style-pattern-overlay") { patternOverlay(layer); return; }
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

const GRADIENT_STYLES = ["Linear", "Radial", "Angle", "Reflected", "Diamond"];

/** Gradient Overlay (M12-T04). FAST: applied on OK, no live update. */
function gradientOverlay(layer) {
  const original = layer.styles || null;
  const old = original?.gradient_overlay;
  const work = old ? structuredClone(old.gradient.gradient) : currentGradientResolved();
  openDialog("style-gradient-overlay", {
    title: `Layer Style — ${layer.name}`,
    width: 460,
    fields: [
      { type: "blend", mode: blendLabel(old?.blend || "normal") },
      { type: "range", label: "Opacity:", value: Math.round((old?.opacity ?? 1) * 100), min: 0, max: 100 },
      { type: "element", el: gradientEditor(work) },
      { type: "select", label: "Style:", options: GRADIENT_STYLES, value: old ? old.gradient.kind[0].toUpperCase() + old.gradient.kind.slice(1) : "Linear" },
      { type: "num", label: "Angle:", value: old?.gradient.angle ?? 90, unit: "°", w: 60 },
      { type: "num", label: "Scale:", value: old?.gradient.scale ?? 100, unit: "%", w: 60 },
      { type: "check", label: "Reverse", value: old?.gradient.reverse ?? false },
    ],
    onOk: (v) => {
      resolveSwatches(work);
      const effect = {
        enabled: true, blend: blendId(v["Blend Mode:"]), opacity: (Number(v["Opacity:"]) || 0) / 100,
        gradient: {
          gradient: work, kind: String(v["Style:"] || "Linear").toLowerCase(), angle: Number(v["Angle:"]) || 0,
          scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)), reverse: !!v.Reverse, dither: true, offset: [0, 0],
        },
      };
      sendCommand({ op: "set_layer_style", layer: { id: layer.id }, styles: { ...(original || {}), gradient_overlay: effect } });
    },
  });
}

/** Pattern Overlay (M12-T04). FAST: applied on OK. */
function patternOverlay(layer) {
  const original = layer.styles || null;
  const old = original?.pattern_overlay;
  const selected = { id: old ? old.pattern : currentPattern() };
  openDialog("style-pattern-overlay", {
    title: `Layer Style — ${layer.name}`,
    fields: [
      { type: "blend", mode: blendLabel(old?.blend || "normal") },
      { type: "range", label: "Opacity:", value: Math.round((old?.opacity ?? 1) * 100), min: 0, max: 100 },
      { type: "element", el: patternList(selected) },
      { type: "num", label: "Scale:", value: old?.scale ?? 100, unit: "%", w: 60 },
    ],
    onOk: (v) => {
      if (selected.id == null) { toast("Pick a pattern (Edit ▸ Define Pattern first)"); return; }
      const effect = { enabled: true, blend: blendId(v["Blend Mode:"]), opacity: (Number(v["Opacity:"]) || 0) / 100, pattern: selected.id, scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)) };
      sendCommand({ op: "set_layer_style", layer: { id: layer.id }, styles: { ...(original || {}), pattern_overlay: effect } });
    },
  });
}
