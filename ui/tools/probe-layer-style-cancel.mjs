// Scratch probe: does Cancel put the style back? (see probe-layer-style.mjs)
import { connect, sleep } from "./cdp.mjs";

const c = await connect();
await c.install();
const layers = () => c.eval("window.__fx.last.layers.layers");
const styled = async () => (await layers()).find((l) => l.name === "Shape")?.styles || null;
const ui = (js) => c.eval(`(() => { ${js} })()`);
const style = (js) => c.eval(`import('./js/native/styles.js').then((m) => { ${js} })`);
const doc = await c.eval("window.__fx.last.active_document.doc");
const shape = (await layers()).find((l) => l.name === "Shape");
await c.send({ type: "command", doc, command: { op: "select_layers", layers: [{ id: shape.id }] } });
await c.send({ type: "command", doc, command: { op: "set_layer_style", layer: { id: shape.id }, styles: null } });
await sleep(600);
console.log("start", JSON.stringify(await styled()));
await style("m.openStyleDialog('style-drop-shadow')");
await sleep(600);
console.log("open", Object.keys((await styled()) || {}));
await ui("[...document.querySelectorAll('.ls-item')].find((r) => r.textContent.trim() === 'Satin').click()");
await sleep(500);
console.log("satin", Object.keys((await styled()) || {}));
console.log("dialogs", await ui("return document.querySelectorAll('.dialog').length"));
await ui("[...document.querySelectorAll('.ls-side .btn')].find((b) => b.textContent === 'Cancel').click()");
await sleep(900);
console.log("after cancel", JSON.stringify(await styled()), "dialogs", await ui("return document.querySelectorAll('.dialog').length"));
c.close();
