// Fotox — barra dei menu e tendine.
// Le voci arrivano da js/data/menus.js; le azioni vengono eseguite dal
// callback passato da main.js (js/actions.js).

import { h, icon } from "./el.js";
import { isChecked } from "./state.js";
import { IMPLEMENTED } from "./data/implemented.js";
import * as bridge from "./native/bridge.js";

// Menu honesty (M7-T09, D-061): in the app an action the engine or a native
// dialog does not implement is greyed out, never a dead end. The UI-local
// prefixes work everywhere.
const LOCAL = ["panel:", "panels:", "tool:", "toggle:", "screen:", "zoom:", "ws:", "par:", "debug:"];
// The milestone an unimplemented family is planned in (docs/tasks/COVERAGE.md).
const PLANNED_IN = { ai: "M13", smart: "M12", type: "M10", filter: "M8", adj: "M8", export: "M14", mode: "M9", sel: "M9", layer: "M12" };
function planned(item) {
  if (!bridge.isNative || item.sub || !item.a || item.dis) return false;
  if (IMPLEMENTED.has(item.a) || [...IMPLEMENTED].some((id) => id.endsWith("*") && item.a.startsWith(id.slice(0, -1)))) return false;
  return item.a.startsWith("dlg:") || !LOCAL.some((p) => item.a.startsWith(p));
}
import { openPopup, closeAll, closeFrom, topPopup, isPopupOpen } from "./popup.js";

let onAction = () => {};
let menubar = null; // { buttons: [{ btn, menu }], index }

export function setMenuAction(fn) {
  onAction = fn;
}

/* ------------------------------------------------------------- costruzione */

function makeRow(item, ctx) {
  const { popup, parent } = ctx;
  if (planned(item)) item = { ...item, dis: true, tip: `Planned for ${PLANNED_IN[item.a.split(":")[0]] || "a later milestone"}` };
  const checked = item.chk ? isChecked(item.chk) : false;
  const row = h("div", {
    class: "mi" + (item.dis ? " off" : "") + (item.sub ? " has-sub" : ""),
    role: "menuitem",
    "aria-disabled": item.dis ? "true" : "false",
    "aria-haspopup": item.sub ? "menu" : null,
    "data-label": item.label,
    "data-tip": item.tip || null,
  },
    h("span", { class: "mi-check" }, checked ? icon("i-check", "ic xs") : null),
    h("span", { class: "mi-label", text: item.label }),
    item.sub
      ? h("span", { class: "mi-arrow" }, icon("i-chevron-right", "ic xs"))
      : (item.short ? h("span", { class: "mi-short", text: item.short }) : null),
  );

  if (!item.dis) {
    row.addEventListener("mouseenter", () => {
      if (deeperThan(popup)) closeFrom(popup);
      highlight(row);
      if (item.sub) openMenuPopup(row, item.sub, { parent: popup });
    });
    row.addEventListener("click", (e) => {
      e.stopPropagation();
      if (item.sub) {
        openMenuPopup(row, item.sub, { parent: popup });
        return;
      }
      closeAll();
      onAction(item, { parent });
    });
  }
  return row;
}

function deeperThan(popup) {
  const top = topPopup();
  return top && top !== popup && top.contains(popup) === false;
}

function fillMenu(container, items, ctx) {
  for (const item of items) {
    if (item.sep) container.append(h("hr"));
    else container.append(makeRow(item, ctx));
  }
  return container;
}

export function openMenuPopup(anchor, items, { parent = null, align = "left", className = "menu-pop" } = {}) {
  const content = h("div", { class: "menu", role: "menu" });
  const popup = openPopup({ anchor, content, parent, align, className, side: parent ? "submenu" : "below" });
  fillMenu(content, items, { popup, parent });
  highlight(firstRow(content));
  return popup;
}

function rowsOf(menu) {
  return [...menu.querySelectorAll(".mi")].filter((r) => !r.classList.contains("off"));
}

function firstRow(menu) {
  return rowsOf(menu)[0] || null;
}

function highlight(row) {
  if (!row) return;
  const menu = row.closest(".menu");
  if (menu) rowsOf(menu).forEach((r) => r.classList.toggle("hl", r === row));
}

function highlighted(menu) {
  return menu.querySelector(".mi.hl:not(.off)");
}

/* ---------------------------------------------------------------- menubar */

export function buildMenubar(container, menus) {
  container.classList.add("menubar");
  const buttons = [];

  menus.forEach((menu) => {
    const btn = h("button", { class: "menubtn", type: "button", text: menu.label, "data-menu": menu.id, "aria-haspopup": "true" });
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const open = topPopup();
      if (open && open._fotoxAnchor === btn) closeAll();
      else openTop(buttonIndex(btn), true);
    });
    btn.addEventListener("mouseenter", () => {
      const idx = buttonIndex(btn);
      if (menubar && menubar.index !== -1 && idx !== menubar.index) openTop(idx, false);
    });
    container.append(btn);
    buttons.push({ btn, menu });
  });

  menubar = { buttons, index: -1 };
  return menubar;
}

function buttonIndex(btn) {
  return menubar ? menubar.buttons.findIndex((b) => b.btn === btn) : -1;
}

function openTop(index, focus) {
  if (!menubar || index < 0 || index >= menubar.buttons.length) return;
  const { btn, menu } = menubar.buttons[index];
  menubar.index = index;
  menubar.buttons.forEach((b, i) => b.btn.classList.toggle("active", i === index));
  const popup = openMenuPopup(btn, menu.items, {});
  if (focus) {
    const first = firstRow(popup.firstElementChild);
    if (first) highlight(first);
  }
  return popup;
}

export function closeMenubar() {
  if (menubar) {
    menubar.index = -1;
    menubar.buttons.forEach((b) => b.btn.classList.remove("active"));
  }
}

/** Menù contestuale al click destro. */
export function openContextMenu(event, items) {
  event.preventDefault();
  closeAll();
  const rect = { left: event.clientX, top: event.clientY, bottom: event.clientY, right: event.clientX, width: 0, height: 0 };
  openMenuPopup(rect, items, { className: "menu-pop context" });
}

export function contextMenuFromEvent(node, items) {
  node.addEventListener("contextmenu", (e) => openContextMenu(e, items));
}

/* --------------------------------------------------------------- tastiera */

export function initMenuKeyboard() {
  document.addEventListener("keydown", (e) => {
    if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
      const key = e.key.toLowerCase();
      const map = { f: "file", e: "edit", i: "image", l: "layer", s: "select", t: "filter", v: "view", w: "window", o: "other" };
      const id = map[key];
      if (id && menubar) {
        const idx = menubar.buttons.findIndex((b) => b.menu.id === id);
        if (idx >= 0) {
          e.preventDefault();
          openTop(idx, true);
          return;
        }
      }
    }

    if (!isPopupOpen()) return;
    const popup = topPopup();
    const menu = popup.querySelector(".menu");
    if (!menu || !popup.classList.contains("menu-pop")) return;

    const rows = rowsOf(menu);
    const cur = highlighted(menu);
    let i = rows.indexOf(cur);

    switch (e.key) {
      case "ArrowDown":
        i = i < 0 ? 0 : (i + 1) % rows.length;
        highlight(rows[i]); e.preventDefault(); break;
      case "ArrowUp":
        i = i < 0 ? rows.length - 1 : (i - 1 + rows.length) % rows.length;
        highlight(rows[i]); e.preventDefault(); break;
      case "Home":
        highlight(rows[0]); e.preventDefault(); break;
      case "End":
        highlight(rows[rows.length - 1]); e.preventDefault(); break;
      case "Enter":
      case " ":
        if (cur) { cur.click(); e.preventDefault(); }
        break;
      case "ArrowRight": {
        if (cur && cur.classList.contains("has-sub")) {
          openSubmenuFromRow(cur, popup);
          e.preventDefault();
        } else if (popup.classList.contains("menu-pop") && !popup._fotoxParent && menubar) {
          closeAll(); closeMenubar();
          openTop((menubar.index + 1) % menubar.buttons.length, true);
          e.preventDefault();
        }
        break;
      }
      case "ArrowLeft": {
        const parent = popup._fotoxParent;
        if (parent) {
          const anchorRow = parent.querySelector("hr + .mi.hl, .mi.hl");
          closeFrom(parent);
          if (anchorRow) highlight(anchorRow);
        } else if (menubar) {
          closeAll(); closeMenubar();
          openTop((menubar.index - 1 + menubar.buttons.length) % menubar.buttons.length, true);
        }
        e.preventDefault();
        break;
      }
      case "Escape":
        closeAll(); closeMenubar(); e.preventDefault(); break;
      default:
        break;
    }
  });

  document.addEventListener("click", (e) => {
    if (!isPopupOpen() && !(e.target.closest && e.target.closest(".menubtn"))) closeMenubar();
  });
}

function openSubmenuFromRow(row, parentPopup) {
  row.dispatchEvent(new MouseEvent("mouseenter"));
  const top = topPopup();
  if (top && top !== parentPopup) {
    const first = firstRow(top.querySelector(".menu"));
    if (first) highlight(first);
  }
}

