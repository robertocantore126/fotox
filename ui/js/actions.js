// Fotox — esecuzione delle azioni richieste da menu, tastiera e pulsanti.
// Le azioni con un effetto visibile sono implementate; tutte le altre
// rispondono con un feedback coerente, così nessuna voce resta muta.

import { state, setFlag, toggleFlag, setTool } from "./state.js";
import { openDialog } from "./dialogs.js";
import { toast, status } from "./tooltip.js";
import { zoomIn, zoomOut, fit, actual, zoomTo } from "./canvas.js";
import * as panels from "./panels.js";
import { dockGroups } from "./data/panels.js";

export function runAction(item) {
  const a = item && item.a ? item.a : "";
  const label = (item && item.label) || a;

  // dialoghi -------------------------------------------------------------
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
  if (a === "tab:close") { toast("Closing “Untitled-1” would close the document (mock)"); return; }
  if (a === "tab:close-all") { toast("All documents would be closed (mock)"); return; }
  if (a.startsWith("doc:recent")) { toast("Open recent document (mock)"); return; }
  if (a.startsWith("doc:save")) { openDialog("export-as"); return; }
  if (a === "doc:revert") { toast("Reverted to the last saved state (mock)"); return; }
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
