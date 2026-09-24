// Fotox — stand-in for the engine when the UI runs in a plain browser.
//
// `bridge.send` hands every message here instead of to native code. The mock
// answers the way the real engine will for things it does not implement, so
// browser mode exercises the same message paths as the app.

import { UI, ENGINE } from "./protocol.js";

// Action ids the UI handles entirely by itself (panels, tools, view flags,
// screen modes, dialogs, zoom, workspaces). The engine has nothing to say
// about them, so the mock stays quiet instead of toasting on every click.
const UI_LOCAL_PREFIXES = ["panel:", "panels:", "tool:", "toggle:", "screen:", "dlg:", "zoom:", "ws:", "par:"];

/** Answer one UI → engine message; returns the replies (possibly none). */
export function handle(message) {
  switch (message.type) {
    case UI.HELLO:
      return [{ type: ENGINE.TOAST, text: "Mock engine connected (browser mode)" }];
    case UI.ACTION:
      if (UI_LOCAL_PREFIXES.some((p) => message.id.startsWith(p))) return [];
      return [{ type: ENGINE.TOAST, text: `${message.id}: not implemented (mock engine)` }];
    default:
      // viewport_bounds / direct_input are shell messages; the rest arrive
      // with documents (M1+). Nothing to answer yet.
      return [];
  }
}
