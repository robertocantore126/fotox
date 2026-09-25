// Fotox — definizioni delle finestre di dialogo.
// Ogni dialogo è puro dato: il motore (js/dialogs.js) sa renderizzare i tipi
// elencati in fondo al file. I filtri semplici sono generati da una tabella
// compatta (filterSpecs) per non ripetere lo stesso schema decine di volte.

const num = (label, value, o = {}) => ({ type: "num", label, value, ...o });
const sel = (label, options, value, o = {}) => ({ type: "select", label, options, value, ...o });
const chk = (label, on = true) => ({ type: "check", label, on });
const rad = (label, options, value = 0, o = {}) => ({ type: "radio", label, options, value, ...o });
const rng = (label, value, o = {}) => ({ type: "range", label, value, min: 0, max: 100, ...o });
const txt = (label, value, o = {}) => ({ type: "text", label, value, ...o });
const lbl = (text, o = {}) => ({ type: "label", text, ...o });
const row = (...fields) => ({ type: "row", fields });
const grp = (label, fields) => ({ type: "group", label, fields });
const sep = { type: "sep" };
const PREVIEW = { type: "preview" };

export const dialogs = {};

/* ---------------------------------------------------------------- File */

dialogs["new-doc"] = {
  title: "New Document", width: 620, icon: "i-doc", wide: true,
  fields: [
    txt("Name:", "Untitled-1"),
    sep,
    { type: "presets", items: [
      ["Clipboard", "1680 × 1050"],
      ["Default Photoshop Size", "504 × 364"],
      ["Web — Large", "1920 × 1080"],
      ["Web — Small", "800 × 600"],
      ["A4 at 300 ppi", "2480 × 3508"],
      ["A5 at 300 ppi", "1748 × 2480"],
      ["Square 1:1", "1080 × 1080"],
      ["Story 9:16", "1080 × 1920"],
      ["Video 4K UHD", "3840 × 2160"],
      ["Mobile App 2x", "750 × 1334"],
      ["Print A3 300 ppi", "3508 × 4961"],
      ["Business Card", "1050 × 600"],
    ] },
    { type: "col", fields: [
      grp("Width and Height", [
        row(sel("Units:", ["Pixels", "Inches", "Centimetres", "Millimetres", "Points", "Picas"], "Pixels", { w: 110 })),
        row(num("Width:", 1080, { unit: "px", w: 90 }), { type: "locksize" }, num("Height:", 1080, { unit: "px", w: 90 })),
        chk("Constrain proportions", true),
      ]),
      grp("Colour Mode", [
        sel("Colour Mode:", ["RGB Color", "Grayscale", "CMYK Color", "Lab Color", "Bitmap"], "RGB Color"),
        num("Bit Depth:", 8, { unit: "bit", w: 70 }),
        sep,
        sel("Background Contents:", ["White", "Black", "Transparent", "Background Colour"], "White"),
      ]),
      grp("Advanced", [
        sel("Colour Profile:", ["sRGB IEC61966-2.1", "Display P3", "Adobe RGB (1998)", "Don't Color Manage"], "sRGB IEC61966-2.1"),
        num("Pixel Aspect Ratio:", 1.0, { w: 70 }),
      ]),
    ] },
  ],
  ok: "Create", cancel: "Cancel",
};

dialogs.open = {
  title: "Open", width: 560, icon: "i-folder",
  fields: [
    lbl("Pick a file from your device — in Fotox this is a mock, nothing is read from disk."),
    { type: "filelist", items: ["hero-banner.png", "mockup-landing.psd", "logo-fotox.ai", "portrait.cr3", "interface.fig", "scan.tiff", "sketch.sketch"] },
    row(sel("Files of type:", ["All Formats", "PSD", "PNG", "JPG", "GIF", "SVG", "PDF", "WebP", "RAW", "Figma"], "All Formats")),
    chk("Open as Smart Object", false),
    { type: "openoptions" },
  ],
  ok: "Open", cancel: "Cancel",
};

dialogs["open-url"] = {
  title: "Open from URL", width: 480, icon: "i-cloud",
  fields: [txt("URL:", "https://"), chk("Import as Smart Object", true), lbl("Remote images are loaded straight into the document.")],
  ok: "Open", cancel: "Cancel",
};

dialogs["open-place"] = dialogs.open;

dialogs["export-as"] = {
  title: "Export As", width: 640, wide: true, icon: "i-doc",
  fields: [
    PREVIEW,
    // Read by label in the app (ui/js/actions.js, M3-T07): keep the labels.
    { type: "col", fields: [
      sel("Format:", ["PNG", "JPG", "TIFF"], "PNG"),
      sel("Bit Depth:", ["Document", "8 bits/channel"], "Document"),
      sel("Transparency:", ["Automatic", "On", "Off"], "Automatic"),
      grp("JPG options", [rng("Quality:", 90, { min: 0, max: 100 }), sel("Chroma:", ["4:4:4 (best)", "4:2:0 (smaller)"], "4:4:4 (best)")]),
      grp("Print", [sel("CMYK:", ["None"], "None")]),
    ] },
  ],
  ok: "Export", cancel: "Cancel",
};

dialogs["save-for-web"] = { ...dialogs["export-as"], title: "Save for Web (Legacy)" };

dialogs["export-layers"] = {
  title: "Export Layers", width: 520, icon: "i-doc",
  fields: [
    { type: "col", fields: [
      sel("Format:", ["PNG", "JPG", "GIF", "SVG", "WebP", "TIFF", "PDF"], "PNG"),
      txt("File name prefix:", "Untitled-1_"),
      sel("Naming:", ["Layer name", "Index + layer name", "Index only"], "Layer name"),
      grp("Layers to include", [chk("Visible layers", true), chk("Hidden layers", false), chk("Groups as folders", true)]),
      grp("Options", [chk("Trim transparent pixels", false), rng("Quality:", 90)]),
    ] },
  ],
  ok: "Export", cancel: "Cancel",
};

dialogs.print = {
  title: "Print", width: 560, icon: "i-doc",
  fields: [
    { type: "col", fields: [grp("Printer", [sel("Printer:", ["Microsoft Print to PDF", "HP LaserJet", "Save as PDF"], "Microsoft Print to PDF"), sel("Preset:", ["Default", "Draft", "High Quality"], "Default")]), grp("Print Settings", [sel("Orientation:", ["Portrait", "Landscape"], "Portrait"), txt("Copies:", "1"), chk("Centre", true), chk("Scale to Fit Media", true)])] },
    PREVIEW,
  ],
  ok: "Print", cancel: "Cancel",
};

dialogs["file-info"] = {
  title: "File Information", width: 520, icon: "i-info",
  fields: [
    txt("Document Title:", "Untitled-1"),
    txt("Author:", ""), txt("Author Title:", ""), txt("Description:", ""),
    txt("Copyright Notice:", "© 2026"), txt("Copyright Info URL:", ""),
    sel("Rating:", ["Unrated", "★", "★★", "★★★", "★★★★", "★★★★★"], "Unrated"),
  ],
  ok: "OK", cancel: "Cancel",
};

dialogs["doc-props"] = {
  title: "Document Properties", width: 520, icon: "i-info",
  fields: [
    { type: "readonly", items: [
      ["File name", "Untitled-1"], ["File type", "Fotox document"], ["Colour mode", "RGB Color, 8 bits/channel"],
      ["Dimensions", "1080 × 1080 px"], ["Resolution", "72 pixels/inch"], ["Layers", "4"],
      ["Channels", "3 (RGB)"], ["Paths", "1"], ["History states", "11"], ["Memory", "8.4 MB"],
    ] },
  ],
  ok: "OK",
};

dialogs["fit-image"] = { title: "Fit Image", width: 380, fields: [num("Constrain Within Width:", 1080, { unit: "px", w: 90 }), num("Constrain Within Height:", 1080, { unit: "px", w: 90 })], ok: "OK", cancel: "Cancel" };
dialogs.batch = { title: "Batch", width: 620, wide: true, fields: [{ type: "col", fields: [grp("Play", [sel("Set:", ["Default Actions", "Fotox Recipes"], "Default Actions"), sel("Action:", ["Sepia Tone", "Vignette", "Export social square"], "Sepia Tone")]), grp("Source", [sel("Source:", ["Folder", "Bridge", "Imported", "Opened Files"], "Folder"), { type: "btn", text: "Choose..." }, chk("Override Action 'Open' Commands", false), chk("Include All Subfolders", true), chk("Suppress File Open Options Dialogs", false)])] }, { type: "col", fields: [grp("Destination", [sel("Destination:", ["None", "Save and Close", "Folder"], "None"), chk("Override Action 'Save As' Commands", false)])] }, { type: "list", rows: ["hero-banner.png", "portrait.cr3", "mockup-landing.psd", "scan.tiff"] }], ok: "Run", cancel: "Cancel" };

dialogs.search = {
  title: "Search", width: 460, icon: "i-zoom",
  fields: [{ type: "searchbox", placeholder: "Search tools, menus and commands..." }, { type: "list", rows: ["Gaussian Blur", "New Document", "Image Size", "Free Transform", "Layers panel", "Export as PNG", "Keyboard Shortcuts"] }],
  ok: null, cancel: "Close",
};

dialogs.duplicate = { title: "Duplicate Image", width: 380, fields: [txt("As:", "Untitled-1 copy"), chk("Only Duplicate: Layers", false)], ok: "OK", cancel: "Cancel" };
dialogs["apply-image"] = { title: "Apply Image", width: 460, fields: [sel("Source:", ["Untitled-1", "Background"], "Untitled-1"), sel("Layer:", ["Background", "Merged"], "Background"), sel("Channel:", ["RGB", "Red", "Green", "Blue"], "RGB"), sel("Blending:", ["Normal", "Multiply", "Screen", "Overlay"], "Normal"), num("Opacity:", 100, { unit: "%", w: 60 }), chk("Preserve Transparency", false)], ok: "OK", cancel: "Cancel" };
dialogs.calculations = { title: "Calculations", width: 520, wide: true, fields: [{ type: "col", fields: [grp("Source 1", [sel("Layer:", ["Merged", "Background"], "Merged"), sel("Channel:", ["RGB", "Red", "Green", "Blue"], "Red")])] }, { type: "col", fields: [grp("Source 2", [sel("Layer:", ["Merged", "Background"], "Background"), sel("Channel:", ["RGB", "Red", "Green", "Blue"], "Green")]), grp("Output", [sel("Result:", ["New Document", "New Channel", "Selection"], "New Document")])] }, { type: "col", fields: [grp("Blending", [sel("Blending:", ["Normal", "Multiply", "Screen"], "Multiply"), num("Opacity:", 100, { unit: "%", w: 60 }), chk("Mask", false)])] }], ok: "OK", cancel: "Cancel" };
dialogs.trap = { title: "Trap", width: 320, fields: [num("Width:", 1, { unit: "px", w: 60 })], ok: "OK", cancel: "Cancel" };
dialogs["variables-define"] = { title: "Variables", width: 520, fields: [{ type: "list", rows: ["No variables defined"] }, row({ type: "btn", text: "New Variable" }, { type: "btn", text: "Delete" })], ok: "OK", cancel: "Cancel" };
dialogs["variables-data"] = { title: "Data Sets", width: 520, fields: [{ type: "list", rows: ["Data Set 1", "Data Set 2"] }, row({ type: "btn", text: "Import..." }, { type: "btn", text: "Export..." })], ok: "OK", cancel: "Cancel" };

/* ----------------------------------------------------- Immagine / selezione */

dialogs["image-size"] = {
  title: "Image Size", width: 480, icon: "i-image",
  fields: [
    { type: "pixels" },
    grp("", [
      row(num("Width:", 1080, { unit: "px", w: 90 }), { type: "locksize" }, num("Height:", 1080, { unit: "px", w: 90 }), { type: "chain", text: "px" }),
      row(num("Resolution:", 72, { w: 90 }), { type: "chain", text: "pixels/inch" }),
      sep,
      row(num("Width:", 15, { unit: "in", w: 90 }), { type: "locksize" }, num("Height:", 15, { unit: "in", w: 90 })),
      row(num("Width:", 381, { unit: "mm", w: 90 }), { type: "locksize" }, num("Height:", 381, { unit: "mm", w: 90 })),
    ]),
    chk("Resample", true),
    sel("", ["Automatic", "Preserve Details 2.0", "Bicubic Sharper (reduction)", "Bicubic Smoother (enlargement)", "Bicubic (smooth gradients)", "Nearest Neighbor (hard edges)", "Bilinear"], "Bicubic (smooth gradients)"),
    chk("Scale Styles", true),
    chk("Constrain Proportions", true),
  ],
  ok: "OK", cancel: "Cancel",
};

dialogs["canvas-size"] = {
  title: "Canvas Size", width: 480, icon: "i-image",
  fields: [
    { type: "pixels" },
    lbl("Current Size: 1080 × 1080 px"),
    grp("New Size", [
      row(num("Width:", 1200, { unit: "px", w: 90 }), { type: "locksize" }, num("Height:", 1200, { unit: "px", w: 90 }), { type: "chain", text: "px" }),
      sel("", ["Pixels", "Percent", "Inches", "Centimetres", "Millimetres"], "Pixels"),
    ]),
    sel("Anchor:", ["", "", "", "", "", "", "", "", ""], "", { type: "anchor" }),
    sel("Canvas Extension Colour:", ["White", "Black", "Grey", "Background", "Foreground", "Transparent"], "White"),
  ],
  ok: "OK", cancel: "Cancel",
};

dialogs["rotate-arbitrary"] = { title: "Rotate Image", width: 320, fields: [num("Angle:", 0, { unit: "°", w: 70 }), rad("Rotate:", ["Clockwise", "Counter Clockwise"], 0, { inline: true })], ok: "OK", cancel: "Cancel" };
dialogs.trim = { title: "Trim", width: 380, fields: [rad("Based On:", ["Transparent Pixels", "Top Left Pixel Colour", "Bottom Right Pixel Colour"], 0), rad("Trim Away:", ["Top", "Left", "Bottom", "Right"], 0, { multi: true }), lbl("Tip: tick every edge you want trimmed.")], ok: "OK", cancel: "Cancel" };

dialogs["color-range"] = { title: "Color Range", width: 560, wide: true, fields: [{ type: "col", fields: [sel("Select:", ["Sampled Colors", "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Highlights", "Midtones", "Shadows", "Skin Tones"], "Sampled Colors"), rad("Selection:", ["Selection", "Image"], 0), rng("Fuzziness:", 40), rng("Range:", 100, { min: 0, max: 200 }), rng("Invert:", 0)] }, { type: "col", fields: [grp("Preview", [PREVIEW])] }], ok: "OK", cancel: "Cancel" };
dialogs["focus-area"] = { title: "Focus Area", width: 560, wide: true, fields: [{ type: "col", fields: [rad("Selection:", ["In-Focus Ranges", "Blur Ranges"], 0), { type: "focus" }, rng("In-Focus Range:", 60), rng("Noise Level:", 20), chk("Show Overlay", true)] }, { type: "col", fields: [grp("Preview", [PREVIEW])] }], ok: "OK", cancel: "Cancel" };

const modifyDialog = (title, extra) => ({ title, width: 340, fields: [num("Amount:", 5, { w: 70, ...extra })], ok: "OK", cancel: "Cancel" });
dialogs["sel-border"] = modifyDialog("Border Selection", { unit: "px", min: 1, max: 200 });
dialogs["sel-smooth"] = modifyDialog("Smooth Selection", { unit: "px", min: 1, max: 100 });
dialogs["sel-expand"] = modifyDialog("Expand Selection", { unit: "px", min: 1, max: 100 });
dialogs["sel-contract"] = modifyDialog("Contract Selection", { unit: "px", min: 1, max: 100 });
dialogs["sel-feather"] = modifyDialog("Feather Selection", { unit: "px", min: 0.2, max: 250 });
dialogs["save-selection"] = { title: "Save Selection", width: 420, fields: [txt("Name:", "Alpha 1"), sel("Document:", ["Untitled-1"], "Untitled-1"), sel("Channel:", ["New", "Alpha 1"], "New"), chk("Replace Channel", true), lbl("The selection is stored as an alpha channel.")], ok: "OK", cancel: "Cancel" };
dialogs["load-selection"] = { title: "Load Selection", width: 420, fields: [sel("Document:", ["Untitled-1"], "Untitled-1"), sel("Channel:", ["Alpha 1"], "Alpha 1"), chk("Invert", false)], ok: "OK", cancel: "Cancel" };
dialogs["new-guide"] = { title: "New Guide", width: 320, fields: [rad("Orientation:", ["Horizontal", "Vertical"], 1, { inline: true }), num("Position:", 540, { unit: "px", w: 70 }), sel("", ["px", "%"], "px", { w: 60 })], ok: "OK", cancel: "Cancel" };
dialogs["guide-layout"] = { title: "New Guide Layout", width: 400, fields: [txt("Preset:", "Custom"), num("Columns:", 3, { w: 60 }), num("Rows:", 3, { w: 60 }), num("Gutter:", 20, { unit: "px", w: 60 }), num("Margin:", 40, { unit: "px", w: 60 })], ok: "OK", cancel: "Cancel" };

/* ---------------------------------------------------------------- Regolazioni */

dialogs["brightness-contrast"] = { title: "Brightness/Contrast", width: 400, icon: "i-sun", fields: [rng("Brightness:", 0, { min: -150, max: 150 }), rng("Contrast:", 0, { min: -100, max: 100 }), chk("Use Legacy", false), PREVIEW], ok: "OK", cancel: "Cancel" };
// Asked by the engine before closing an unsaved document (M3-T06); the
// buttons are supplied by ui/js/native/documents.js.
// Colour management (M4-T03/T04). The option lists of proof-setup and of the
// Export As CMYK menu are filled by ui/js/native/color.js from the engine.
dialogs["assign-profile"] = { title: "Assign Profile", width: 420, fields: [lbl("The numbers stay; the colours will look different."), sel("Profile:", ["sRGB IEC61966-2.1", "Adobe RGB (1998)", "Display P3", "ProPhoto RGB"], "sRGB IEC61966-2.1")], ok: "OK", cancel: "Cancel" };
dialogs["convert-profile"] = { title: "Convert to Profile", width: 440, fields: [sel("Destination:", ["sRGB IEC61966-2.1", "Adobe RGB (1998)", "Display P3", "ProPhoto RGB"], "sRGB IEC61966-2.1"), sel("Intent:", ["Perceptual", "Relative Colorimetric", "Saturation", "Absolute Colorimetric"], "Relative Colorimetric"), chk("Use Black Point Compensation", true)], ok: "OK", cancel: "Cancel" };
dialogs["proof-setup"] = { title: "Customize Proof Condition", width: 460, fields: [sel("Device to Simulate:", ["(no CMYK profile found)"]), sel("Rendering Intent:", ["Perceptual", "Relative Colorimetric", "Saturation", "Absolute Colorimetric"], "Relative Colorimetric"), chk("Black Point Compensation", true), chk("Simulate Paper Color", false)], ok: "OK", cancel: "Cancel" };
dialogs["save-changes"] = { title: "Fotox", width: 420, icon: "i-info", plain: true, fields: [lbl("Save changes to the document before closing?")] };
dialogs.levels = { title: "Levels", width: 520, icon: "i-adjust", fields: [sel("Preset:", ["Default", "Increase Contrast", "Lighter", "Darker", "Midtones Brighter"], "Default"), sel("Channel:", ["RGB", "Red", "Green", "Blue"], "RGB"), { type: "histo" }, rng("Input Black:", 0, { min: 0, max: 253 }), rng("Gamma (x100):", 100, { min: 1, max: 999 }), rng("Input White:", 255, { min: 2, max: 255 }), rng("Output Black:", 0, { min: 0, max: 255 }), rng("Output White:", 255, { min: 0, max: 255 }), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs.curves = { title: "Curves", width: 520, icon: "i-paths", fields: [sel("Preset:", ["Default", "Strong Contrast", "Custom"], "Default"), sel("Channel:", ["RGB", "Red", "Green", "Blue"], "RGB"), { type: "curve" }, row(num("Input:", 0, { w: 55 }), num("Output:", 255, { w: 55 })), row({ type: "btn", text: "Smooth" }, { type: "btn", text: "Linear" }, { type: "btn", text: "Reset", curve: "reset" }), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs.exposure = { title: "Exposure", width: 400, icon: "i-sun", fields: [rng("Exposure:", 0, { min: -20, max: 20 }), rng("Offset:", 0, { min: -50, max: 50 }), rng("Gamma Correction:", 100, { min: 1, max: 300 }), { type: "pipette", text: "Set black point" }, { type: "pipette", text: "Set white point" }, { type: "pipette", text: "Set grey point" }, chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs.vibrance = { title: "Vibrance", width: 400, fields: [rng("Vibrance:", 0, { min: -100, max: 100 }), rng("Saturation:", 0, { min: -100, max: 100 }), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["hue-saturation"] = { title: "Hue/Saturation", width: 440, fields: [sel("Preset:", ["Default", "Old Style", "Cyanotype", "Strong"], "Default"), sel("Channel:", ["Master", "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"], "Master"), rng("Hue:", 0, { min: -180, max: 180 }), rng("Saturation:", 0, { min: -100, max: 100 }), rng("Lightness:", 0, { min: -100, max: 100 }), chk("Colorize", false), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["color-balance"] = { title: "Color Balance", width: 440, fields: [sel("Tone:", ["Shadows", "Midtones", "Highlights"], "Midtones"), rng("Cyan–Red:", 0, { min: -100, max: 100 }), rng("Magenta–Green:", 0, { min: -100, max: 100 }), rng("Yellow–Blue:", 0, { min: -100, max: 100 }), chk("Preserve Luminosity", true), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["black-white"] = { title: "Black & White", width: 480, fields: [rng("Reds:", 40, { min: -200, max: 300 }), rng("Yellows:", 60, { min: -200, max: 300 }), rng("Greens:", 40, { min: -200, max: 300 }), rng("Cyans:", 60, { min: -200, max: 300 }), rng("Blues:", 20, { min: -200, max: 300 }), rng("Magentas:", 80, { min: -200, max: 300 }), chk("Tint", false), rng("Tint Hue:", 35, { min: 0, max: 360 }), rng("Tint Saturation:", 25, { min: 0, max: 100 }), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["photo-filter"] = { title: "Photo Filter", width: 430, fields: [sel("Filter:", ["Warming Filter (85)", "Warming Filter (81)", "Cooling Filter (80)", "Cooling Filter (82)", "Sepia", "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Violet", "Magenta", "Underwater"], "Warming Filter (85)"), rng("Density:", 25), chk("Preserve Luminosity", true), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["channel-mixer"] = { title: "Channel Mixer", width: 460, fields: [sel("Preset:", ["Default", "Black & White Infrared", "Monochrome"], "Default"), sel("Output Channel:", ["Red", "Green", "Blue"], "Red"), rng("Red:", 100, { min: -200, max: 200 }), rng("Green:", 0, { min: -200, max: 200 }), rng("Blue:", 0, { min: -200, max: 200 }), rng("Constant:", 0, { min: -200, max: 200 }), chk("Monochrome", false), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["color-lookup"] = { title: "Color Lookup", width: 420, fields: [sel("3DLUT File:", ["None", "Crisp Warm", "Fall Colors", "Foggy Night", "Kodak 5218", "Teal and Orange", "Vintage"], "None"), sel("Abstract Profile:", ["None", "Abstract", "Subtle", "Vivid"], "None"), rng("Blend:", 100), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs.posterize = { title: "Posterize", width: 340, fields: [rng("Levels:", 4, { min: 2, max: 255 }), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs.threshold = { title: "Threshold", width: 420, fields: [{ type: "histo" }, rng("Threshold Level:", 128, { min: 1, max: 255 }), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["gradient-map"] = { title: "Gradient Map", width: 400, fields: [sel("Gradient:", ["Black, White", "Violet, Orange", "Blue, Red, Yellow", "Copper", "Sepia"], "Black, White"), { type: "gradientbar" }, chk("Reverse", false), chk("Preview", true)], ok: "OK", cancel: "Cancel" };
dialogs["selective-color"] = { title: "Selective Color", width: 460, fields: [sel("Preset:", ["Default", "Absolute", "Custom"], "Default"), sel("Colors:", ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Whites", "Neutrals", "Blacks"], "Reds"), rng("Cyan:", 0, { min: -100, max: 100 }), rng("Magenta:", 0, { min: -100, max: 100 }), rng("Yellow:", 0, { min: -100, max: 100 }), rng("Black:", 0, { min: -100, max: 100 }), rad("Method:", ["Relative", "Absolute"], 0, { inline: true })], ok: "OK", cancel: "Cancel" };
dialogs["shadows-highlights"] = { title: "Shadows/Highlights", width: 480, fields: [grp("Shadows", [rng("Amount:", 35), rng("Tonal Width:", 50), rng("Radius:", 30, { min: 1, max: 250 })]), grp("Highlights", [rng("Amount:", 0), rng("Tonal Width:", 50), rng("Radius:", 30, { min: 1, max: 250 })]), grp("Adjustments", [rng("Color Correction:", 20), rng("Midtone Contrast:", 0, { min: -100, max: 100 })])], ok: "OK", cancel: "Cancel" };
dialogs["hdr-toning"] = { title: "HDR Toning", width: 480, fields: [grp("Tone and Detail", [sel("Preset:", ["Default", "Crisp", "Monochrome Artistic", "Surreal"], "Default"), rng("Exposure:", 0, { min: -10, max: 10 }), rng("Detail:", 100), rng("Edge Glow:", 0), rng("Tone Compression:", 0, { min: 0, max: 100 })]), grp("Advanced", [sel("Colour:", ["Natural", "Vivid", "Sepia", "Monochrome"], "Natural")])], ok: "OK", cancel: "Cancel" };
dialogs["match-color"] = { title: "Match Color", width: 440, fields: [sel("Source:", ["Untitled-1", "None"], "None"), sel("Layer:", ["Background", "Merged"], "Background"), rng("Luminance:", 100), rng("Color Intensity:", 100), rng("Fade:", 0)], ok: "OK", cancel: "Cancel" };
dialogs["replace-color"] = { title: "Replace Color", width: 440, fields: [{ type: "preview" }, { type: "color", label: "Target", value: "#7ac74f" }, rng("Fuzziness:", 40), { type: "color", label: "Replacement", value: "#3b82f6" }, rng("Hue:", 0, { min: -180, max: 180 }), rng("Saturation:", 0, { min: -100, max: 100 }), rng("Lightness:", 0, { min: -100, max: 100 })], ok: "OK", cancel: "Cancel" };

/* ---------------------------------------------------------------- Livelli */

dialogs["blending-options"] = {
  title: "Layer Style", width: 620, wide: true, icon: "i-fx",
  fields: [
    { type: "stylelist", items: ["Styles", "Blending Options: Default", "Drop Shadow", "Inner Shadow", "Outer Glow", "Inner Glow", "Bevel & Emboss", "Satin", "Color Overlay", "Gradient Overlay", "Pattern Overlay", "Stroke"] },
    { type: "col", fields: [
      grp("General Blending", [sel("Blend Mode:", ["Normal", "Dissolve", "Multiply", "Screen", "Overlay", "Soft Light", "Hard Light"], "Normal"), num("Opacity:", 100, { unit: "%", w: 60 })]),
      grp("Advanced Blending", [num("Fill Opacity:", 100, { unit: "%", w: 60 }), { type: "channelsrow" }, chk("Blend Interior Effects as Group", false), chk("Blend Clipped Layers as Group", false), chk("Transparency Shapes Layer", false), chk("Layer Mask Hides Effects", false), chk("Vector Mask Hides Effects", false)]),
      grp("Blend If: Grey", [{ type: "blendif", left: "This Layer", right: "Underlying Layer" }]),
    ] },
  ],
  ok: "OK", cancel: "Cancel",
};

const styleDialog = (title, extra) => ({
  title: "Layer Style — " + title, width: 560, icon: "i-fx",
  fields: [
    { type: "stylelist", items: ["Styles", "Blending Options: Default", "Drop Shadow", "Inner Shadow", "Outer Glow", "Inner Glow", "Bevel & Emboss", "Satin", "Color Overlay", "Gradient Overlay", "Pattern Overlay", "Stroke"], active: title },
    { type: "col", fields: extra },
  ],
  ok: "OK", cancel: "Cancel",
});

dialogs["style-drop-shadow"] = styleDialog("Drop Shadow", [
  { type: "blend", mode: "Multiply" }, { type: "color", label: "Colour", value: "#000000" },
  rng("Opacity:", 75), num("Angle:", 120, { unit: "°", w: 60 }), chk("Use Global Light", true),
  num("Distance:", 5, { unit: "px", w: 60 }), rng("Spread:", 0), num("Size:", 5, { unit: "px", w: 60 }),
  num("Noise:", 0, { unit: "%", w: 60 }), chk("Layer Knocks Out Drop Shadow", true),
]);
dialogs["style-inner-shadow"] = styleDialog("Inner Shadow", [{ type: "blend", mode: "Multiply" }, { type: "color", label: "Colour", value: "#000000" }, rng("Opacity:", 75), num("Angle:", 120, { unit: "°", w: 60 }), num("Distance:", 5, { unit: "px", w: 60 }), rng("Choke:", 0), num("Size:", 5, { unit: "px", w: 60 })]);
dialogs["style-outer-glow"] = styleDialog("Outer Glow", [{ type: "blend", mode: "Screen" }, rng("Opacity:", 75), rng("Noise:", 0), { type: "color", label: "Colour", value: "#f5d442" }, { type: "glowtype" }, rng("Spread:", 0), num("Size:", 9, { unit: "px", w: 60 }), rng("Range:", 50, { min: 1, max: 100 })]);
dialogs["style-inner-glow"] = styleDialog("Inner Glow", [{ type: "blend", mode: "Screen" }, rng("Opacity:", 75), rng("Noise:", 0), { type: "color", label: "Colour", value: "#f5d442" }, { type: "glowtype", value: "Center" }, rng("Choke:", 0), num("Size:", 9, { unit: "px", w: 60 })]);
dialogs["style-bevel"] = styleDialog("Bevel & Emboss", [
  grp("Structure", [sel("Style:", ["Inner Bevel", "Outer Bevel", "Emboss", "Pillow Emboss", "Stroke Emboss"], "Inner Bevel"), sel("Technique:", ["Smooth", "Chisel Hard", "Chisel Soft"], "Smooth"), num("Depth:", 100, { unit: "%", w: 60 }), rad("Direction:", ["Up", "Down"], 0, { inline: true }), num("Size:", 6, { unit: "px", w: 60 }), num("Soften:", 0, { unit: "px", w: 60 })]),
  grp("Shading", [num("Angle:", 120, { unit: "°", w: 60 }), num("Altitude:", 30, { unit: "°", w: 60 }), sel("Highlight Mode:", ["Screen", "Normal", "Multiply"], "Screen"), { type: "color", label: "Highlight", value: "#ffffff" }, rng("Opacity:", 75), sel("Shadow Mode:", ["Multiply", "Normal", "Screen"], "Multiply"), { type: "color", label: "Shadow", value: "#000000" }, rng("Opacity:", 75)]),
]);
dialogs["style-satin"] = styleDialog("Satin", [{ type: "blend", mode: "Multiply" }, { type: "color", label: "Colour", value: "#000000" }, rng("Opacity:", 50), num("Angle:", 19, { unit: "°", w: 60 }), num("Distance:", 11, { unit: "px", w: 60 }), num("Size:", 14, { unit: "px", w: 60 }), chk("Invert", true)]);
dialogs["style-color-overlay"] = styleDialog("Color Overlay", [{ type: "blend", mode: "Normal" }, { type: "color", label: "Colour", value: "#7c5cff" }, rng("Opacity:", 100)]);
dialogs["style-gradient-overlay"] = styleDialog("Gradient Overlay", [{ type: "blend", mode: "Normal" }, rng("Opacity:", 100), { type: "gradientbar" }, sel("Style:", ["Linear", "Radial", "Angle", "Reflected", "Diamond"], "Linear"), num("Angle:", 90, { unit: "°", w: 60 }), num("Scale:", 100, { unit: "%", w: 60 }), chk("Reverse", false), chk("Align with Layer", true), rad("Method:", ["Align", "Dither"], 0, { inline: true })]);
dialogs["style-pattern-overlay"] = styleDialog("Pattern Overlay", [{ type: "blend", mode: "Normal" }, rng("Opacity:", 100), { type: "patternpick" }, num("Scale:", 100, { unit: "%", w: 60 }), num("Angle:", 0, { unit: "°", w: 60 })]);
dialogs["style-stroke"] = styleDialog("Stroke", [{ type: "stroke", value: "Colour" }, { type: "color", label: "Colour", value: "#ffffff" }, num("Size:", 2, { unit: "px", w: 60 }), sel("Position:", ["Outside", "Inside", "Centre"], "Outside"), sel("Blend Mode:", ["Normal", "Multiply", "Screen"], "Normal"), rng("Opacity:", 100), chk("Overprint", false)]);

dialogs["fill-solid"] = { title: "New Layer — Solid Colour", width: 380, icon: "i-swatch", fields: [{ type: "color", label: "Fill colour", value: "#efc36a" }, sel("Mode:", ["Normal", "Multiply", "Screen", "Overlay"], "Normal"), num("Opacity:", 100, { unit: "%", w: 60 })], ok: "OK", cancel: "Cancel" };
dialogs["fill-gradient"] = { title: "New Layer — Gradient Fill", width: 420, fields: [{ type: "gradientbar" }, sel("Style:", ["Linear", "Radial", "Angle", "Reflected", "Diamond"], "Linear"), num("Angle:", 90, { unit: "°", w: 60 }), num("Scale:", 100, { unit: "%", w: 60 }), chk("Reverse", false) ], ok: "OK", cancel: "Cancel" };
dialogs["fill-pattern"] = { title: "New Layer — Pattern Fill", width: 420, fields: [{ type: "patternpick" }, num("Scale:", 100, { unit: "%", w: 60 })], ok: "OK", cancel: "Cancel" };
dialogs["warp-text"] = { title: "Warp Text", width: 460, fields: [sel("Style:", ["None", "Arc", "Arc Lower", "Arch", "Bulge", "Fish", "Flag", "Rise", "Wave", "Shell Lower"], "Arc"), rad("Horizontal / Vertical:", ["Horizontal", "Vertical"], 0, { inline: true }), rng("Bend:", 50, { min: -100, max: 100 }), rng("Horizontal Distortion:", 0, { min: -100, max: 100 }), rng("Vertical Distortion:", 0, { min: -100, max: 100 })], ok: "OK", cancel: "Cancel" };
dialogs["layer-lock"] = { title: "Lock All Layers", width: 320, fields: [chk("Lock Transparent Pixels", true), chk("Lock Image Pixels", true), chk("Lock Position", false)], ok: "OK", cancel: "Cancel" };
dialogs.defringe = { title: "Defringe", width: 320, fields: [num("Width:", 1, { unit: "px", w: 60 })], ok: "OK", cancel: "Cancel" };

/* ---------------------------------------------------------------- Preferenze */

dialogs.prefs = {
  title: "Preferences — General", width: 620, wide: true, icon: "i-props",
  fields: [
    { type: "prefsnav", items: ["General", "Interface", "Performance", "Guides, Grid & Slices", "Plugins", "Scratch Disks"] },
    { type: "col", fields: [
      grp("Display", [chk("Auto-Show the Home Screen", false), chk("Use Shift Key for Tool Switch", true), chk("Show Tool Tips", true), chk("Show Only Exact Pixel", false)]),
      grp("Export", [chk("Ask for filename when exporting", true), chk("Keep original PNG metadata", false), sel("Default export format:", ["PNG", "JPG", "WebP", "SVG", "PSD"], "PNG")]),
      grp("History & Clipboard", [sel("History States:", ["10", "20", "50", "100", "200"], "50"), sel("Cache Levels:", ["1", "2", "4", "8"], "4"), chk("Copy as PNG", true)]),
      grp("Startup", [chk("Restore open documents", false), chk("Check for updates", true), chk("Send anonymous usage data", false)]),
    ] },
  ],
  ok: "OK", cancel: "Cancel",
};
dialogs["prefs-interface"] = { ...dialogs.prefs, title: "Preferences — Interface", fields: [dialogs.prefs.fields[0], dialogs.prefs.fields[1]] };
dialogs["prefs-performance"] = { ...dialogs.prefs, title: "Preferences — Performance", fields: [dialogs.prefs.fields[0], dialogs.prefs.fields[1]] };
dialogs["prefs-guides"] = { title: "Preferences — Guides, Grid & Slices", width: 520, icon: "i-grid", fields: [grp("Guides", [{ type: "color", label: "Colour", value: "#00a0ff" }, sel("Style:", ["Lines", "Dashed Lines"], "Lines")]), grp("Grid", [{ type: "color", label: "Colour", value: "#cccccc" }, sel("Style:", ["Lines", "Dashed Lines", "Dots"], "Dots"), num("Gridline Every:", 100, { unit: "px", w: 70 }), num("Subdivisions:", 10, { w: 60 })]), grp("Slices", [{ type: "color", label: "Line Colour", value: "#000000" }, chk("Show Slice Numbers", true)])], ok: "OK", cancel: "Cancel" };
dialogs["prefs-plugins"] = { title: "Preferences — Plugins", width: 520, icon: "i-libs", fields: [{ type: "list", rows: ["No third-party plugins installed", "Fotox Bridge (built in)", "OpenType features (built in)"] }, chk("Load third-party plugins on startup", false), { type: "btn", text: "Install from URL..." }], ok: "OK", cancel: "Cancel" };
dialogs.env = { title: "Environment Settings", width: 480, fields: [sel("Scratch disk:", ["Browser memory", "Cloud (slow)"], "Browser memory"), num("Max memory:", 2048, { unit: "MB", w: 70 }), chk("Use GPU acceleration", true), chk("Warn on unsupported files", true)], ok: "OK", cancel: "Cancel" };

dialogs.shortcuts = {
  title: "Keyboard Shortcuts", width: 640, wide: true, icon: "i-key",
  fields: [
    { type: "shortcutgroups" },
  ],
  ok: null, cancel: "Close",
};

/* ---------------------------------------------------------------- Altro */

dialogs.about = {
  title: "About Fotox", width: 460, icon: "i-account", plain: true,
  fields: [
    { type: "about" },
  ],
  ok: null, cancel: "Close",
};

// I valori dell'ambiente sono una funzione: vengono letti quando la finestra si
// apre, non quando questo file viene importato (i dati restano dati puri).
dialogs.diagnostics = {
  title: "Diagnostics", width: 520, plain: true, ok: null, cancel: "Close",
  fields: [{
    type: "readonly",
    items: () => [
      ["Renderer", "Canvas 2D"],
      ["User agent", String(navigator.userAgent || "").slice(0, 60)],
      ["Device pixel ratio", Number(window.devicePixelRatio || 1).toFixed(2)],
      ["Viewport", `${window.innerWidth} × ${window.innerHeight}`],
      ["Colour depth", String(window.screen ? window.screen.colorDepth : "—")],
    ],
  }],
};
dialogs.feedback = { title: "Send Feedback", width: 520, fields: [sel("Topic:", ["Bug report", "Feature request", "Something else"], "Bug report"), txt("Subject:", ""), { type: "textarea", label: "Message", value: "" }, chk("Include diagnostics", true)], ok: "Send", cancel: "Cancel" };
dialogs["whats-new"] = { title: "What's New in Fotox", width: 520, plain: true, fields: [{ type: "list", rows: ["Rebuilt menu engine with keyboard navigation", "Panel dock with collapsible groups", "Full filter dialog set", "Layer style stack", "Brush preset panel"] }], ok: null, cancel: "Close" };
dialogs["sign-in"] = { title: "Sign In", width: 420, fields: [sel("Provider:", ["Email", "Google", "GitHub"], "Email"), txt("Email:", ""), txt("Password:", "", { password: true }), chk("Remember me", true), lbl("Accounts are not part of this mock.")], ok: "Sign In", cancel: "Cancel" };
dialogs["new-workspace"] = { title: "New Workspace", width: 380, fields: [txt("Name:", "My workspace"), chk("Capture panel locations", true), chk("Capture keyboard shortcuts", false)], ok: "OK", cancel: "Cancel" };
dialogs["install-plugin"] = { title: "Install Plugin", width: 460, fields: [txt("Plugin URL:", "https://"), lbl("Plugins add custom filters and automations to Fotox.")], ok: "Install", cancel: "Cancel" };
dialogs["import-figma"] = { title: "Import from Figma", width: 460, fields: [txt("File URL or key:", ""), sel("Import as:", ["Editable layers", "Flattened image", "Smart Object"], "Editable layers"), chk("Include hidden layers", false)], ok: "Import", cancel: "Cancel" };
dialogs["import-sketch"] = { title: "Import from Sketch", width: 460, fields: [txt("File:", ""), chk("Rasterize symbols", false)], ok: "Import", cancel: "Cancel" };
dialogs.generate = { title: "Generate with AI", width: 520, fields: [sel("Mode:", ["Fill selection", "Expand canvas", "Remove object", "Create image"], "Fill selection"), { type: "textarea", label: "Prompt", value: "a calm blue sky with soft clouds" }, sel("Style:", ["Photo", "Illustration", "3D render", "Watercolour"], "Photo"), num("Variations:", 3, { w: 60 })], ok: "Generate", cancel: "Cancel" };
dialogs["color-picker"] = { title: "Colour Picker", width: 420, icon: "i-color", fields: [{ type: "colorpicker" }], ok: "OK", cancel: "Cancel" };
dialogs.fill = { title: "Fill", width: 380, fields: [sel("Use:", ["Foreground Colour", "Background Colour", "Colour...", "Pattern", "Black", "White", "50% Grey"], "Foreground Colour"), sel("Mode:", ["Normal", "Multiply", "Screen", "Overlay"], "Normal"), num("Opacity:", 100, { unit: "%", w: 60 }), chk("Preserve Transparency", false)], ok: "OK", cancel: "Cancel" };
dialogs.stroke = { title: "Stroke", width: 380, fields: [num("Width:", 2, { unit: "px", w: 60 }), { type: "color", label: "Colour", value: "#111111" }, sel("Location:", ["Inside", "Centre", "Outside"], "Centre"), sel("Blending:", ["Normal", "Multiply", "Screen"], "Normal"), num("Opacity:", 100, { unit: "%", w: 60 }), chk("Preserve Transparency", false)], ok: "OK", cancel: "Cancel" };

/* -------------------------------------------------- Filtri (tabella compatta) */

const filterSpecs = {
  "gaussian-blur": ["Gaussian Blur", [num("Radius:", 4, { unit: "px", w: 70, range: [0, 1000] })], true],
  "motion-blur": ["Motion Blur", [num("Angle:", 0, { unit: "°", w: 70 }), num("Distance:", 20, { unit: "px", w: 70 })], true],
  "radial-blur": ["Radial Blur", [num("Amount:", 10, { w: 70 }), rad("Blur Method:", ["Spin", "Zoom"], 0), sel("Quality:", ["Draft", "Good", "Best"], "Good")], true],
  "box-blur": ["Box Blur", [num("Radius:", 8, { unit: "px", w: 70 })], true],
  "surface-blur": ["Surface Blur", [num("Radius:", 5, { unit: "px", w: 70 }), num("Threshold:", 15, { w: 70 })], true],
  "lens-blur": ["Lens Blur", [sel("Depth Map Source:", ["None", "Transparency"], "None"), num("Radius:", 15, { unit: "px", w: 70 }), num("Blade Curvature:", 0, { w: 70 }), num("Rotation:", 0, { unit: "°", w: 70 }), num("Brightness:", 0, { w: 70 }), chk("More Accurate", false), num("Threshold:", 255, { w: 70 }), sel("Noise Amount:", ["0", "5", "10", "20", "50"], "0")], true],
  displace: ["Displace", [num("Horizontal Scale:", 10, { w: 70 }), num("Vertical Scale:", 10, { w: 70 }), rad("Displacement Map:", ["Stretch To Fit", "Tile"], 0), rad("Undefined Areas:", ["Repeat Edge Pixels", "Wrap Around"], 0)], false],
  pinch: ["Pinch", [num("Amount:", 35, { w: 70, range: [-100, 100] })], true],
  ripple: ["Ripple", [num("Amount:", 30, { w: 70 }), sel("Size:", ["Small", "Medium", "Large"], "Medium")], true],
  shear: ["Shear", [{ type: "shearcurve" }, rad("Undefined Areas:", ["Repeat Edge Pixels", "Wrap Around"], 0)], true],
  spherize: ["Spherize", [num("Amount:", 60, { w: 70, range: [-100, 100] }), sel("Mode:", ["Normal", "Horizontal Only", "Vertical Only"], "Normal")], true],
  twirl: ["Twirl", [num("Angle:", 90, { unit: "°", w: 70, range: [-999, 999] })], true],
  wave: ["Wave", [num("Number of Generators:", 5, { w: 70 }), row(num("Wavelength:", 10, { w: 55 }), num("to", 120, { w: 55 })), row(num("Amplitude:", 5, { w: 55 }), num("to", 35, { w: 55 })), row(num("Scale:", 100, { w: 55, unit: "%" }), num("", 100, { w: 55, unit: "%" }), rad("", ["Horiz.", "Vert."], 0, { inline: true })), rad("Type:", ["Sine", "Triangle", "Square"], 0), grp("Undefined Areas", [rad("", ["Wrap Around", "Repeat Edge Pixels"], 0, { inline: true })])], true],
  "polar-coordinates": ["Polar Coordinates", [rad("", ["Rectangular to Polar", "Polar to Rectangular"], 0)], true],
  "add-noise": ["Add Noise", [num("Amount:", 12, { unit: "%", w: 70 }), rad("Distribution:", ["Uniform", "Gaussian"], 1), chk("Monochromatic", false)], true],
  "dust-scratches": ["Dust & Scratches", [num("Radius:", 1, { unit: "px", w: 70 }), num("Threshold:", 0, { w: 70 })], true],
  median: ["Median", [num("Radius:", 2, { unit: "px", w: 70 })], true],
  "color-halftone": ["Color Halftone", [num("Max Radius:", 8, { unit: "px", w: 70 }), { type: "cmykrow" }], true],
  crystallize: ["Crystallize", [num("Cell Size:", 10, { w: 70 })], true],
  mezzotint: ["Mezzotint", [sel("Type:", ["Fine Dots", "Medium Dots", "Grainy", "Coarse", "Short Lines", "Medium Lines", "Long Lines", "Short Strokes"], "Fine Dots")], true],
  mosaic: ["Mosaic", [num("Cell Size:", 12, { unit: "px", w: 70 })], true],
  pointillize: ["Pointillize", [num("Cell Size:", 8, { w: 70 })], true],
  fibers: ["Fibers", [num("Variance:", 16, { w: 70 }), num("Strength:", 4, { w: 70 }), { type: "btn", text: "Randomise" }], true],
  "lens-flare": ["Lens Flare", [num("Brightness:", 100, { unit: "%", w: 70 }), sel("Lens Type:", ["50-300mm Zoom", "35mm Prime", "105mm Prime", "Movie Prime"], "50-300mm Zoom"), { type: "flarepos" }], true],
  "lighting-effects": ["Lighting Effects", [sel("Style:", ["Default", "Flood Light", "Triple Down", "Blue Omni", "Crossing", "Flashlight"], "Default"), grp("Light Type", [sel("", ["Spot", "Point", "Infinite"], "Spot"), chk("On", true), sel("Colour:", ["White", "Warm", "Cool"], "White"), num("Intensity:", 35, { w: 60 })]), rad("Preview:", ["Texture", "3D", "Light"], 0, { inline: true }), grp("Properties", [num("Gloss:", 0, { w: 60 }), num("Material:", 69, { w: 60 }), num("Exposure:", 0, { w: 60 }), num("Ambience:", 8, { w: 60 })])], true],
  flame: ["Flame", [sel("Type:", ["One Flame", "One Flame Along Path", "Multiple Flames Along Path"], "One Flame"), num("Width:", 80, { unit: "px", w: 70 }), num("Flame Lines:", 36, { w: 70 }), num("Soot:", 30, { w: 70 }), num("Spark:", 30, { w: 70 }), { type: "btn", text: "Randomise" }, grp("Advanced", [sel("Flame Type:", ["Sharper", "Soft", "Pointed"], "Sharper"), num("Angles:", 0, { w: 60 }), num("Curl:", 0, { w: 60 }), num("Flame Angle:", 0, { w: 60 })])], true],
  tree: ["Tree", [sel("Base Tree Type:", ["Default", "Acacia", "Birch", "Cherry", "Oak", "Palm", "Pine", "Willow"], "Default"), sel("Leaves Type:", ["Default", "Ashes", "Fruit", "Maple", "Pine"], "Default"), { type: "color", label: "Tree Colour", value: "#6b4f2a" }, { type: "color", label: "Leaves Colour", value: "#4f8b3b" }, num("Tree Size:", 80, { w: 70 }), num("Leaves Amount:", 70, { w: 70 }), num("Branches Amount:", 40, { w: 70 }), { type: "btn", text: "Randomise" }], true],
  "picture-frame": ["Picture Frame", [sel("Frame:", ["Aluminium", "Black Wood", "Canvas", "Silver", "Wood"], "Wood"), { type: "color", label: "Frame Colour", value: "#8a6b48" }, num("Size:", 60, { w: 70 }), num("Padding:", 20, { w: 70 }), chk("Inside", false), { type: "btn", text: "Randomise" }], true],
  "smart-sharpen": ["Smart Sharpen", [num("Amount:", 100, { unit: "%", w: 70 }), num("Radius:", 1, { unit: "px", w: 70 }), grp("Noise Reduction", [num("Amount:", 10, { unit: "%", w: 70 }), num("Radius:", 1, { unit: "px", w: 70 })]), sel("Remove:", ["Gaussian Blur", "Lens Blur", "Motion Blur"], "Gaussian Blur"), sel("Shadows:", ["0", "25", "50", "75", "100"], "50"), sel("Highlights:", ["0", "25", "50", "75", "100"], "50"), chk("More Accurate", false)], true],
  "unsharp-mask": ["Unsharp Mask", [num("Amount:", 50, { unit: "%", w: 70 }), num("Radius:", 1, { unit: "px", w: 70 }), num("Threshold:", 0, { w: 70 })], true],
  emboss: ["Emboss", [num("Angle:", 135, { unit: "°", w: 70 }), num("Height:", 3, { unit: "px", w: 70 }), num("Amount:", 100, { unit: "%", w: 70 })], true],
  "glowing-edges": ["Glowing Edges", [num("Edge Width:", 2, { w: 70 }), num("Edge Brightness:", 6, { w: 70 }), num("Smoothness:", 5, { w: 70 })], true],
  "oil-paint": ["Oil Paint", [num("Stylization:", 6.4, { w: 70 }), num("Cleanliness:", 5, { w: 70 }), num("Scale:", 1, { w: 70 }), num("Bristle Detail:", 2.6, { w: 70 }), sel("Lighting:", ["Unchecked", "Checked"], "Unchecked"), chk("Across Layers", false)], true],
  tiles: ["Tiles", [num("Number of Tiles:", 10, { w: 70 }), num("Maximum Offset:", 10, { unit: "%", w: 70 }), grp("Fill Empty Area With", [rad("", ["Background Colour", "Foreground Colour", "Inverse Image", "Unaltered Image"], 0)])], true],
  "trace-contour": ["Trace Contour", [num("Level:", 128, { w: 70 }), rad("Edge:", ["Lower", "Upper"], 0)], true],
  wind: ["Wind", [rad("Method:", ["Wind", "Blast", "Stagger"], 0), rad("Direction:", ["From the Right", "From the Left"], 0), num("Strength:", 50, { w: 70 })], true],
  deinterlace: ["De-Interlace", [grp("Eliminate:", [rad("", ["Odd Fields", "Even Fields"], 0)]), grp("Create New Fields by:", [rad("", ["Interpolation", "Duplication"], 0)])], true],
  "custom-filter": ["Custom", [{ type: "matrix" }, row(num("Scale:", 1, { w: 60 }), num("Offset:", 0, { w: 60 }))], true],
  "high-pass": ["High Pass", [num("Radius:", 10, { unit: "px", w: 70 })], true],
  maximum: ["Maximum", [num("Radius:", 1, { unit: "px", w: 70 }), sel("Preserve:", ["Squareness", "Roundness"], "Squareness")], true],
  minimum: ["Minimum", [num("Radius:", 1, { unit: "px", w: 70 }), sel("Preserve:", ["Squareness", "Roundness"], "Squareness")], true],
  offset: ["Offset", [num("Horizontal:", 100, { unit: "px", w: 70 }), num("Vertical:", 100, { unit: "px", w: 70 }), rad("Undefined Areas:", ["Set to Transparent", "Repeat Edge Pixels", "Wrap Around"], 1)], true],
  fade: ["Fade", [sel("Mode:", ["Normal", "Multiply", "Screen", "Overlay"], "Normal"), num("Opacity:", 50, { unit: "%", w: 70 })], true],
  liquify: ["Liquify", [{ type: "liquify" }], true],
  "vanishing-point": ["Vanishing Point", [{ type: "vanishing" }], true],
  "filter-gallery": ["Filter Gallery", [{ type: "gallery" }], true],
};

for (const [id, [title, fields, preview]] of Object.entries(filterSpecs)) {
  dialogs[id] = {
    title, width: preview ? 520 : 420, icon: "i-fx",
    fields: preview ? [...fields, chk("Preview", true), PREVIEW] : fields,
    ok: "OK", cancel: "Cancel",
  };
}

// dialogo di ripiego: qualunque id non definito mostra una finestra coerente
export function dialogDef(id) {
  if (dialogs[id]) return dialogs[id];
  const pretty = id.replace(/^(dlg|filter|ext|misc|ai|tpl|ws|acct|doc|img|adj|sel|layer|mask|order|align|dist|mode|screen|par):/, "").replace(/-/g, " ");
  return {
    title: pretty.replace(/\b\w/g, (c) => c.toUpperCase()),
    width: 420,
    fields: [lbl("This command is part of the Fotox interface mock: the dialog exists so every menu entry responds, but the operation itself is not implemented.")],
    ok: null, cancel: "Close",
  };
}
