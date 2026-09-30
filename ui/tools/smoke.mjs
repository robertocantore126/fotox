// Fotox — smoke suite: real user flows against the running app.
//
// Each scenario drives the real UI (DevTools: clicks, drags, keys in the
// panels and dialogs), the real canvas (OS mouse input) and checks what the
// engine answers and what is really on screen (screen pixels). Unit tests
// cannot see the bugs this catches: a UI control that sends nothing, an
// HTML feature the embedded browser does not support, a frame that never
// shows the new content.
//
//   $env:GRAPHITE_BROWSER_DEBUG_PORT = 9222; cargo xtask run      (one shell)
//   node ui/tools/smoke.mjs [name-filter]                           (another)
//
// Keep the app window visible and maximised while it runs (screen pixels,
// real mouse). Exit code 1 when a scenario fails. Add a scenario for every
// bug a user finds by hand: it is how the bug stays found.

import { connect, sleep } from "./cdp.mjs";

// No page reload: reloading the embedded UI can leave it blank and cut the
// page → engine channel (2026-09-27). Start the app fresh instead.
const c = await connect();
await c.install();

// ---------------------------------------------------------------- helpers

const doc = () => c.eval("window.__fx.last.active_document.doc");
const layers = () => c.eval("window.__fx.last.layers.layers");

async function newDoc(w = 800, h = 600) {
  const s = await c.seq();
  await c.send({ type: "action", id: "doc:new", args: { width: w, height: h, ppi: 72, depth: 8, background: "white" } });
  await c.waitFor("layers", "m.layers.length === 1", { after: s });
  await c.waitFor("view", "true", { after: s });
  await sleep(300);
}

async function command(cmd, expect = "true") {
  const s = await c.seq();
  await c.send({ type: "command", doc: await doc(), command: cmd });
  return c.waitFor("layers", expect, { after: s });
}

const rect = (x, y, w, h, rgba) => ({
  op: "add_layer", name: null,
  layer: { shape: { shape: { kind: "rect", w, h, radii: [0, 0, 0, 0] }, fill: { kind: "solid", rgba }, stroke: null, transform: [1, 0, 0, 1, x, y] } },
});

/** The centre of the visible element matching `js` (an expression over `document`), scrolled into view. */
async function spot(js) {
  const p = await c.eval(`(() => { const e = ${js}; if (!e) return null; e.scrollIntoView({ block: "nearest" }); const r = e.getBoundingClientRect(); return r.width ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : null; })()`);
  if (!p) throw new Error(`not on screen: ${js}`);
  return p;
}
const byTip = (tip) => spot(`[...document.querySelectorAll('[data-tip]')].find((e) => e.dataset.tip === ${JSON.stringify(tip)} && e.getBoundingClientRect().width > 0)`);
const byText = (text) => spot(`[...document.querySelectorAll('*')].find((e) => e.children.length === 0 && e.textContent.trim() === ${JSON.stringify(text)} && e.getBoundingClientRect().width > 0)`);
const dialogButton = (text) => spot(`[...document.querySelectorAll('.dialog button')].find((b) => b.textContent.trim() === ${JSON.stringify(text)} && b.getBoundingClientRect().width > 0)`);
const dialogOpen = () => c.eval("[...document.querySelectorAll('.dialog')].some((d) => d.getBoundingClientRect().width > 0)");

/** Wait until the screen at document point `p` satisfies `ok([r, g, b])`. */
async function screenAt(p, ok, what, timeout = 6000) {
  const until = Date.now() + timeout;
  let last;
  for (;;) {
    [last] = await c.pixels([p]);
    if (ok(last)) return last;
    if (Date.now() > until) throw new Error(`${what}: the screen at ${p} is ${last}`);
    await sleep(250);
  }
}
const isBlue = ([r, g, b]) => b > 200 && r < 60 && g < 60;
const isRed = ([r, g, b]) => r > 200 && g < 60 && b < 60;
const isWhite = ([r, g, b]) => r > 240 && g > 240 && b > 240;
const isDark = ([r, g, b]) => r + g + b < 300;
const assert = (cond, message) => { if (!cond) throw new Error(message); };

// ---------------------------------------------------------------- scenarios

const scenarios = {
  async "layers reorder by dragging a row"() {
    await newDoc();
    await command({ op: "add_layer", layer: "pixel", name: null });
    await command({ op: "add_layer", layer: "pixel", name: null });
    const rows = await c.eval(`[...document.querySelectorAll('.nrow')].map((r) => { const b = r.getBoundingClientRect(); return { x: b.left + b.width / 2, y: b.top + b.height / 2, h: b.height }; })`);
    const [top, , bottom] = rows;
    const s = await c.seq();
    await c.drag(bottom.x, bottom.y, top.x, top.y - top.h * 0.35, 16);
    const after = await c.waitFor("layers", "true", { after: s, timeout: 3000 });
    assert(after.layers[0].name === "Background", `the dragged row did not move: ${after.layers.map((l) => l.name)}`);
  },

  async "a shape added after an edit is drawn"() {
    await newDoc();
    await command({ op: "add_layer", layer: "pixel", name: null });
    await command(rect(50, 50, 200, 200, [0, 0, 65535, 65535]));
    await screenAt([150, 150], isBlue, "the shape");
    await screenAt([500, 400], isWhite, "the canvas around it");
  },

  async "the Type tool's text is drawn once committed"() {
    await newDoc();
    await command({ op: "add_layer", layer: "pixel", name: null });
    const tool = await byTip("Horizontal Type Tool");
    await c.click(tool.x, tool.y);
    await sleep(300);
    await c.canvasClick(100, 150);
    await sleep(600);
    await c.type("HELLO WORLD");
    await sleep(400);
    await c.key("Enter", { modifiers: 2 });
    await sleep(1000);
    assert((await layers()).some((l) => l.kind === "text"), "no text layer");
    const scan = [];
    for (let y = 110; y <= 190; y += 3) for (let x = 95; x <= 260; x += 3) scan.push([x, y]);
    const dark = (await c.pixels(scan)).filter(isDark).length;
    assert(dark > 20, `the text is not on screen (${dark} dark samples)`);
    const move = await byTip("Move Tool (V)").catch(() => null);
    if (move) await c.click(move.x, move.y);
  },

  async "a layer style from the fx menu is drawn"() {
    await newDoc();
    await command({ op: "add_layer", layer: "pixel", name: null });
    await command(rect(50, 50, 200, 200, [0, 0, 65535, 65535]));
    await screenAt([150, 150], isBlue, "the shape");
    const fx = await byTip("Add a layer style");
    await c.click(fx.x, fx.y);
    await sleep(300);
    const item = await byText("Color Overlay...");
    await c.click(item.x, item.y);
    await sleep(600);
    const ok = await dialogButton("OK");
    await c.click(ok.x, ok.y);
    await screenAt([150, 150], isRed, "the Color Overlay");
  },

  async "Free Transform: a drag from the middle moves, a click away commits"() {
    await newDoc();
    await command(rect(100, 100, 200, 200, [0, 0, 65535, 65535]));
    await command({ op: "rasterize", layers: ["active"] }, "m.layers.every((l) => l.kind === 'pixel')");
    let s = await c.seq();
    await c.send({ type: "action", id: "xf:free", args: null });
    await c.waitFor("transform_box", "m.up === true", { after: s });
    await sleep(400);
    await c.canvasDrag(200, 200, 300, 250);
    await sleep(400);
    s = await c.seq();
    await c.canvasClick(700, 550);
    await c.waitFor("transform_box", "m.up === false", { after: s });
    await screenAt([350, 300], isBlue, "the moved layer");
    await screenAt([150, 150], isWhite, "its old place");
  },

  async "the Adjustments panel makes a layer with a live, cancellable dialog"() {
    await newDoc();
    const button = await spot(`[...document.querySelectorAll('.adj-btn')].find((b) => b.dataset.tip === 'Brightness/Contrast')`);
    const s = await c.seq();
    await c.click(button.x, button.y);
    await c.waitFor("layers", "m.layers.some((l) => l.kind === 'adjustment')", { after: s });
    await sleep(800);
    assert(await dialogOpen(), "no dialog opened");
    const slider = await c.eval(`(() => { const d = [...document.querySelectorAll('.dialog')].find((d) => d.getBoundingClientRect().width > 0); const lab = [...d.querySelectorAll('*')].find((e) => e.children.length === 0 && e.textContent.trim() === 'Brightness:'); const r = lab.parentElement.querySelector('input[type=range]').getBoundingClientRect(); return { x0: r.left + 3, x1: r.right - 3, y: r.top + r.height / 2 }; })()`);
    await c.drag((slider.x0 + slider.x1) / 2, slider.y, slider.x0, slider.y, 12);
    // A corner of the canvas, clear of the dialog.
    await screenAt([20, 20], ([r]) => r < 235, "the live preview");
    const history = await c.eval("JSON.stringify(window.__fx.last.history)");
    const cancel = await dialogButton("Cancel");
    await c.click(cancel.x, cancel.y);
    await screenAt([20, 20], isWhite, "the canvas after Cancel");
    assert(history === (await c.eval("JSON.stringify(window.__fx.last.history)")), "Cancel changed the history");
  },
};

// ---------------------------------------------------------------- run

const filter = (process.argv[2] || "").toLowerCase();
let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  if (filter && !name.toLowerCase().includes(filter)) continue;
  const started = Date.now();
  try {
    await run();
    console.log(`  ok    ${name}  (${Date.now() - started} ms)`);
  } catch (error) {
    failed += 1;
    console.log(`  FAIL  ${name}\n        ${error.message}`);
    // Leave no dialog or box behind for the next scenario.
    await c.key("Escape").catch(() => {});
    await sleep(300);
  }
}
c.close();
console.log(failed ? `\n${failed} failed` : "\nall passed");
process.exit(failed ? 1 : 0);
