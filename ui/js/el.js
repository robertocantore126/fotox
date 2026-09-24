// Fotox — helper DOM minimale (nessuna dipendenza esterna).

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

