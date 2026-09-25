// Fotox — esecuzione delle azioni richieste da menu, tastiera e pulsanti.
// Le azioni con un effetto visibile sono implementate; tutte le altre
// rispondono con un feedback coerente, così nessuna voce resta muta.

import { state, setFlag, toggleFlag, setTool } from "./state.js";
import { openDialog } from "./dialogs.js";
import { toast, status } from "./tooltip.js";
import { zoomIn, zoomOut, fit, actual, zoomTo, toggleFpsOverlay } from "./canvas.js";
import * as panels from "./panels.js";
import { dockGroups } from "./data/panels.js";
import * as bridge from "./native/bridge.js";
import { UI } from "./native/protocol.js";
import { isEngineFilter, openFilterDialog } from "./native/filters.js";
import { cmykProfiles, isColorDialog, openColorDialog } from "./native/color.js";
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
  if (bridge.isNative && (a.startsWith("layer:") || a.startsWith("hist:"))) return;
  // Other debug actions are the engine's (sent above); nothing to do here.
  if (a.startsWith("debug:")) {
    if (!bridge.isNative) toast(label + " needs the app (not available in a browser)");
    return;
  }

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
  // In the app, colour management dialogs and the proof toggles (M4-T03/T04).
  if (a.startsWith("dlg:") && bridge.isNative && isColorDialog(a.slice(4))) { openColorDialog(a.slice(4)); return; }
  if ((a === "view:proof-colors" || a === "view:gamut-warning") && bridge.isNative) return;
  // In the app, Gaussian Blur and Unsharp Mask preview live and apply as a
  // job in the engine (M4-T05).
  if (a.startsWith("dlg:") && bridge.isNative && isEngineFilter(a.slice(4))) { openFilterDialog(a.slice(4)); return; }
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
  if (a === "doc:revert") { toast("Reverted to the last saved state (mock)"); return; }
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
