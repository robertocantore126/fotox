// Fotox — tooltip delegati e feedback (messaggio di stato).

import { h } from "./el.js";
import { popupLayer } from "./popup.js";

let tip = null;
let timer = null;
let current = null;

function ensureTip() {
  if (!tip) {
    tip = h("div", { class: "tooltip", role: "tooltip" });
    popupLayer().append(tip);
  }
  return tip;
}

function show(target) {
  const text = target.dataset.tip;
  if (!text) return;
  const el = ensureTip();
  el.innerHTML = "";
  const key = target.dataset.tipKey;
  el.append(h("span", { class: "tip-text", text }));
  if (key) el.append(h("span", { class: "tip-key", text: key }));
  el.classList.add("show");
  const r = target.getBoundingClientRect();
  el.style.left = "0px";
  el.style.top = "0px";
  const w = el.offsetWidth;
  let x = r.right + 8;
  if (x + w > window.innerWidth - 6) x = Math.max(6, r.left - w - 8);
  let y = r.top + r.height / 2 - el.offsetHeight / 2;
  y = Math.max(6, Math.min(window.innerHeight - el.offsetHeight - 6, y));
  el.style.left = Math.round(x) + "px";
  el.style.top = Math.round(y) + "px";
}

export function hideTooltip() {
  current = null;
  if (timer) clearTimeout(timer);
  timer = null;
  if (tip) tip.classList.remove("show");
}

export function initTooltips() {
  document.addEventListener("mouseover", (e) => {
    const target = e.target.closest && e.target.closest("[data-tip]");
    if (target === current) return;
    hideTooltip();
    if (!target) return;
    current = target;
    timer = setTimeout(() => show(target), 420);
  });
  document.addEventListener("mousedown", hideTooltip, true);
  document.addEventListener("scroll", hideTooltip, true);
  window.addEventListener("blur", hideTooltip);
}

/** Messaggio temporaneo in basso a destra (feedback delle azioni mock). */
export function toast(message, kind = "info") {
  const el = h("div", { class: "toast " + kind }, h("span", { class: "toast-msg", text: message }));
  popupLayer().append(el);
  requestAnimationFrame(() => el.classList.add("show"));
  setTimeout(() => {
    el.classList.remove("show");
    setTimeout(() => el.remove(), 300);
  }, 2400);
  return el;
}

/** Scrive un messaggio nella barra di stato. */
export function status(message) {
  const el = document.getElementById("statusmsg");
  if (!el) return;
  el.textContent = message;
  el.classList.add("flash");
  setTimeout(() => el.classList.remove("flash"), 900);
}
