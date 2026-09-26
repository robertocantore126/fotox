// Fotox — contenuto della barra opzioni, uno schema per strumento.
// Tipi di controllo: select | toggle | num | text | btngroup | swatch | label | gap

const MODES = ["Normal", "Dissolve", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light", "Color Dodge", "Color Burn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Color", "Luminosity"];

// The Selection Mode button group (M5-T04). `key` gives the control a name in
// `readOptions()`, which is what the engine reads as the tool's "Mode"; the
// value is the index of the pressed button (0 New, 1 Add, 2 Subtract,
// 3 Intersect).
const SELECTION_MODE = { type: "btngroup", key: "Mode", icons: ["i-marquee", "i-plus", "i-minus", "i-object-select"], titles: ["New selection", "Add to selection", "Subtract from selection", "Intersect with selection"], active: 0 };

export const optionBars = {
  _default: [{ type: "label", text: "No options for this tool" }],

  move: [
    { type: "toggle", text: "Auto-Select", on: false },
    { type: "select", key: "Select", options: ["Layer", "Group"], value: "Layer" },
    { type: "toggle", text: "Show Transform Controls", on: false },
    { type: "gap" },
    // Align / Distribute (M7-T04): the same actions as Layer ▸ Align.
    { type: "btngroup", icons: ["i-shape-rect", "i-props", "i-shape-rect"], titles: ["Align left edges", "Align horizontal centers", "Align right edges"], actions: ["align:left", "align:hcenter", "align:right"] },
    { type: "btngroup", icons: ["i-shape-rect", "i-props", "i-shape-rect"], titles: ["Align top edges", "Align vertical centers", "Align bottom edges"], actions: ["align:top", "align:vcenter", "align:bottom"] },
    { type: "btngroup", icons: ["i-layers", "i-props", "i-layers"], titles: ["Distribute horizontal centers", "Distribute vertical centers"], actions: ["dist:hcenter", "dist:vcenter"] },
  ],

  marquee: [
    SELECTION_MODE, { type: "gap" },
    { type: "num", text: "Feather:", value: "0", unit: "px", width: 40 },
    { type: "select", text: "Style:", options: ["Normal", "Fixed Ratio", "Fixed Size"], value: "Normal", enables: ["Width", "Height"] },
    { type: "num", text: "Width:", value: "64", width: 48 },
    { type: "num", text: "Height:", value: "64", width: 48 },
    { type: "toggle", text: "Anti-alias", on: true },
  ],
  "marquee-ellipse": null, "marquee-row": null, "marquee-col": null,
  lasso: [SELECTION_MODE, { type: "gap" }, { type: "num", text: "Feather:", value: "0", unit: "px", width: 40 }, { type: "toggle", text: "Anti-alias", on: true }],
  "lasso-poly": [SELECTION_MODE, { type: "gap" }, { type: "num", text: "Feather:", value: "0", unit: "px", width: 40 }, { type: "toggle", text: "Anti-alias", on: true }],
  "lasso-magnet": [SELECTION_MODE, { type: "gap" }, { type: "num", text: "Width:", value: "10", unit: "px", width: 40 }, { type: "select", text: "Contrast:", options: ["1%", "5%", "10%", "25%", "50%", "75%", "100%"], value: "10%" }, { type: "num", text: "Frequency:", value: "57", width: 40 }],
  "quick-select": [SELECTION_MODE, { type: "gap" }, { type: "toggle", text: "Sample All Layers", on: false }, { type: "toggle", text: "Auto-Enhance", on: true }, { type: "gap" }, { type: "num", text: "Size:", value: "30", unit: "px", width: 40 }, { type: "num", text: "Hardness:", value: "100", unit: "%", width: 40 }],
  "magic-wand": [SELECTION_MODE, { type: "gap" }, { type: "num", text: "Tolerance:", value: "32", width: 40 }, { type: "toggle", text: "Anti-alias", on: true }, { type: "toggle", text: "Contiguous", on: true }, { type: "toggle", text: "Sample All Layers", on: false }],
  "object-select": [{ type: "btngroup", icons: ["i-object-select", "i-lasso"], titles: ["Rectangle", "Lasso"], active: 0 }, { type: "toggle", text: "Sample All Layers", on: false }],

  // The crop tool reads Ratio / W / H and the Delete Cropped Pixels toggle
  // (M6-T03): the engine's crop command takes exactly those.
  crop: [
    { type: "select", text: "Ratio:", options: ["Unconstrained", "1:1 (Square)", "5:4", "4:3", "3:2", "16:9", "Original Ratio"], value: "Unconstrained" },
    { type: "num", text: "W:", value: "", width: 44 }, { type: "num", text: "H:", value: "", width: 44 },
    { type: "gap" },
    { type: "toggle", text: "Delete Cropped Pixels", on: false },
    { type: "gap" },
    // The ✓ and ✗ are actions, not values: the engine commits the box with
    // the same `Enter` / `Escape` the keyboard sends.
    { type: "btngroup", icons: ["i-check", "i-close"], titles: ["Crop (Enter)", "Cancel (Esc)"], actions: ["tool:commit", "tool:cancel"] },
  ],
  // Free Transform's bar (M6-T04), shown while the box is up whatever the
  // tool: the interpolation, Warp, and the commit / cancel buttons.
  _transform: [
    { type: "label", text: "Free Transform" },
    { type: "gap" },
    // Numeric fields (M6-T09): empty = keep; the status bar shows the live values.
    { type: "num", text: "X:", value: "", unit: "px", width: 52 }, { type: "num", text: "Y:", value: "", unit: "px", width: 52 },
    { type: "num", text: "W:", value: "", unit: "%", width: 44 }, { type: "num", text: "H:", value: "", unit: "%", width: 44 },
    { type: "num", text: "Angle:", value: "", unit: "°", width: 44 },
    { type: "gap" },
    { type: "select", text: "Interpolation:", options: ["Nearest Neighbor", "Bilinear", "Bicubic", "Bicubic Smoother", "Bicubic Sharper", "Bicubic Automatic", "Lanczos 3"], value: "Bicubic" },
    { type: "gap" },
    { type: "btngroup", icons: ["i-grid"], titles: ["Switch between free transform and warp modes"], actions: ["xf:warp"] },
    { type: "gap" },
    { type: "btngroup", icons: ["i-check", "i-close"], titles: ["Commit Transform (Enter)", "Cancel Transform (Esc)"], actions: ["tool:commit", "tool:cancel"] },
  ],
  "crop-persp": [{ type: "label", text: "Drag the corner handles to define the perspective plane" }],
  slice: [{ type: "btngroup", icons: ["i-slice"], titles: ["Slice"], active: 0 }, { type: "toggle", text: "Show Slice Numbers", on: false }],
  "slice-select": [{ type: "btngroup", icons: ["i-slice"], titles: ["Slice select"], active: 0 }, { type: "toggle", text: "Show Slice Numbers", on: false }],

  eyedropper: [{ type: "select", text: "Sample Size:", options: ["Point Sample", "3 by 3 Average", "5 by 5 Average", "11 by 11 Average", "51 by 51 Average"], value: "Point Sample" }, { type: "select", text: "Sample:", options: ["All Layers", "Current Layer"], value: "All Layers" }, { type: "toggle", text: "Show Sampling Ring", on: true }],
  sampler: [{ type: "label", text: "Click up to 4 spots to read their colour values" }],
  "ruler-tool": [{ type: "label", text: "Drag to measure — results appear in the Info panel" }],
  "note-tool": [{ type: "btngroup", icons: ["i-note"], titles: ["Note"], active: 0 }, { type: "btn", text: "Clear All" }],
  counting: [{ type: "btngroup", icons: ["i-counting"], titles: ["Counting"], active: 0 }, { type: "btn", text: "Reset Count" }],

  heal: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "num", text: "Hardness:", value: "50", unit: "%", width: 40 }, { type: "select", text: "Type:", options: ["Proximity Match"], value: "Proximity Match" }],
  "heal-brush": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "num", text: "Hardness:", value: "50", unit: "%", width: 40 }, { type: "select", text: "Mode:", options: ["Normal", "Multiply", "Screen"], value: "Normal" }, { type: "toggle", text: "Aligned", on: true }, { type: "toggle", text: "Sample All Layers", on: true }, { type: "toggle", text: "Pressure for size", on: false }],
  patch: [{ type: "btngroup", icons: ["i-patch", "i-patch"], titles: ["Source", "Destination"], active: 0 }, { type: "toggle", text: "Transparent", on: false }],
  "content-move": [{ type: "btngroup", icons: ["i-content-move"], titles: ["Move"], active: 0 }, { type: "select", text: "Mode:", options: ["Move", "Extend", "Duplicate"], value: "Move" }, { type: "select", text: "Structure:", options: ["1", "2", "3", "4", "5"], value: "1" }, { type: "num", text: "Color:", value: "0", width: 36 }],
  "red-eye": [{ type: "num", text: "Pupil Size:", value: "50", unit: "%", width: 40 }, { type: "num", text: "Darken Amount:", value: "50", unit: "%", width: 40 }],

  brush: [
    { type: "brushpreset" },
    { type: "num", text: "Size:", value: "40", unit: "px", width: 40 },
    { type: "num", text: "Hardness:", value: "75", unit: "%", width: 40 },
    { type: "select", text: "Mode:", options: MODES, value: "Normal" },
    { type: "range", text: "Opacity:", value: 100 },
    { type: "toggle", text: "Pressure for opacity", on: false },
    { type: "range", text: "Flow:", value: 100 },
    { type: "num", text: "Smoothing:", value: "10", unit: "%", width: 36 },
    { type: "toggle", text: "Pressure for size", on: false },
  ],
  pencil: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "3", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Opacity:", value: 100 }, { type: "toggle", text: "Pressure for opacity", on: false }, { type: "num", text: "Smoothing:", value: "0", unit: "%", width: 36 }, { type: "toggle", text: "Pressure for size", on: false }],
  "color-replace": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "30", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: ["Color", "Hue", "Saturation", "Luminosity"], value: "Color" }, { type: "select", text: "Limits:", options: ["Contiguous", "Discontiguous", "Find Edges"], value: "Contiguous" }, { type: "range", text: "Tolerance:", value: 30 }],
  "mixer-brush": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "60", unit: "px", width: 40 }, { type: "select", text: "Preset:", options: ["Dry", "Moist", "Wet", "Very Wet"], value: "Wet" }, { type: "range", text: "Mix:", value: 50 }, { type: "range", text: "Flow:", value: 100 }],
  clone: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "60", unit: "px", width: 40 }, { type: "num", text: "Hardness:", value: "50", unit: "%", width: 40 }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Opacity:", value: 100 }, { type: "range", text: "Flow:", value: 100 }, { type: "toggle", text: "Aligned", on: true }, { type: "select", text: "Sample:", options: ["Current Layer", "All Layers"], value: "All Layers" }, { type: "toggle", text: "Pressure for size", on: false }],
  "pattern-stamp": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "60", unit: "px", width: 40 }, { type: "toggle", text: "Aligned", on: true }, { type: "toggle", text: "Impressionist", on: false }],
  "history-brush": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Opacity:", value: 50 }],
  "art-history": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "select", text: "Style:", options: ["Tight Short", "Tight Medium", "Loose Long", "Dab", "Tight Curl"], value: "Tight Medium" }, { type: "range", text: "Opacity:", value: 100 }],
  eraser: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "num", text: "Hardness:", value: "50", unit: "%", width: 40 }, { type: "select", text: "Mode:", options: ["Brush", "Pencil", "Block"], value: "Brush" }, { type: "range", text: "Opacity:", value: 100 }, { type: "toggle", text: "Pressure for opacity", on: false }, { type: "range", text: "Flow:", value: 100 }, { type: "num", text: "Smoothing:", value: "0", unit: "%", width: 36 }, { type: "toggle", text: "Pressure for size", on: false }],
  "eraser-bg": [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "select", text: "Limits:", options: ["Contiguous", "Discontiguous", "Find Edges"], value: "Contiguous" }, { type: "range", text: "Tolerance:", value: 50 }, { type: "toggle", text: "Protect Foreground Color", on: false }],
  "eraser-magic": [{ type: "num", text: "Tolerance:", value: "32", width: 40 }, { type: "toggle", text: "Anti-alias", on: true }, { type: "toggle", text: "Contiguous", on: true }, { type: "toggle", text: "Sample All Layers", on: false }],
  gradient: [{ type: "gradient" }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Opacity:", value: 100 }, { type: "toggle", text: "Reverse", on: false }, { type: "toggle", text: "Dither", on: true }, { type: "toggle", text: "Transparency", on: true }],
  "paint-bucket": [{ type: "select", text: "Fill:", options: ["Foreground", "Pattern"], value: "Foreground" }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Opacity:", value: 100 }, { type: "num", text: "Tolerance:", value: "32", width: 40 }, { type: "toggle", text: "All Layers", on: false }],
  blur: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "30", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Strength:", value: 50 }, { type: "toggle", text: "Sample All Layers", on: false }],
  sharpen: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "30", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Strength:", value: 50 }, { type: "toggle", text: "Sample All Layers", on: false }],
  smudge: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "30", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: MODES, value: "Normal" }, { type: "range", text: "Strength:", value: 50 }, { type: "toggle", text: "Finger Painting", on: false }, { type: "toggle", text: "Sample All Layers", on: false }],
  dodge: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "select", text: "Range:", options: ["Midtones", "Shadows", "Highlights"], value: "Midtones" }, { type: "range", text: "Exposure:", value: 50 }, { type: "toggle", text: "Protect Tones", on: false }],
  burn: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "select", text: "Range:", options: ["Midtones", "Shadows", "Highlights"], value: "Midtones" }, { type: "range", text: "Exposure:", value: 50 }],
  sponge: [{ type: "brushpreset" }, { type: "num", text: "Size:", value: "40", unit: "px", width: 40 }, { type: "select", text: "Mode:", options: ["Saturate", "Desaturate"], value: "Saturate" }, { type: "range", text: "Flow:", value: 50 }],

  pen: [{ type: "btngroup", icons: ["i-pen", "i-shape-rect"], titles: ["Path", "Shape"], active: 0 }, { type: "btngroup", icons: ["i-plus", "i-minus", "i-check", "i-object-select"], titles: ["Add to shape area", "Subtract from shape area", "Intersect shape areas", "Exclude overlapping shape areas"], active: 0 }, { type: "select", text: "Width:", options: ["1 px", "2 px", "3 px", "4 px", "5 px"], value: "1 px" }, { type: "toggle", text: "Rubber Band", on: true }, { type: "toggle", text: "Auto Add/Delete", on: false }],
  "pen-freeform": [{ type: "toggle", text: "Magnetic", on: false }, { type: "num", text: "Width:", value: "5", unit: "px", width: 40 }, { type: "num", text: "Contrast:", value: "20", unit: "%", width: 40 }],
  "pen-curvature": [{ type: "label", text: "Click to add smooth points, double-click to close the path" }],
  "anchor-add": [{ type: "label", text: "Click a path segment to add an anchor point" }],
  "anchor-del": [{ type: "label", text: "Click an anchor point to remove it" }],
  "anchor-convert": [{ type: "label", text: "Drag a direction handle to convert a point" }],

  type: [
    { type: "select", key: "Font", text: "", options: ["Open Sans", "Arimo", "Bitter", "Lato", "Lora", "Merriweather", "Montserrat", "Open Sans Condensed", "Oswald", "Playfair Display", "Poppins", "Raleway", "Roboto", "Roboto Condensed", "Source Sans Pro", "Ubuntu"], value: "Open Sans", width: 130 },
    { type: "select", key: "Style", text: "", options: ["Regular", "Italic", "Bold", "Bold Italic"], value: "Regular" },
    { type: "num", key: "Size", text: "", value: "24", unit: "pt", width: 40 },
    { type: "select", key: "Anti-alias", text: "", options: ["Sharp", "Crisp", "Strong", "Smooth", "None"], value: "Sharp" },
    { type: "btngroup", key: "Align", icons: ["i-quote", "i-props", "i-quote"], titles: ["Left align text", "Center text", "Right align text"], active: 0 },
    { type: "select", text: "", options: ["Faux Bold", "Faux Italic"], value: "Faux Bold" },
    { type: "swatch", title: "Text colour" },
  ],
  "type-vertical": null, "type-mask": null, "type-mask-vertical": null,

  "path-select": [{ type: "btngroup", icons: ["i-path-select", "i-direct-select"], titles: ["Path Selection", "Direct Selection"], active: 0 }, { type: "btngroup", icons: ["i-plus", "i-minus", "i-check", "i-object-select"], titles: ["Add to shape area", "Subtract from shape area", "Intersect shape areas", "Exclude overlapping shape areas"], active: 0 }, { type: "btn", text: "Combine" }, { type: "select", text: "Align:", options: ["No Change", "Align To Selection", "Align To Canvas"], value: "Align To Selection" }],
  "direct-select": [{ type: "label", text: "Show Bounding Box: off" }, { type: "toggle", text: "Show Bounding Box", on: false }],

  shape: [{ type: "btngroup", icons: ["i-shape-rect", "i-shape-custom"], titles: ["Shape", "Path"], active: 0 }, { type: "select", text: "", options: ["Pixels", "Path", "Shape"], value: "Shape" }, { type: "swatch", title: "Fill" }, { type: "swatch", title: "Stroke", outline: true }, { type: "num", text: "", value: "1", unit: "px", width: 40 }, { type: "select", text: "", options: ["Solid", "Dashed", "Dotted"], value: "Solid" }, { type: "select", text: "Sides:", options: ["3", "4", "5", "6", "8", "12"], value: "5" }, { type: "toggle", text: "Snap to Pixels", on: false }],
  "shape-rounded": [{ type: "num", text: "Radius:", value: "10", unit: "px", width: 40 }, { type: "select", text: "", options: ["Pixels", "Path", "Shape"], value: "Shape" }],
  "shape-ellipse": [{ type: "select", text: "", options: ["Pixels", "Path", "Shape"], value: "Shape" }],
  "shape-polygon": [{ type: "num", text: "Sides:", value: "5", width: 36 }, { type: "select", text: "Kind:", options: ["Smooth", "Star"], value: "Smooth" }],
  "shape-line": [{ type: "num", text: "Weight:", value: "3", unit: "px", width: 40 }],
  "shape-custom": [{ type: "select", text: "Shape:", options: ["Chirp", "Cloud", "Dog", "Flower", "Leaf", "Moon", "Shield", "Star", "Sun", "Zap"], value: "Star" }],
  "shape-3d": [{ type: "label", text: "Drag to draw a 3D object, then use the 3D panel to rotate it" }],

  hand: [{ type: "toggle", text: "Scroll All Windows", on: false }, { type: "toggle", text: "Zoom to Fit on Resize", on: false }],
  "rotate-view": [{ type: "btn", text: "Reset View", action: "view:reset-rotation" }, { type: "toggle", text: "Rotate All Windows", on: false }],
  zoom: [{ type: "btngroup", icons: ["i-zoom-in", "i-zoom-out"], titles: ["Zoom In", "Zoom Out"], active: 0 }, { type: "toggle", text: "Resize Windows to Fit", on: true }, { type: "toggle", text: "Scrubby Zoom", on: false }, { type: "btn", text: "Fit on Screen" }, { type: "btn", text: "100%" }],
  "quick-mask": [{ type: "label", text: "Masked areas are protected while you paint" }, { type: "btn", text: "Exit Quick Mask" }],
  screen: [{ type: "label", text: "Press F repeatedly: Standard · Full Screen with Menu · Full Screen" }],
};

export function optionsFor(toolId) {
  // gli strumenti raggruppati ereditano la barra del primo della famiglia:
  // si continua a togliere il suffisso finché non si trova una barra vera.
  let id = toolId;
  for (let i = 0; i < 3 && id; i++) {
    if (optionBars[id]) return optionBars[id];
    const next = id.replace(/-(ellipse|row|col|poly|magnet|persp|select|tool|vertical|mask|bg|magic|freeform|curvature|add|del|convert|rounded|line|custom|3d|brush|move|eye|stamp|history|replace|view|in|out)$/, "");
    if (next === id) break;
    id = next;
  }
  return optionBars[id] || optionBars._default;
}
