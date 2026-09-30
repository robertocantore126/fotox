// Fotox — live Layer Style dialogs (M6-T08/T09).
//
// Layer ▸ Layer Style ▸ Drop Shadow / Inner Shadow / Outer Glow / Color
// Overlay / Stroke and Blending Options edit the active layer. Every change
// sends the whole `LayerStyles` (`set_layer_style`); the engine merges the
// repeated edits into one history step. Cancel sends the styles back as they
// were. Gradient and Pattern Overlay are live too (they used to apply on OK
// only, so nothing seemed to happen while their page was open).

import { closeTopDialog, dialogValues, openDialog } from "../dialogs.js";
import { toast } from "../tooltip.js";
import { activeLayerInfo, sendCommand } from "./layers-panel.js";
import { hexToRgba16, rgba16ToHex } from "./tools.js";
import { currentGradientResolved, gradientEditor, resolveSwatches } from "./gradients.js";
import { currentPattern, patternList } from "./patterns.js";
import { activeDocumentInfo } from "./documents.js";
import { viewZoom } from "./overview-panels.js";

/**
 * While a style dialog is open, a drag on the image (outside the dialog)
 * moves the effect: `onDelta(dx, dy)` in document pixels. Photoshop lets
 * Gradient and Pattern Overlay be dragged into place this way. The click
 * that ends the drag does not reach the backdrop (which would close the
 * dialog).
 */
function canvasDrag(dlg, onDelta) {
  const vp = document.getElementById("viewport");
  if (!vp || !dlg) return;
  const inside = (e) => {
    const r = vp.getBoundingClientRect();
    return e.clientX >= r.left && e.clientX < r.right && e.clientY >= r.top && e.clientY < r.bottom;
  };
  const down = (e) => {
    if (!dlg.isConnected) { document.removeEventListener("mousedown", down, true); return; }
    if (e.button !== 0 || !e.target.classList || !e.target.classList.contains("modal-scrim") || !inside(e)) return;
    e.preventDefault();
    e.stopPropagation();
    const k = (window.devicePixelRatio || 1) / viewZoom();
    let last = { x: e.clientX, y: e.clientY };
    const move = (m) => {
      const dx = (m.clientX - last.x) * k, dy = (m.clientY - last.y) * k;
      last = { x: m.clientX, y: m.clientY };
      if (dx || dy) onDelta(dx, dy);
    };
    const swallow = (c) => { c.stopPropagation(); c.preventDefault(); };
    const up = () => {
      window.removeEventListener("mousemove", move, true);
      window.removeEventListener("mouseup", up, true);
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, true), 0);
    };
    window.addEventListener("mousemove", move, true);
    window.addEventListener("mouseup", up, true);
  };
  document.addEventListener("mousedown", down, true);
}

/** Set a dialog's range field by its label, as if the user moved it. */
function setRange(dlg, label, value) {
  for (const line of dlg.querySelectorAll(".dlg-line")) {
    const l = line.querySelector(".dlg-field-label");
    const range = line.querySelector(".dlg-range");
    if (!l || l.textContent !== label || !range) continue;
    range.value = String(Math.round(value));
    const out = line.querySelector(".dlg-input.num");
    if (out) out.value = range.value;
    range.dispatchEvent(new Event("input", { bubbles: true }));
    range.dispatchEvent(new Event("change", { bubbles: true }));
    return;
  }
}

const blendId = (name) => String(name || "Normal").toLowerCase().replace(/[^a-z]+/g, "_").replace(/_add_?$/, "");
const blendLabel = (id) => String(id || "normal").split("_").map((w) => w[0].toUpperCase() + w.slice(1)).join(" ");

// The document's Global Light (degrees), from its `DocumentInfo`; the value
// just sent wins until the engine's answer arrives.
let lightSent = null;
function globalLight() {
  const info = activeDocumentInfo();
  const doc = info && Number.isFinite(info.global_light) ? info.global_light : 120;
  if (lightSent !== null && Math.abs(doc - lightSent) < 1e-9) lightSent = null;
  return lightSent ?? doc;
}
function setGlobalLight(angle) {
  lightSent = angle;
  sendCommand({ op: "set_global_light", angle });
}
const pct = (v) => Math.round(v * 100);
const num = (v, fallback = 0) => (Number.isFinite(Number(v)) ? Number(v) : fallback);

/**
 * The angle a dialog shows and the Use Global Light box. As in Photoshop,
 * moving the Angle while the box is ticked moves the document's Global
 * Light, so every effect that uses it turns together.
 */
function light(v, e) {
  const angle = num(v["Angle:"], e.angle);
  const global = !!v["Use Global Light"];
  if (global && angle !== globalLight()) setGlobalLight(angle);
  return { angle: global ? e.angle : angle, use_global_light: global };
}

/**
 * Fill Type (Stroke, Outer Glow): the fill kept while its type is unchanged,
 * else a new one from the Gradient tool's current gradient or the current
 * pattern.
 */
const FILL_LABEL = { color: "Color", gradient: "Gradient", pattern: "Pattern" };
function fillFrom(label, old) {
  const type = { Color: "color", Gradient: "gradient", Pattern: "pattern" }[label] || "color";
  if (old && old.type === type) return old;
  if (type === "gradient") {
    const g = currentGradientResolved();
    if (!g) return { type: "color" };
    return { type: "gradient", align: true, gradient: { gradient: g, kind: "linear", angle: 90, scale: 100, reverse: false, dither: true, offset: [0, 0] } };
  }
  if (type === "pattern") {
    const id = currentPattern();
    if (id == null) { toast("Pick a pattern first (Edit ▸ Define Pattern)"); return { type: "color" }; }
    return { type: "pattern", pattern: id, scale: 100, angle: 0, phase: [0, 0], align: true };
  }
  return { type: "color" };
}

// dialog id → [styles field, defaults, to dialog values, from dialog values]
const EFFECTS = {
  "style-drop-shadow": {
    field: "drop_shadow",
    defaults: { enabled: true, blend: "multiply", color: [0, 0, 0, 65535], opacity: 0.75, angle: 120, use_global_light: true, distance: 5, spread: 0, size: 5, noise: 0 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": pct(e.opacity), "Angle:": e.use_global_light ? globalLight() : e.angle, "Use Global Light": e.use_global_light, "Distance:": e.distance, "Spread:": e.spread, "Size:": e.size, "Noise:": e.noise || 0 }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, ...light(v, e), distance: v["Distance:"], spread: v["Spread:"], size: v["Size:"], noise: num(v["Noise:"]) }),
  },
  "style-inner-shadow": {
    field: "inner_shadow",
    defaults: { enabled: true, blend: "multiply", color: [0, 0, 0, 65535], opacity: 0.75, angle: 120, use_global_light: true, distance: 5, choke: 0, size: 5 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": pct(e.opacity), "Angle:": e.use_global_light ? globalLight() : e.angle, "Use Global Light": e.use_global_light, "Distance:": e.distance, "Choke:": e.choke, "Size:": e.size }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, ...light(v, e), distance: v["Distance:"], choke: v["Choke:"], size: v["Size:"] }),
  },
  "style-outer-glow": {
    field: "outer_glow",
    defaults: { enabled: true, blend: "screen", opacity: 0.75, color: [65535, 65535, 48830, 65535], spread: 0, size: 5, noise: 0 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": pct(e.opacity), "Noise:": e.noise || 0, "Fill Type:": FILL_LABEL[e.fill?.type] || "Color", "Spread:": e.spread, "Size:": e.size }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, noise: num(v["Noise:"]), fill: fillFrom(v["Fill Type:"], e.fill), spread: v["Spread:"], size: v["Size:"] }),
  },
  "style-color-overlay": {
    field: "color_overlay",
    defaults: { enabled: true, blend: "normal", color: [65535, 0, 0, 65535], opacity: 1 },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": pct(e.opacity) }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100 }),
  },
  "style-stroke": {
    field: "stroke",
    defaults: { enabled: true, size: 3, position: "outside", blend: "normal", opacity: 1, color: [0, 0, 0, 65535] },
    toValues: (e) => ({ Colour: rgba16ToHex(e.color), "Fill Type:": FILL_LABEL[e.fill?.type] || "Color", "Size:": e.size, "Position:": { outside: "Outside", inside: "Inside", center: "Centre" }[e.position], "Blend Mode:": blendLabel(e.blend), "Opacity:": pct(e.opacity) }),
    fromValues: (v, e) => ({ ...e, color: hexToRgba16(v.Colour), fill: fillFrom(v["Fill Type:"], e.fill), size: Math.max(0, v["Size:"]), position: { Outside: "outside", Inside: "inside", Centre: "center" }[v["Position:"]] || "outside", blend: blendId(v["Blend Mode:"]), opacity: v["Opacity:"] / 100 }),
  },
  // M12-T04.
  "style-inner-glow": {
    field: "inner_glow",
    defaults: { enabled: true, blend: "screen", opacity: 0.75, color: [65535, 65535, 48830, 65535], choke: 0, size: 5, noise: 0, source: "edge" },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": pct(e.opacity), "Noise:": e.noise || 0, "Source:": e.source === "center" ? "Center" : "Edge", "Choke:": e.choke, "Size:": e.size }),
    fromValues: (v, e) => ({ ...e, blend: blendId(v["Blend Mode:"]), color: hexToRgba16(v.Colour), opacity: v["Opacity:"] / 100, noise: num(v["Noise:"]), source: v["Source:"] === "Center" ? "center" : "edge", choke: v["Choke:"], size: v["Size:"] }),
  },
  "style-satin": {
    field: "satin",
    defaults: { enabled: true, blend: "multiply", color: [0, 0, 0, 65535], opacity: 0.5, angle: 19, distance: 11, size: 14, invert: true },
    toValues: (e) => ({ "Blend Mode:": blendLabel(e.blend), Colour: rgba16ToHex(e.color), "Opacity:": pct(e.opacity), "Angle:": e.angle, "Distance:": e.distance, "Size:": e.size, Invert: e.invert }),
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
      "Depth:": e.depth, "Direction:": e.up ? "Up" : "Down", "Size:": e.size, "Soften:": e.soften,
      "Angle:": e.use_global_light ? globalLight() : e.angle, "Use Global Light": e.use_global_light, "Altitude:": e.altitude,
      "Highlight Mode:": blendLabel(e.highlight_blend), Highlight: rgba16ToHex(e.highlight_color), "Highlight Opacity:": pct(e.highlight_opacity),
      "Shadow Mode:": blendLabel(e.shadow_blend), Shadow: rgba16ToHex(e.shadow_color), "Shadow Opacity:": pct(e.shadow_opacity),
    }),
    fromValues: (v, e) => ({
      ...e,
      style: { "Inner Bevel": "inner_bevel", "Outer Bevel": "outer_bevel", Emboss: "emboss", "Pillow Emboss": "pillow_emboss" }[v["Style:"]] || "inner_bevel",
      depth: v["Depth:"], up: v["Direction:"] !== "Down", size: v["Size:"], soften: v["Soften:"],
      ...light(v, e), altitude: v["Altitude:"],
      highlight_blend: blendId(v["Highlight Mode:"]), highlight_color: hexToRgba16(v.Highlight), highlight_opacity: v["Highlight Opacity:"] / 100,
      shadow_blend: blendId(v["Shadow Mode:"]), shadow_color: hexToRgba16(v.Shadow), shadow_opacity: v["Shadow Opacity:"] / 100,
    }),
  },
};

// Contours (shadows, glows, satin) and Bevel & Emboss's Technique, Contour,
// Gloss Contour, Anti-aliased and Texture: added to the pages above.
const CONTOURS = [["linear", "Linear"], ["cone", "Cone"], ["cone_inverted", "Cone - Inverted"], ["cove_deep", "Cove - Deep"], ["cove_shallow", "Cove - Shallow"], ["gaussian", "Gaussian"], ["half_round", "Half Round"], ["ring", "Ring"], ["ring_double", "Ring - Double"], ["rolling_slope", "Rolling Slope - Descending"], ["rounded_steps", "Rounded Steps"], ["sawtooth", "Sawtooth 1"]];
const contourLabel = (id) => (CONTOURS.find(([k]) => k === id) || CONTOURS[0])[1];
const contourId = (label) => (CONTOURS.find(([, n]) => n === label) || CONTOURS[0])[0];
for (const id of ["style-drop-shadow", "style-inner-shadow", "style-outer-glow", "style-inner-glow", "style-satin"]) {
  const spec = EFFECTS[id];
  const to = spec.toValues, from = spec.fromValues;
  spec.toValues = (e) => ({ ...to(e), "Contour:": contourLabel(e.contour) });
  spec.fromValues = (v, e) => ({ ...from(v, e), contour: contourId(v["Contour:"]) });
}
{
  const TECH = [["smooth", "Smooth"], ["chisel_hard", "Chisel Hard"], ["chisel_soft", "Chisel Soft"]];
  const spec = EFFECTS["style-bevel"];
  const to = spec.toValues, from = spec.fromValues;
  spec.toValues = (e) => ({
    ...to(e),
    "Technique:": (TECH.find(([k]) => k === e.technique) || TECH[0])[1],
    "Contour:": contourLabel(e.contour), "Gloss Contour:": contourLabel(e.gloss_contour), "Anti-aliased": !!e.anti_aliased,
    Texture: !!e.texture, "Texture Scale:": e.texture?.scale ?? 100, "Texture Depth:": e.texture?.depth ?? 100,
    "Invert Texture": !!e.texture?.invert, "Link Texture with Layer": e.texture ? e.texture.align !== false : true,
  });
  spec.fromValues = (v, e) => {
    let texture = null;
    if (v.Texture) {
      const pattern = e.texture?.pattern ?? currentPattern();
      if (pattern == null) toast("Pick a pattern for the texture first (Edit ▸ Define Pattern)");
      else texture = { pattern, scale: Math.max(1, num(v["Texture Scale:"], 100)), depth: num(v["Texture Depth:"], 100), invert: !!v["Invert Texture"], align: !!v["Link Texture with Layer"] };
    }
    const out = {
      ...from(v, e),
      technique: (TECH.find(([, n]) => n === v["Technique:"]) || TECH[0])[0],
      contour: contourId(v["Contour:"]), gloss_contour: contourId(v["Gloss Contour:"]), anti_aliased: !!v["Anti-aliased"],
    };
    if (texture) out.texture = texture; else delete out.texture;
    return out;
  };
}

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
  if (layer.kind === "adjustment") { toast("An adjustment layer cannot have a layer style"); return; }
  openPage(id, session(layer));
}

function openPage(id, S) {
  if (id === "blending-options") { blendingOptions(S); return; }
  if (id === "style-gradient-overlay") { gradientOverlay(S); return; }
  if (id === "style-pattern-overlay") { patternOverlay(S); return; }
  const spec = EFFECTS[id];
  const layer = S.layer;
  // Opening an effect's page turns it on, as clicking its name does in Photoshop.
  const effect = { ...spec.defaults, ...((S.styles && S.styles[spec.field]) || {}), enabled: true };
  const current = () => (S.styles && S.styles[spec.field]) || effect;
  const withEffect = (values) => ({ ...(S.styles || {}), [spec.field]: spec.fromValues(values, current()) });
  // Show the effect at once, as Photoshop does when the page opens.
  send(S, withEffect(spec.toValues(effect)));
  const apply = (values, dialog) => {
    const styles = withEffect(values);
    send(S, styles);
    // Moving the angle unticks Use Global Light (see `light`).
    const e = styles[spec.field];
    if (dialog && "Use Global Light" in values && e.use_global_light !== !!values["Use Global Light"]) dialog.set({ "Use Global Light": e.use_global_light });
  };
  openDialog(id, {
    title: `Layer Style — ${layer.name}`,
    values: spec.toValues(effect),
    styleList: sidebar(S),
    onChange: apply,
    onOk: (values) => apply(values),
    onCancel: () => cancel(S),
  });
}

function blendingOptions(S) {
  const layer = S.layer;
  const set = (props) => sendCommand({ op: "set_layer_props", layer: { id: layer.id }, props });
  const from = (v) => ({ opacity: Math.min(1, Math.max(0, v["Opacity:"] / 100)), fill: Math.min(1, Math.max(0, v["Fill Opacity:"] / 100)), blend: blendId(v["Blend Mode:"]) });
  // Blend If (Gray): kept in the layer's styles, like Photoshop's descriptor.
  const start = S.styles?.blend_if || { this_layer: [0, 0, 255, 255], underlying: [0, 0, 255, 255] };
  const blendIf = { this_layer: [...start.this_layer], underlying: [...start.underlying] };
  const sendBlendIf = () => {
    const open = (r) => r[0] === 0 && r[1] === 0 && r[2] === 255 && r[3] === 255;
    const value = open(blendIf.this_layer) && open(blendIf.underlying) ? undefined : { this_layer: [...blendIf.this_layer], underlying: [...blendIf.underlying] };
    const styles = { ...(S.styles || {}) };
    if (value) styles.blend_if = value; else delete styles.blend_if;
    send(S, Object.keys(styles).length ? styles : null);
  };
  openDialog("blending-options", {
    title: `Layer Style — ${layer.name}`,
    values: { "Blend Mode:": blendLabel(layer.blend), "Opacity:": pct(layer.opacity), "Fill Opacity:": pct(layer.fill) },
    fields: [
      { type: "stylelist", items: SIDEBAR, active: "Blending Options: Default" },
      { type: "col", fields: [
        { type: "group", label: "General Blending", fields: [{ type: "blend", mode: blendLabel(layer.blend) }, range("Opacity:", pct(layer.opacity), 0, 100, "%")] },
        { type: "group", label: "Advanced Blending", fields: [range("Fill Opacity:", pct(layer.fill), 0, 100, "%")] },
        { type: "element", el: blendIfEditor(blendIf, sendBlendIf) },
      ] },
    ],
    styleList: sidebar(S),
    onChange: (v) => set(from(v)),
    onOk: (v) => set(from(v)),
    onCancel: () => cancel(S),
  });
}

/**
 * Blend If (Gray): two black-to-white bars, each with a black and a white
 * handle. Alt-drag splits a handle into its two halves (the fade), as in
 * Photoshop; a plain drag moves both halves.
 */
function blendIfEditor(model, onChange) {
  const W = 256;
  const box = document.createElement("div");
  box.className = "dlg-group blendif";
  const title = document.createElement("div");
  title.className = "dlg-group-title";
  title.textContent = "Blend If: Gray (Alt-drag splits a handle)";
  box.append(title);
  const bar = (label, key) => {
    const row = document.createElement("div");
    row.className = "blendif-row";
    row.style.cssText = "margin:6px 0 14px";
    const name = document.createElement("div");
    name.className = "dlg-label";
    const out = document.createElement("span");
    out.style.cssText = "float:right;opacity:.8";
    name.append(label, out);
    const track = document.createElement("div");
    track.style.cssText = `position:relative;width:${W}px;height:12px;background:linear-gradient(90deg,#000,#fff);border:1px solid var(--border, #555);`;
    const handles = [0, 1, 2, 3].map((i) => {
      const hnd = document.createElement("div");
      hnd.style.cssText = `position:absolute;top:12px;width:0;height:0;border-left:5px solid transparent;border-right:5px solid transparent;border-bottom:8px solid ${i < 2 ? "#111" : "#eee"};margin-left:-5px;cursor:ew-resize;`;
      hnd.style.filter = "drop-shadow(0 0 1px #888)";
      track.append(hnd);
      return hnd;
    });
    const draw = () => {
      const r = model[key];
      handles.forEach((hnd, i) => { hnd.style.left = `${(r[i] / 255) * W}px`; });
      out.textContent = `${r[0] === r[1] ? r[0] : `${r[0]}/${r[1]}`}   ${r[2] === r[3] ? r[2] : `${r[2]}/${r[3]}`}`;
    };
    const drag = (e, i) => {
      e.preventDefault();
      e.stopPropagation();
      const split = e.altKey;
      const rect = track.getBoundingClientRect();
      const pair = i < 2 ? [0, 1] : [2, 3];
      let pick = null; // Alt on a joined handle: the half the first move goes towards
      const move = (m) => {
        const v = Math.round(Math.min(255, Math.max(0, ((m.clientX - rect.left) / rect.width) * 255)));
        const r = model[key];
        if (split && pick === null) pick = r[pair[0]] === r[pair[1]] ? (v < r[i] ? pair[0] : pair[1]) : i;
        if (split) i = pick;
        if (split || r[pair[0]] !== r[pair[1]]) r[i] = v; else { r[pair[0]] = v; r[pair[1]] = v; }
        // Keep the order: black low ≤ black high ≤ white low ≤ white high.
        if (i === 0) r[1] = Math.max(r[1], r[0]);
        if (i === 1) r[0] = Math.min(r[0], r[1]);
        if (i === 2) r[3] = Math.max(r[3], r[2]);
        if (i === 3) r[2] = Math.min(r[2], r[3]);
        if (r[1] > r[2]) { if (i < 2) { r[2] = r[1]; r[3] = Math.max(r[3], r[2]); } else { r[1] = r[2]; r[0] = Math.min(r[0], r[1]); } }
        draw();
        onChange();
      };
      const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    };
    handles.forEach((hnd, i) => hnd.addEventListener("mousedown", (e) => {
      // Two halves on the same spot: the upper one of the pair is picked
      // when dragging right, the lower one when dragging left; Alt picks by index.
      drag(e, i);
    }));
    draw();
    row.append(name, track);
    return row;
  };
  const reset = document.createElement("button");
  reset.className = "btn small";
  reset.type = "button";
  reset.textContent = "Reset";
  box.append(bar("This Layer:", "this_layer"), bar("Underlying Layer:", "underlying"), reset);
  reset.addEventListener("click", () => {
    model.this_layer = [0, 0, 255, 255];
    model.underlying = [0, 0, 255, 255];
    box.replaceWith(blendIfEditor(model, onChange));
    onChange();
  });
  return box;
}

const GRADIENT_STYLES = ["Linear", "Radial", "Angle", "Reflected", "Diamond"];
const range = (label, value, min, max, unit) => ({ type: "range", label, value, min, max, unit });

/** Gradient Overlay (M12-T04), live. */
function gradientOverlay(S) {
  const layer = S.layer;
  const old = S.styles?.gradient_overlay;
  const work = old ? structuredClone(old.gradient.gradient) : currentGradientResolved();
  // Dragging on the image moves the gradient (its centre's offset).
  const gradOffset = [...(old?.gradient.offset || [0, 0])];
  // Align with Layer is Photoshop's default; styles saved before it existed
  // were placed over the canvas.
  const align = old ? !!old.align : true;
  let last = null;
  const effectOf = (v) => {
    resolveSwatches(work);
    return {
      enabled: true, blend: blendId(v["Blend Mode:"]), opacity: num(v["Opacity:"], 100) / 100, align: !!v["Align with Layer"],
      gradient: {
        gradient: structuredClone(work), kind: String(v["Style:"] || "Linear").toLowerCase(), angle: num(v["Angle:"], 90),
        scale: Math.min(1000, Math.max(1, num(v["Scale:"], 100))), reverse: !!v.Reverse, dither: v.Dither !== false, offset: [gradOffset[0], gradOffset[1]],
      },
    };
  };
  const apply = (v) => { last = v; send(S, { ...(S.styles || {}), gradient_overlay: effectOf(v) }); };
  const dlg = openDialog("style-gradient-overlay", {
    title: `Layer Style — ${layer.name}`,
    width: 660,
    fields: [
      { type: "stylelist", items: SIDEBAR, active: "Gradient Overlay" },
      { type: "col", fields: [
        { type: "blend", mode: blendLabel(old?.blend || "normal") },
        range("Opacity:", pct(old?.opacity ?? 1), 0, 100, "%"),
        // The gradient editor edits `work` in place; its edits re-apply the
        // dialog's last values.
        { type: "element", el: gradientEditor(work, () => apply(last || {})) },
        { type: "select", label: "Style:", options: GRADIENT_STYLES, value: old ? old.gradient.kind[0].toUpperCase() + old.gradient.kind.slice(1) : "Linear" },
        range("Angle:", old?.gradient.angle ?? 90, -180, 180, "°"),
        range("Scale:", old?.gradient.scale ?? 100, 10, 150, "%"),
        { type: "check", label: "Reverse", on: old?.gradient.reverse ?? false },
        { type: "check", label: "Align with Layer", on: align },
        { type: "check", label: "Dither", on: old?.gradient.dither ?? true },
      ] },
    ],
    styleList: sidebar(S),
    onChange: (v) => apply(v),
    onOk: (v) => apply(v),
    onCancel: () => cancel(S),
  });
  canvasDrag(dlg, (dx, dy) => { gradOffset[0] += dx; gradOffset[1] += dy; apply(last || dialogValues(dlg)); });
  // Show it at once, as the other pages do.
  apply(dialogValues(dlg));
}

/** Pattern Overlay (M12-T04), live. */
function patternOverlay(S) {
  const layer = S.layer;
  const old = S.styles?.pattern_overlay;
  const selected = { id: old ? old.pattern : currentPattern() };
  let last = null;
  const apply = (v) => {
    last = v;
    if (selected.id == null) return;
    const effect = {
      enabled: true, blend: blendId(v["Blend Mode:"]), opacity: num(v["Opacity:"], 100) / 100, pattern: selected.id,
      scale: Math.min(1000, Math.max(1, num(v["Scale:"], 100))), angle: num(v["Angle:"]), phase: [num(v["Offset X:"], old?.phase?.[0] ?? 0), num(v["Offset Y:"], old?.phase?.[1] ?? 0)], align: !!v["Link with Layer"],
    };
    send(S, { ...(S.styles || {}), pattern_overlay: effect });
  };
  const dlg = openDialog("style-pattern-overlay", {
    title: `Layer Style — ${layer.name}`,
    width: 640,
    fields: [
      { type: "stylelist", items: SIDEBAR, active: "Pattern Overlay" },
      { type: "col", fields: [
        { type: "blend", mode: blendLabel(old?.blend || "normal") },
        range("Opacity:", pct(old?.opacity ?? 1), 0, 100, "%"),
        { type: "element", el: patternList(selected, () => apply(last || {})) },
        range("Scale:", old?.scale ?? 100, 1, 1000, "%"),
        range("Angle:", old?.angle ?? 0, -180, 180, "°"),
        // Where the pattern starts (Photoshop drags it on the canvas).
        range("Offset X:", Math.round(old?.phase?.[0] ?? 0), -2000, 2000, "px"),
        range("Offset Y:", Math.round(old?.phase?.[1] ?? 0), -2000, 2000, "px"),
        { type: "check", label: "Link with Layer", on: old ? !!old.align : true },
      ] },
    ],
    styleList: sidebar(S),
    onChange: (v) => apply(v),
    onOk: (v) => {
      if (selected.id == null) { toast("Pick a pattern (Edit ▸ Define Pattern first)"); return; }
      apply(v);
    },
    onCancel: () => cancel(S),
  });
  // Dragging on the image moves the pattern (Offset X / Y).
  canvasDrag(dlg, (dx, dy) => {
    const v = dialogValues(dlg);
    setRange(dlg, "Offset X:", num(v["Offset X:"]) + dx);
    setRange(dlg, "Offset Y:", num(v["Offset Y:"]) + dy);
  });
  apply(dialogValues(dlg));
}
