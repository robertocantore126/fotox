// Fotox — gradients in the app (M8-T03): the option bar's gradient picker,
// the Gradient Editor, and Layer ▸ New Fill Layer ▸ Gradient.
//
// A gradient is the engine's `fx_core::gradient::Gradient` as JSON; a stop
// colour may be "fg" / "bg", which the engine replaces with the swatches when
// it paints (so "Foreground to Background" follows the colours).

import { h, icon, clear, dragOn } from "../el.js";
import { state, emit } from "../state.js";
import { openDialog, openColorPopover, askText } from "../dialogs.js";
import { openDropdown, selectButton } from "../popup.js";
import { registerControl } from "../optionsbar.js";
import * as bridge from "./bridge.js";
import { UI } from "./protocol.js";
import { activeLayerInfo, sendCommand, activeLayerId } from "./layers-panel.js";
import { prefValue, setPrefs } from "./prefs.js";
import { toast } from "../tooltip.js";

const stop = (location, color) => ({ location, midpoint: 0.5, color });
const op = (location, opacity) => ({ location, midpoint: 0.5, opacity });
const hex = (s) => [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16) / 255);

/** Photoshop's basic presets. */
export const PRESETS = {
  "Foreground to Background": { colors: [stop(0, "fg"), stop(1, "bg")], opacities: [], method: "perceptual" },
  "Foreground to Transparent": { colors: [stop(0, "fg"), stop(1, "fg")], opacities: [op(0, 1), op(1, 0)], method: "perceptual" },
  "Black, White": { colors: [stop(0, [0, 0, 0]), stop(1, [1, 1, 1])], opacities: [], method: "perceptual" },
  "Red, Green": { colors: [stop(0, hex("#e11e1e")), stop(1, hex("#1ea01e"))], opacities: [], method: "perceptual" },
  "Violet, Orange": { colors: [stop(0, hex("#29166f")), stop(1, hex("#f39200"))], opacities: [], method: "perceptual" },
  "Blue, Red, Yellow": { colors: [stop(0, hex("#0a00b2")), stop(0.5, hex("#ff0000")), stop(1, hex("#fffc00"))], opacities: [], method: "perceptual" },
  "Copper": { colors: [stop(0, hex("#97461a")), stop(0.3, hex("#fbd8c5")), stop(0.83, hex("#6c2e16")), stop(1, hex("#efdbcd"))], opacities: [], method: "perceptual" },
  "Chrome": { colors: [stop(0, hex("#29868b")), stop(0.5, hex("#ffffff")), stop(0.52, hex("#1c1c1c")), stop(0.64, hex("#8b5a2b")), stop(1, hex("#ffffff"))], opacities: [], method: "perceptual" },
  "Transparent Rainbow": { colors: [stop(0, [1, 0, 0]), stop(0.2, [1, 1, 0]), stop(0.4, [0, 1, 0]), stop(0.6, [0, 1, 1]), stop(0.8, [0, 0, 1]), stop(1, [1, 0, 1])], opacities: [op(0, 0), op(0.1, 1), op(0.9, 1), op(1, 0)], method: "perceptual" },
};

/** The user's saved gradients (`[{name, gradient}]`, preferences `gradient_presets`). */
function customGradients() {
  const list = prefValue("gradient_presets");
  return Array.isArray(list) ? list.filter((p) => p && typeof p.name === "string" && p.gradient && Array.isArray(p.gradient.colors)) : [];
}

/** Every preset by name: Photoshop's, then the user's saved ones. */
export function allGradients() {
  const out = { ...PRESETS };
  for (const p of customGradients()) out[p.name] = p.gradient;
  return out;
}

/** Save `g` as a preset (asks for the name); resolves to the name, or null. */
export async function saveGradientPreset(g) {
  const name = await askText("New Gradient Preset", "Name:", "Custom Gradient " + (customGradients().length + 1));
  if (!name) return null;
  if (PRESETS[name]) { toast(`"${name}" is a built-in preset: choose another name`); return null; }
  // A saved name again replaces it.
  const list = customGradients().filter((p) => p.name !== name);
  setPrefs({ gradient_presets: [...list, { name, gradient: structuredClone(g) }] });
  toast(`Gradient "${name}" saved`);
  return name;
}

/** Delete one of the saved gradients (a pick from a list of them). */
function deleteGradientPreset(anchor, after) {
  const list = customGradients();
  if (!list.length) { toast("No saved gradients to delete"); return; }
  openDropdown({
    anchor, items: list.map((p) => p.name), value: "", width: 200,
    onPick: (name) => {
      setPrefs({ gradient_presets: customGradients().filter((p) => p.name !== name) });
      toast(`Gradient "${name}" deleted`);
      after?.();
    },
  });
}

let current = structuredClone(PRESETS["Foreground to Background"]);
let currentName = "Foreground to Background";

function toCss(c) {
  if (c === "fg") return state.colors.fg;
  if (c === "bg") return state.colors.bg;
  return `rgb(${c.map((v) => Math.round(v * 255)).join(",")})`;
}

/** A CSS preview of a gradient (FAST: midpoints and methods not shown). */
export function gradientCss(g) {
  const stops = g.colors.map((s) => `${toCss(s.color)} ${Math.round(s.location * 100)}%`);
  return `linear-gradient(90deg, ${stops.join(", ")})`;
}

function toHex(c) {
  if (c === "fg") return state.colors.fg;
  if (c === "bg") return state.colors.bg;
  return "#" + c.map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("");
}

// The UI runs in off-screen CEF, which draws neither the native colour
// chooser of <input type="color"> nor a <select>'s popup list: the editor
// uses the app's own colour popover and dropdowns instead.

/** A colour chip: a click opens the colour popover. */
function colorChip(hexValue, onInput) {
  const chip = h("button", { class: "ls-color", type: "button", "data-tip": "Pick a colour", style: { background: hexValue } });
  chip.addEventListener("click", (e) => {
    e.stopPropagation();
    openColorPopover(chip, hexValue, (next) => {
      hexValue = next;
      chip.style.background = next;
      onInput(next);
    });
  });
  return chip;
}

const choice = (pairs, value, onPick) => selectButton(pairs, value, onPick);

/** Straight RGBA (0..1) of `g` at `t`, as `fx_core::gradient::Gradient::eval` (sRGB mix for the preview). */
function sampleGradient(g, t) {
  const remap = (f, m) => {
    m = Math.min(0.95, Math.max(0.05, m ?? 0.5));
    return Math.abs(m - 0.5) < 1e-6 ? f : Math.pow(Math.min(1, Math.max(0, f)), Math.log(0.5) / Math.log(m));
  };
  const walk = (stops, value, mixf, fallback) => {
    if (!stops.length) return fallback;
    if (stops.length === 1 || t <= stops[0].location) return value(stops[0]);
    for (let i = 0; i < stops.length - 1; i++) {
      const a = stops[i], b = stops[i + 1];
      if (t <= b.location) {
        const f = b.location > a.location ? (t - a.location) / (b.location - a.location) : 1;
        return mixf(value(a), value(b), remap(f, a.midpoint));
      }
    }
    return value(stops[stops.length - 1]);
  };
  const rgb = walk(g.colors, (s) => hex(toHex(s.color)), (a, b, f) => a.map((v, k) => v + (b[k] - v) * f), [0, 0, 0]);
  const alpha = walk(g.opacities || [], (s) => s.opacity, (a, b, f) => a + (b - a) * f, 1);
  return [...rgb, alpha];
}

/** Paint `g` on `canvas` over a checkerboard (opacity shows). */
function paintGradient(canvas, g) {
  const ctx = canvas.getContext("2d");
  const { width: w, height: hh } = canvas;
  for (let y = 0; y < hh; y += 6) for (let x = 0; x < w; x += 6) {
    ctx.fillStyle = ((x + y) / 6) % 2 ? "#cfcfcf" : "#fff";
    ctx.fillRect(x, y, 6, 6);
  }
  for (let x = 0; x < w; x++) {
    const [r, gg, b, a] = sampleGradient(g, x / (w - 1));
    ctx.fillStyle = `rgba(${Math.round(r * 255)},${Math.round(gg * 255)},${Math.round(b * 255)},${a})`;
    ctx.fillRect(x, 0, 1, hh);
  }
}

/**
 * The Gradient Editor as a DOM node editing `g` in place; `changed()` after
 * every edit. Photopea's layout: the gradient bar with the opacity stops
 * above it and the colour stops below. Click a track to add a stop, drag a
 * stop to move it, drag it off the track (or Delete) to remove it, drag the
 * diamond to move the midpoint; the selected stop's fields are underneath.
 */
export function gradientEditor(g, changed = () => {}, { colorOnly = false } = {}) {
  const root = h("div", { class: "grad-editor" });
  if (!Array.isArray(g.opacities)) g.opacities = [];
  // The stop being edited: { kind: "color" | "opacity", stop } (the object, so sorting keeps it).
  let sel = g.colors[0] ? { kind: "color", stop: g.colors[0] } : null;
  const listOf = (kind) => (kind === "color" ? g.colors : g.opacities);
  const sortAll = () => { g.colors.sort((a, b) => a.location - b.location); g.opacities.sort((a, b) => a.location - b.location); };
  const pct = (v) => Math.round(v * 1000) / 10;
  const firstSel = () => (g.colors[0] ? { kind: "color", stop: g.colors[0] } : null);

  const render = () => {
    clear(root);
    const presetBtn = h("button", { class: "btn small", type: "button", text: "Presets…", onclick: (e) => {
      const all = allGradients();
      openDropdown({
        anchor: e.currentTarget, items: Object.keys(all).filter((name) => !colorOnly || !all[name].opacities?.length), value: "", width: 200,
        onPick: (name) => {
          Object.assign(g, structuredClone(all[name]));
          if (!Array.isArray(g.opacities)) g.opacities = [];
          sel = firstSel();
          render();
          changed();
        },
      });
    } });
    const saveBtn = h("button", { class: "btn small", type: "button", text: "Save…", "data-tip": "Save this gradient as a preset", onclick: () => saveGradientPreset(g) });
    const deleteBtn = h("button", { class: "btn small", type: "button", text: "Delete…", "data-tip": "Delete a saved gradient", onclick: (e) => deleteGradientPreset(e.currentTarget) });
    const method = choice([["perceptual", "Perceptual"], ["linear", "Linear"], ["classic", "Classic"]], g.method || "perceptual", (m) => { g.method = m; changed(); });
    root.append(h("div", { class: "dlg-line" }, presetBtn, saveBtn, deleteBtn, colorOnly ? null : h("span", { class: "dlg-field-label", text: "Method:" }), colorOnly ? null : method));

    // The bar and its two tracks share one horizontal frame: 0 % and 100 %
    // are the bar's edges, so the stops line up with it.
    const canvas = h("canvas", { class: "gx-bar", width: 480, height: 26 });
    const opTrack = colorOnly ? null : h("div", { class: "gx-track top", "data-tip": "Click to add an opacity stop" });
    const colTrack = h("div", { class: "gx-track bottom", "data-tip": "Click to add a colour stop" });
    const frame = h("div", { class: "gx-frame" }, opTrack, canvas, colTrack);
    const fields = h("div", { class: "gx-fields" });
    root.append(frame, fields);

    const xToLoc = (clientX) => {
      const r = canvas.getBoundingClientRect();
      return Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    };
    const remove = (kind, s) => {
      const list = listOf(kind);
      const i = list.indexOf(s);
      if (i >= 0) list.splice(i, 1);
      sel = firstSel();
    };
    // Redraw what a drag changes (not the whole editor: the drag keeps going).
    const paint = () => {
      paintGradient(canvas, g);
      if (opTrack) { clear(opTrack); drawStops("opacity", opTrack); }
      clear(colTrack);
      drawStops("color", colTrack);
    };
    const stopFill = (kind, s) => {
      if (kind === "color") return toHex(s.color);
      const v = Math.round(255 * (1 - s.opacity));
      return `rgb(${v},${v},${v})`;
    };
    function drawStops(kind, track) {
      const stops = listOf(kind);
      for (const s of stops) {
        const on = !!sel && sel.stop === s;
        const tip = kind === "color" ? `Colour stop at ${pct(s.location)} %` : `Opacity ${Math.round(s.opacity * 100)} % at ${pct(s.location)} %`;
        const marker = h("div", { class: "gx-stop " + kind + (on ? " sel" : ""), style: { left: s.location * 100 + "%" }, "data-tip": tip },
          h("span", { class: "gx-chip", style: { background: stopFill(kind, s) } }));
        track.append(marker);
        // Press: select. Drag sideways: move. Dragged well off the track: it goes on release.
        let gone = false;
        let y0 = 0;
        dragOn(marker, (m) => {
          if (m.type === "mousedown") return;
          const removable = kind === "opacity" || g.colors.length > 1;
          if (removable && Math.abs(m.clientY - y0) > 28) {
            if (!gone) { gone = true; marker.classList.add("going"); }
            return;
          }
          if (gone) { gone = false; marker.classList.remove("going"); }
          s.location = xToLoc(m.clientX);
          sortAll();
          marker.style.left = s.location * 100 + "%";
          paintGradient(canvas, g);
          showFields();
          changed();
        }, {
          onDown: (e) => {
            e.stopPropagation();
            y0 = e.clientY;
            gone = false;
            if (!(sel && sel.stop === s)) { sel = { kind, stop: s }; showFields(); }
            for (const m of root.querySelectorAll(".gx-stop.sel")) m.classList.remove("sel");
            marker.classList.add("sel");
          },
          onUp: () => {
            if (gone) { remove(kind, s); showFields(); changed(); }
            // Redraw the tracks: the midpoint diamond follows the selection.
            paint();
          },
        });
      }
      // The selected stop's midpoint, toward the next stop.
      if (sel && sel.kind === kind) {
        const i = stops.indexOf(sel.stop);
        const a = sel.stop;
        const next = stops[i + 1];
        if (i >= 0 && next && next.location > a.location) {
          const at = () => (a.location + (next.location - a.location) * (a.midpoint ?? 0.5)) * 100 + "%";
          const mid = h("div", { class: "gx-mid", style: { left: at() }, "data-tip": "Midpoint" });
          track.append(mid);
          dragOn(mid, (m) => {
            if (m.type === "mousedown") return;
            const t = xToLoc(m.clientX);
            a.midpoint = Math.min(0.95, Math.max(0.05, (t - a.location) / (next.location - a.location)));
            mid.style.left = at();
            paintGradient(canvas, g);
            showFields();
            changed();
          }, { onDown: (e) => { e.stopPropagation(); } });
        }
      }
    }
    // A press on an empty part of a track adds a stop there, with the colour
    // (or opacity) the gradient already has at that point.
    const addOn = (track, kind) => track && track.addEventListener("mousedown", (e) => {
      if (e.button !== 0 || e.target !== track) return;
      e.preventDefault();
      const t = xToLoc(e.clientX);
      const [r, gg, b, a] = sampleGradient(g, t);
      if (kind === "opacity" && !g.opacities.length) g.opacities.push(op(0, 1), op(1, 1));
      const s = kind === "color" ? stop(t, [r, gg, b]) : op(t, a);
      listOf(kind).push(s);
      sortAll();
      sel = { kind, stop: s };
      paint();
      showFields();
      changed();
    });
    addOn(opTrack, "opacity");
    addOn(colTrack, "color");

    // The selected stop's fields (Photopea: Colour / Opacity, Location, Midpoint, Delete).
    function showFields() {
      clear(fields);
      if (!sel || !listOf(sel.kind).includes(sel.stop)) {
        fields.append(h("div", { class: "dlg-note", text: colorOnly
          ? "Click a stop to edit it, or under the bar to add one."
          : "Click a stop to edit it; click under the bar to add a colour stop, above it to add an opacity stop." }));
        return;
      }
      const s = sel.stop;
      const kind = sel.kind;
      const refresh = () => { paint(); changed(); };
      const loc = numBox(s.location * 100, (v) => { s.location = v / 100; sortAll(); refresh(); showFields(); });
      const mid = numBox((s.midpoint ?? 0.5) * 100, (v) => { s.midpoint = Math.min(95, Math.max(5, v)) / 100; refresh(); });
      const removable = kind === "opacity" || g.colors.length > 1;
      const del = h("button", { class: "btn small", type: "button", text: "Delete", disabled: !removable, "data-tip": "Delete this stop (or drag it off the bar)", onclick: () => { remove(kind, s); paint(); showFields(); changed(); } });
      const label = (t) => h("span", { class: "dlg-field-label", text: t });
      const unit = () => h("span", { class: "dlg-unit", text: "%" });
      if (kind === "color") {
        const chip = colorChip(toHex(s.color), (next) => {
          const wasSwatch = typeof s.color === "string";
          s.color = hex(next);
          refresh();
          if (wasSwatch) showFields();
        });
        const which = choice([["rgb", "Colour"], ["fg", "Foreground"], ["bg", "Background"]], typeof s.color === "string" ? s.color : "rgb", (v) => {
          s.color = v === "rgb" ? hex(toHex(s.color)) : v;
          refresh();
          showFields();
        });
        fields.append(h("div", { class: "gx-row" }, label("Colour:"), chip, which));
      } else {
        const o = numBox(s.opacity * 100, (v) => { s.opacity = v / 100; refresh(); });
        fields.append(h("div", { class: "gx-row" }, label("Opacity:"), o, unit()));
      }
      fields.append(h("div", { class: "gx-row" }, label("Location:"), loc, unit(), label("Midpoint:"), mid, unit(), h("span", { class: "gx-gap" }), del));
    }

    paint();
    showFields();
  };
  render();
  return root;
}

function numBox(value, set) {
  const input = h("input", { class: "dlg-input num", type: "text", value: Math.round(value), style: { width: "44px" } });
  const apply = (v) => { v = Math.min(100, Math.max(0, v)); input.value = String(Math.round(v)); set(v); };
  input.addEventListener("change", () => apply(Number(input.value) || 0));
  return input;
}

/** Edit ▸ the option bar gradient in the Gradient Editor. */
export function openGradientEditor(onDone) {
  const work = structuredClone(current);
  openDialog("gradient-editor", {
    title: "Gradient Editor",
    width: 460,
    fields: [{ type: "element", el: gradientEditor(work) }],
    onOk: () => { current = work; currentName = "Custom"; onDone?.(); },
  });
}

/** The option bar's gradient picker (`key: "Gradient"`). */
function pickerControl() {
  const preview = h("span", { class: "gradient-preview", style: { background: gradientCss(current) } });
  const refresh = () => { preview.style.background = gradientCss(current); el.dataset.tip = currentName; };
  const notify = () => { refresh(); emit("brush:changed"); };
  const el = h("button", {
    class: "ob-gradient", type: "button", "data-tip": currentName,
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({
        anchor: el, items: [...Object.keys(allGradients()), "Edit…"], value: currentName, width: 200,
        onPick: (name) => {
          if (name === "Edit…") { openGradientEditor(notify); return; }
          const preset = allGradients()[name];
          if (!preset) return;
          current = structuredClone(preset);
          currentName = name;
          notify();
        },
      });
    },
  }, preview, icon("i-chevron-down", "ic xs"));
  return { el, read: () => structuredClone(current) };
}

/* ------------------------------------------------------ fill layer dialog */

const STYLES = ["Linear", "Radial", "Angle", "Reflected", "Diamond"];

/** Layer ▸ New Fill Layer ▸ Gradient (or edit the active one's). */
export function openGradientFillDialog(edit = false) {
  const info = edit ? activeLayerInfo() : null;
  const old = info?.fill_layer?.fill === "gradient" ? info.fill_layer : null;
  const work = structuredClone(old ? old.gradient : current);
  openDialog("fill-gradient", {
    title: old ? "Gradient Fill" : "New Layer — Gradient Fill",
    width: 460,
    fields: [
      { type: "element", el: gradientEditor(work) },
      { type: "select", label: "Style:", options: STYLES, value: old ? old.kind[0].toUpperCase() + old.kind.slice(1) : "Linear" },
      { type: "num", label: "Angle:", value: old ? old.angle : 90, unit: "°", w: 60 },
      { type: "num", label: "Scale:", value: old ? old.scale : 100, unit: "%", w: 60 },
      { type: "check", label: "Reverse", value: old ? old.reverse : false },
      { type: "check", label: "Dither", value: old ? old.dither : true },
    ],
    onOk: (v) => {
      const content = {
        // Keep what the dialog does not show (the centre's offset, the
        // mirror a flipped canvas set).
        ...(old || {}),
        fill: "gradient",
        gradient: work,
        kind: String(v["Style:"] || "Linear").toLowerCase(),
        angle: Number(v["Angle:"]) || 0,
        scale: Math.min(1000, Math.max(1, Number(v["Scale:"]) || 100)),
        reverse: !!v.Reverse,
        dither: v.Dither !== false,
        offset: old ? old.offset : [0, 0],
      };
      resolveSwatches(content.gradient);
      if (old) sendCommand({ op: "set_fill_layer", layer: { id: activeLayerId() }, content });
      else sendCommand({ op: "add_layer", layer: { fill: { content } }, name: null });
    },
  });
}

/** A fill layer keeps fixed colours: "fg"/"bg" become the current swatches. */
function resolveSwatches(g) {
  for (const s of g.colors) if (typeof s.color === "string") s.color = hex(s.color === "fg" ? state.colors.fg : state.colors.bg);
}

export function initGradients() {
  registerControl("gradient", pickerControl);
}

export function isGradientDialog(id) {
  return id === "fill-gradient" || id === "gradient-editor";
}

export function openGradientDialog(id) {
  if (id === "fill-gradient") openGradientFillDialog(false);
  else openGradientEditor(() => emit("brush:changed"));
}

export { bridge, UI };

/** The current gradient, its swatch stops resolved (Gradient Overlay, M12-T04). */
export function currentGradientResolved() {
  const g = structuredClone(current);
  resolveSwatches(g);
  return g;
}

export { resolveSwatches };
