// Ctrl + right-click identifies UI code without activating the clicked control.
import { sourceOf } from "./inspector-source.js";
import { emit } from "./state.js";

let host = null;
let previousFocus = null;
let selection = null;
let inspectGesture = false;
export function isInspectorOpen() { return host !== null; }

function selector(node) {
  const parts = [];
  for (let el = node; el && parts.length < 6; el = el.parentElement) {
    if (el.id) { parts.unshift(`#${CSS.escape(el.id)}`); break; }
    let part = el.localName + [...el.classList].slice(0, 3).map((c) => `.${CSS.escape(c)}`).join("");
    const siblings = el.parentElement ? [...el.parentElement.children].filter((x) => x.localName === el.localName) : [];
    if (siblings.length > 1) part += `:nth-of-type(${siblings.indexOf(el) + 1})`;
    parts.unshift(part);
  }
  return parts.join(" > ");
}

function cssReferences(node) {
  const results = new Set();
  function visit(rules, file) {
    for (const rule of rules) {
      if (results.size >= 12) return;
      if (rule.selectorText) {
        try { if (node.matches(rule.selectorText)) results.add(`${file}: ${rule.selectorText}`); } catch (_) {}
      } else if (rule.cssRules) visit(rule.cssRules, file);
    }
  }
  for (const sheet of document.styleSheets) {
    if (!sheet.href) continue;
    try { visit(sheet.cssRules, "ui/" + sheet.href.split("/ui/").pop().split("/").slice(-2).join("/")); } catch (_) {}
  }
  return [...results];
}

function describe(clicked, target) {
  let owner = target;
  while (owner && !sourceOf(owner)?.frames.length) owner = owner.parentElement;
  const origin = owner ? sourceOf(owner) : null;
  const label = target.getAttribute("aria-label") || target.dataset.tip || target.dataset.label || target.title || target.textContent.trim().replace(/\s+/g, " ").slice(0, 100) || target.id || target.localName;
  const lines = ["Fotox UI reference", `Element: ${label}`, `Selector: ${selector(target)}`];
  if (clicked !== target) lines.push(`Clicked child: ${selector(clicked)}`);
  if (origin) {
    lines.push(`${owner === target ? "Creation source" : "Nearest UI ancestor source"}:`, ...origin.frames);
    if (origin.handlers.length) lines.push("Declared handlers:", ...origin.handlers);
  } else lines.push("Creation source: unavailable (source capture disabled or element created outside the DOM helper).");
  for (let el = target, count = 0; el && count < 5; el = el.parentElement, count++) {
    const context = Object.entries(el.dataset).filter(([key]) => !/^tip/.test(key)).map(([key, value]) => `${key}=${value}`);
    if (context.length) lines.push(`Context (${el.localName}): ${context.join(", ")}`);
  }
  const css = cssReferences(target);
  if (css.length) lines.push("Matching style rules:", ...css);
  if (target.id === "viewport" || target.closest("#viewport")) lines.push("Native viewport: this identifies the canvas UI surface, not a document pixel or layer. Engine input: crates/fx-app/src/input.rs; rendering: crates/fx-render/.");
  lines.push("", "Requested improvement: [describe what you want changed]", "Locate the element using the source, selector and context above. Source locations describe UI creation; follow its handlers/actions for behavior changes.");
  return { label, text: lines.join("\n") };
}

function close() {
  if (!host) return;
  host.remove(); host = null; selection = null; inspectGesture = false;
  emit("overlays");
  if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
}

function open(clicked) {
  const target = clicked.closest("button, input, select, textarea, [role=menuitem], [data-slot], [data-item]") || clicked;
  const reference = describe(clicked, target);
  if (!host) previousFocus = document.activeElement;
  else host.remove();
  host = document.createElement("div");
  host.id = "fotox-inspector";
  host.style.cssText = "position:fixed;inset:0;z-index:2147483647;pointer-events:none";
  const root = host.attachShadow({ mode: "open" });
  root.innerHTML = `<style>
    :host { color-scheme:dark; font:13px/1.5 'Segoe UI',sans-serif; }
    * { box-sizing:border-box; }
    .outline { position:fixed; border:2px solid #58a6ff; background:#58a6ff18; pointer-events:none; }
    .card { pointer-events:auto; position:absolute; right:16px; top:16px; width:min(540px,calc(100vw - 32px)); max-height:calc(100vh - 32px); overflow:auto; background:#252525; color:#eee; border:1px solid #666; border-radius:10px; box-shadow:0 8px 40px #0009; padding:18px; }
    h2 { margin:0 0 6px; font-size:18px; } p { margin:6px 0 12px; } .name { color:#8cc4ff; overflow-wrap:anywhere; }
    textarea { width:100%; height:260px; max-height:42vh; resize:vertical; background:#171717; border:1px solid #666; color:#eee; padding:10px; font:12px/1.5 Consolas,monospace; }
    button { cursor:pointer; border:1px solid #777; border-radius:5px; padding:7px 12px; background:#444; color:white; font:inherit; } button:focus-visible, textarea:focus-visible { outline:2px solid #8cc4ff; }
    .primary { background:#2169c5; } .row { display:flex; gap:8px; margin-top:12px; flex-wrap:wrap; } .status { min-height:20px; color:#b6d9ff; }
  </style><div class="outline"></div><section class="card" role="dialog" aria-modal="true" aria-label="Fotox element inspector"><h2>Element inspector</h2><p class="name"></p><p>Copy this reference, then tell your AI what to improve.</p><textarea aria-label="Copyable element reference" readonly spellcheck="false"></textarea><div class="row"><button class="primary">Copy for AI</button><button class="select">Select text</button><button class="close">Close (Esc)</button></div><p class="status" role="status"></p><p>Ctrl + right-click another element to inspect it.</p></section>`;
  for (const type of ["pointerdown", "pointerup", "mousedown", "mouseup", "click", "dblclick", "auxclick", "contextmenu", "wheel"]) {
    root.addEventListener(type, (event) => event.stopPropagation());
  }
  const area = root.querySelector("textarea");
  area.value = reference.text;
  root.querySelector(".name").textContent = reference.label;
  selection = { target, outline: root.querySelector(".outline") };
  document.body.append(host);
  const updateOutline = () => {
    if (!selection) return;
    const box = selection.target.getBoundingClientRect();
    Object.assign(selection.outline.style, { left: box.left + "px", top: box.top + "px", width: box.width + "px", height: box.height + "px" });
  };
  updateOutline();
  const select = () => { area.focus(); area.select(); };
  root.querySelector(".select").onclick = select;
  root.querySelector(".close").onclick = close;
  root.querySelector(".primary").onclick = async () => {
    let copied = false;
    // CEF/file origins may lack Clipboard API permission. execCommand remains
    // a user-gesture fallback, followed by explicit selection for manual copy.
    select();
    try { copied = document.execCommand("copy"); } catch (_) {}
    if (!copied) {
      try { await navigator.clipboard.writeText(reference.text); copied = true; } catch (_) {}
    }
    if (!host || host.shadowRoot !== root) return;
    root.querySelector(".status").textContent = copied ? "Copied. Paste into your AI chat." : "Text selected. Press Ctrl+C to copy.";
  };
  root.querySelector(".primary").focus();
  emit("overlays");
}

export function initInspector() {
  // Window capture runs before popup dismissal, tool gestures and document shortcuts.
  const inside = (event) => host && event.composedPath().includes(host);
  const stop = (event) => { event.preventDefault(); event.stopImmediatePropagation(); };
  window.addEventListener("pointerdown", (event) => {
    if (inside(event)) return;
    if (event.ctrlKey && event.button === 2) { inspectGesture = true; stop(event); open(event.target); }
    else if (host) stop(event);
  }, true);
  for (const type of ["mousedown", "mouseup", "pointerup", "click", "dblclick", "auxclick", "wheel"]) {
    window.addEventListener(type, (event) => {
      if (inside(event)) return;
      if ((inspectGesture && event.button === 2) || host) stop(event);
    }, { capture: true, passive: false });
  }
  window.addEventListener("contextmenu", (event) => {
    if (inside(event)) return;
    if (event.ctrlKey || inspectGesture) {
      stop(event);
      if (!inspectGesture) open(event.target);
      inspectGesture = false;
    } else if (host) stop(event);
  }, true);
  window.addEventListener("keydown", (event) => {
    if (!host) return;
    event.stopImmediatePropagation();
    if (event.key === "Escape") { event.preventDefault(); close(); }
    if (event.key === "Tab") {
      const root = host.shadowRoot;
      const items = [...root.querySelectorAll("textarea, button")];
      const index = items.indexOf(root.activeElement);
      event.preventDefault();
      items[(index + (event.shiftKey ? items.length - 1 : 1)) % items.length].focus();
    }
  }, true);
  window.addEventListener("keyup", (event) => { if (host) event.stopImmediatePropagation(); }, true);
  for (const type of ["pointermove", "mousemove", "mouseover", "mouseout", "mouseenter", "mouseleave"]) {
    window.addEventListener(type, (event) => { if (host && !inside(event)) event.stopImmediatePropagation(); }, true);
  }
  window.addEventListener("blur", () => { inspectGesture = false; });
  window.addEventListener("resize", close);
}
