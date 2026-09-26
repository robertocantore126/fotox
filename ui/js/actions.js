// Fotox — esecuzione delle azioni richieste da menu, tastiera e pulsanti.
// Le azioni con un effetto visibile sono implementate; tutte le altre
// rispondono con un feedback coerente, così nessuna voce resta muta.

import { state, setFlag, toggleFlag, setTool, emit } from "./state.js";
import { openDialog } from "./dialogs.js";
import { toast, status } from "./tooltip.js";
import { zoomIn, zoomOut, fit, actual, zoomTo, toggleFpsOverlay } from "./canvas.js";
import * as panels from "./panels.js";
import { newAdjustmentLayer } from "./native/layers-panel.js";
import { dockGroups } from "./data/panels.js";
import * as bridge from "./native/bridge.js";
import { UI } from "./native/protocol.js";
import { isEngineFilter, openFilterDialog } from "./native/filters.js";
import { cmykProfiles, isColorDialog, openColorDialog } from "./native/color.js";
import { isImageDialog, openImageDialog } from "./native/image.js";
import { isStyleDialog, openStyleDialog } from "./native/styles.js";
import { openNewDocument } from "./native/newdoc.js";
import { isGuideDialog, openGuideDialog } from "./native/guides.js";
import { isPrefsDialog, openPrefsDialog } from "./native/prefs.js";
import { activeDocument } from "./native/documents.js";
import { isGradientDialog, openGradientDialog, openGradientFillDialog } from "./native/gradients.js";
import { isPatternDialog, openPatternFillDialog } from "./native/patterns.js";
import { isChannelDialog, openChannelDialog, channelNames } from "./native/channels-panel.js";
import { isSelectionDialog, openSelectionDialog } from "./native/selections.js";
import { activeLayerInfo } from "./native/layers-panel.js";
import { dialogDef } from "./data/dialogs.js";

export function runAction(item) {
  const a = item && item.a ? item.a : "";
  const label = (item && item.label) || a;

  // Every action also goes to the engine, which ignores the ones it does not
  // own (docs/PROTOCOL.md §4). The local behaviour below stays as it is.
  if (a) bridge.send({ type: UI.ACTION, id: a });

  // dialoghi -------------------------------------------------------------
  // Frame-time overlay (Ctrl+Alt+F): only the app has a render thread to measure.
  if (a === "debug:fps") {
    if (bridge.isNative) toggleFpsOverlay();
    else toast("The frame-time overlay measures the app's render thread (not available in a browser)");
    return;
  }
  // In the app, layer and history actions are the engine's (sent above): it
  // answers with the result, or a toast for what is not implemented yet.
  // Layer Content Options (M8-T03/T06): the fill layer's dialog.
  if (a === "layer:content-options" && bridge.isNative) {
    const fill = activeLayerInfo()?.fill_layer;
    if (fill?.fill === "gradient") openGradientFillDialog(true);
    else if (fill?.fill === "pattern") emit("pattern:content-options");
    return;
  }
  if (bridge.isNative && (a.startsWith("layer:") || a.startsWith("hist:"))) return;
  // M8/M9 engine actions (brushes, patterns, measuring tools, Define Pattern).
  if (bridge.isNative && ["brush:", "pattern:", "sampler:", "ruler:", "notes:", "count:", "select-mask:", "channels:", "misc:define-", "path:", "vmask:", "type:work-path", "type:to-shape", "smart:", "raster:smart", "layer:artboard", "comps:", "slices:", "misc:export-slices", "misc:export-artboards"].some((p) => a.startsWith(p))) return;
  // So are the Image menu's rotations and crops (M6-T02/T03), Free Transform
  // and its submenu (M6-T04), the Select menu (M5), Filter ▸ Last Filter and
  // Layer ▸ Rasterize (M6-T06): the mock's "not implemented" toast must not
  // follow them.
  if (bridge.isNative && ["img:", "xf:", "sel:", "filter:", "raster:"].some((p) => a.startsWith(p))) return;
  // Other debug actions are the engine's (sent above); nothing to do here.
  if (a.startsWith("debug:")) {
    if (!bridge.isNative) toast(label + " needs the app (not available in a browser)");
    return;
  }

  // In the app, Select ▸ Modify sends the command (M5-T10).
  const modify = { "dlg:sel-border": "border", "dlg:sel-smooth": "smooth", "dlg:sel-expand": "expand", "dlg:sel-contract": "contract", "dlg:sel-feather": "feather" }[a];
  if (modify && bridge.isNative) {
    openDialog(a.slice(4), {
      onOk: (v) => {
        const px = Number(v["Amount:"]);
        const doc = activeDocument();
        if (doc == null || !(px > 0)) return;
        bridge.send({ type: UI.COMMAND, doc, command: { op: "modify_selection", modify: { kind: modify, px } } });
      },
    });
    return;
  }
  // In the app, Edit ▸ Fill (Shift+F5) sends its values to the engine, which
  // resolves the swatch colours (M5-T05).
  if (a === "dlg:fill" && bridge.isNative) {
    openDialog("fill", {
      onOk: (v) => bridge.send({
        type: UI.ACTION, id: "edit:fill",
        args: { use: v["Use:"], mode: v["Mode:"], opacity: Number(v["Opacity:"]), preserve: !!v["Preserve Transparency"] },
      }),
    });
    return;
  }
  // Clipboard, Fill shortcuts and masks from the selection are the engine's.
  if (bridge.isNative && (a.startsWith("clip:") || a.startsWith("edit:fill") || a === "mask:reveal-sel" || a === "mask:hide-sel")) return;
  // In the app, Export As collects the options, then the shell shows the save
  // dialog for the chosen format and the engine exports (M3-T07).
  if (a === "dlg:export-as" && bridge.isNative) {
    // The CMYK menu lists the profiles the engine found (M4-T04).
    const cmyk = cmykProfiles();
    const withCmyk = (fields) => fields.map((f) => (f.fields ? { ...f, fields: withCmyk(f.fields) } : f.label === "CMYK:" ? { ...f, options: ["None", ...cmyk.map((p) => p.name)] } : f));
    openDialog("export-as", {
      fields: withCmyk(dialogDef("export-as").fields || []),
      onOk: (v) => bridge.send({
        type: UI.ACTION, id: "export:as",
        args: {
          // CMYK is written as TIFF.
          format: (cmyk.find((p) => p.name === v["CMYK:"]) ? "tif" : { JPG: "jpg", TIFF: "tif" }[v["Format:"]]) || "png",
          cmyk: (cmyk.find((p) => p.name === v["CMYK:"]) || {}).path || "",
          eight_bit: v["Bit Depth:"] === "8 bits/channel",
          transparency: { On: "on", Off: "off" }[v["Transparency:"]] || "auto",
          quality: v["Quality:"],
          chroma: String(v["Chroma:"]).startsWith("4:2:0") ? "420" : "444",
        },
      }),
    });
    return;
  }
  // In the app, Image ▸ Trim (M6-T03) looks at the layer's pixels in the
  // engine: the dialog's choices travel with the action.
  if (a === "dlg:trim" && bridge.isNative) {
    openDialog("trim", {
      onOk: (v) => bridge.send({
        type: UI.ACTION, id: "edit:trim",
        args: { based_on: v["Based On:"], away: Array.isArray(v["Trim Away:"]) ? v["Trim Away:"] : [] },
      }),
    });
    return;
  }
  // In the app, Image Size / Canvas Size / Rotate Arbitrary are the engine's
  // commands (M6-T02).
  if (a.startsWith("dlg:") && bridge.isNative && isImageDialog(a.slice(4))) { openImageDialog(a.slice(4)); return; }
  // In the app, colour management dialogs and the proof toggles (M4-T03/T04).
  if (a.startsWith("dlg:") && bridge.isNative && isColorDialog(a.slice(4))) { openColorDialog(a.slice(4)); return; }
  if ((a === "view:proof-colors" || a === "view:gamut-warning") && bridge.isNative) return;
  // In the app, Gaussian Blur and Unsharp Mask preview live and apply as a
  // job in the engine (M4-T05).
  if (a.startsWith("dlg:") && bridge.isNative && isEngineFilter(a.slice(4))) { openFilterDialog(a.slice(4)); return; }
  // In the app, File ▸ New builds a real document (M7-T01).
  if (a === "dlg:new-doc" && bridge.isNative) { openNewDocument(); return; }
  // Type ▸ Warp Text (M10-T08).
  // M11 warp sessions: the engine puts its bar up.
  const warps = { "dlg:liquify": "warp:liquify", "misc:puppet-warp": "warp:puppet", "misc:perspective-warp": "warp:perspective", "warp:straighten": "warp:straighten" };
  if (warps[a] && bridge.isNative) {
    bridge.send({ type: UI.ACTION, id: warps[a] });
    return;
  }
  // Image ▸ Adjustments ▸ Color Lookup / Selective Color (M12-T05): as
  // adjustment layers.
  if ((a === "dlg:color-lookup" || a === "dlg:selective-color") && bridge.isNative) {
    newAdjustmentLayer(a === "dlg:color-lookup" ? "Color Lookup..." : "Selective Color...");
    return;
  }
  // Edit ▸ Content-Aware Scale (M11-T05). FAST: a dialog instead of the
  // transform box.
  if (a === "misc:content-aware-scale" && bridge.isNative) {
    const names = channelNames();
    const def = dialogDef("content-aware-scale");
    openDialog("content-aware-scale", {
      fields: def.fields.map((f) => (f.label === "Protect:" ? { ...f, options: ["None", ...names] } : f)),
      onOk: (v) => bridge.send({
        type: UI.ACTION, id: "edit:content-aware-scale",
        args: { width: Number(v["Width:"]), height: Number(v["Height:"]), amount: Number(v["Amount:"]), protect: names.indexOf(v["Protect:"]), skin: !!v["Protect Skin Tones"] },
      }),
    });
    return;
  }
  if (a === "dlg:content-aware-fill" && bridge.isNative) {
    openDialog("content-aware-fill", {
      onOk: (v) => bridge.send({ type: UI.ACTION, id: "edit:content-aware-fill", args: { output: v["Output To:"], seed: Number(v["Seed:"]) || 1 } }),
    });
    return;
  }
  if (a === "dlg:warp-text" && bridge.isNative) {
    openDialog("warp-text", {
      onOk: (v) => bridge.send({ type: UI.ACTION, id: "type:warp", args: { style: v["Style:"], bend: Number(v["Bend:"]) } }),
    });
    return;
  }
  // Gradients (M8-T03): New Fill Layer ▸ Gradient, the Gradient Editor.
  if (a.startsWith("dlg:") && bridge.isNative && isGradientDialog(a.slice(4))) { openGradientDialog(a.slice(4)); return; }
  if (a.startsWith("dlg:") && bridge.isNative && isPatternDialog(a.slice(4))) { openPatternFillDialog(false); return; }
  if (a.startsWith("dlg:") && bridge.isNative && isChannelDialog(a.slice(4))) { openChannelDialog(a.slice(4)); return; }
  if (a.startsWith("dlg:") && bridge.isNative && isSelectionDialog(a.slice(4))) { openSelectionDialog(a.slice(4)); return; }
  // In the app, Preferences are the engine's file (M7-T09).
  if (a.startsWith("dlg:") && bridge.isNative && isPrefsDialog(a.slice(4))) { openPrefsDialog(a.slice(4)); return; }
  // In the app, guides are the engine's (M7-T06).
  if (a.startsWith("dlg:") && bridge.isNative && isGuideDialog(a.slice(4))) { openGuideDialog(a.slice(4)); return; }
  // In the app, the five layer styles and Blending Options are live (M6-T08).
  if (a.startsWith("dlg:") && bridge.isNative && isStyleDialog(a.slice(4))) { openStyleDialog(a.slice(4)); return; }
  // In the app, Open is the native file dialog (the shell shows it).
  if (a === "dlg:open" && bridge.isNative) { status(label); return; }
  if (a.startsWith("dlg:")) { openDialog(a.slice(4)); status(label); return; }

  // pannelli -------------------------------------------------------------
  if (a.startsWith("panel:toggle:")) { panels.togglePanel(a.slice(13)); return; }
  if (a.startsWith("panel:closegroup:")) { panels.closeGroup(a.slice(17)); return; }
  if (a.startsWith("panel:collapse:")) { panels.collapseGroup(a.slice(15), true); return; }
  if (a.startsWith("panel:expand:")) { panels.collapseGroup(a.slice(13), false); return; }
  if (a === "panel:expandall") { dockGroups.forEach((g) => panels.collapseGroup(g.id, false)); return; }
  // le voci del menu Finestra mostrano una spunta: il click apre o chiude
  if (a.startsWith("panel:")) { panels.togglePanel(a.slice(6)); return; }
  if (a === "panels:all") { panels.setAllPanels(true); toast("All panels shown"); return; }
  if (a === "panels:none") { panels.setAllPanels(false); toast("All panels hidden"); return; }
  if (a === "panels:reset") { panels.resetPanels(); toast("Workspace reset"); return; }

  // strumenti ------------------------------------------------------------
  // The option bar's ✓ and ✗ (M6-T03) are the active tool's commit and
  // cancel: the engine owns them (sent above), they are not tool picks.
  if (a === "tool:commit" || a === "tool:cancel") return;
  if (a.startsWith("tool:")) { setTool(a.slice(5)); return; }

  // vista ----------------------------------------------------------------
  if (a === "zoom:in") { zoomIn(); return; }
  if (a === "zoom:out") { zoomOut(); return; }
  if (a === "zoom:fit") { fit(); status("Fit on screen"); return; }
  if (a === "zoom:100") { actual(); status("Actual pixels"); return; }
  if (a === "zoom:fill") { zoomTo(200); status("Fill screen"); return; }
  if (a === "zoom:print") { zoomTo(72); status("Print size"); return; }

  if (a.startsWith("toggle:")) {
    const key = a.slice(7);
    const on = toggleFlag(key);
    toast(`${prettyLabel(label)}: ${on ? "on" : "off"}`);
    return;
  }
  if (a === "guides:clear") { toast("Guides cleared"); return; }
  if (a === "view:clear") { toast("Cleared the last filter and any guides"); return; }
  if (a === "toggle:all-extras") { setFlag("extras", !state.flags.extras); toast(`Extras: ${state.flags.extras ? "on" : "off"}`); return; }

  // modi schermo ---------------------------------------------------------
  if (a === "screen:cycle") {
    const order = ["standard", "menubar", "full"];
    const next = order[(order.indexOf(state.screenMode) + 1) % order.length];
    applyScreenMode(next);
    status("Screen mode: " + next);
    return;
  }
  if (a.startsWith("screen:")) { applyScreenMode(a.slice(7)); return; }

  if (a.startsWith("par:")) {
    ["par:square", "par:ntsc", "par:pal", "par:ana2"].forEach((k) => setFlag(k, k === a));
    toast(label);
    return;
  }
  if (a.startsWith("mode:")) {
    const chk = ["mode:rgb", "mode:8"];
    if (chk.includes(a)) { setFlag(a, true); toast(label + " — already the working mode"); return; }
    toast(`Mode: ${label} (needs a real conversion — mock)`);
    return;
  }

  // documenti e schede ---------------------------------------------------
  // In the app the engine closes the documents (sent above).
  if ((a === "tab:close" || a === "tab:close-all") && bridge.isNative) return;
  if (a === "tab:close") { toast("Closing “Untitled-1” would close the document (mock)"); return; }
  if (a === "tab:close-all") { toast("All documents would be closed (mock)"); return; }
  if (a.startsWith("doc:recent")) { toast("Open recent document (mock)"); return; }
  // In the app, Save / Save As are the engine's (sent above): it saves, or the
  // shell shows the native save dialog (M3-T06).
  if ((a === "doc:save" || a === "doc:save-as") && bridge.isNative) { status(label); return; }
  if (a.startsWith("doc:save")) { openDialog("export-as"); return; }
  if (a === "doc:revert" && !bridge.isNative) { toast("Reverted to the last saved state (mock)"); return; }
  // In the app, PNG and TIFF export are real: the shell shows the save dialog.
  if ((a === "export:png" || a === "export:tiff" || a === "export:jpg") && bridge.isNative) { status(label); return; }
  if (a.startsWith("export:")) { openDialog("export-as"); return; }

  // IA, estensioni, account ---------------------------------------------
  if (a === "ai:generate" || a === "ai:fill") { openDialog("generate"); return; }
  if (a.startsWith("ai:")) { toast(label + " — the AI engine is not part of this mock"); return; }
  if (a.startsWith("ext:") || a.startsWith("tpl:") || a.startsWith("acct:")) { toast(label + " (mock)"); return; }
  if (a === "app:exit") { toast("Fotox stays open — this is a browser mock"); return; }
  if (a === "app:startpage") { toast("The start page is not part of this mock"); return; }
  if (a.startsWith("ws:")) { toast("Workspace: " + label); return; }
  if (a.startsWith("win:")) { toast(label + " (mock)"); return; }

  // tutto il resto: feedback coerente ------------------------------------
  toast(`${prettyLabel(label)} — not implemented in this mock`);
  status(`${prettyLabel(label)}: interface only`);
}

function prettyLabel(label) {
  return label.replace(/\.\.\.$/, "").replace(/\s+/g, " ").trim();
}

export function applyScreenMode(mode) {
  state.screenMode = mode;
  ["standard", "menubar", "full"].forEach((m) => setFlag(`screen:${m}`, m === mode));
  document.body.classList.toggle("screen-full", mode === "full");
  document.body.classList.toggle("screen-menubar", mode === "menubar");
  toast("Screen mode: " + (mode === "menubar" ? "full screen with menu bar" : mode === "full" ? "full screen" : "standard"));
}
