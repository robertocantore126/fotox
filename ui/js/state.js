// Fotox — stato applicativo + emettitore minimale.

const listeners = new Map();

export const state = {
  tool: "move",
  zoom: 100,
  screenMode: "standard", // standard | menubar | full
  doc: { name: "Untitled-1", w: 1080, h: 1080, mode: "RGB", bits: 8 },
  tab: 0,
  flags: {
    rulers: false,
    grid: false,
    pixelgrid: false,
    guides: true,
    lockguides: false,
    snap: true,
    "snap-guides": true,
    "snap-grid": false,
    "snap-layers": true,
    "snap-slices": false,
    "snap-bounds": false,
    seledges: true,
    layeredges: false,
    targetpath: true,
    maskvis: false,
    channels: false,
    count: false,
    slices: false,
    notes: false,
    extras: true,
    "all-extras": false,
    panels: true,
    doctabs: true,
    "all-layer-edges": false,
    "sel-layer-edges": false,
    workspace: false,
    "win:1up": true,
    "par:square": true,
    "mode:rgb": true,
    "mode:8": true,
    "screen:standard": true,
    "screen:menubar": false,
    "screen:full": false,
  },
  /** pannelli aperti nel dock, per id */
  openPanels: {
    color: true, swatches: true, styles: true,
    layers: true, channels: true, paths: true,
    history: true, actions: true, layerscomps: true,
    adjustments: true, properties: true, histogram: true,
    navigator: true, info: true, measurements: true,
    character: false, paragraph: false,
    brush: false, "brush-settings": false, toolpresets: false,
    libraries: false, timeline: false, notes: false,
  },
  colors: { fg: "#1e1e22", bg: "#ffffff" },
};

export function on(evt, fn) {
  if (!listeners.has(evt)) listeners.set(evt, new Set());
  listeners.get(evt).add(fn);
  return () => listeners.get(evt).delete(fn);
}

export function emit(evt, payload) {
  const set = listeners.get(evt);
  if (set) for (const fn of [...set]) fn(payload);
}

export function setFlag(key, value) {
  state.flags[key] = value;
  emit("flag", key);
  emit("change", key);
}

export function toggleFlag(key) {
  setFlag(key, !state.flags[key]);
  return state.flags[key];
}

export function setTool(id) {
  if (state.tool === id) return;
  state.tool = id;
  emit("tool", id);
}

export function setZoom(z) {
  state.zoom = Math.max(3, Math.min(3200, Math.round(z)));
  emit("zoom", state.zoom);
}

export function setColors(fg, bg) {
  if (fg) state.colors.fg = fg;
  if (bg) state.colors.bg = bg;
  emit("colors");
}

/** Controlla le voci con spunta (usata dal motore dei menu). */
export function isChecked(key) {
  if (!key) return false;
  if (key.startsWith("panel:")) return !!state.openPanels[key.slice(6)];
  if (key.startsWith("screen:")) return state.flags[key] === true;
  return !!state.flags[key];
}
