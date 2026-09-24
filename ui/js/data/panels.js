// Fotox — layout del dock e definizione dei pannelli.
// kind indica al motore (js/panels.js) quale renderer usare per il contenuto.

export const panelDefs = {
  color: { title: "Color", icon: "i-color", kind: "color" },
  swatches: { title: "Swatches", icon: "i-swatches", kind: "swatches" },
  styles: { title: "Styles", icon: "i-styles", kind: "styles" },
  layers: { title: "Layers", icon: "i-layers", kind: "layers" },
  channels: { title: "Channels", icon: "i-channels", kind: "channels" },
  paths: { title: "Paths", icon: "i-paths", kind: "paths" },
  history: { title: "History", icon: "i-history", kind: "history" },
  actions: { title: "Actions", icon: "i-actions", kind: "actions" },
  layerscomps: { title: "Layer Comps", icon: "i-presets", kind: "simple", note: "No layer comps yet" },
  adjustments: { title: "Adjustments", icon: "i-adjust", kind: "adjustments" },
  properties: { title: "Properties", icon: "i-props", kind: "properties" },
  histogram: { title: "Histogram", icon: "i-props", kind: "histogram" },
  navigator: { title: "Navigator", icon: "i-navigator", kind: "navigator" },
  info: { title: "Info", icon: "i-info", kind: "info" },
  measurements: { title: "Measurements", icon: "i-ruler", kind: "measurements" },
  character: { title: "Character", icon: "i-type", kind: "character" },
  paragraph: { title: "Paragraph", icon: "i-quote", kind: "paragraph" },
  brush: { title: "Brush", icon: "i-brush", kind: "brush" },
  "brush-settings": { title: "Brush Settings", icon: "i-brush", kind: "brush-settings" },
  toolpresets: { title: "Tool Presets", icon: "i-presets", kind: "toolpresets" },
  libraries: { title: "Libraries", icon: "i-libs", kind: "libraries" },
  timeline: { title: "Timeline", icon: "i-video", kind: "timeline" },
  notes: { title: "Notes", icon: "i-note", kind: "simple", note: "No notes in this document" },
};

// Gruppi di schede del dock destro, nell'ordine in cui appaiono.
export const dockGroups = [
  { id: "g-core", tabs: ["color", "swatches", "styles"], open: true },
  { id: "g-stack", tabs: ["layers", "channels", "paths"], open: true },
  { id: "g-history", tabs: ["history", "actions", "layerscomps"], open: true },
  { id: "g-adjust", tabs: ["adjustments", "properties", "histogram"], open: true },
  { id: "g-nav", tabs: ["navigator", "info", "measurements"], open: true },
  { id: "g-text", tabs: ["character", "paragraph"], open: false },
  { id: "g-brush", tabs: ["brush", "brush-settings", "toolpresets"], open: false },
  { id: "g-libs", tabs: ["libraries", "timeline", "notes"], open: false },
];

/** Scheda attiva di partenza per ogni gruppo. */
export const initialActiveTab = {
  "g-core": "color",
  "g-stack": "layers",
  "g-history": "history",
  "g-adjust": "adjustments",
  "g-nav": "info",
  "g-text": "character",
  "g-brush": "brush",
  "g-libs": "libraries",
};

/** Dati finti usati dai renderer: danno credibilità ai pannelli. */
export const mock = {
  layers: [
    { name: "Heading", kind: "type", opacity: 100, mode: "Normal", visible: true, locked: false, content: "AG" },
    { name: "Colour wash", kind: "fill", mode: "Soft Light", opacity: 65, visible: true, locked: false, color: "#7c5cff" },
    { name: "Photo", kind: "image", mode: "Normal", opacity: 100, visible: true, locked: false },
    { name: "Background", kind: "bg", mode: "Normal", opacity: 100, visible: false, locked: true, color: "#1c1c1e" },
  ],
  history: [
    "Open", "New Layer", "Brush Tool", "Paint Bucket", "Gaussian Blur", "Move",
    "Free Transform", "Deselect", "Type Tool", "Layer Style", "Levels",
  ],
  swatches: ["#000000", "#474747", "#8a8a8a", "#b0b0b0", "#ffffff", "#d64545", "#f0913a", "#f5d442", "#7ac74f", "#3fb6a8", "#3b82f6", "#5b62d6", "#8b5cf6", "#c04cc0", "#e86ca0", "#7b4b2a", "#c89b6b", "#efe1c6", "#123042", "#0d1b2a"],
  styles: ["Simple Inner Shadow", "Chrome", "Gel", "Glass Button", "Dashed Outline", "Sunset", "Neon Glow", "Sepia"],
  adjustments: [
    ["i-sun", "Brightness/Contrast"], ["i-adjust", "Levels"], ["i-paths", "Curves"], ["i-sun", "Exposure"],
    ["i-droplet", "Vibrance"], ["i-color", "Hue/Saturation"], ["i-color", "Color Balance"], ["i-adjust", "Black & White"],
    ["i-sun", "Photo Filter"], ["i-channels", "Channel Mixer"], ["i-color", "Color Lookup"], ["i-slider", "Invert"],
    ["i-presets", "Posterize"], ["i-adjust", "Threshold"], ["i-gradient", "Gradient Map"], ["i-color", "Selective Color"],
  ],
  brushPresets: [
    ["Soft Round", 22], ["Soft Round", 9], ["Hard Round", 12], ["Hard Round", 4], ["Chalk", 28],
    ["Spatter", 26], ["Airbrush", 40], ["Splatter", 33], ["Fan", 18], ["Fire", 30],
    ["Fuzzball", 24], ["Confetti", 20], ["Star", 16], ["Dune Grass", 27], ["Leaves", 21],
  ],
  toolPresets: ["Brush — Soft Round 22", "Eraser — Hard Round 40", "Gradient — Black to White", "Type — Open Sans 24", "Crop — 16:9", "Shape — Rounded 10px"],
  actions: [
    ["Default Actions", ["Sepia Tone", "Vignette", "Fade to Black", "Sharpen Edges"]],
    ["Fotox Recipes", ["Export social square", "Watermark bottom right", "Contact sheet"]],
  ],
  measurements: [
    ["1", "Length", "420 px", "12°", "Area", "176400"],
    ["2", "Length", "180 px", "90°", "Area", "32400"],
  ],
  infoRows: [
    ["R", "255"], ["G", "255"], ["B", "255"],
    ["X", "512 px"], ["Y", "341 px"],
    ["W", "1920 px"], ["H", "1280 px"],
  ],
  character: [
    ["Font", "Open Sans"], ["Style", "Regular"], ["Size", "24 pt"], ["Leading", "auto"],
    ["Tracking", "0"], ["Kerning", "Auto"], ["Vertical Scale", "100%"], ["Horizontal Scale", "100%"],
  ],
  paragraph: [
    ["Alignment", "Left"], ["Left Indent", "0 pt"], ["Right Indent", "0 pt"], ["First Line", "0 pt"],
    ["Space Before", "0 pt"], ["Space After", "0 pt"], ["Hyphenate", "off"],
  ],
};
