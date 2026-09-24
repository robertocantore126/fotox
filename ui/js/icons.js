// Fotox — caricamento del set di icone.
//
// Lo sprite (assets/icons.svg) viene letto una volta sola, ogni simbolo viene
// convertito in markup e memorizzato: al momento di creare un'icona il markup
// viene inserito direttamente nell'svg. Così non dipendiamo né da <use> né da
// regole CSS che debbano attraversare l'albero ombra, e le icone restano
// colorabili con currentColor.

import { sprite } from "./el.js";

const STROKE_DEFAULTS = {
  fill: "none",
  stroke: "currentColor",
  "stroke-width": "1.35",
  "stroke-linecap": "round",
  "stroke-linejoin": "round",
};

export async function loadSprite(url = "assets/icons.svg") {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Sprite non caricato: ${res.status}`);
  const text = await res.text();
  const doc = new DOMParser().parseFromString(text, "image/svg+xml");

  const failed = doc.querySelector("parsererror");
  if (failed) throw new Error("Lo sprite SVG non è un XML valido");

  const map = new Map();
  const ser = new XMLSerializer();
  for (const sym of doc.querySelectorAll("symbol")) {
    const inner = [...sym.children];
    for (const el of inner) {
      if (el.nodeType !== 1) continue;
      // la classe .f segna le forme piene: diventa un attributo di presentazione
      const cls = el.getAttribute("class") || "";
      if (cls.split(/\s+/).includes("f")) {
        el.removeAttribute("class");
        el.setAttribute("fill", "currentColor");
        el.setAttribute("stroke", "none");
      }
    }
    map.set(sym.id, inner.map((el) => ser.serializeToString(el)).join(""));
  }

  sprite.map = map;
  sprite.defaults = STROKE_DEFAULTS;
  return map.size;
}

export function spriteSize() {
  return sprite.map ? sprite.map.size : 0;
}
