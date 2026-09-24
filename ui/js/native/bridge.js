// Fotox — the bridge between the UI and the native shell/engine.
//
// Inside the app, the vendored Graphite shell injects `window.sendNativeMessage`
// and `window.initializeNativeCommunication` before any script runs
// (docs/GRAPHITE.md §1), so their presence is what "native mode" means. In a
// plain browser neither exists; the UI must keep working there.
//
// Usage:
//   import * as bridge from "./native/bridge.js";
//   bridge.init();                          // once, at startup
//   bridge.send({ type: "hello", ui_version: "…" });
//   bridge.on("toast", (msg, payload) => …);

import { encodeJson, encodeBinary, decode } from "./protocol.js";

/** True inside the Fotox app, false in a plain browser. */
export const isNative = typeof window.sendNativeMessage === "function";

const listeners = new Map(); // message type → Set of callbacks
let started = false;

/**
 * Start the bridge. In native mode: mark `<body class="native">`, install the
 * receiver, then tell the shell it may start delivering messages (it queues
 * them until this call). Safe to call more than once.
 */
export function init() {
  if (started) return;
  started = true;
  if (!isNative) return;
  document.body.classList.add("native");
  window.receiveNativeMessage = receive;
  window.initializeNativeCommunication();
}

/** Send a JSON message (an object with a `type` field). */
export function send(message) {
  // In a plain browser there is no engine to talk to yet: M0-T05 routes these
  // to the mock engine. Until then they are dropped on purpose.
  if (!isNative) return;
  window.sendNativeMessage(encodeJson(message));
}

/** Send a JSON header followed by raw bytes. */
export function sendBinary(header, bytes) {
  if (!isNative) return;
  window.sendNativeMessage(encodeBinary(header, bytes));
}

/**
 * Call `fn(message, payload)` for every incoming message of `type`.
 * Returns a function that removes the listener.
 */
export function on(type, fn) {
  if (!listeners.has(type)) listeners.set(type, new Set());
  listeners.get(type).add(fn);
  return () => listeners.get(type).delete(fn);
}

function receive(buffer) {
  let frame;
  try {
    frame = decode(buffer);
  } catch (error) {
    console.error("fotox bridge: dropping a malformed message:", error);
    return;
  }
  const handlers = listeners.get(frame.message.type);
  if (!handlers || handlers.size === 0) {
    console.warn(`fotox bridge: no handler for "${frame.message.type}"`);
    return;
  }
  for (const fn of handlers) fn(frame.message, frame.payload);
}
