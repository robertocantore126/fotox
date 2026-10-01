// Fotox — motore dei popup (tendine, sottomenu, dropdown, menu contestuali).
// Gestisce una pila di livelli: ogni popup sa chi è il suo genitore, così i
// sottomenu si aprono accanto e la chiusura avviene dal più profondo.

import { h, icon } from "./el.js";
import { emit } from "./state.js";

/** @type {{el:HTMLElement, parent:HTMLElement|null}[]} */
const stack = [];
let layer = null;

export function popupLayer() {
  if (!layer) layer = document.getElementById("popuplayer");
  return layer;
}

export function isPopupOpen() {
  return stack.length > 0;
}

export function topPopup() {
  return stack.length ? stack[stack.length - 1].el : null;
}

/** true se il nodo vive dentro un popup aperto. */
export function insidePopup(node) {
  return stack.some((s) => s.el.contains(node) || s.el === node);
}

function place(el, anchorRect, { align = "left", side = "below", width = 0, parentRect = null } = {}) {
  const pad = 6;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  el.style.visibility = "hidden";
  el.style.left = "0px";
  el.style.top = "0px";
  el.style.maxHeight = "";
  // Taller than the window: it scrolls instead of running off the screen.
  if (el.offsetHeight > vh - 2 * pad) {
    el.style.maxHeight = vh - 2 * pad + "px";
    el.style.overflowY = "auto";
  }
  const w = width || el.offsetWidth;
  const hgt = el.offsetHeight;

  let x = anchorRect.left;
  let y = anchorRect.bottom;

  if (side === "right") {
    x = anchorRect.right + 4;
    y = anchorRect.top - 6;
    if (y + hgt > vh - pad) y = Math.max(pad, vh - hgt - pad);
  } else if (side === "submenu" && parentRect) {
    x = parentRect.right - 4;
    y = parentRect.top - 6;
    if (x + w > vw - pad) x = Math.max(pad, parentRect.left - w + 4);
    if (y + hgt > vh - pad) y = Math.max(pad, vh - hgt - pad);
  } else if (side === "above") {
    y = anchorRect.top - hgt;
  } else if (side === "cursor") {
    x = anchorRect.left;
    y = anchorRect.top;
    if (x + w > vw - pad) x = Math.max(pad, vw - w - pad);
    if (y + hgt > vh - pad) y = Math.max(pad, y - hgt);
  }

  if (align === "right") x = anchorRect.right - w;
  if (align === "center") x = anchorRect.left + (anchorRect.width - w) / 2;

  if (x + w > vw - pad) x = Math.max(pad, vw - w - pad);
  if (x < pad) x = pad;
  if (y + hgt > vh - pad) y = Math.max(pad, (side === "below" ? anchorRect.top - hgt : vh - hgt - pad));
  if (y < pad) y = pad;

  el.style.left = Math.round(x) + "px";
  el.style.top = Math.round(y) + "px";
  el.style.visibility = "";
  return el;
}

/**
 * Apre un popup.
 * @param {object} o
 * @param {HTMLElement|DOMRect} o.anchor  elemento o rettangolo a cui ancorarsi
 * @param {HTMLElement} o.content         contenuto del popup
 * @param {string} [o.className]          classi aggiuntive
 * @param {string} [o.align]              left | right | center
 * @param {string} [o.side]               below | above | submenu | cursor
 * @param {number} [o.width]
 * @param {HTMLElement|null} [o.parent]   popup genitore (per i sottomenu)
 * @param {Function} [o.onClose]
 */
export function openPopup(o) {
  const {
    anchor, content, className = "", align = "left", side = "below",
    width = 0, parent = null, onClose = null, keep = false,
  } = o;

  if (!keep) closeFrom(parent);

  const el = h("div", { class: `popup ${className}`.trim(), role: "dialog" }, content);
  el._fotoxAnchor = anchor;
  el._fotoxParent = parent;
  el._fotoxOnClose = onClose;

  popupLayer().append(el);
  const anchorRect = anchor instanceof Element ? anchor.getBoundingClientRect() : anchor;
  const parentRect = parent ? parent.getBoundingClientRect() : null;
  el._fotoxPlace = { align, side, width };
  place(el, anchorRect, { align, side, width, parentRect });

  const entry = { el, parent, onClose };
  if (parent) {
    const idx = stack.findIndex((s) => s.el === parent);
    stack.splice(idx + 1, stack.length - idx - 1, entry); // chiude i fratelli più profondi
  } else {
    stack.push(entry);
  }
  el.dataset.depth = String(stack.length - 1);
  emit("overlays");
  return el;
}

/**
 * Places `el` again against its anchor: for content added after
 * `openPopup` (a menu's rows), which the first placement measured empty.
 */
export function reposition(el) {
  const anchor = el._fotoxAnchor;
  const anchorRect = anchor instanceof Element ? anchor.getBoundingClientRect() : anchor;
  const parentRect = el._fotoxParent ? el._fotoxParent.getBoundingClientRect() : null;
  place(el, anchorRect, { ...el._fotoxPlace, parentRect });
}

/** Chiude i popup più profondi di `parent` (o tutti se parent è null). */
export function closeFrom(parent = null, keepLast = false) {
  const limit = parent ? stack.findIndex((s) => s.el === parent) + 1 : (keepLast ? stack.length : 0);
  let closed = false;
  while (stack.length > Math.max(0, limit)) {
    const entry = stack.pop();
    entry.el.remove();
    if (entry.onClose) entry.onClose();
    closed = true;
  }
  if (closed) emit("overlays");
  return closed;
}

export function closeAll() {
  return closeFrom(null, false);
}

/** Apre un elenco di scelte (usato da select, combo dei pannelli, menu ⋯). */
export function openDropdown({ anchor, items, value, onPick, className = "dropdown", align = "left", width = 0 }) {
  const content = h("div", { class: "optlist" });

  for (const item of items) {
    const isObj = typeof item === "object" && item !== null;
    const label = isObj ? item.label : item;
    const disabled = isObj && item.disabled;
    const selected = isObj ? item.value === value : label === value;
    const row = h("div", {
      class: "opt" + (selected ? " sel" : "") + (disabled ? " off" : ""),
      title: label,
      onclick: (e) => {
        e.stopPropagation();
        if (disabled) return;
        closeAll();
        if (onPick) onPick(isObj ? item.value : label);
      },
      onmouseenter: () => { if (!disabled) focusRow(content, row); },
    }, h("span", { class: "opt-label", text: label }), isObj && item.short ? h("span", { class: "opt-key", text: item.short }) : null);
    content.append(row);
  }
  return openPopup({ anchor, content, className: "dropdown " + className, align, width });
}

function focusRow(list, row) {
  [...list.querySelectorAll(".opt")].forEach((r) => r.classList.toggle("hl", r === row && !r.classList.contains("off")));
}

/* ------------------------------------------------------------ interazione */

let wired = false;

export function initPopupEngine() {
  if (wired) return;
  wired = true;

  // click fuori da qualunque popup: chiude tutto
  document.addEventListener("mousedown", (e) => {
    if (!isPopupOpen()) return;
    if (insidePopup(e.target)) return;
    // la barra dei menu gestisce da sola il click sui propri pulsanti, così il
    // click sulla stessa voce chiude invece di riaprire
    if (e.target.closest && e.target.closest(".menubar")) return;
    closeAll();
  }, true);

  document.addEventListener("contextmenu", (e) => {
    if (isPopupOpen() && insidePopup(e.target)) return;
  });

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && isPopupOpen()) {
      closeAll();
      e.preventDefault();
    }
  });

  window.addEventListener("resize", () => closeAll());
  window.addEventListener("blur", () => closeAll());
  // A scroll anywhere outside closes the popups (a menu must not stay glued to
  // a panel that scrolled away) — but a scroll *inside* a popup is the popup's
  // own scroller: a long drop-down list (the blend modes on the Layers panel)
  // scrolls on the wheel, and this listener used to close the whole popup on
  // the first notch, so its options below the fold could never be reached
  // (R2-12).
  document.addEventListener(
    "scroll",
    (e) => {
      if (e.target instanceof Node && insidePopup(e.target)) return;
      closeAll();
    },
    true,
  );
}

/**
 * A drop-down list as a button: the app's own dropdown over `[value, label]`
 * pairs. The UI runs in off-screen CEF, which never draws a native
 * <select>'s list, so the app uses this instead. `btn.value` reads and sets
 * the value, like a <select>'s.
 */
export function selectButton(pairs, value, onPick, { width = 110, className = "dlg-select", style = {} } = {}) {
  const labelOf = (v) => (pairs.find(([k]) => k === v) || pairs[0] || ["", ""])[1];
  let current = pairs.some(([k]) => k === value) ? value : pairs[0]?.[0];
  const val = h("span", { class: "pf-value", text: labelOf(current) });
  const btn = h("button", { class: className, type: "button", style: { minWidth: width + "px", ...style } }, val, icon("i-chevron-down", "ic xs"));
  Object.defineProperty(btn, "value", {
    get: () => current,
    set: (v) => { current = v; val.textContent = labelOf(v); },
  });
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    openDropdown({
      anchor: btn, items: pairs.map(([, l]) => l), value: labelOf(current), width: Math.max(130, btn.offsetWidth),
      onPick: (label) => {
        const next = (pairs.find(([, l]) => l === label) || pairs[0])[0];
        btn.value = next;
        onPick?.(next);
      },
    });
  });
  return btn;
}
