// Scratch probe (not a gate): the Layer Style window rebuilt after Photoshop
// (2026-10-01) driven through the real app — its layout, several instances
// of one effect, the new options, Cancel, the Layers panel's effect rows,
// Create Layers, Global Light and Scale Effects.
//
//   $env:GRAPHITE_BROWSER_DEBUG_PORT = 9222; cargo xtask run
//   node ui/tools/probe-layer-style.mjs

import { connect, sleep } from "./cdp.mjs";

const c = await connect();
await c.reload();
await sleep(1500);
await c.install();
const doc = () => c.eval("window.__fx.last.active_document.doc");
const layers = () => c.eval("window.__fx.last.layers.layers");
const styled = async () => (await layers()).find((l) => l.name === "Shape")?.styles;
const out = {};
const check = (name, ok, detail) => { out[name] = ok ? "ok" : `FAIL ${JSON.stringify(detail)}`; console.log(name, out[name]); };
async function command(cmd) {
  const s = await c.seq();
  await c.send({ type: "command", doc: await doc(), command: cmd });
  return c.waitFor("layers", "true", { after: s });
}
const ui = (js) => c.eval(`(() => { ${js} })()`);
const style = (js) => c.eval(`import('./js/native/styles.js').then((m) => { ${js} })`);

{
  const s = await c.seq();
  await c.send({ type: "action", id: "doc:new", args: { width: 800, height: 600, ppi: 72, depth: 8, background: "white" } });
  await c.waitFor("layers", "m.layers.length === 1", { after: s });
  await sleep(400);
}
await command({ op: "add_layer", name: "Shape", layer: { shape: { shape: { kind: "rect", w: 300, h: 200, radii: [0, 0, 0, 0] }, fill: { kind: "solid", rgba: [0, 0, 65535, 65535] }, stroke: null, transform: [1, 0, 0, 1, 250, 200] } } });
await sleep(500);

// The window: sidebar, page, OK / Cancel / New Style / Preview.
await style("m.openStyleDialog('style-drop-shadow')");
await sleep(700);
out.sidebar = await ui("return [...document.querySelectorAll('.ls-item .ls-item-name')].map((e) => e.textContent)");
out.buttons = await ui("return [...document.querySelectorAll('.ls-side .btn')].map((e) => e.textContent)");
out.page = await ui("return document.querySelector('.ls-title')?.textContent");
out.pageRows = await ui("return [...document.querySelectorAll('.ls-pane .ls-label')].map((e) => e.textContent)");
check("dropShadowOn", (await styled())?.drop_shadow?.length === 1, await styled());

// + adds a second Drop Shadow; its page shows "(1)".
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.includes('Drop Shadow')).querySelector('.ls-plus').click()");
await sleep(600);
check("twoShadows", (await styled())?.drop_shadow?.length === 2, (await styled())?.drop_shadow?.length);
out.pageAfterPlus = await ui("return document.querySelector('.ls-title')?.textContent");

// A page switch keeps the one window (no new dialog).
const before = await ui("return document.querySelectorAll('.dialog').length");
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.trim() === 'Stroke').click()");
await sleep(500);
const after = await ui("return document.querySelectorAll('.dialog').length");
check("oneWindow", before === after && before === 1, { before, after });
out.strokeRows = await ui("return [...document.querySelectorAll('.ls-pane .ls-label')].map((e) => e.textContent)");
check("strokeOn", (await styled())?.stroke?.length === 1, (await styled())?.stroke);

// Outer Glow: Precise, Range, Jitter reach the style.
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.trim() === 'Outer Glow').click()");
await sleep(400);
await ui("[...document.querySelectorAll('.ls-pane .btn.tiny')].find((b) => b.textContent === 'Precise').click()");
await sleep(400);
check("glowPrecise", (await styled())?.outer_glow?.[0]?.technique === "precise", (await styled())?.outer_glow);

// Blending Options: Channels and Layer Mask Hides Effects.
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.trim() === 'Blending Options').click()");
await sleep(400);
await ui("[...document.querySelectorAll('.ls-pane .dlg-checkline')].find((l) => l.textContent === 'R').click()");
await sleep(400);
check("channels", JSON.stringify((await styled())?.channels) === "[false,true,true]", (await styled())?.channels);
await ui("[...document.querySelectorAll('.ls-pane .dlg-checkline')].find((l) => l.textContent === 'R').click()");
await sleep(300);

// The Contour Editor opens over the window.
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.trim() === 'Satin').click()");
await sleep(400);
await ui("document.querySelector('.ls-pane .ls-contour').click()");
await sleep(400);
check("contourEditor", await ui("return !!document.querySelector('.ls-contour-editor')"), null);
await ui("const cv = document.querySelector('.ls-contour-editor'); const r = cv.getBoundingClientRect(); for (const t of ['mousedown']) cv.dispatchEvent(new MouseEvent(t, { clientX: r.left + r.width / 2, clientY: r.top + 10, bubbles: true })); window.dispatchEvent(new MouseEvent('mouseup'));");
await sleep(200);
await ui("[...document.querySelectorAll('.dialog')].at(-1).querySelector('.btn.primary').click()");
await sleep(500);
check("customContour", typeof (await styled())?.satin?.[0]?.contour === "object", (await styled())?.satin?.[0]?.contour);

// Cancel puts everything back (no style at all).
await ui("[...document.querySelectorAll('.ls-side .btn')].find((b) => b.textContent === 'Cancel').click()");
await sleep(700);
check("cancelReverts", !(await styled()), await styled());

// Open again, a Stroke and a Color Overlay, OK.
await style("m.openStyleDialog('style-stroke')");
await sleep(500);
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.trim() === 'Color Overlay').click()");
await sleep(400);
await ui("[...document.querySelectorAll('.ls-side .btn')].find((b) => b.textContent === 'OK').click()");
await sleep(800);
const s1 = await styled();
check("okKeeps", s1?.stroke?.length === 1 && s1?.color_overlay?.length === 1, s1);
out.pixelsStyled = await c.pixels([[400, 300], [248, 300], [240, 300]]);

// The Layers panel lists the effects with eyes.
out.fxRows = await ui("return [...document.querySelectorAll('.fx-row .fx-name')].map((e) => e.textContent)");
await ui("[...document.querySelectorAll('.fx-row')].find((r) => r.textContent.includes('Color Overlay')).querySelector('.peye').click()");
await sleep(600);
check("fxEye", (await styled())?.color_overlay?.[0]?.enabled === false, (await styled())?.color_overlay);

// Hide All Effects / Show All Effects.
await c.send({ type: "action", id: "layer:hide-effects", args: {} });
await sleep(600);
check("hideAll", (await styled())?.effects_visible === false, await styled());
await c.send({ type: "action", id: "layer:show-effects", args: {} });
await sleep(600);

// Scale Effects 200 %: the stroke doubles.
await c.send({ type: "command", doc: await doc(), command: { op: "scale_effects", layers: [{ id: (await layers()).find((l) => l.name === "Shape").id }], percent: 200 } });
await sleep(800);
check("scaleEffects", (await styled())?.stroke?.[0]?.size === 6, (await styled())?.stroke);

// Global Light with altitude.
await c.send({ type: "command", doc: await doc(), command: { op: "set_global_light", angle: 45, altitude: 60 } });
await sleep(800);
const info = await c.eval("window.__fx.last.document_changed?.info");
check("globalLight", info?.global_light === 45 && info?.global_altitude === 60, info);

// Create Layers: the stroke becomes a layer, the style is gone.
const n0 = (await layers()).length;
await c.send({ type: "action", id: "layer:create-effect-layers", args: {} });
await sleep(1500);
const after2 = await layers();
check("createLayers", after2.length === n0 + 1 && !after2.find((l) => l.name === "Shape")?.styles, after2.map((l) => [l.name, !!l.styles]));
out.pixelsAfterCreate = await c.pixels([[400, 300], [248, 300], [240, 300]]);

console.log(JSON.stringify(out, null, 2));
c.close();
