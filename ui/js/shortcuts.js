// Fotox — scorciatoie da tastiera.

import { setTool, emit } from "./state.js";
import { runAction } from "./actions.js";
import { closeAllDialogs, isDialogOpen } from "./dialogs.js";
import { toolSlots } from "./data/tools.js";

const combos = [
  ["ctrl+n", "New...", "dlg:new-doc"],
  ["ctrl+alt+n", "New...", "dlg:new-doc"],
  ["ctrl+o", "Open...", "dlg:open"],
  ["ctrl+alt+f", "Frame-time overlay", "debug:fps"],
  ["ctrl+s", "Save", "doc:save"],
  ["ctrl+shift+s", "Save As", "doc:save-as"],
  ["ctrl+shift+c", "Copy Merged", "clip:copy-merged"],
  ["ctrl+alt+i", "Image Size...", "dlg:image-size"],
  ["ctrl+alt+c", "Canvas Size...", "dlg:canvas-size"],
  ["ctrl+l", "Levels...", "dlg:levels"],
  ["ctrl+m", "Curves...", "dlg:curves"],
  ["ctrl+u", "Hue/Saturation...", "dlg:hue-saturation"],
  ["ctrl+b", "Colour Balance...", "dlg:color-balance"],
  ["ctrl+k", "Preferences...", "dlg:prefs"],
  ["ctrl+t", "Free Transform", "misc:free-transform"],
  ["ctrl+j", "Layer via Copy", "layer:via-copy"],
  ["ctrl+g", "Group Layers", "layer:group"],
  ["ctrl+e", "Merge Layers", "layer:merge"],
  ["ctrl+a", "Select All", "sel:all"],
  ["ctrl+d", "Deselect", "sel:none"],
  ["ctrl+shift+i", "Inverse Selection", "sel:inverse"],
  ["ctrl+p", "Print...", "dlg:print"],
  ["ctrl+f", "Last Filter", "filter:last"],
  ["ctrl+w", "Close", "tab:close"],
  ["ctrl+0", "Fit on Screen", "zoom:fit"],
  ["ctrl+1", "Actual Pixels", "zoom:100"],
  ["ctrl++", "Zoom In", "zoom:in"],
  ["ctrl+=", "Zoom In", "zoom:in"],
  ["ctrl+-", "Zoom Out", "zoom:out"],
  ["ctrl+r", "Rulers", "toggle:rulers"],
  ["ctrl+'", "Grid", "toggle:grid"],
  ["ctrl+;", "Guides", "toggle:guides"],
  ["ctrl+h", "Extras", "toggle:extras"],
  ["ctrl+shift+z", "Redo", "hist:redo"],
  ["ctrl+z", "Undo", "hist:undo"],
  ["ctrl+alt+z", "Toggle Last State", "hist:toggle"],
  ["f5", "Brush panel", "panel:toggle:brush"],
  ["f6", "Colour panel", "panel:toggle:color"],
  ["f7", "Layers panel", "panel:toggle:layers"],
  ["f8", "Info panel", "panel:toggle:info"],
  ["f9", "Actions panel", "panel:toggle:actions"],
  ["f11", "Full screen", "screen:cycle"],
];

const toolKeys = {};
for (const slot of toolSlots) toolKeys[slot.key.toLowerCase()] = slot.id;

function comboOf(e) {
  const parts = [];
  if (e.ctrlKey || e.metaKey) parts.push("ctrl");
  if (e.altKey) parts.push("alt");
  if (e.shiftKey) parts.push("shift");
  let key = e.key;
  if (key === " ") key = "space";
  parts.push(key.toLowerCase());
  return parts.join("+");
}

export function initShortcuts() {
  document.addEventListener("keydown", (e) => {
    const typing = e.target && (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA" || e.target.isContentEditable);

    if (e.key === "Escape") {
      if (isDialogOpen()) { closeAllDialogs(); e.preventDefault(); return; }
      if (typing) { e.target.blur(); return; }
    }

    if (typing) return;

    if (e.key === "Tab" && !e.ctrlKey && !e.altKey) {
      runAction({ label: "Panels", a: "toggle:panels" });
      e.preventDefault();
      return;
    }

    const combo = comboOf(e);
    const hit = combos.find(([c]) => c === combo);
    if (hit) {
      runAction({ label: hit[1], a: hit[2] });
      e.preventDefault();
      return;
    }

    if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key.toLowerCase() === "f") {
      runAction({ label: "Screen Mode", a: "screen:cycle" });
      e.preventDefault();
      return;
    }

    if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key.length === 1) {
      const id = toolKeys[e.key.toLowerCase()];
      if (id) {
        setTool(id);
        emit("tool", id);
        e.preventDefault();
      }
    }
  });
}
