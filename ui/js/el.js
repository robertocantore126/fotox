// Fotox — helper DOM minimale (nessuna dipendenza esterna).

import { rememberSource } from "./inspector-source.js";

export function h(tag, props = null, ...kids) {
  const el = document.createElement(tag);
  if (props) {
    for (const [k, v] of Object.entries(props)) {
      if (v === null || v === undefined || v === false) continue;
      if (k === "class" || k === "className") el.className = v;
      else if (k === "text") el.textContent = v;
      else if (k === "html") el.innerHTML = v;
      else if (k === "style" && typeof v === "object") Object.assign(el.style, v);
      else if (k === "dataset") Object.assign(el.dataset, v);
      else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v);
      else if (k === "value" || k === "checked" || k === "disabled" || k === "selected") el[k] = v;
      else el.setAttribute(k, v === true ? "" : v);
    }
  }
  rememberSource(el, h, props);
  add(el, kids);
  return el;
}

export function add(parent, kids) {
  for (const kid of kids.flat(4)) {
    if (kid === null || kid === undefined || kid === false) continue;
    parent.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  }
  return parent;
}

const SVG_NS = "http://www.w3.org/2000/svg";

/** Mappa delle icone, riempita da js/icons.js al momento dell'avvio. */
export const sprite = { map: null, defaults: null };

/** Icona dal set (assets/icons.svg), colorabile via currentColor. */
export function icon(id, cls = "ic", extra = {}) {
  const svg = document.createElementNS(SVG_NS, "svg");
  rememberSource(svg, icon);
  svg.setAttribute("class", cls);
  svg.setAttribute("viewBox", "0 0 20 20");
  svg.setAttribute("aria-hidden", "true");
  for (const [k, v] of Object.entries(extra)) svg.setAttribute(k, v);

  const markup = sprite.map ? sprite.map.get(id) : null;
  if (markup === null || markup === undefined) {
    if (sprite.map) console.warn("icona mancante:", id);
    return svg;
  }
  const g = document.createElementNS(SVG_NS, "g");
  for (const [k, v] of Object.entries(sprite.defaults || {})) g.setAttribute(k, v);
  g.innerHTML = markup;
  svg.append(g);
  return svg;
}

export function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
  return node;
}


/**
 * A left-button drag that starts on `el`: `onMove(event)` at the press and on
 * every move until the release, wherever the pointer goes; `onUp(event)` at
 * the release. Mouse events on the window, not pointer capture: the app's
 * off-screen CEF gets forwarded mouse events, and with capture the drag
 * stopped (or never followed) once the pointer left the element — the
 * colour pickers' marker could hardly be moved. `onDown(event)` may return
 * false to leave the press alone.
 */
export function dragOn(el, onMove, { onDown = null, onUp = null } = {}) {
  el.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    if (onDown && onDown(e) === false) return;
    // No text selection or native drag may take the gesture over.
    e.preventDefault();
    const move = (m) => {
      if (!(m.buttons & 1)) { up(m); return; } // the release happened elsewhere
      onMove(m);
    };
    const up = (u) => {
      window.removeEventListener("mousemove", move, true);
      window.removeEventListener("mouseup", up, true);
      onUp?.(u);
    };
    window.addEventListener("mousemove", move, true);
    window.addEventListener("mouseup", up, true);
    onMove(e);
  });
}
