// Fotox — the Color panel: foreground / background swatches, a colour wheel
// (hue ring + saturation/brightness square, Rob 2026-09-29) or RGB sliders,
// and the R, G, B and hex fields. The panel edits the swatch that is active
// (click the other one to switch; click the active one for the Color Picker).
// Picking a colour is not a document edit: no history step.

import { h, icon } from "../el.js";
import { state, on, emit, setColors } from "../state.js";

const MODE_KEY = "fotox.colorPanel.mode";
const RING = 14; // hue ring thickness, px
const GAP = 4; // between the ring and the square

let root = null;
let target = "fg"; // which swatch the panel edits
let hsv = [0, 0, 0]; // kept here so hue survives black, white and greys
let lastHex = null; // the hex this panel last set (echoes are not re-read)

const clamp = (v, a, b) => Math.min(b, Math.max(a, v));

function parseHex(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(hex).trim());
  if (!m) return null;
  const v = parseInt(m[1], 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}
const toHex = (rgb) => "#" + rgb.map((c) => clamp(Math.round(c), 0, 255).toString(16).padStart(2, "0")).join("");

/** rgb 0..255 → [h 0..360, s 0..1, v 0..1]. */
function toHsv([r, g, b]) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  let hue = 0;
  if (d) {
    if (max === r) hue = ((g - b) / d) % 6;
    else if (max === g) hue = (b - r) / d + 2;
    else hue = (r - g) / d + 4;
    hue = (hue * 60 + 360) % 360;
  }
  return [hue, max ? d / max : 0, max];
}

function fromHsv([hue, s, v]) {
  const c = v * s, x = c * (1 - Math.abs(((hue / 60) % 2) - 1)), m = v - c;
  const [r, g, b] = hue < 60 ? [c, x, 0] : hue < 120 ? [x, c, 0] : hue < 180 ? [0, c, x] : hue < 240 ? [0, x, c] : hue < 300 ? [x, 0, c] : [c, 0, x];
  return [(r + m) * 255, (g + m) * 255, (b + m) * 255];
}

function mode() {
  try { return localStorage.getItem(MODE_KEY) || "wheel"; } catch { return "wheel"; }
}
function setMode(m) {
  try { localStorage.setItem(MODE_KEY, m); } catch { /* session only */ }
  render();
}

/** Set the edited swatch from the panel (the wheel, a field). */
function apply(rgb, fromHsvValue = null) {
  const hex = toHex(rgb);
  hsv = fromHsvValue || toHsv(rgb.map((c) => clamp(Math.round(c), 0, 255)));
  lastHex = hex;
  if (target === "fg") setColors(hex, null);
  else setColors(null, hex);
}

/** The Color panel element (persistent; it follows every colour change). */
export function colorPanel() {
  if (!root) {
    root = h("div", { class: "pcolor ncolor" });
    on("colors", () => sync());
    render();
  }
  return root;
}

/** Another part of the app changed a colour (eyedropper, swatches, swap). */
function sync() {
  const hex = (target === "fg" ? state.colors.fg : state.colors.bg).toLowerCase();
  if (hex !== lastHex) {
    const rgb = parseHex(hex);
    if (rgb) {
      const next = toHsv(rgb);
      // Greys have no hue: keep the one on the ring.
      hsv = next[1] === 0 || next[2] === 0 ? [hsv[0], next[1], next[2]] : next;
    }
    lastHex = hex;
  }
  refresh?.();
}

let refresh = null;

function render() {
  if (!root) return;
  root.replaceChildren();
  const current = () => (target === "fg" ? state.colors.fg : state.colors.bg);
  lastHex = current().toLowerCase();
  const rgb0 = parseHex(current()) || [0, 0, 0];
  const start = toHsv(rgb0);
  hsv = start[1] === 0 || start[2] === 0 ? [hsv[0], start[1], start[2]] : start;

  // Swatches: the active one is framed.
  const fg = h("button", { class: "big-swatch fg", type: "button" });
  const bg = h("button", { class: "big-swatch bg", type: "button" });
  const pick = (which) => {
    if (target === which) emit("ask-dialog", "color-picker");
    else { target = which; render(); }
  };
  fg.addEventListener("click", () => pick("fg"));
  bg.addEventListener("click", () => pick("bg"));
  const swatches = h("div", { class: "swatch-stack" }, bg, fg);

  const body = h("div", { class: "ncolor-body" });
  const fields = h("div", { class: "pcolor-fields" });
  const inputs = ["R", "G", "B"].map((label, k) => {
    const input = h("input", { class: "pf-num", type: "text" });
    const slider = h("input", { class: "pminirange", type: "range", min: 0, max: 255 });
    const set = (v) => {
      const rgb = parseHex(current()) || [0, 0, 0];
      rgb[k] = clamp(Number(v) || 0, 0, 255);
      apply(rgb);
    };
    input.addEventListener("change", () => set(input.value));
    slider.addEventListener("input", () => set(slider.value));
    const row = h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: label }), input);
    if (mode() === "sliders") row.append(slider);
    fields.append(row);
    return { input, slider };
  });
  const hex = h("input", { class: "pf-num hex", type: "text" });
  hex.addEventListener("change", () => {
    const rgb = parseHex(hex.value);
    if (rgb) apply(rgb);
  });
  fields.append(h("div", { class: "pf-row narrow" }, h("span", { class: "pf-label", text: "#" }), hex));

  let drawWheel = () => {};
  if (mode() === "wheel") {
    const wheel = h("canvas", { class: "ncolor-wheel", "data-tip": "Ring: hue · square: saturation (→) and brightness (↑)" });
    body.append(wheel);
    drawWheel = wheelControl(wheel);
  }
  body.append(fields);

  const menu = h("button", {
    class: "pbar-btn", type: "button", "data-tip": mode() === "wheel" ? "Show RGB sliders" : "Show the colour wheel",
    onclick: () => setMode(mode() === "wheel" ? "sliders" : "wheel"),
  }, icon(mode() === "wheel" ? "i-props" : "i-color", "ic sm"));
  root.append(h("div", { class: "ncolor-top" }, swatches, h("span", { class: "pf-label ncolor-target", text: target === "fg" ? "Foreground" : "Background" })), body,
    h("div", { class: "pbar" }, h("span", { class: "pbar-gap" }), menu));

  refresh = () => {
    fg.style.background = state.colors.fg;
    bg.style.background = state.colors.bg;
    fg.classList.toggle("editing", target === "fg");
    bg.classList.toggle("editing", target === "bg");
    const rgb = parseHex(current()) || [0, 0, 0];
    inputs.forEach(({ input, slider }, k) => {
      if (document.activeElement !== input) input.value = String(rgb[k]);
      slider.value = String(rgb[k]);
    });
    if (document.activeElement !== hex) hex.value = current().replace("#", "").toUpperCase();
    drawWheel();
  };
  refresh();
}

/**
 * The wheel on `canvas`: a hue ring around a saturation/brightness square
 * for the ring's hue. Returns the redraw function.
 */
function wheelControl(canvas) {
  const dpr = window.devicePixelRatio || 1;
  let size = 0;
  let ringImage = null;
  const geometry = () => {
    const outer = size / 2;
    const inner = outer - RING;
    const half = (inner - GAP) / Math.SQRT2; // the square fits inside the ring
    return { c: size / 2, outer, inner, half };
  };
  const draw = () => {
    const width = Math.max(120, Math.min(220, (canvas.parentElement?.clientWidth || 200) - 8));
    if (width !== size) {
      size = width;
      canvas.width = size * dpr;
      canvas.height = size * dpr;
      canvas.style.width = size + "px";
      canvas.style.height = size + "px";
      ringImage = null;
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const { c, outer, inner, half } = geometry();
    ctx.clearRect(0, 0, size, size);
    // Ring (cached: it never changes).
    if (!ringImage) {
      const conic = ctx.createConicGradient(-Math.PI / 2, c, c);
      for (let a = 0; a <= 360; a += 30) conic.addColorStop(a / 360, `hsl(${a},100%,50%)`);
      ctx.fillStyle = conic;
      ctx.beginPath();
      ctx.arc(c, c, outer, 0, Math.PI * 2);
      ctx.arc(c, c, inner, 0, Math.PI * 2, true);
      ctx.fill();
      ringImage = ctx.getImageData(0, 0, canvas.width, canvas.height);
    } else {
      ctx.putImageData(ringImage, 0, 0);
    }
    // Square: white → hue across, black upward from the bottom.
    const x0 = c - half, y0 = c - half, side = half * 2;
    const across = ctx.createLinearGradient(x0, 0, x0 + side, 0);
    across.addColorStop(0, "#fff");
    across.addColorStop(1, `hsl(${hsv[0]},100%,50%)`);
    ctx.fillStyle = across;
    ctx.fillRect(x0, y0, side, side);
    const down = ctx.createLinearGradient(0, y0, 0, y0 + side);
    down.addColorStop(0, "rgba(0,0,0,0)");
    down.addColorStop(1, "#000");
    ctx.fillStyle = down;
    ctx.fillRect(x0, y0, side, side);
    // Markers.
    const a = ((hsv[0] - 90) * Math.PI) / 180;
    const r = (outer + inner) / 2;
    marker(ctx, c + Math.cos(a) * r, c + Math.sin(a) * r, RING / 2 - 1);
    marker(ctx, x0 + hsv[1] * side, y0 + (1 - hsv[2]) * side, 5);
  };

  let dragging = null; // "ring" | "square"
  // Pointer → wheel coordinates; CSS may show the canvas smaller than drawn.
  const at = (e) => {
    const b = canvas.getBoundingClientRect();
    const k = b.width ? size / b.width : 1;
    return [(e.clientX - b.left) * k, (e.clientY - b.top) * k];
  };
  const update = (e) => {
    const [x, y] = at(e);
    const { c, half } = geometry();
    if (dragging === "ring") {
      const hue = ((Math.atan2(y - c, x - c) * 180) / Math.PI + 90 + 360) % 360;
      const next = [hue, hsv[1], hsv[2]];
      apply(fromHsv(next), next);
    } else {
      const s = clamp((x - (c - half)) / (half * 2), 0, 1);
      const v = clamp(1 - (y - (c - half)) / (half * 2), 0, 1);
      const next = [hsv[0], s, v];
      apply(fromHsv(next), next);
    }
  };
  canvas.addEventListener("pointerdown", (e) => {
    const [x, y] = at(e);
    const { c, inner, outer, half } = geometry();
    const d = Math.hypot(x - c, y - c);
    if (d >= inner - 2 && d <= outer + 2) dragging = "ring";
    else if (Math.abs(x - c) <= half + 4 && Math.abs(y - c) <= half + 4) dragging = "square";
    else return;
    canvas.setPointerCapture(e.pointerId);
    update(e);
  });
  canvas.addEventListener("pointermove", (e) => { if (dragging) update(e); });
  const stop = () => { dragging = null; };
  canvas.addEventListener("pointerup", stop);
  canvas.addEventListener("pointercancel", stop);
  return draw;
}

function marker(ctx, x, y, r) {
  ctx.lineWidth = 2;
  ctx.strokeStyle = "#000";
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.stroke();
  ctx.lineWidth = 1.2;
  ctx.strokeStyle = "#fff";
  ctx.beginPath();
  ctx.arc(x, y, r - 1, 0, Math.PI * 2);
  ctx.stroke();
}
