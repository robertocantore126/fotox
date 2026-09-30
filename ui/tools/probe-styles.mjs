// Scratch probe (not a gate): Gradient / Pattern Overlay through the real
// Layer Style dialogs, the brush library, AI status, Object Selection.
import { connect, sleep } from "./cdp.mjs";

const c = await connect();
await c.install();
const doc = () => c.eval("window.__fx.last.active_document.doc");
const layers = () => c.eval("window.__fx.last.layers.layers");
async function command(cmd, expect = "true") {
  const s = await c.seq();
  await c.send({ type: "command", doc: await doc(), command: cmd });
  return c.waitFor("layers", expect, { after: s });
}
const only = process.argv[2] || "";
const out = {};
{
  const s = await c.seq();
  await c.send({ type: "action", id: "doc:new", args: { width: 800, height: 600, ppi: 72, depth: 8, background: "white" } });
  await c.waitFor("layers", "m.layers.length === 1", { after: s });
  await sleep(400);
}
await command({ op: "add_layer", layer: "pixel", name: null });
await command({ op: "add_layer", name: null, layer: { shape: { shape: { kind: "rect", w: 300, h: 200, radii: [0, 0, 0, 0] }, fill: { kind: "solid", rgba: [0, 0, 65535, 65535] }, stroke: null, transform: [1, 0, 0, 1, 100, 100] } } });
await sleep(500);
const column = [[250, 104], [250, 150], [250, 200], [250, 250], [250, 296]];

if (!only || only === "styles") {
  // Gradient Overlay the way the menu opens it; the page is live.
  await c.eval(`import('./js/native/styles.js').then((m) => m.openStyleDialog('style-gradient-overlay'))`);
  await sleep(1200);
  out.gradientLive = await c.pixels(column);
  await c.eval(`[...document.querySelectorAll('.dialog .btn.primary')].at(-1).click()`);
  await sleep(800);
  out.gradientStyle = (await layers()).find((l) => l.styles?.gradient_overlay)?.styles.gradient_overlay.align;
  // Pattern Overlay.
  await c.eval(`import('./js/native/styles.js').then((m) => m.openStyleDialog('style-pattern-overlay'))`);
  await sleep(1200);
  out.patternLive = await c.pixels([[110, 110], [130, 120], [150, 140], [200, 180], [300, 250]]);
  await c.eval(`[...document.querySelectorAll('.dialog .btn.primary')].at(-1).click()`);
  await sleep(600);
  // Drop Shadow page: the sliders and the colour popover are there.
  await c.eval(`import('./js/native/styles.js').then((m) => m.openStyleDialog('style-drop-shadow'))`);
  await sleep(600);
  out.dropShadowSliders = await c.eval("[...document.querySelectorAll('.dialog .dlg-line')].filter((l) => l.querySelector('.dlg-range')).map((l) => l.querySelector('.dlg-field-label')?.textContent)");
  await c.eval("document.querySelector('.dialog .dlg-color').click()");
  await sleep(300);
  out.colorPopoverSliders = await c.eval("document.querySelectorAll('.cp-popover .cp-slider').length");
  // Drag the R slider of the popover to 255: the shadow turns red.
  await c.eval("(() => { const r = document.querySelector('.cp-popover .cp-slider'); r.value = 255; r.dispatchEvent(new Event('input', { bubbles: true })); })()");
  await sleep(800);
  out.shadowColour = (await layers()).find((l) => l.styles?.drop_shadow)?.styles.drop_shadow.color;
  await c.eval(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))`);
  await c.eval(`[...document.querySelectorAll('.dialog .btn.primary')].at(-1)?.click()`);
  await sleep(300);
}

if (!only || only === "brushes") {
  const s = await c.seq();
  await c.send({ type: "action", id: "brush:list", args: {} });
  const m = await c.waitFor("brushes", "true", { after: s }).catch(() => null);
  out.brushGroups = m ? [...new Set(m.presets.map((p) => p.group))] : "no brushes message";
  out.brushCount = m ? m.presets.length : 0;
  out.tipThumb = m ? m.presets.every((p) => typeof p.tip_thumb === "string" && p.tip_thumb.length > 100) : false;
}

if (!only || only === "ai") {
  const s = await c.seq();
  await c.send({ type: "action", id: "ai:status", args: {} });
  await sleep(1500);
  out.aiKeys = await c.eval("Object.keys(window.__fx.last)");
  out.ai = await c.eval("JSON.stringify(Object.values(window.__fx.last).find((m) => m && m._ai)?._ai || null)");
}

console.log(JSON.stringify(out, null, 1));
c.close();
