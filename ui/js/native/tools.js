// Fotox — tool ↔ engine glue (M5-T01).
//
// Colour transport ([16-bit RGBA] ↔ the UI's CSS hex) and the eyedropper's
// answer. Tool options themselves go through `optionsbar.js` + `main.js`.

import { setColors, state } from "../state.js";
import * as bridge from "./bridge.js";
import { ENGINE, UI } from "./protocol.js";

/** `#rgb` / `#rrggbb` → 16-bit straight RGBA (white for an invalid value). */
export function hexToRgba16(hex) {
  const match = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(String(hex).trim());
  if (!match) return [0, 0, 0, 65535];
  let digits = match[1];
  if (digits.length === 3) digits = digits[0] + digits[0] + digits[1] + digits[1] + digits[2] + digits[2];
  const value = parseInt(digits, 16);
  return [((value >> 16) & 255) * 257, ((value >> 8) & 255) * 257, (value & 255) * 257, 65535];
}

/** 16-bit straight RGBA → `#rrggbb` (alpha is dropped). */
export function rgba16ToHex(rgba) {
  const channels = rgba.slice(0, 3).map((v) => Math.round(v / 257).toString(16).padStart(2, "0"));
  return `#${channels.join("")}`;
}

/** Tell the engine the current foreground/background colours (M5-T01). */
export function sendColors() {
  if (!bridge.isNative) return;
  bridge.send({
    type: UI.SET_COLORS,
    fg: hexToRgba16(state.colors.fg),
    bg: hexToRgba16(state.colors.bg),
  });
}

/** Wire the engine answers the tools produce. */
export function initTools() {
  bridge.on(ENGINE.COLOR_PICKED, (m) => {
    const hex = rgba16ToHex(m.rgba);
    // `setColors` only writes the swatches it is given.
    if (m.target === "bg") setColors(undefined, hex);
    else setColors(hex, undefined);
  });
}
