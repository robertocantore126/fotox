// Fotox — scorciatoie da tastiera.

import { setTool, emit } from "./state.js";
import { runAction } from "./actions.js";
import { closeAllDialogs, isDialogOpen } from "./dialogs.js";
import { isPopupOpen } from "./popup.js";
import { optionValue, setOption } from "./optionsbar.js";
import { toolSlots } from "./data/tools.js";
import * as bridge from "./native/bridge.js";
import { UI } from "./native/protocol.js";

// Keys the viewport tools own (M5-T04): Escape cancels the marquee or lasso
// being drawn, Enter closes a polygonal lasso, Backspace/Delete drops its last
// point. Everything else stays with the menus and the shortcut map below, and
// the engine ignores these when no tool has anything in progress.
const VIEWPORT_KEYS = new Set(["Escape", "Enter", "Backspace", "Delete", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"]);

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
  ["ctrl+shift+j", "Layer via Cut", "layer:via-cut"],
  ["ctrl+c", "Copy", "clip:copy"],
  ["ctrl+x", "Cut", "clip:cut"],
  ["ctrl+v", "Paste", "clip:paste"],
  ["ctrl+shift+v", "Paste in Place", "clip:paste-special"],
  ["shift+f5", "Fill...", "dlg:fill"],
  ["alt+backspace", "Fill with Foreground", "edit:fill-fg"],
  ["ctrl+backspace", "Fill with Background", "edit:fill-bg"],
  ["alt+shift+backspace", "Fill with Foreground (Preserve Transparency)", "edit:fill-fg-preserve"],
  ["ctrl+shift+backspace", "Fill with Background (Preserve Transparency)", "edit:fill-bg-preserve"],
  ["ctrl+g", "Group Layers", "layer:group"],
  ["ctrl+e", "Merge Layers", "layer:merge"],
  ["ctrl+y", "Proof Colors", "view:proof-colors"],
  ["ctrl+shift+y", "Gamut Warning", "view:gamut-warning"],
  ["ctrl+shift+e", "Merge Visible", "layer:merge-visible"],
  ["ctrl+alt+shift+e", "Stamp Visible", "layer:stamp-visible"],
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

/** The next brush size for `[` (down) or `]` (up), in Photoshop's steps. */
function sizeStep(size, up) {
  const step = (v) => (v < 10 ? 1 : v < 100 ? 10 : v < 200 ? 25 : v < 500 ? 50 : 100);
  if (up) return Math.min(5000, size + step(size));
  const down = size - step(size - 1);
  return Math.max(1, down);
}

let lastDigit = null;

/** Handle a painting shortcut; `true` when the key was one. */
function brushKey(e) {
  if (e.code === "BracketLeft" || e.code === "BracketRight") {
    const up = e.code === "BracketRight";
    if (e.shiftKey) {
      const hardness = optionValue("Hardness");
      if (hardness == null) return false;
      return setOption("Hardness", Math.max(0, Math.min(100, hardness + (up ? 25 : -25))));
    }
    const size = optionValue("Size");
    if (size == null) return false;
    return setOption("Size", sizeStep(size, up));
  }
  if (!e.shiftKey && /^Digit[0-9]$/.test(e.code) && optionValue("Opacity") != null) {
    const digit = Number(e.code.slice(5));
    const now = performance.now();
    let value = digit === 0 ? 100 : digit * 10;
    if (lastDigit && now - lastDigit.at < 600) {
      value = lastDigit.digit * 10 + digit;
      lastDigit = null;
    } else {
      lastDigit = { digit, at: now };
    }
    return setOption("Opacity", value);
  }
  return false;
}

export function initShortcuts() {
  document.addEventListener("keydown", (e) => {
    const typing = e.target && (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA" || e.target.isContentEditable);

    if (e.key === "Escape") {
      if (isDialogOpen()) { closeAllDialogs(); e.preventDefault(); return; }
      if (typing) { e.target.blur(); return; }
    }

    // A viewport key (M5-T04): the tools get it before the menus do.
    // Arrow keys move the selection outline (Shift = 10 px): the engine
    // gets "Shift+ArrowLeft". Menus keep their own arrow navigation.
    if (!typing && !isDialogOpen() && !isPopupOpen() && !e.ctrlKey && !e.metaKey && !e.altKey && VIEWPORT_KEYS.has(e.key)) {
      bridge.send({ type: UI.KEY, key: e.shiftKey && e.key.startsWith("Arrow") ? `Shift+${e.key}` : e.key });
      e.preventDefault();
      return;
    }

    if (typing) return;

    // Painting shortcuts (M5-T09): [ and ] change the size, Shift+[ and ]
    // the hardness by 25 %, the number keys the opacity (1 = 10 % … 0 = 100 %;
    // two digits typed quickly = that exact value).
    if (!e.ctrlKey && !e.metaKey && !e.altKey && !isDialogOpen() && brushKey(e)) {
      e.preventDefault();
      return;
    }

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
