// Fotox — the Type tool's keyboard side (M6-T07).
//
// The engine lays the text out and draws it; the UI owns the keyboard and the
// IME. While a type session is open a hidden <textarea> holds the focus, and
// every change of its text or selection goes to the engine as `text_edit`
// (UTF-8 byte offsets, which is what the engine indexes). Esc cancels,
// Ctrl+Enter / keypad Enter commit (sent as the viewport keys Escape / Enter).

import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";
import { optionsFor } from "../data/options.js";

const encoder = new TextEncoder();
let area = null;
let lastSent = "";

/** UTF-16 index in `text` → UTF-8 byte offset. */
function toBytes(text, index) {
  return encoder.encode(text.slice(0, index)).length;
}

/** UTF-8 byte offset → UTF-16 index in `text`. */
function fromBytes(text, bytes) {
  let count = 0;
  for (let i = 0; i < text.length; i++) {
    if (count >= bytes) return i;
    const code = text.codePointAt(i);
    count += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    if (code >= 0x10000) i++;
  }
  return text.length;
}

function send() {
  if (!area) return;
  const text = area.value;
  const selection = [toBytes(text, area.selectionStart), toBytes(text, area.selectionEnd)];
  const key = text + "\u0000" + selection.join(",");
  if (key === lastSent) return;
  lastSent = key;
  bridge.send({ type: UI.TEXT_EDIT, text, selection });
}

function ensureArea() {
  if (area) return area;
  area = document.createElement("textarea");
  area.id = "type-input";
  area.setAttribute("autocomplete", "off");
  area.setAttribute("spellcheck", "false");
  // Invisible, but focusable and in the layout (so the IME has somewhere to go).
  Object.assign(area.style, {
    position: "fixed", left: "50%", top: "50%", width: "2px", height: "2px",
    opacity: "0", border: "0", padding: "0", resize: "none", zIndex: "-1",
  });
  area.addEventListener("input", send);
  area.addEventListener("select", send);
  area.addEventListener("keyup", send);
  area.addEventListener("mouseup", send);
  document.addEventListener("selectionchange", () => { if (document.activeElement === area) send(); });
  area.addEventListener("keydown", (e) => {
    // Keep the shortcut map away from the keys typed into the text.
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      bridge.send({ type: UI.KEY, key: "Escape" });
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey || e.code === "NumpadEnter")) {
      e.preventDefault();
      bridge.send({ type: UI.KEY, key: "Enter" });
    }
  });
  document.body.append(area);
  return area;
}

/** Wire the Type tool's messages. `onFonts` re-renders the option bar. */
export function initType(onFonts) {
  bridge.on(ENGINE.TEXT_EDIT, ({ open, text, selection }) => {
    if (!open) {
      if (area) { area.blur(); area.value = ""; }
      lastSent = "";
      return;
    }
    const el = ensureArea();
    el.value = text;
    const [start, end] = selection;
    el.setSelectionRange(fromBytes(text, start), fromBytes(text, end));
    lastSent = text + "\u0000" + selection.join(",");
    el.focus({ preventScroll: true });
  });
  bridge.on(ENGINE.FONTS, ({ families }) => {
    const bar = optionsFor("type");
    const font = bar && bar.find((f) => f.key === "Font");
    if (!font || !families.length) return;
    font.options = families.map((f) => f.name);
    if (!font.options.includes(font.value)) font.value = font.options.includes("Arial") ? "Arial" : font.options[0];
    onFonts();
  });
}
