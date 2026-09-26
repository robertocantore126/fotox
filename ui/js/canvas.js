// Fotox — area di lavoro: disegna un documento finto, gestisce zoom, pan,
// righelli e guide. Il contenuto è interamente generato dal codice.

import { initGuides } from "./native/guides.js";
import { h, icon, clear } from "./el.js";
import { state, setZoom, emit, on } from "./state.js";
import * as bridge from "./native/bridge.js";
import { UI, ENGINE } from "./native/protocol.js";
import { initDocumentTabs } from "./native/documents.js";

let canvasEl = null;
let scrollEl = null;
let wrapEl = null;
let rulerTop = null;
let rulerLeft = null;
let guidesLayer = null;
let zoomMode = "fit"; // fit | custom
// Native mode: the latest `view` message from the engine and the #viewport
// element the rulers measure.
let nativeView = null;
let viewportEl = null;
let fpsOverlay = null;

const RULER = 18;

export function getDocCanvas() {
  return canvasEl;
}

export function initWorkspace(host) {
  clear(host);
  host.classList.add("workspace");

  const tabs = h("div", { class: "doctabs", id: "doctabs" });
  const tab = h("div", { class: "doctab active" },
    icon("i-image", "ic sm"),
    h("span", { class: "doctab-label", text: `${state.doc.name} @ ${state.zoom}% (${state.doc.mode}/${state.doc.bits})` }),
    h("button", { class: "doctab-x", type: "button", "data-tip": "Close document", onclick: () => emit("mock", "Close document") }, icon("i-close", "ic xs")));
  const newTab = h("button", { class: "doctab-add", type: "button", "data-tip": "Create a new document", onclick: () => emit("action", "dlg:new-doc") }, icon("i-plus", "ic sm"));
  tabs.append(tab, newTab);

  const rulerCorner = h("div", { class: "ruler-corner", text: "px" });
  rulerTop = h("canvas", { class: "ruler ruler-top", width: 600, height: RULER });
  rulerLeft = h("canvas", { class: "ruler ruler-left", width: RULER, height: 600 });

  if (bridge.isNative) {
    // Inside the app the document is drawn natively under a transparent hole
    // (docs/ARCHITECTURE.md §2): no demo canvas, no DOM scrolling.
    const viewport = h("div", { id: "viewport" });
    const gridLayer = h("div", { class: "grid-layer" });
    fpsOverlay = h("div", { class: "fps-overlay", hidden: true });
    const rulerRow = h("div", { class: "ruler-row" }, rulerCorner, rulerTop);
    const body = h("div", { class: "workspace-body" },
      h("div", { class: "ruler-col" }, rulerLeft),
      h("div", { class: "workspace-main" }, viewport, gridLayer, fpsOverlay));
    host.append(tabs, rulerRow, body);
    viewportEl = viewport;
    // Guides, grid and snapping are the engine's (M7-T06); the CSS grid mock is off.
    gridLayer.hidden = true;
    initGuides(rulerTop, rulerLeft, viewport);
    bridge.on(ENGINE.STATUS, showStatus);
    // Tabs come from the engine's documents, not the demo document.
    initDocumentTabs(tabs, newTab);
    on("flag", (key) => {
      if (key === "rulers") applyRulers();
      if (key === "grid" || key === "pixelgrid") applyGrid();
    });
    // The engine owns zoom and pan: rulers, tab label and status bar follow
    // its `view` messages (docs/PROTOCOL.md §5).
    bridge.on(ENGINE.VIEW, (view) => {
      nativeView = view;
      state.zoom = view.zoom * 100;
      updateStatusZoom(state.zoom); // tab labels: native/documents.js
      drawRulers();
    });
    requestAnimationFrame(() => { applyRulers(); applyGrid(); });
    reportViewportBounds(viewport);
    return { setZoom: zoomTo, zoomIn, zoomOut, fit, actual };
  }

  canvasEl = h("canvas", { class: "doc-canvas", width: state.doc.w, height: state.doc.h });
  drawArtwork(canvasEl.getContext("2d"), state.doc.w, state.doc.h);

  guidesLayer = h("div", { class: "guides-layer" },
    h("span", { class: "guide v", style: { left: "25%" } }),
    h("span", { class: "guide h", style: { top: "33%" } }));

  const stage = h("div", { class: "stage" }, canvasEl, guidesLayer);
  scrollEl = h("div", { class: "doc-scroll" }, stage);

  const gridLayer = h("div", { class: "grid-layer" });
  const rulerRow = h("div", { class: "ruler-row" }, rulerCorner, rulerTop);
  const body = h("div", { class: "workspace-body" },
    h("div", { class: "ruler-col" }, rulerLeft),
    h("div", { class: "workspace-main" }, scrollEl, gridLayer));

  host.append(tabs, rulerRow, body);

  scrollEl.addEventListener("scroll", () => { drawRulers(); positionGuides(); });
  initPanning();

  on("zoom", (z) => { applyZoom(z); updateStatusZoom(z); });
  on("flag", (key) => {
    if (key === "rulers") applyRulers();
    if (key === "grid" || key === "pixelgrid") applyGrid();
    if (key === "guides") applyGuides();
  });
  on("colors", () => {});

  // misura disponibile e adatta lo zoom
  requestAnimationFrame(() => {
    applyZoom(state.zoom);
    applyRulers();
    applyGrid();
    applyGuides();
    drawRulers();
    positionGuides();
    fitInitial();
  });

  window.addEventListener("resize", () => {
    if (zoomMode === "fit") fit();
    else applyZoom(state.zoom);
    drawRulers();
  });

  return { setZoom: zoomTo, zoomIn, zoomOut, fit, actual };
}

/* ------------------------------------------------ status and fps overlay */

/** Show or hide the frame-time overlay (action `debug:fps`, Ctrl+Alt+F). */
export function toggleFpsOverlay() {
  if (fpsOverlay) fpsOverlay.hidden = !fpsOverlay.hidden;
}

// `status` from the engine, twice per second: memory in the status bar, frame
// statistics in the overlay (docs/PROTOCOL.md §5).
function showStatus(s) {
  const gb = (bytes) => (bytes / 1e9).toFixed(bytes < 1e8 ? 2 : 1);
  const mem = document.getElementById("statusmem");
  if (mem) {
    const m = s.memory;
    mem.textContent = `RAM ${gb(m.hot_bytes + m.warm_bytes)} GB`;
    mem.title = `Tiles in RAM ${gb(m.hot_bytes)} GB · compressed ${gb(m.warm_bytes)} GB · ` +
      `scratch disk ${gb(m.scratch_bytes)} GB · GPU ${gb(m.gpu_bytes)} GB`;
  }
  if (fpsOverlay && !fpsOverlay.hidden) {
    fpsOverlay.textContent =
      `${s.fps.toFixed(0)} fps\n` +
      `frame p50 ${s.frame_ms_p50.toFixed(1)} ms · p99 ${s.frame_ms_p99.toFixed(1)} ms\n` +
      `uploads ${s.uploads} · loading ${s.pending_loads}` +
      // Brush input → pixels (M5-T11), while painting.
      (s.input_latency_ms_p99 ? `\ninput p50 ${s.input_latency_ms_p50.toFixed(1)} ms · p99 ${s.input_latency_ms_p99.toFixed(1)} ms` : "");
  }
}

/* ------------------------------------------------------- native viewport */

// Tell the shell where the viewport hole is, in physical window pixels
// (`viewport_bounds`, docs/PROTOCOL.md §4). The hole moves or resizes when the
// window resizes, the dock is dragged, rulers/tabs are toggled or the screen
// mode changes; every one of those resizes #viewport or one of the boxes
// around it, so observing them all catches every layout change. Unchanged
// rectangles are not re-sent.
function reportViewportBounds(viewport) {
  let last = "";
  const sendBounds = () => {
    const rect = viewport.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const bounds = { x: rect.left * dpr, y: rect.top * dpr, width: rect.width * dpr, height: rect.height * dpr };
    const key = `${bounds.x},${bounds.y},${bounds.width},${bounds.height}`;
    if (key === last) return;
    last = key;
    bridge.send({ type: UI.VIEWPORT_BOUNDS, ...bounds });
    drawRulers();
  };
  const observer = new ResizeObserver(sendBounds);
  for (const el of [viewport, document.getElementById("app"), document.querySelector(".workspace"), document.querySelector(".middle")]) {
    if (el) observer.observe(el);
  }
  window.addEventListener("resize", sendBounds);
  // Moving to a monitor with another scale changes the physical rectangle
  // without changing the CSS one, so no observer above fires.
  const watchScale = () => {
    matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`)
      .addEventListener("change", () => { sendBounds(); watchScale(); }, { once: true });
  };
  watchScale();
  sendBounds();
}

/* ------------------------------------------------------------- disegno */

function drawArtwork(ctx, w, hgt) {
  // cielo
  const sky = ctx.createLinearGradient(0, 0, w * 0.3, hgt);
  sky.addColorStop(0, "#101830");
  sky.addColorStop(0.45, "#2a3f6b");
  sky.addColorStop(0.72, "#7a5aa8");
  sky.addColorStop(1, "#e08a6a");
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, w, hgt);

  // sole
  const sunX = w * 0.68;
  const sunY = hgt * 0.46;
  const glow = ctx.createRadialGradient(sunX, sunY, 4, sunX, sunY, w * 0.3);
  glow.addColorStop(0, "rgba(255,236,190,0.95)");
  glow.addColorStop(0.25, "rgba(255,196,120,0.45)");
  glow.addColorStop(1, "rgba(255,150,90,0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, w, hgt);
  ctx.beginPath();
  ctx.arc(sunX, sunY, w * 0.052, 0, Math.PI * 2);
  ctx.fillStyle = "#fff2cf";
  ctx.fill();

  // stelle
  for (let i = 0; i < 90; i++) {
    const x = Math.random() * w;
    const y = Math.random() * hgt * 0.42;
    const r = Math.random() * 1.6 + 0.4;
    ctx.globalAlpha = 0.25 + Math.random() * 0.6;
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  // montagne su tre piani
  const layers = [
    ["#1b2340", 0.62, 0.16],
    ["#141a30", 0.72, 0.13],
    ["#0c1020", 0.83, 0.1],
  ];
  for (const [color, base, amp] of layers) {
    ctx.beginPath();
    ctx.moveTo(0, hgt);
    const y0 = hgt * base;
    ctx.lineTo(0, y0);
    const peaks = 5 + Math.floor(amp * 40);
    for (let i = 0; i <= peaks; i++) {
      const x = (w / peaks) * i;
      const y = y0 - Math.sin(i * 1.7) * hgt * amp - Math.sin(i * 0.6) * hgt * amp * 0.6;
      ctx.lineTo(x, y);
    }
    ctx.lineTo(w, hgt);
    ctx.closePath();
    ctx.fillStyle = color;
    ctx.fill();
  }

  // riflesso sotto le montagne
  const refl = ctx.createLinearGradient(0, hgt * 0.9, 0, hgt);
  refl.addColorStop(0, "rgba(255,214,160,0.16)");
  refl.addColorStop(0.5, "rgba(255,214,160,0.06)");
  refl.addColorStop(1, "rgba(255,214,160,0)");
  ctx.fillStyle = refl;
  ctx.fillRect(0, hgt * 0.9, w, hgt * 0.1);

  // blocco titolo in stile locandina
  ctx.fillStyle = "rgba(10,12,20,0.55)";
  const bx = w * 0.08;
  const by = hgt * 0.62;
  const bw = w * 0.5;
  const bh = hgt * 0.22;
  ctx.beginPath();
  ctx.roundRect ? ctx.roundRect(bx, by, bw, bh, w * 0.02) : ctx.rect(bx, by, bw, bh);
  ctx.fill();
  ctx.fillStyle = "#ffffff";
  ctx.font = `600 ${Math.round(w * 0.052)}px "Segoe UI", system-ui, sans-serif`;
  ctx.fillText("Golden Hour", bx + w * 0.03, by + hgt * 0.075);
  ctx.font = `400 ${Math.round(w * 0.024)}px "Segoe UI", system-ui, sans-serif`;
  ctx.fillStyle = "rgba(255,255,255,0.78)";
  ctx.fillText("A Fotox demo document — replace it with your own image.", bx + w * 0.03, by + hgt * 0.115);
  ctx.fillStyle = "rgba(255,255,255,0.16)";
  ctx.fillRect(bx + w * 0.03, by + hgt * 0.14, bw * 0.6, 2);

  // piccola etichetta in alto a destra, per dare l'idea di un design
  ctx.fillStyle = "rgba(255,255,255,0.82)";
  const lx = w * 0.63;
  const ly = hgt * 0.05;
  const lw = w * 0.29;
  const lh = hgt * 0.042;
  ctx.beginPath();
  ctx.roundRect ? ctx.roundRect(lx, ly, lw, lh, lh / 2) : ctx.rect(lx, ly, lw, lh);
  ctx.fill();
  ctx.fillStyle = "rgba(20,22,30,0.85)";
  ctx.font = `500 ${Math.round(w * 0.019)}px "Segoe UI", system-ui, sans-serif`;
  ctx.fillText("FOTOX STUDIO", lx + lw * 0.13, ly + lh * 0.68);
}

/* --------------------------------------------------------------- zoom/pan */

function applyZoom(z) {
  if (!canvasEl) return;
  canvasEl.style.width = (state.doc.w * z / 100) + "px";
  canvasEl.style.height = (state.doc.h * z / 100) + "px";
  canvasEl.classList.toggle("smooth", z < 120);
  updateZoomLabels(z);
  applyGrid();
  drawRulers();
  positionGuides();
}

// In native mode the engine owns the view: the zoom:* action ids reach it
// through actions.js, and explicit values go out as `set_zoom`.
export function zoomTo(z) {
  if (bridge.isNative) {
    bridge.send({ type: UI.SET_ZOOM, doc: nativeView ? nativeView.doc : 0, zoom: z / 100 });
    return;
  }
  zoomMode = "custom";
  setZoom(z);
}

export function zoomIn() {
  if (bridge.isNative) return;
  setZoom(state.zoom * 1.25);
}

export function zoomOut() {
  if (bridge.isNative) return;
  setZoom(state.zoom / 1.25);
}

export function actual() {
  if (bridge.isNative) return;
  setZoom(100);
  center();
}

export function fit() {
  if (bridge.isNative) return;
  zoomMode = "fit";
  if (!scrollEl) return;
  const pad = 64;
  const z = Math.min((scrollEl.clientWidth - pad) / state.doc.w, (scrollEl.clientHeight - pad) / state.doc.h) * 100;
  setZoom(z);
  requestAnimationFrame(center);
}

function fitInitial() {
  fit();
}

function center() {
  if (!scrollEl) return;
  scrollEl.scrollLeft = (scrollEl.scrollWidth - scrollEl.clientWidth) / 2;
  scrollEl.scrollTop = (scrollEl.scrollHeight - scrollEl.clientHeight) / 2;
  drawRulers();
  positionGuides();
}

function initPanning() {
  let panning = false;
  let start = null;
  scrollEl.addEventListener("mousedown", (e) => {
    const isHand = state.tool === "hand" || e.button === 1 || (e.altKey && e.button === 0);
    if (!isHand) return;
    panning = true;
    start = { x: e.clientX, y: e.clientY, left: scrollEl.scrollLeft, top: scrollEl.scrollTop };
    scrollEl.classList.add("panning");
    e.preventDefault();
  });
  document.addEventListener("mousemove", (e) => {
    if (!panning) return;
    scrollEl.scrollLeft = start.left - (e.clientX - start.x);
    scrollEl.scrollTop = start.top - (e.clientY - start.y);
  });
  document.addEventListener("mouseup", () => {
    if (!panning) return;
    panning = false;
    scrollEl.classList.remove("panning");
  });
  scrollEl.addEventListener("wheel", (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    setZoom(state.zoom * (e.deltaY < 0 ? 1.1 : 0.9));
  }, { passive: false });
  scrollEl.addEventListener("dblclick", () => { if (state.tool === "zoom") actual(); });
}

/* -------------------------------------------------------------- righelli */

function applyRulers() {
  const on = state.flags.rulers;
  const host = document.querySelector(".workspace");
  if (!host) return;
  host.classList.toggle("no-rulers", !on);
  if (on) requestAnimationFrame(drawRulers);
}

// Per axis: CSS pixels per document pixel, and the document coordinate at
// CSS position 0 of the ruler. Browser mode reads the DOM scroll position;
// native mode maps the engine's view (zoom is in *physical* pixels per
// document pixel, centred in the viewport).
function rulerMapping() {
  if (bridge.isNative) {
    if (!nativeView || !viewportEl) return null;
    const dpr = window.devicePixelRatio || 1;
    const rect = viewportEl.getBoundingClientRect();
    const scale = nativeView.zoom / dpr;
    return {
      w: rect.width, h: rect.height, scale,
      x0: nativeView.center_x - rect.width / 2 / scale,
      y0: nativeView.center_y - rect.height / 2 / scale,
    };
  }
  if (!scrollEl) return null;
  const scale = state.zoom / 100;
  const docLeft = (scrollEl.scrollWidth - state.doc.w * scale) / 2;
  const docTop = (scrollEl.scrollHeight - state.doc.h * scale) / 2;
  return {
    w: scrollEl.clientWidth, h: scrollEl.clientHeight, scale,
    x0: (scrollEl.scrollLeft - docLeft) / scale,
    y0: (scrollEl.scrollTop - docTop) / scale,
  };
}

function drawRulers() {
  if (!rulerTop || !rulerLeft || !state.flags.rulers) return;
  const m = rulerMapping();
  if (!m) return;
  const dpr = window.devicePixelRatio || 1;
  const w = m.w;
  const hgt = m.h;
  const scale = m.scale;
  for (const [cv, size, horizontal] of [[rulerTop, w, true], [rulerLeft, hgt, false]]) {
    cv.width = Math.max(1, Math.floor(size * dpr));
    cv.height = Math.floor(RULER * dpr);
    if (!horizontal) { cv.width = Math.floor(RULER * dpr); cv.height = Math.max(1, Math.floor(size * dpr)); }
    const ctx = cv.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, size, RULER);
    ctx.fillStyle = "#33363d";
    ctx.fillRect(0, 0, horizontal ? size : RULER, horizontal ? RULER : size);

    const docAt0 = horizontal ? m.x0 : m.y0;
    // scala dei tick: la più piccola che lascia almeno 44 px tra le etichette
    const steps = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 2000, 2500, 5000, 10000, 25000];
    let step = steps.find((s) => s * scale >= 44) || 50000;
    ctx.font = '9px "Segoe UI", system-ui, sans-serif';
    ctx.fillStyle = "#9aa0ab";
    ctx.strokeStyle = "#5b616c";
    ctx.lineWidth = 1;
    const from = Math.floor(docAt0 / step) * step - step * 2;
    const to = from + (size / scale) + step * 4;
    for (let doc = from; doc <= to; doc += step) {
      const p = (doc - docAt0) * scale;
      if (p < -60 || p > size + 60) continue;
      ctx.beginPath();
      if (horizontal) { ctx.moveTo(Math.round(p) + 0.5, RULER - 5); ctx.lineTo(Math.round(p) + 0.5, RULER); }
      else { ctx.moveTo(RULER - 5, Math.round(p) + 0.5); ctx.lineTo(RULER, Math.round(p) + 0.5); }
      ctx.stroke();
      if (horizontal) {
        ctx.save();
        ctx.translate(Math.round(p) + 3, 9);
        ctx.fillText(String(Math.round(doc)), 0, 0);
        ctx.restore();
      } else {
        ctx.save();
        ctx.translate(4, Math.round(p) + 3);
        ctx.rotate(-Math.PI / 2);
        ctx.fillText(String(Math.round(doc)), 0, 0);
        ctx.restore();
      }
    }
  }
}

function applyGrid() {
  const host = document.querySelector(".workspace");
  // In the app the engine draws the grid (M7-T06). FAST: no pixel grid there.
  if (!host || bridge.isNative) return;
  host.classList.toggle("show-grid", !!state.flags.grid);
  host.classList.toggle("show-pixelgrid", !!state.flags.pixelgrid);
  const layer = host.querySelector(".grid-layer");
  if (layer && state.flags.grid) {
    const step = Math.max(8, 100 * state.zoom / 100);
    layer.style.backgroundImage = `linear-gradient(to right, rgba(255,255,255,.14) 1px, transparent 1px), linear-gradient(to bottom, rgba(255,255,255,.14) 1px, transparent 1px)`;
    layer.style.backgroundSize = `${step}px ${step}px`;
  }
}

function applyGuides() {
  if (guidesLayer) guidesLayer.style.display = state.flags.guides ? "" : "none";
}

function positionGuides() {
  if (!guidesLayer || !canvasEl || !scrollEl) return;
  const scale = state.zoom / 100;
  guidesLayer.style.width = canvasEl.style.width;
  guidesLayer.style.height = canvasEl.style.height;
  const v = guidesLayer.querySelector(".guide.v");
  const hg = guidesLayer.querySelector(".guide.h");
  if (v) v.style.left = state.doc.w * 0.25 * scale + "px";
  if (hg) hg.style.top = state.doc.h * 0.33 * scale + "px";
}

function updateStatusZoom(z) {
  const el = document.getElementById("statuszoom");
  if (el) el.querySelector(".pf-value").textContent = formatZoom(z) + "%";
}

function updateZoomLabels(z) {
  const label = document.querySelector(".doctab-label");
  if (label) label.textContent = `${state.doc.name} @ ${formatZoom(z)}% (${state.doc.mode}/${state.doc.bits})`;
  updateStatusZoom(z);
}

// Photoshop shows fractions below 10 % ("3.33%") and whole numbers above.
function formatZoom(z) {
  return z < 10 ? String(Math.round(z * 100) / 100) : String(Math.round(z));
}
