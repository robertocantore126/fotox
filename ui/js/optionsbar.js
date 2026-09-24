// Fotox — barra opzioni contestuale allo strumento attivo.

import { h, icon, clear } from "./el.js";
import { optionsFor } from "./data/options.js";
import { openDropdown } from "./popup.js";
import { state, emit } from "./state.js";

const MODE_LIST = ["Normal", "Dissolve", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light", "Color Dodge", "Color Burn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Color", "Luminosity"];

export function renderOptionsBar(container, toolId) {
  clear(container);
  const schema = optionsFor(toolId) || [];
  const fam = toolId;
  for (const spec of schema) container.append(control(spec, fam));
  container.append(h("span", { class: "ob-tail" }));
}

function control(spec, fam) {
  switch (spec.type) {
    case "gap": return h("span", { class: "ob-gap" });
    case "sep": return h("span", { class: "ob-sep" });
    case "label": return h("span", { class: "ob-label", text: spec.text });
    case "btn": return h("button", { class: "ob-btn", type: "button", text: spec.text, onclick: () => emit("mock", spec.text) });
    case "toggle": return toggle(spec);
    case "num": return num(spec);
    case "text": return textField(spec);
    case "range": return range(spec);
    case "select": return select(spec);
    case "btngroup": return buttonGroup(spec);
    case "swatch": return swatch(spec);
    case "gradient": return gradient("Black to White");
    case "brushpreset": return brushPreset();
    default: return h("span", { class: "ob-label", text: spec.text || spec.type });
  }
}

function toggle(spec) {
  const box = h("span", { class: "ob-check" + (spec.on ? " on" : ""), role: "checkbox", "aria-checked": spec.on ? "true" : "false" },
    spec.on ? icon("i-check", "ic xs") : null);
  const wrap = h("button", {
    class: "ob-toggle" + (spec.disabled ? " off" : ""), type: "button", "data-tip": spec.text,
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
  return wrap;
}

function num(spec) {
  const input = h("input", {
    class: "ob-num" + (spec.disabled ? " off" : ""), type: "text", value: spec.value, inputmode: "decimal",
    style: { width: (spec.width || 48) + "px" }, disabled: spec.disabled || false,
  });
  return h("span", { class: "ob-field" }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null, input, spec.unit ? h("span", { class: "ob-unit", text: spec.unit }) : null);
}

function textField(spec) {
  return h("span", { class: "ob-field" }, spec.label ? h("span", { class: "ob-label", text: spec.label }) : null,
    h("input", { class: "ob-num", type: spec.password ? "password" : "text", value: spec.value, style: { width: (spec.width || 120) + "px" } }));
}

function range(spec) {
  const min = spec.min ?? 0;
  const max = spec.max ?? 100;
  const out = h("span", { class: "ob-unit val", text: spec.value + "%" });
  const input = h("input", { class: "ob-range", type: "range", min, max, value: spec.value });
  input.addEventListener("input", () => { out.textContent = input.value + "%"; });
  return h("span", { class: "ob-field" }, h("span", { class: "ob-label", text: spec.label }), input, out);
}

function select(spec) {
  const valueEl = h("span", { class: "ob-value", text: spec.value ?? (spec.options && spec.options[0]) ?? "" });
  const btn = h("button", {
    class: "ob-select" + (spec.disabled ? " off" : ""), type: "button", "data-tip": spec.label || spec.value,
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
  return btn;
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
  return group;
}

function swatch(spec) {
  return h("button", {
    class: "ob-swatch" + (spec.outline ? " outline" : ""), type: "button", "data-tip": spec.title || "Colour",
    style: { background: spec.outline ? "#22242a" : state.colors.fg },
    onclick: () => emit("mock", spec.title || "Colour picker"),
  });
}

function gradient(label) {
  return h("button", { class: "ob-gradient", type: "button", "data-tip": label, onclick: () => emit("mock", "Gradient picker") },
    h("span", { class: "gradient-preview", style: { background: "linear-gradient(90deg,#0b0b0d,#ffffff)" } }),
    icon("i-chevron-down", "ic xs"));
}

function brushPreset() {
  return h("button", { class: "ob-brush", type: "button", "data-tip": "Brush preset", onclick: () => emit("panel:open", "brush") },
    h("span", { class: "brush-thumb" }, h("span", { class: "brush-dot" })));
}

export { MODE_LIST };
