// Fotox — inventario strumenti.
// Ogni voce dell'array principale è uno slot della colonna strumenti; il primo
// strumento è quello visibile, gli altri vivono nel flyout (click prolungato o
// click destro sullo slot).

export const toolSlots = [
  { id: "move", name: "Move Tool", key: "V", icon: "i-move", flyout: [] },
  {
    id: "marquee", name: "Rectangular Marquee Tool", key: "M", icon: "i-marquee",
    flyout: [
      { id: "marquee-ellipse", name: "Elliptical Marquee Tool", key: "M", icon: "i-marquee-ellipse" },
      { id: "marquee-row", name: "Single Row Marquee Tool", key: "M", icon: "i-marquee-row" },
      { id: "marquee-col", name: "Single Column Marquee Tool", key: "M", icon: "i-marquee-col" },
    ],
  },
  {
    id: "lasso", name: "Lasso Tool", key: "L", icon: "i-lasso",
    flyout: [
      { id: "lasso-poly", name: "Polygonal Lasso Tool", key: "L", icon: "i-lasso-poly" },
      { id: "lasso-magnet", name: "Magnetic Lasso Tool", key: "L", icon: "i-lasso-magnet" },
    ],
  },
  {
    id: "quick-select", name: "Quick Selection Tool", key: "W", icon: "i-quick-select",
    flyout: [
      { id: "magic-wand", name: "Magic Wand Tool", key: "W", icon: "i-wand" },
      { id: "object-select", name: "Object Selection Tool", key: "W", icon: "i-object-select" },
    ],
  },
  {
    id: "crop", name: "Crop Tool", key: "C", icon: "i-crop",
    flyout: [
      { id: "crop-persp", name: "Perspective Crop Tool", key: "C", icon: "i-crop-persp" },
      { id: "slice", name: "Slice Tool", key: "C", icon: "i-slice" },
      { id: "slice-select", name: "Slice Select Tool", key: "C", icon: "i-slice" },
    ],
  },
  {
    id: "eyedropper", name: "Eyedropper Tool", key: "I", icon: "i-eyedropper",
    flyout: [
      { id: "sampler", name: "Color Sampler Tool", key: "I", icon: "i-sampler" },
      { id: "ruler-tool", name: "Ruler Tool", key: "I", icon: "i-ruler" },
      { id: "note-tool", name: "Note Tool", key: "I", icon: "i-note" },
      { id: "counting", name: "Counting Tool", key: "I", icon: "i-counting" },
    ],
  },
  {
    id: "heal", name: "Spot Healing Brush Tool", key: "J", icon: "i-spot-heal",
    flyout: [
      { id: "heal-brush", name: "Healing Brush Tool", key: "J", icon: "i-heal" },
      { id: "patch", name: "Patch Tool", key: "J", icon: "i-patch" },
      { id: "content-move", name: "Content-Aware Move Tool", key: "J", icon: "i-content-move" },
      { id: "red-eye", name: "Red Eye Tool", key: "J", icon: "i-red-eye" },
    ],
  },
  {
    id: "brush", name: "Brush Tool", key: "B", icon: "i-brush",
    flyout: [
      { id: "pencil", name: "Pencil Tool", key: "B", icon: "i-pencil" },
      { id: "color-replace", name: "Color Replacement Tool", key: "B", icon: "i-color-replace" },
      { id: "mixer-brush", name: "Mixer Brush Tool", key: "B", icon: "i-mixer-brush" },
    ],
  },
  {
    id: "clone", name: "Clone Stamp Tool", key: "S", icon: "i-clone-stamp",
    flyout: [{ id: "pattern-stamp", name: "Pattern Stamp Tool", key: "S", icon: "i-pattern-stamp" }],
  },
  {
    id: "history-brush", name: "History Brush Tool", key: "Y", icon: "i-history-brush",
    flyout: [{ id: "art-history", name: "Art History Brush", key: "Y", icon: "i-art-history" }],
  },
  {
    id: "eraser", name: "Eraser Tool", key: "E", icon: "i-eraser",
    flyout: [
      { id: "eraser-bg", name: "Background Eraser Tool", key: "E", icon: "i-bg-eraser" },
      { id: "eraser-magic", name: "Magic Eraser Tool", key: "E", icon: "i-magic-eraser" },
    ],
  },
  {
    id: "gradient", name: "Gradient Tool", key: "G", icon: "i-gradient",
    flyout: [{ id: "paint-bucket", name: "Paint Bucket Tool", key: "G", icon: "i-paint-bucket" }],
  },
  {
    id: "blur", name: "Blur Tool", key: "R", icon: "i-blur",
    flyout: [
      { id: "sharpen", name: "Sharpen Tool", key: "R", icon: "i-sharpen" },
      { id: "smudge", name: "Smudge Tool", key: "R", icon: "i-smudge" },
    ],
  },
  {
    id: "dodge", name: "Dodge Tool", key: "O", icon: "i-dodge",
    flyout: [
      { id: "burn", name: "Burn Tool", key: "O", icon: "i-burn" },
      { id: "sponge", name: "Sponge Tool", key: "O", icon: "i-sponge" },
    ],
  },
  {
    id: "pen", name: "Pen Tool", key: "P", icon: "i-pen",
    flyout: [
      { id: "pen-freeform", name: "Freeform Pen Tool", key: "P", icon: "i-pen-freeform" },
      { id: "pen-curvature", name: "Curvature Pen Tool", key: "P", icon: "i-pen-curvature" },
      { id: "anchor-add", name: "Add Anchor Point Tool", key: "P", icon: "i-anchor-add" },
      { id: "anchor-del", name: "Delete Anchor Point Tool", key: "P", icon: "i-anchor-del" },
      { id: "anchor-convert", name: "Convert Point Tool", key: "P", icon: "i-anchor-convert" },
    ],
  },
  {
    id: "type", name: "Horizontal Type Tool", key: "T", icon: "i-type",
    flyout: [
      { id: "type-vertical", name: "Vertical Type Tool", key: "T", icon: "i-type-vertical" },
      { id: "type-mask", name: "Horizontal Type Mask Tool", key: "T", icon: "i-type-mask" },
      { id: "type-mask-vertical", name: "Vertical Type Mask Tool", key: "T", icon: "i-type-mask" },
    ],
  },
  {
    id: "path-select", name: "Path Selection Tool", key: "A", icon: "i-path-select",
    flyout: [{ id: "direct-select", name: "Direct Selection Tool", key: "A", icon: "i-direct-select" }],
  },
  {
    id: "shape", name: "Rectangle Tool", key: "U", icon: "i-shape-rect",
    flyout: [
      { id: "shape-rounded", name: "Rounded Rectangle Tool", key: "U", icon: "i-shape-rounded" },
      { id: "shape-ellipse", name: "Ellipse Tool", key: "U", icon: "i-shape-ellipse" },
      { id: "shape-polygon", name: "Polygon Tool", key: "U", icon: "i-shape-polygon" },
      { id: "shape-triangle", name: "Triangle Tool", key: "U", icon: "i-shape-polygon" },
      { id: "shape-line", name: "Line Tool", key: "U", icon: "i-shape-line" },
      { id: "shape-custom", name: "Custom Shape Tool", key: "U", icon: "i-shape-custom" },
      { id: "shape-3d", name: "3D Object Tool", key: "U", icon: "i-3d" },
    ],
  },
  {
    id: "hand", name: "Hand Tool", key: "H", icon: "i-hand",
    flyout: [{ id: "rotate-view", name: "Rotate View Tool", key: "H", icon: "i-rotate" }],
  },
  { id: "zoom", name: "Zoom Tool", key: "Z", icon: "i-zoom", flyout: [] },
  { id: "quick-mask", name: "Edit in Quick Mask Mode", key: "Q", icon: "i-quick-mask", flyout: [] },
  { id: "screen", name: "Cycle Screen Mode", key: "F", icon: "i-screen-mode", flyout: [] },
];

/** Tutti gli strumenti in un unico elenco piatto (slot + flyout). */
export const allTools = toolSlots.flatMap((s) => [s, ...s.flyout]);

export function findTool(id) {
  return allTools.find((t) => t.id === id) || toolSlots[0];
}

/** Slot che ospita uno strumento (serve per evidenziare il pulsante giusto). */
export function slotOf(id) {
  return toolSlots.find((s) => s.id === id || s.flyout.some((f) => f.id === id)) || toolSlots[0];
}
