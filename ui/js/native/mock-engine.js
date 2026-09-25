// Fotox — stand-in for the engine when the UI runs in a plain browser.
//
// `bridge.send` hands every message here instead of to native code. The mock
// answers the way the real engine will for things it does not implement, so
// browser mode exercises the same message paths as the app.

import { UI, ENGINE } from "./protocol.js";

// Action ids the UI handles entirely by itself (panels, tools, view flags,
// screen modes, dialogs, zoom, workspaces). The engine has nothing to say
// about them, so the mock stays quiet instead of toasting on every click.
// Mirror of `fx_protocol::UI_LOCAL_ACTION_PREFIXES`.
const UI_LOCAL_PREFIXES = ["panel:", "panels:", "tool:", "toggle:", "screen:", "dlg:", "zoom:", "ws:", "par:", "debug:"];

/** Answer one UI → engine message; returns the replies (possibly none). */
export function handle(message) {
  switch (message.type) {
    case UI.HELLO:
      return [{ type: ENGINE.TOAST, text: "Mock engine connected (browser mode)" }];
    case UI.ACTION:
      // In the browser there is no engine pointer routing, so the mock has no
      // way to sample on a click; `debug:pick-color` stands in for it and
      // exercises the ColorPicked path with a fixed colour.
      if (message.id === "debug:pick-color") {
        return [{ type: ENGINE.COLOR_PICKED, rgba: [65535, 0, 0, 65535], target: "fg" }];
      }
      if (UI_LOCAL_PREFIXES.some((p) => message.id.startsWith(p))) return [];
      return [{ type: ENGINE.TOAST, text: `${message.id}: not implemented (mock engine)` }];
    case UI.TOOL_OPTIONS:
    case UI.SET_COLORS:
    // A viewport key (M5-T04): only a tool with an operation in progress
    // answers it, and the mock has none.
    case UI.KEY:
      // Tool state is kept by the engine; the mock accepts and stays quiet.
      return [];
    default:
      // viewport_bounds / direct_input are shell messages; the rest arrive
      // with documents (M1+). Nothing to answer yet.
      return [];
  }
}
