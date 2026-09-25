// Fotox — avvio: costruisce il chrome (barra titolo, menu, opzioni, strumenti,
// area di lavoro, dock, barra di stato) e collega tutti i motori.

import { h, icon, clear } from "./el.js";
import { loadSprite } from "./icons.js";
import { state, on, emit, setTool, setColors } from "./state.js";
import { menus } from "./data/menus.js";
import { toolSlots, findTool } from "./data/tools.js";
import { initPopupEngine, openDropdown, openPopup, closeAll, isPopupOpen } from "./popup.js";
import { buildMenubar, setMenuAction, initMenuKeyboard } from "./menu.js";
import { renderOptionsBar, onOptionsChange, readOptions, currentBar } from "./optionsbar.js";
import { renderDock, focusPanel, togglePanel } from "./panels.js";
import { openDialog, isDialogOpen } from "./dialogs.js";
import { initWorkspace, zoomTo } from "./canvas.js";
import { initTooltips, toast, status } from "./tooltip.js";
import { runAction } from "./actions.js";
import { initShortcuts } from "./shortcuts.js";
import * as bridge from "./native/bridge.js";
import { UI, ENGINE } from "./native/protocol.js";
import { initNativePanels } from "./native/layers-panel.js";
import { initColor } from "./native/color.js";
import { initTools, sendColors } from "./native/tools.js";

const UI_VERSION = "0.1.0";

/* ----------------------------------------------------------------- logo */

function logoMark() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 32 32");
  svg.setAttribute("class", "logo-mark");
  svg.innerHTML = `
    <rect x="1" y="1" width="30" height="30" rx="8" fill="#2f6df6"/>
    <path d="M9 23V9h13v3.2h-9.3v3.1h8.2v3.2h-8.2V23z" fill="#ffffff"/>`;
  return svg;
}

/* ---------------------------------------------------------------- chrome */

function buildShell() {
  const app = document.getElementById("app");
  clear(app);

  const topbar = h("header", { class: "topbar" });
  const brand = h("div", { class: "brand", title: "Fotox" }, logoMark(), h("span", { class: "brand-name", text: "Fotox" }));
  const menubar = h("nav", { class: "menuarea", "aria-label": "Main menu" });
  const topRight = h("div", { class: "topbar-right" },
    h("button", {
      class: "iconbtn", type: "button", "data-tip": "Search menus and tools", "data-tip-key": "Ctrl+F",
      onclick: () => openDialog("search"),
    }, icon("i-zoom", "ic sm")),
    h("button", {
      class: "iconbtn", type: "button", "data-tip": "Generate with AI",
      onclick: () => openDialog("generate"),
    }, icon("i-zap", "ic sm")),
    h("button", {
      class: "accountbtn", type: "button", "data-tip": "Account",
      onclick: () => openDialog("sign-in"),
    }, icon("i-account", "ic sm"), h("span", { text: "Account" })),
  );
  topbar.append(brand, menubar, topRight);

  const optionsbar = h("div", { class: "optionsbar", id: "optionsbar" });
  const toolbar = h("div", { class: "toolbar", id: "toolbar" });
  const workspace = h("main", { class: "workspace", id: "workspace" });
  const dock = h("aside", { class: "dock", id: "dock" });
  const middle = h("div", { class: "middle" }, toolbar, workspace, dock);
  const statusbar = h("footer", { class: "statusbar", id: "statusbar" });

  app.append(topbar, optionsbar, middle, statusbar);
  return { app, menubar, toolbar, workspace, dock, statusbar, optionsbar };
}

/* -------------------------------------------------------------- strumenti */

const slotCurrent = new Map(); // slot id → strumento selezionato in quel gruppo

function buildToolbar(container) {
  clear(container);
  container.classList.add("toolbar");
  for (const slot of toolSlots) {
    const btn = h("button", {
      class: "toolbtn", type: "button", "data-slot": slot.id,
      "data-tip": slot.name, dataset: { tip: slot.name, tipKey: slot.key },
    }, icon(slot.icon, "ic"));
    if (slot.flyout.length) btn.append(h("span", { class: "flyout-dot" }));

    btn.addEventListener("click", () => pickTool(slot.id, slot.id));
    btn.addEventListener("contextmenu", (e) => { e.preventDefault(); openFlyout(slot, btn); });
    let pressTimer = null;
    btn.addEventListener("mousedown", () => {
      pressTimer = setTimeout(() => openFlyout(slot, btn), 420);
    });
    for (const evt of ["mouseup", "mouseleave"]) {
      btn.addEventListener(evt, () => { if (pressTimer) clearTimeout(pressTimer); pressTimer = null; });
    }
    container.append(btn);
  }

  // blocco colori in fondo alla colonna
  const swatches = h("div", { class: "toolswatches" });
  const fg = h("button", { class: "mini-swatch fg", type: "button", dataset: { tip: "Set foreground colour" }, onclick: () => openDialog("color-picker") });
  const bg = h("button", { class: "mini-swatch bg", type: "button", dataset: { tip: "Set background colour" }, onclick: () => openDialog("color-picker") });
  const swap = h("button", { class: "mini-btn swap", type: "button", dataset: { tip: "Swap foreground and background" }, onclick: () => { setColors(state.colors.bg, state.colors.fg); toast("Swapped foreground and background colours"); } }, icon("i-swap", "ic sm"));
  const reset = h("button", { class: "mini-btn reset", type: "button", dataset: { tip: "Default foreground and background colours" }, onclick: () => { setColors("#000000", "#ffffff"); toast("Default colours restored"); } }, icon("i-reset-bw", "ic sm"));
  swatches.append(h("div", { class: "swatch-pair" }, bg, fg), h("div", { class: "swatch-tools" }, swap, reset));
  container.append(swatches);

  const extras = h("div", { class: "toolextras" },
    h("button", { class: "toolbtn small", type: "button", dataset: { tip: "Edit in Quick Mask Mode", tipKey: "Q" }, onclick: () => pickTool("quick-mask", "quick-mask") }, icon("i-quick-mask", "ic sm")),
    h("button", { class: "toolbtn small", type: "button", dataset: { tip: "Cycle screen mode", tipKey: "F" }, onclick: () => runAction({ label: "Screen Mode", a: "screen:cycle" }) }, icon("i-screen-mode", "ic sm")),
  );
  container.append(extras);
  refreshSwatches();
}

// The engine keeps the active tool's option-bar values (M5-T01).
function sendToolOptions(options) {
  if (!bridge.isNative) return;
  // The bar on show: the active tool's, or Free Transform's (M6-T04).
  bridge.send({ type: UI.TOOL_OPTIONS, tool: currentBar() || state.tool, options });
}

function pickTool(toolId, slotId) {
  slotCurrent.set(slotId, toolId);
  setTool(toolId);
  const slot = toolSlots.find((s) => s.id === slotId);
  const btn = document.querySelector(`.toolbtn[data-slot="${slotId}"]`);
  if (btn && slot) {
    const tool = slot.flyout.find((f) => f.id === toolId) || slot;
    const existing = btn.querySelector(".ic");
    if (existing) existing.replaceWith(icon(tool.icon, "ic"));
    btn.dataset.tip = tool.name;
    btn.dataset.tipKey = tool.key;
  }
  refreshSwatches();
}

function openFlyout(slot, anchor) {
  if (!slot.flyout.length) return null;
  const items = [slot, ...slot.flyout];
  const content = h("div", { class: "flyout" });
  for (const tool of items) {
    content.append(h("button", {
      class: "flyout-item" + (state.tool === tool.id ? " sel" : ""), type: "button",
      dataset: { tip: `${tool.name} (${tool.key})` },
      onclick: () => { pickTool(tool.id, slot.id); closeAll(); },
    }, icon(tool.icon, "ic"), h("span", { class: "flyout-label", text: tool.name }), h("span", { class: "flyout-key", text: tool.key })));
  }
  return openPopup({ anchor, content, side: "right", className: "flyout-pop" });
}

/* ------------------------------------------------------------- barra stato */

function buildStatusbar(el) {
  clear(el);
  el.append(
    h("div", { class: "status-left" }, h("span", { class: "statusmsg", id: "statusmsg", text: "Ready" })),
    h("div", { class: "status-center" },
      h("button", {
        class: "status-zoom", id: "statuszoom", type: "button", dataset: { tip: "Zoom level" },
        onclick: (e) => openDropdown({
          anchor: e.currentTarget, value: "", width: 110,
          items: ["3200%", "1600%", "800%", "400%", "200%", "100%", "66.7%", "50%", "33.3%", "25%", "12.5%", "Fit on Screen", "Fill Screen", "Actual Pixels"],
          onPick: (v) => {
            // Actions, not direct calls: in the app the engine owns the view.
            if (v === "Fit on Screen") return runAction({ a: "zoom:fit", label: v });
            if (v === "Fill Screen") return zoomTo(200);
            if (v === "Actual Pixels") return runAction({ a: "zoom:100", label: v });
            zoomTo(parseFloat(v));
          },
        }),
      }, h("span", { class: "pf-value", text: state.zoom + "%" }), icon("i-chevron-down", "ic xs")),
    ),
    h("div", { class: "status-right" },
      h("span", { class: "status-item", id: "statusdocsize", text: `${state.doc.w} × ${state.doc.h} px` }),
      h("span", { class: "status-item", text: `${state.doc.mode}/${state.doc.bits}` }),
      h("span", { class: "status-item", id: "statusmem", text: "8.4 MB" }),
      h("button", { class: "status-btn", type: "button", dataset: { tip: "Interface is a mock: no pixels are written" }, onclick: () => toast("Fotox is a navigable interface mock — no file is written to disk") }, icon("i-info", "ic sm")),
    ),
  );
}

function refreshSwatches() {
  const fg = document.querySelector(".mini-swatch.fg");
  const bg = document.querySelector(".mini-swatch.bg");
  if (fg) fg.style.background = state.colors.fg;
  if (bg) bg.style.background = state.colors.bg;
}

/* ---------------------------------------------------------------- avvio */

async function boot() {
  // First, so `body.native` is set before any part of the chrome is built.
  bridge.init();
  await loadSprite();

  const shell = buildShell();
  buildMenubar(shell.menubar, menus);
  setMenuAction((item) => runAction(item));

  renderOptionsBar(shell.optionsbar, state.tool);
  onOptionsChange(shell.optionsbar, sendToolOptions);
  buildToolbar(shell.toolbar);
  renderDock(shell.dock);
  initWorkspace(shell.workspace);
  buildStatusbar(shell.statusbar);

  initPopupEngine();
  initTooltips();
  initMenuKeyboard();
  initShortcuts();

  // eventi -------------------------------------------------------------
  on("tool", (id) => {
    // Every tool change passes here (toolbar, flyouts, single-key shortcuts,
    // menu actions): tell the engine, which pans with the Hand tool and will
    // route viewport input to the active tool (docs/tasks/M5.md, M5-T01).
    if (bridge.isNative) bridge.send({ type: UI.ACTION, id: "tool:" + id });
    renderOptionsBar(shell.optionsbar, id);
    sendToolOptions(readOptions());
    document.querySelectorAll(".toolbtn[data-slot]").forEach((b) => {
      const slot = toolSlots.find((s) => s.id === b.dataset.slot);
      const on_ = slot && (slot.id === id || slot.flyout.some((f) => f.id === id));
      b.classList.toggle("active", !!on_);
    });
    const tool = findTool(id);
    status(`${tool.name} (${tool.key})`);
  });

  // A Free Transform box (M6-T04) swaps in its own option bar while it is up.
  bridge.on(ENGINE.TRANSFORM_BOX, ({ up }) => {
    renderOptionsBar(shell.optionsbar, up ? "_transform" : state.tool);
    sendToolOptions(readOptions());
  });

  on("colors", () => { refreshSwatches(); sendColors(); });
  on("mock", (msg) => toast(msg));
  // A control that is an action rather than a value (M6-T03: the crop tool's
  // ✓ and ✗) takes the same path as a menu item or a shortcut.
  on("action", (id) => runAction({ label: id, a: id }));
  on("ask-dialog", (id) => openDialog(id));
  on("panel:open", (id) => focusPanel(id));
  on("zoom:set", (z) => zoomTo(z));
  on("flag", (key) => {
    if (key === "panels") document.body.classList.toggle("no-panels", !state.flags.panels);
    if (key === "doctabs") document.body.classList.toggle("no-doctabs", !state.flags.doctabs);
  });

  // engine bridge ------------------------------------------------------
  if (bridge.isNative) {
    initNativePanels();
    initColor();
    initTools();
  }
  bridge.on(ENGINE.TOAST, (m) => toast(m.text));
  bridge.on(ENGINE.ERROR, (m) => toast(m.text, "error"));
  // Pointer input over the viewport belongs to the engine only while no
  // popup, menu or dialog is open (the shell routes it; M0-T06).
  let directInput = true;
  on("overlays", () => {
    const enabled = !isPopupOpen() && !isDialogOpen();
    if (enabled === directInput) return;
    directInput = enabled;
    bridge.send({ type: UI.DIRECT_INPUT, enabled });
  });
  bridge.send({ type: UI.HELLO, ui_version: UI_VERSION });

  emit("tool", state.tool);
  sendColors();
  status("Fotox 1.0 — interface mock · press Alt for the menus, F for screen modes");

  // Il bootstrap diagnostico di index.html resta in ascolto per 1,5 s: se il
  // chrome è davvero comparso, spegne il watchdog e non mostra nulla.
  if (window.__fotoxDiag) window.__fotoxDiag.ok();
}

boot();
