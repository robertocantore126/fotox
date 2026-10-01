// Development provenance stays outside the DOM and is released with each node.
const origins = new WeakMap();
let capture = true;
try { capture = localStorage.getItem("fotox.inspector.sources") !== "off"; } catch (_) {}

export function rememberSource(node, factory, props) {
  if (!capture) return;
  const trace = new Error();
  if (Error.captureStackTrace) Error.captureStackTrace(trace, factory);
  const handlers = Object.entries(props || {}).filter(([key, value]) => key.startsWith("on") && typeof value === "function")
    .map(([key, value]) => `${key}: ${value.name || "anonymous callback"}`);
  origins.set(node, { trace, handlers });
}

export function sourceOf(node) {
  const origin = origins.get(node);
  if (!origin) return null;
  const frames = String(origin.trace.stack || "").split("\n").filter((line) => /\/js\//.test(line) && !/\/(el|inspector-source)\.js:/.test(line));
  return { frames: frames.slice(0, 4).map((line) => line.trim().replace(/(?:[a-z]+:\/\/[^\s(]*?|file:\/\/\/[^\s(]*?)\/js\//gi, "ui/js/")), handlers: origin.handlers };
}
