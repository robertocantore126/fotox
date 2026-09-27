// Fotox — live Layer Style dialogs (M6-T08/T09).
//
// Layer ▸ Layer Style ▸ Drop Shadow / Inner Shadow / Outer Glow / Color
// Overlay / Stroke and Blending Options edit the active layer. Every change
// sends the whole `LayerStyles` (`set_layer_style`); the engine merges the
// repeated edits into one history step. Cancel sends the styles back as they
// were.

import { closeTopDialog, openDialog } from "../dialogs.js";
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

/** The Layer Style sidebar's pages: display name → dialog id and styles field. */
const PAGES = {
  "Blending Options: Default": { dialog: "blending-options" },
  "Drop Shadow": { dialog: "style-drop-shadow", field: "drop_shadow" },
  "Inner Shadow": { dialog: "style-inner-shadow", field: "inner_shadow" },
  "Outer Glow": { dialog: "style-outer-glow", field: "outer_glow" },
  "Inner Glow": { dialog: "style-inner-glow", field: "inner_glow" },
  "Bevel & Emboss": { dialog: "style-bevel", field: "bevel" },
  "Satin": { dialog: "style-satin", field: "satin" },
  "Color Overlay": { dialog: "style-color-overlay", field: "color_overlay" },
  "Gradient Overlay": { dialog: "style-gradient-overlay", field: "gradient_overlay" },
  "Pattern Overlay": { dialog: "style-pattern-overlay", field: "pattern_overlay" },
  "Stroke": { dialog: "style-stroke", field: "stroke" },
};
const SIDEBAR = ["Styles", ...Object.keys(PAGES)];

/**
 * One Layer Style dialog, across its pages: the layer, the styles and
 * blending as the dialog found them (Cancel goes back there, whatever page
 * it is pressed on) and the styles as they are now (every page builds on
 * them, so a toggle on one page is not undone by a slider on another).
 */
function session(layer) {
  return {
    layer,
    start: layer.styles || null,
    props: { opacity: layer.opacity, fill: layer.fill, blend: layer.blend },
    styles: layer.styles || null,
  };
}

function send(S, styles) {
  S.styles = styles;
  sendCommand({ op: "set_layer_style", layer: { id: S.layer.id }, styles });
}

function cancel(S) {
  sendCommand({ op: "set_layer_style", layer: { id: S.layer.id }, styles: S.start });
  sendCommand({ op: "set_layer_props", layer: { id: S.layer.id }, props: S.props });
}

/** The sidebar of page `active` (see dialogs.js `wireStyleList`). */
function sidebar(S) {
  const enabled = {};
  for (const [name, page] of Object.entries(PAGES)) {
    const e = page.field && S.styles && S.styles[page.field];
    enabled[name] = !!e && e.enabled !== false;
  }
  return {
    enabled,
    onToggle: (name, on) => {
      const { field, dialog } = PAGES[name];
      const e = S.styles && S.styles[field];
      if (e) send(S, { ...S.styles, [field]: { ...e, enabled: on } });
      else if (on && EFFECTS[dialog]) send(S, { ...(S.styles || {}), [field]: { ...EFFECTS[dialog].defaults } });
      // A new gradient or pattern overlay needs its settings: open its page.
      else if (on) { closeTopDialog(); openPage(dialog, S); }
    },
    onPick: (name) => openPage(PAGES[name].dialog, S),
  };
}

/** Open a live style dialog for the active layer. */
export function openStyleDialog(id) {
  const layer = activeLayerInfo();
  if (!layer) { toast("Select a layer first"); return; }
  if (layer.kind === "group" || layer.kind === "adjustment") { toast("Layer styles need a pixel, shape, text or fill layer"); return; }
  openPage(id, session(layer));
}

function openPage(id, S) {
  if (id === "blending-options") { blendingOptions(S); return; }
  if (id === "style-gradient-overlay") { gradientOverlay(S); return; }
  if (id === "style-pattern-overlay") { patternOverlay(S); return; }
  const spec = EFFECTS[id];
  const layer = S.layer;
  // Opening an effect's page turns it on, as clicking its name does in Photoshop.
  const effect = { ...((S.styles && S.styles[spec.field]) || spec.defaults), enabled: true };
  const current = () => (S.styles && S.styles[spec.field]) || effect;
  const withEffect = (values) => ({ ...(S.styles || {}), [spec.field]: spec.fromValues(values, current()) });
  // Show the effect at once, as Photoshop does when the page opens.
  send(S, withEffect(spec.toValues(effect)));
  openDialog(id, {
    title: `Layer Style — ${layer.name}`,
    values: spec.toValues(effect),
    styleList: sidebar(S),
    onChange: (values) => send(S, withEffect(values)),
    onOk: (values) => send(S, withEffect(values)),
    onCancel: () => cancel(S),
  });
}

function blendingOptions(S) {
  const layer = S.layer;
  const set = (props) => sendCommand({ op: "set_layer_props", layer: { id: layer.id }, props });
  const from = (v) => ({ opacity: Math.min(1, Math.max(0, v["Opacity:"] / 100)), fill: Math.min(1, Math.max(0, v["Fill Opacity:"] / 100)), blend: blendId(v["Blend Mode:"]) });
  openDialog("blending-options", {
    title: `Layer Style — ${layer.name}`,
    values: { "Blend Mode:": blendLabel(layer.blend), "Opacity:": Math.round(layer.opacity * 100), "Fill Opacity:": Math.round(layer.fill * 100) },
    styleList: sidebar(S),
    onChange: (v) => set(from(v)),
    onOk: (v) => set(from(v)),
    onCancel: () => cancel(S),
  });
}

const GRADIENT_STYLES = ["Linear", "Radial", "Angle", "Reflected", "Diamond"];

/** Gradient Overlay (M12-T04). FAST: applied on OK, no live update. */
function gradientOverlay(S) {
  const layer = S.layer;
  const old = S.styles?.gradient_overlay;
  const work = old ? structuredClone(old.gradient.gradient) : currentGradientResolved();
  openDialog("style-gradient-overlay", {
    title: `Layer Style — ${layer.name}`,
    width: 640,
    fields: [
      { type: "stylelist", items: SIDEBAR, active: "Gradient Overlay" },
      { type: "col", fields: [
        { type: "blend", mode: blendLabel(old?.blend || "normal") },
        { type: "range", label: "Opacity:", value: Math.round((old?.opacity ?? 1) * 100), min: 0, max: 100 },
        { type: "element", el: gradientEditor(work) },
        { type: "select", label: "Style:", options: GRADIENT_STYLES, value: old ? old.gradient.kind[0].toUpperCase() + old.gradient.kind.slice(1) : "Linear" },
        { type: "num", label: "Angle:", value: old?.gradient.angle ?? 90, unit: "°", w: 60 },
        { type: "num", label: "Scale:", value: old?.gradient.scale ?? 100, unit: "%", w: 60 },
        { type: "check", label: "Reverse", value: old?.gradient.reverse ?? false },
      ] },
    ],
    styleList: sidebar(S),
    onOk: (v) => {
      resolveSwatches(work);
      const effect = {
        enabled: true, blend: blendId(v["Blend Mode:"]), opacity: (Number(v["Opacity:"]) || 0) / 100,
        gradient: {
          gradient: work, kind: String(v["Style:"] || "Linear").toLowerCase(), angle: Number(v["Angle:"]) || 0,
          scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)), reverse: !!v.Reverse, dither: true, offset: [0, 0],
        },
      };
      send(S, { ...(S.styles || {}), gradient_overlay: effect });
    },
    onCancel: () => cancel(S),
  });
}

/** Pattern Overlay (M12-T04). FAST: applied on OK. */
function patternOverlay(S) {
  const layer = S.layer;
  const old = S.styles?.pattern_overlay;
  const selected = { id: old ? old.pattern : currentPattern() };
  openDialog("style-pattern-overlay", {
    title: `Layer Style — ${layer.name}`,
    width: 620,
    fields: [
      { type: "stylelist", items: SIDEBAR, active: "Pattern Overlay" },
      { type: "col", fields: [
        { type: "blend", mode: blendLabel(old?.blend || "normal") },
        { type: "range", label: "Opacity:", value: Math.round((old?.opacity ?? 1) * 100), min: 0, max: 100 },
        { type: "element", el: patternList(selected) },
        { type: "num", label: "Scale:", value: old?.scale ?? 100, unit: "%", w: 60 },
      ] },
    ],
    styleList: sidebar(S),
    onOk: (v) => {
      if (selected.id == null) { toast("Pick a pattern (Edit ▸ Define Pattern first)"); return; }
      const effect = { enabled: true, blend: blendId(v["Blend Mode:"]), opacity: (Number(v["Opacity:"]) || 0) / 100, pattern: selected.id, scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)) };
      send(S, { ...(S.styles || {}), pattern_overlay: effect });
    },
    onCancel: () => cancel(S),
  });
}
