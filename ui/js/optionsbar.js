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

export function renderOptionsBar(container, toolId) {
  clear(container);
  fields = [];
  const schema = optionsFor(toolId) || [];
  for (const spec of schema) {
    const { el, read } = control(spec);
    fields.push({ key: keyOf(spec), read });
    container.append(el);
  }
  container.append(h("span", { class: "ob-tail" }));
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
    if (changeHandler) changeHandler(readOptions());
  };
  container.addEventListener("input", notify);
  container.addEventListener("change", notify);
  container.addEventListener("click", notify);
}

/** The option key of a control: its label text without the trailing colon. */
function keyOf(spec) {
  if (!spec.text) return null;
  return spec.text.replace(/:\s*$/, "");
}

/** Build one control; `read` returns its value (or `null` when valueless). */
function control(spec) {
  switch (spec.type) {
    case "gap": return { el: h("span", { class: "ob-gap" }), read: null };
    case "sep": return { el: h("span", { class: "ob-sep" }), read: null };
    case "label": return { el: h("span", { class: "ob-label", text: spec.text }), read: null };
    case "btn": return { el: h("button", { class: "ob-btn", type: "button", text: spec.text, onclick: () => emit("mock", spec.text) }), read: null };
    case "toggle": return toggle(spec);
    case "num": return num(spec);
    case "text": return textField(spec);
    case "range": return range(spec);
    case "select": return select(spec);
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
  return { el: wrap, read: () => wrap.getAttribute("aria-pressed") === "true" };
}

function num(spec) {
  const input = h("input", {
    class: "ob-num" + (spec.disabled ? " off" : ""), type: "text", value: spec.value, inputmode: "decimal",
    style: { width: (spec.width || 48) + "px" }, disabled: spec.disabled || false,
  });
  const el = h("span", { class: "ob-field" }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null, input, spec.unit ? h("span", { class: "ob-unit", text: spec.unit }) : null);
  return { el, read: () => number(input.value) };
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
  return { el, read: () => number(input.value) };
}

function select(spec) {
  const valueEl = h("span", { class: "ob-value", text: spec.value ?? (spec.options && spec.options[0]) ?? "" });
  const btn = h("button", {
    class: "ob-select" + (spec.disabled ? " off" : ""), type: "button", "data-tip": spec.label || spec.text || spec.value,
    onclick: (e) => {
      e.stopPropagation();
      if (spec.disabled) return;
      openDropdown({
        anchor: btn, items: spec.options, value: valueEl.textContent,
        width: Math.max(120, btn.offsetWidth),
        onPick: (v) => { valueEl.textContent = v; emit("mock", `${spec.label || "Value"}: ${v}`); },
      });
    },
  }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null, valueEl, icon("i-chevron-down", "ic xs"));
  if (spec.width) btn.style.minWidth = spec.width + "px";
  return { el: btn, read: () => String(valueEl.textContent) };
}

function buttonGroup(spec) {
  const group = h("span", { class: "ob-btngroup" });
  (spec.icons || []).forEach((ic, i) => {
    const b = h("button", {
      class: "ob-iconbtn" + (i === spec.active ? " on" : ""), type: "button",
      "data-tip": (spec.titles && spec.titles[i]) || "",
      onclick: () => {
        [...group.children].forEach((c, j) => c.classList.toggle("on", j === i));
      },
    }, icon(ic, "ic"));
    group.append(b);
  });
  return { el: group, read: () => [...group.children].findIndex((c) => c.classList.contains("on")) };
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

function brushPreset() {
  const el = h("button", { class: "ob-brush", type: "button", "data-tip": "Brush preset", onclick: () => emit("panel:open", "brush") },
    h("span", { class: "brush-thumb" }, h("span", { class: "brush-dot" })));
  return { el, read: null };
}

/** A numeric field's value, or `null` while it is empty or not a number. */
function number(text) {
  const value = parseFloat(text);
  return Number.isFinite(value) ? value : null;
}

export { MODE_LIST };
