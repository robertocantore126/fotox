// Fotox — barra opzioni contestuale allo strumento attivo.

import { h, icon, clear } from "./el.js";
import { optionsFor } from "./data/options.js";
import { openDropdown } from "./popup.js";
import { state, emit } from "./state.js";

const MODE_LIST = ["Normal", "Dissolve", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light", "Color Dodge", "Color Burn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Color", "Luminosity"];

// The controls currently on the bar, in order, with a reader for each. The
// engine reads their values through `readOptions()` (M5-T01).
let fields = [];
let changeHandler = null;
const wired = new WeakSet();
// Each tool keeps its own values while the app runs, like Photoshop's option
// bar (M5-T09): switching tools and back restores them.
const memory = new Map();
let currentTool = null;

export function renderOptionsBar(container, toolId) {
  clear(container);
  fields = [];
  const schema = optionsFor(toolId) || [];
  // Style ▸ Width/Height: a schema entry may name the fields it enables
  // (`enables`), so the marquee's size fields are greyed out under Normal
  // (M5-T04). A dropdown pick has to re-run it: the value lives in a popup,
  // not in the container, so no DOM event reaches us from there.
  const sync = () => {
    for (const { spec, el } of fields) {
      if (spec?.type !== "select" || !spec.enables) continue;
      const value = el.querySelector(".ob-value").textContent;
      for (const field of fields) {
        if (!spec.enables.includes(field.key)) continue;
        field.el.classList.toggle("off", !value || value === "Normal");
        const input = field.el.querySelector("input");
        if (input) input.disabled = !value || value === "Normal";
      }
    }
  };
  currentTool = toolId;
  const remembered = memory.get(toolId) || {};
  for (const spec of schema) {
    const { el, read, write } = control(spec, sync);
    const key = keyOf(spec);
    fields.push({ key, spec, el, read, write });
    if (write && key in remembered) write(remembered[key]);
    container.append(el);
  }
  sync();
  container.append(h("span", { class: "ob-tail" }));
}

/** The id whose option bar is shown (a tool, or `"_transform"` while a Free Transform box is up). */
export function currentBar() {
  return currentTool;
}

/** The value of option `key` of the active tool, or `null`. */
export function optionValue(key) {
  const field = fields.find((f) => f.key === key);
  return field && field.read ? field.read() : null;
}

/**
 * Set option `key` of the active tool (the `[`/`]` size keys, the number
 * keys for opacity, brush presets — M5-T09) and tell the engine. Returns
 * whether the bar has that option.
 */
export function setOption(key, value) {
  const field = fields.find((f) => f.key === key);
  if (!field || !field.write) return false;
  field.write(value);
  remember();
  if (changeHandler) changeHandler(readOptions());
  return true;
}

/**
 * Show selection mode `index` on the Mode buttons while a modifier is held
 * (M5-T10), or the bar's own choice again with `null`. The engine is not
 * told: the modifier itself picks the mode at the press.
 */
export function showModeHint(index) {
  const field = fields.find((f) => f.key === "Mode" && f.spec.type === "btngroup");
  if (!field) return;
  const buttons = [...field.el.children];
  if (index == null) {
    if (field.hinted == null) return;
    buttons.forEach((b, j) => b.classList.toggle("on", j === field.hinted));
    field.hinted = null;
    return;
  }
  if (field.hinted == null) field.hinted = buttons.findIndex((b) => b.classList.contains("on"));
  buttons.forEach((b, j) => b.classList.toggle("on", j === index));
}

function remember() {
  if (currentTool) memory.set(currentTool, readOptions());
}

/**
 * The option bar's current values, keyed by the field text without the
 * trailing colon (the keys `UiToEngine::ToolOptions` expects). Controls that
 * carry no value (labels, gaps, buttons) are skipped.
 */
export function readOptions() {
  const out = {};
  for (const { key, read } of fields) {
    if (key && read) out[key] = read();
  }
  return out;
}

/**
 * Call `cb(options)` whenever a control on the bar changes (M5-T01). The
 * container's listeners are wired once, however often the bar is re-rendered.
 */
export function onOptionsChange(container, cb) {
  changeHandler = cb;
  if (wired.has(container)) return;
  wired.add(container);
  const notify = () => {
    remember();
    if (changeHandler) changeHandler(readOptions());
  };
  container.addEventListener("input", notify);
  container.addEventListener("change", notify);
  container.addEventListener("click", notify);
}

/**
 * The option key of a control: `key` when the schema names one (a button
 * group has no label to borrow), else its label text without the colon.
 */
function keyOf(spec) {
  if (spec.key) return spec.key;
  if (!spec.text) return null;
  return spec.text.replace(/:\s*$/, "");
}

/**
 * Build one control; `read` returns its value (or `null` when valueless), and
 * `changed` is called after a pick that a `select` has to react to.
 */
function control(spec, changed) {
  switch (spec.type) {
    case "gap": return { el: h("span", { class: "ob-gap" }), read: null };
    case "sep": return { el: h("span", { class: "ob-sep" }), read: null };
    case "label": return { el: h("span", { class: "ob-label", text: spec.text }), read: null };
    case "btn": return { el: h("button", { class: "ob-btn", type: "button", text: spec.text, onclick: () => emit("mock", spec.text) }), read: null };
    case "toggle": return toggle(spec);
    case "num": return num(spec);
    case "text": return textField(spec);
    case "range": return range(spec);
    case "select": return select(spec, changed);
    case "btngroup": return buttonGroup(spec);
    case "swatch": return swatch(spec);
    case "gradient": return gradient("Black to White");
    case "brushpreset": return brushPreset();
    default: return { el: h("span", { class: "ob-label", text: spec.text || spec.type }), read: null };
  }
}

function toggle(spec) {
  const box = h("span", { class: "ob-check" + (spec.on ? " on" : ""), role: "checkbox", "aria-checked": spec.on ? "true" : "false" },
    spec.on ? icon("i-check", "ic xs") : null);
  const wrap = h("button", {
    class: "ob-toggle" + (spec.disabled ? " off" : ""), type: "button", "data-tip": spec.text,
    "aria-pressed": spec.on ? "true" : "false",
    onclick: (e) => {
      e.stopPropagation();
      const on = !box.classList.contains("on");
      box.classList.toggle("on", on);
      box.setAttribute("aria-checked", on ? "true" : "false");
      clear(box);
      if (on) box.append(icon("i-check", "ic xs"));
      wrap.setAttribute("aria-pressed", on ? "true" : "false");
    },
  }, box, h("span", { class: "ob-text", text: spec.text }));
  const write = (on) => {
    box.classList.toggle("on", !!on);
    box.setAttribute("aria-checked", on ? "true" : "false");
    clear(box);
    if (on) box.append(icon("i-check", "ic xs"));
    wrap.setAttribute("aria-pressed", on ? "true" : "false");
  };
  return { el: wrap, read: () => wrap.getAttribute("aria-pressed") === "true", write };
}

function num(spec) {
  const input = h("input", {
    class: "ob-num" + (spec.disabled ? " off" : ""), type: "text", value: spec.value, inputmode: "decimal",
    style: { width: (spec.width || 48) + "px" }, disabled: spec.disabled || false,
  });
  const el = h("span", { class: "ob-field" }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null, input, spec.unit ? h("span", { class: "ob-unit", text: spec.unit }) : null);
  return { el, read: () => number(input.value), write: (v) => { input.value = String(v); } };
}

function textField(spec) {
  const input = h("input", { class: "ob-num", type: spec.password ? "password" : "text", value: spec.value, style: { width: (spec.width || 120) + "px" } });
  const el = h("span", { class: "ob-field" }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null, input);
  return { el, read: () => String(input.value) };
}

function range(spec) {
  const min = spec.min ?? 0;
  const max = spec.max ?? 100;
  const out = h("span", { class: "ob-unit val", text: spec.value + "%" });
  const input = h("input", { class: "ob-range", type: "range", min, max, value: spec.value });
  input.addEventListener("input", () => { out.textContent = input.value + "%"; });
  const el = h("span", { class: "ob-field" }, h("span", { class: "ob-label", text: spec.label }), input, out);
  const write = (v) => { input.value = String(v); out.textContent = input.value + "%"; };
  return { el, read: () => number(input.value), write };
}

function select(spec, changed) {
  const valueEl = h("span", { class: "ob-value", text: spec.value ?? (spec.options && spec.options[0]) ?? "" });
  const btn = h("button", {
    class: "ob-select" + (spec.disabled ? " off" : ""), type: "button", "data-tip": spec.label || spec.text || spec.value,
    onclick: (e) => {
      e.stopPropagation();
      if (spec.disabled) return;
      openDropdown({
        anchor: btn, items: spec.options, value: valueEl.textContent,
        width: Math.max(120, btn.offsetWidth),
        onPick: (v) => {
          valueEl.textContent = v;
          if (changed) changed();
          emit("mock", `${spec.label || "Value"}: ${v}`);
        },
      });
    },
  }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null, valueEl, icon("i-chevron-down", "ic xs"));
  if (spec.width) btn.style.minWidth = spec.width + "px";
  return { el: btn, read: () => String(valueEl.textContent), write: (v) => { valueEl.textContent = String(v); } };
}

function buttonGroup(spec) {
  const group = h("span", { class: "ob-btngroup" });
  (spec.icons || []).forEach((ic, i) => {
    // A group that acts (`actions`: one action id per icon) does not select
    // anything: its click is the engine's, like a menu item's (M6-T03's crop
    // ✓ and ✗).
    const action = spec.actions && spec.actions[i];
    const b = h("button", {
      class: "ob-iconbtn" + (i === spec.active ? " on" : ""), type: "button",
      "data-tip": (spec.titles && spec.titles[i]) || "",
      onclick: () => {
        if (action) return emit("action", action);
        [...group.children].forEach((c, j) => c.classList.toggle("on", j === i));
      },
    }, icon(ic, "ic"));
    group.append(b);
  });
  const write = (i) => [...group.children].forEach((c, j) => c.classList.toggle("on", j === i));
  return { el: group, read: () => [...group.children].findIndex((c) => c.classList.contains("on")), write };
}

function swatch(spec) {
  const el = h("button", {
    class: "ob-swatch" + (spec.outline ? " outline" : ""), type: "button", "data-tip": spec.title || "Colour",
    style: { background: spec.outline ? "#22242a" : state.colors.fg },
    onclick: () => emit("mock", spec.title || "Colour picker"),
  });
  return { el, read: null };
}

function gradient(label) {
  const el = h("button", { class: "ob-gradient", type: "button", "data-tip": label, onclick: () => emit("mock", "Gradient picker") },
    h("span", { class: "gradient-preview", style: { background: "linear-gradient(90deg,#0b0b0d,#ffffff)" } }),
    icon("i-chevron-down", "ic xs"));
  return { el, read: null };
}

// The built-in round brushes (M5-T09): name → [size px, hardness %].
const PRESETS = [
  ["Hard Round 5 px", 5, 100], ["Hard Round 19 px", 19, 100], ["Hard Round 60 px", 60, 100], ["Hard Round 200 px", 200, 100],
  ["Soft Round 19 px", 19, 0], ["Soft Round 60 px", 60, 0], ["Soft Round 200 px", 200, 0], ["Soft Round 500 px", 500, 0],
];

function brushPreset() {
  const el = h("button", {
    class: "ob-brush", type: "button", "data-tip": "Brush preset",
    onclick: (e) => {
      e.stopPropagation();
      openDropdown({
        anchor: el, items: PRESETS.map(([n]) => n), value: "", width: 180,
        onPick: (name) => {
          const preset = PRESETS.find(([n]) => n === name);
          if (!preset) return;
          setOption("Size", preset[1]);
          setOption("Hardness", preset[2]);
        },
      });
    },
  }, h("span", { class: "brush-thumb" }, h("span", { class: "brush-dot" })));
  return { el, read: null };
}

/** A numeric field's value, or `null` while it is empty or not a number. */
function number(text) {
  const value = parseFloat(text);
  return Number.isFinite(value) ? value : null;
}

export { MODE_LIST };
