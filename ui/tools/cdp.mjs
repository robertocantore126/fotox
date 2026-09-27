// Fotox — drive the real app's UI over the Chrome DevTools Protocol.
//
// Start the app with the debugging port on:
//   $env:GRAPHITE_BROWSER_DEBUG_PORT = 9222; cargo xtask run
// then, from the repository root:
//   node ui/tools/cdp.mjs eval "document.title"
//   node ui/tools/cdp.mjs reload
//   node ui/tools/cdp.mjs click 640 300
//   node ui/tools/cdp.mjs drag 1300 330 1300 390
//   node ui/tools/cdp.mjs shot out.png          (the UI layer only: the canvas
//                                                 is drawn by wgpu under it)
//
// The same functions drive the scenarios in `ui/tools/smoke.mjs`. Node 22+
// (global WebSocket, fetch); no dependencies.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const PORT = Number(process.env.FOTOX_CDP_PORT || process.env.GRAPHITE_BROWSER_DEBUG_PORT || 9222);

export async function connect(port = PORT) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = targets.find((t) => t.type === "page");
  if (!page) throw new Error(`no page on port ${port}: is the app running with GRAPHITE_BROWSER_DEBUG_PORT=${port}?`);
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = () => fail(new Error("cannot open the DevTools socket")); });
  let next = 1;
  const pending = new Map();
  const listeners = new Set();
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      const { ok, fail } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) fail(new Error(`${msg.error.message} (${msg.error.code})`));
      else ok(msg.result);
    } else if (msg.method) {
      for (const l of listeners) l(msg);
    }
  };
  const send = (method, params = {}) => new Promise((ok, fail) => {
    const id = next++;
    pending.set(id, { ok, fail });
    ws.send(JSON.stringify({ id, method, params }));
  });

  const api = {
    send,
    on: (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
    close: () => ws.close(),

    /** Evaluate `expression` in the page (awaiting promises) and return its value. */
    async eval(expression) {
      const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
      return r.result.value;
    },

    async reload() {
      await send("Page.enable");
      const loaded = new Promise((ok) => { const off = api.on((m) => { if (m.method === "Page.loadEventFired") { off(); ok(); } }); });
      await send("Page.reload", { ignoreCache: true });
      await loaded;
    },

    /** Real pointer input, as the OS would send it (page CSS pixels). */
    async mouse(type, x, y, { button = "left", buttons, clickCount = 1, modifiers = 0 } = {}) {
      const held = buttons ?? (type === "mouseReleased" ? 0 : button === "left" ? 1 : button === "right" ? 2 : 0);
      await send("Input.dispatchMouseEvent", { type, x, y, button, buttons: held, clickCount, modifiers });
    },
    async click(x, y, opts = {}) {
      await api.mouse("mouseMoved", x, y, { ...opts, buttons: 0 });
      await api.mouse("mousePressed", x, y, opts);
      await api.mouse("mouseReleased", x, y, opts);
    },
    async drag(x0, y0, x1, y1, steps = 12) {
      await api.mouse("mouseMoved", x0, y0, { buttons: 0 });
      await api.mouse("mousePressed", x0, y0);
      for (let i = 1; i <= steps; i++) {
        await api.mouse("mouseMoved", x0 + ((x1 - x0) * i) / steps, y0 + ((y1 - y0) * i) / steps);
      }
      await api.mouse("mouseReleased", x1, y1);
    },
    async key(key, { code = key, modifiers = 0, text } = {}) {
      await send("Input.dispatchKeyEvent", { type: "keyDown", key, code, modifiers, text });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, modifiers });
    },
    async type(text) {
      await send("Input.insertText", { text });
    },
    async screenshot() {
      const r = await send("Page.captureScreenshot", { format: "png" });
      return Buffer.from(r.data, "base64");
    },
    /**
     * Record what the engine sends: `window.__fx.last[type]` is the latest
     * message of each type, `__fx.seq` counts them, `__fx.bridge` is the live
     * bridge module (the UI's own instance). Once per page load.
     */
    async install() {
      return api.eval(`(async () => {
        if (window.__fx) return "already";
        const { decode } = await import("./js/native/protocol.js");
        const bridge = await import("./js/native/bridge.js");
        const fx = (window.__fx = { last: {}, log: [], seq: 0, bridge });
        const receive = window.receiveNativeMessage;
        window.receiveNativeMessage = (buffer) => {
          try {
            const m = decode(buffer).message;
            fx.seq += 1;
            fx.last[m.type] = m;
            fx.log.push({ seq: fx.seq, type: m.type });
            if (fx.log.length > 1000) fx.log.shift();
          } catch (_) { /* the UI reports malformed frames itself */ }
          return receive(buffer);
        };
        return "installed";
      })()`);
    },
    /** Send a UI → engine message through the UI's bridge. */
    async send(message) {
      await api.eval(`window.__fx.bridge.send(${JSON.stringify(message)})`);
    },
    /** The sequence number of the last engine message (to wait for newer ones). */
    async seq() {
      return api.eval("window.__fx.seq");
    },
    /**
     * Wait until the latest `type` message newer than `after` satisfies
     * `predicate` (a JS expression over `m`); returns it.
     */
    async waitFor(type, predicate = "true", { after = 0, timeout = 5000 } = {}) {
      const until = Date.now() + timeout;
      for (;;) {
        const got = await api.eval(`(() => {
          const m = window.__fx.last[${JSON.stringify(type)}];
          const seq = window.__fx.log.filter((e) => e.type === ${JSON.stringify(type)}).at(-1)?.seq ?? 0;
          return m && seq > ${after} && (${predicate}) ? m : null;
        })()`);
        if (got) return got;
        if (Date.now() > until) throw new Error(`no "${type}" message with ${predicate} within ${timeout} ms`);
        await sleep(50);
      }
    },
    /** Document pixel `(dx, dy)` → page pixels, from the last `view` message. */
    async docToPage(dx, dy) {
      const v = await api.eval("window.__fx.last.view");
      const r = await api.eval("document.getElementById('viewport').getBoundingClientRect().toJSON()");
      const a = ((v.rotation_deg || 0) * Math.PI) / 180;
      const [ox, oy] = [(dx - v.center_x) * v.zoom, (dy - v.center_y) * v.zoom];
      return { x: r.left + r.width / 2 + ox * Math.cos(a) - oy * Math.sin(a), y: r.top + r.height / 2 + ox * Math.sin(a) + oy * Math.cos(a) };
    },
    /** What is on screen at document points `[[x, y], ...]`: `[[r, g, b], ...]`. */
    async pixels(points) {
      const at = [];
      for (const [x, y] of points) {
        const p = await api.docToPage(x, y);
        at.push(`${p.x},${p.y}`);
      }
      return win("pixels", ...at).split(/\r?\n/).filter(Boolean).map((l) => l.split(",").map(Number));
    },
    /** A real OS click on the canvas at document point `(dx, dy)`. */
    async canvasClick(dx, dy, modifier = "") {
      const p = await api.docToPage(dx, dy);
      win("click", `${p.x},${p.y}`, ...(modifier ? [modifier] : []));
    },
    /** A real OS drag on the canvas between document points. */
    async canvasDrag(x0, y0, x1, y1, steps = 16) {
      const a = await api.docToPage(x0, y0);
      const b = await api.docToPage(x1, y1);
      win("drag", `${a.x},${a.y}`, `${b.x},${b.y}`, String(steps));
    },
    /** The centre of the first element matching `selector` (page pixels), or null. */
    async center(selector) {
      return api.eval(`(() => { const e = document.querySelector(${JSON.stringify(selector)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height }; })()`);
    },
  };
  return api;
}

export const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));

/** Run `ui/tools/win.ps1` (real OS pointer input, screen pixels). */
export function win(...args) {
  const script = fileURLToPath(new URL("./win.ps1", import.meta.url));
  return execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args]).toString();
}

// ---- command line
if (process.argv[1]?.endsWith("cdp.mjs")) {
  const [cmd, ...args] = process.argv.slice(2);
  const cdp = await connect();
  try {
    switch (cmd) {
      case "eval": console.log(JSON.stringify(await cdp.eval(args.join(" ")), null, 2)); break;
      case "reload": await cdp.reload(); console.log("reloaded"); break;
      case "click": await cdp.click(Number(args[0]), Number(args[1])); break;
      case "drag": await cdp.drag(...args.slice(0, 4).map(Number)); break;
      case "key": await cdp.key(args[0]); break;
      case "type": await cdp.type(args.join(" ")); break;
      case "shot": {
        const fs = await import("node:fs");
        fs.writeFileSync(args[0] || "ui-shot.png", await cdp.screenshot());
        console.log(args[0] || "ui-shot.png");
        break;
      }
      default: console.log("usage: node ui/tools/cdp.mjs eval <js> | reload | click x y | drag x0 y0 x1 y1 | key <Key> | type <text> | shot [file]");
    }
  } finally {
    cdp.close();
  }
}
