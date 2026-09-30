// Fotox — static audit: which UI controls do nothing in the app.
//
// The menus are already honest (check-data.mjs greys out what the engine
// does not implement). Everything else is drawn by hand and can look alive
// while doing nothing: a panel built from mock data, a button that only shows
// the "mock" toast, a dialog nothing reads, a tool option the engine never
// looks at. This reads the UI and the engine sources and names each one.
//
//   node ui/tools/audit-ui.mjs            print the report
//   node ui/tools/audit-ui.mjs --write    also write docs/reports/UI-AUDIT.md
//
// Static, so it cannot see behaviour: a control that sends a real command
// the engine then mishandles is not here (that is ui/tools/smoke.mjs's job).
// "Likely" findings are strings found nowhere in the code that reads them;
// a false alarm is possible there, a missed dead control less so.

import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const ui = join(dirname(fileURLToPath(import.meta.url)), "..");
const repo = join(ui, "..");
const { IMPLEMENTED } = await import(new URL("../js/data/implemented.js", import.meta.url));
const { panelDefs } = await import(new URL("../js/data/panels.js", import.meta.url));
const { optionBars } = await import(new URL("../js/data/options.js", import.meta.url));
const { dialogs } = await import(new URL("../js/data/dialogs.js", import.meta.url));
const { allTools } = await import(new URL("../js/data/tools.js", import.meta.url));

function files(dir, ext) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...files(path, ext));
    else if (name.endsWith(ext)) out.push(path);
  }
  return out;
}
const read = (path) => readFileSync(path, "utf8").replace(/\r\n/g, "\n");

// What the app runs: the engine and shell, the native UI modules (not the
// browser mock engine), the action dispatcher and the shell's main.
const rust = ["crates/fx-engine/src", "crates/fx-app/src"].flatMap((d) => files(join(repo, d), ".rs")).map(read).join("\n");
const nativeFiles = files(join(ui, "js/native"), ".js").filter((f) => !f.endsWith("mock-engine.js"));
const appJs = [...nativeFiles, join(ui, "js/actions.js"), join(ui, "js/main.js"), join(ui, "js/optionsbar.js")].map(read).join("\n");
/** Whether `name` (a label or key) is read anywhere in the app's code. */
const isRead = (name) => {
  const bare = name.replace(/:\s*$/, "");
  if ([bare, `${bare}:`].some((n) => rust.includes(`"${n}"`) || appJs.includes(`"${n}"`) || appJs.includes(`'${n}'`) || appJs.includes(`\`${n}\``))) return true;
  // Read as a property: `v.Highlight`.
  return /^[A-Za-z_]\w*$/.test(bare) && new RegExp(String.raw`\b\w+\.${bare}\b`).test(appJs);
};
/** Whether the app handles action id `a` (engine, UI-local families, listeners). */
const UI_LOCAL = ["panel:", "tool:", "toggle:", "screen:", "zoom:"];
const handled = (a) =>
  IMPLEMENTED.has(a) ||
  IMPLEMENTED.has(a.split(":")[0] + ":*") ||
  UI_LOCAL.some((p) => a.startsWith(p)) ||
  appJs.includes(`on("${a}"`) ||
  appJs.includes(`"${a}"`) && read(join(ui, "js/actions.js")).includes(`"${a}"`);

const findings = []; // { area, what, why, where, likely }
const add = (area, what, why, where, likely = false) => findings.push({ area, what, why, where, likely });
const lineOf = (text, index) => text.slice(0, index).split("\n").length;

// ------------------------------------------------------------------ panels

{
  const path = join(ui, "js/panels.js");
  const src = read(path);
  const lines = src.split("\n");
  const quick = new Set([...(/const QUICK = \{([^}]*)\}/.exec(src)?.[1] || "").matchAll(/"([^"]+)":/g)].map((m) => m[1]));
  const starts = [];
  lines.forEach((l, i) => {
    const m = /^ {2}([a-zA-Z]+)\((def)?\) \{$/.exec(l);
    if (m) starts.push({ name: m[1], line: i });
  });
  const titlesOf = (kind) => Object.values(panelDefs).filter((d) => d.kind === kind).map((d) => d.title);
  // A line that reaches the app: engine, colours, the app's own modules.
  const REAL = /bridge\.|setColors\(|emit\("action"|native[A-Z]\w*\.|setTool\(/;
  for (const [k, { name, line }] of starts.entries()) {
    const titles = titlesOf(name);
    if (!titles.length) continue; // a helper, not a panel
    const panel = titles.join(" / ");
    const end = k + 1 < starts.length ? starts[k + 1].line : lines.length;
    let body = lines.slice(line + 1, end).map((text, i) => ({ text, at: `ui/js/panels.js:${line + 2 + i}` }));
    if (/if \(bridge\.isNative\) return /.test((body.find((b) => b.text.trim()) || {}).text || "")) continue; // the app's own panel
    // What follows an app-only early return is the browser mock-up's.
    const cut = body.findIndex((b) => /if \(bridge\.isNative\) return /.test(b.text));
    if (cut >= 0) body = body.slice(0, cut + 1);
    const items = [];
    const real = body.some((b) => REAL.test(b.text) || [...b.text.matchAll(/emit\("ask-dialog", "([a-z0-9-]+)"\)/g)].some((m) => IMPLEMENTED.has(`dlg:${m[1]}`)));
    for (const { text: l, at } of body) {
      if (/emit\("mock"/.test(l) && !REAL.test(l)) {
        const what = /emit\("mock", ([^)]*?)\)/.exec(l)[1].replace(/`|"/g, "").slice(0, 40);
        items.push([`a control that only shows the mock toast (${what})`, at]);
      }
      // `QUICK[label] ? action : mock`: the labels not in the table are mock.
      if (/QUICK\[label\]/.test(l)) {
        const near = body.map((b) => b.text).join("\n");
        const labels = [...(/\.\.\.\[(\[\[[^\n]*\]\])\]\.map/.exec(near)?.[1] || "").matchAll(/\["([^"]+)", "i-/g)].map((m) => m[1]);
        for (const q of labels.filter((x) => !quick.has(x))) items.push([`button “${q}” only shows the mock toast`, at]);
      }
      for (const m of l.matchAll(/barBtn\("[^"]+", "([^"]+)"\)/g)) items.push([`button “${m[1]}” with no action`, at]);
      for (const m of l.matchAll(/emit\("ask-dialog", "([a-z0-9-]+)"\)/g)) {
        if (!IMPLEMENTED.has(`dlg:${m[1]}`)) items.push([`opens the mock dialog “${m[1]}” (nothing handles its OK)`, at]);
      }
      for (const m of l.matchAll(/(?:numberRow|fieldRow)\("([^"]+)"/g)) items.push([`field “${m[1]}” shows a made-up value and is not wired`, at]);
      if (/addEventListener\("input", \(\) => \{ \w+\.value = \w+\.value; \}\)/.test(l)) items.push(["a slider that only moves its own number box", at]);
      if (/h\("input"/.test(l) && !/addEventListener|on[a-z]+:|oninput|onchange/.test(l) && !/\bconst \w+ = h\("input".*\n?/.test("")) {
        if (!body.some((b) => /\.addEventListener\(/.test(b.text) && b.text.includes((/const (\w+) = h\("input"/.exec(l) || [])[1] + "."))) items.push(["an input nothing reads", at]);
      }
      if (/classList\.toggle\("on"\)/.test(l)) items.push(["a toggle that only changes its own look", at]);
    }
    if (!real) add("Panels", panel, `the whole panel is a mock-up (${items.length} controls, made-up content)`, `ui/js/panels.js:${line + 1}`);
    else for (const [why, at] of items) add("Panels", panel, why, at);
  }
}

// ------------------------------------------------------------- option bars

{
  const path = join(ui, "js/data/options.js");
  const src = read(path);
  const nameOf = (id) => (allTools.find((t) => t.id === id) || {}).name || id;
  // Tools the UI runs itself (main.js's UI_TOOLS) or the view handles.
  const uiTools = new Set([...(/const UI_TOOLS = new Set\(\[([^\]]*)\]/.exec(read(join(ui, "js/main.js")))?.[1] || "").matchAll(/"([^"]+)"/g)].map((m) => m[1]));
  const toolImplemented = (id) => IMPLEMENTED.has(`tool:${id}`) || uiTools.has(id) || rust.includes(`tool == "${id}"`);
  for (const [tool, spec] of Object.entries(optionBars)) {
    if (!Array.isArray(spec) || tool.startsWith("_")) continue;
    const where = `ui/js/data/options.js:${lineOf(src, src.indexOf(`  ${tool.includes("-") ? `"${tool}"` : tool}:`))}`;
    if (!toolImplemented(tool) && allTools.some((t) => t.id === tool)) {
      add("Tools", nameOf(tool), "the tool is not implemented: its option bar is decoration", where);
      continue;
    }
    for (const c of spec) {
      if (!c || typeof c !== "object") continue;
      const label = c.key || (c.text ? c.text.replace(/:\s*$/, "") : null);
      if (c.type === "btn") {
        if (!c.action) add("Option bars", `${nameOf(tool)} ▸ ${c.text}`, "a button that only shows the mock toast", where);
        else if (!handled(c.action)) add("Option bars", `${nameOf(tool)} ▸ ${c.text}`, `its action “${c.action}” is implemented nowhere`, where);
        continue;
      }
      if (c.type === "btngroup" && c.actions) {
        c.actions.forEach((a, i) => { if (!handled(a)) add("Option bars", `${nameOf(tool)} ▸ ${(c.titles || [])[i] || a}`, `its action “${a}” is implemented nowhere`, where); });
        continue;
      }
      if (!["toggle", "num", "select", "text", "btngroup", "swatch"].includes(c.type) || !label) continue;
      if (!isRead(label)) add("Option bars", `${nameOf(tool)} ▸ ${label}`, "sent to the engine, but no code reads an option of that name", where, true);
    }
  }
}

// ----------------------------------------------- context menus in panels

for (const file of nativeFiles) {
  const src = read(file);
  const rel = relative(repo, file).replace(/\\/g, "/");
  for (const m of src.matchAll(/item\("([^"]+)",\s*"([a-z][a-z0-9-]*:[a-z0-9:-]+)"/g)) {
    if (!handled(m[2])) add("Context menus", m[1], `its action “${m[2]}” is implemented nowhere`, `${rel}:${lineOf(src, m.index)}`);
  }
}

// ------------------------------------------------------- dialog fields

{
  const src = read(join(ui, "js/data/dialogs.js"));
  const fieldsOf = (fields) => (fields || []).flatMap((f) => (f.fields ? fieldsOf(f.fields) : [f]));
  const SKIP = new Set(["Preview", "Name:", "Name"]);
  // Dialogs the app opens with fields of its own: the data's fields are
  // never shown there (the app's code is what the user sees).
  // Dialogs something else answers before the data dialog could open: the
  // shell (a native file dialog) or an actions.js branch that does not open
  // it (Color Lookup picks a file).
  const actionsJs = read(join(ui, "js/actions.js"));
  const intercepted = (id) => {
    if (rust.includes(`"dlg:${id}" =>`)) return true;
    const at = actionsJs.indexOf(`a === "dlg:${id}"`);
    return at >= 0 && !actionsJs.slice(at, at + 400).includes(`openDialog("${id}"`);
  };
  const own = new Set();
  for (const file of nativeFiles) {
    const code = read(file);
    const claimed = [...code.matchAll(/id === "([a-z0-9-]+)"/g)].map((m) => m[1]);
    for (const m of code.matchAll(/openDialog\(("([a-z0-9-]+)"|[a-zA-Z.]+),\s*\{/g)) {
      const window = code.slice(m.index, m.index + 1500);
      const next = window.indexOf("openDialog(", 12);
      if (!(next < 0 ? window : window.slice(0, next)).includes("fields:")) continue;
      if (m[2]) own.add(m[2]);
      else claimed.forEach((id) => own.add(id));
    }
  }
  for (const [id, def] of Object.entries(dialogs)) {
    if (!IMPLEMENTED.has(`dlg:${id}`) || own.has(id) || intercepted(id) || !def || !def.fields) continue;
    const where = `ui/js/data/dialogs.js:${lineOf(src, Math.max(0, src.indexOf(`"${id}"`)))}`;
    for (const f of fieldsOf(def.fields)) {
      if (!f.label || !["num", "select", "check", "radio", "range", "text", "color"].includes(f.type) || SKIP.has(f.label)) continue;
      if (!isRead(f.label)) add("Dialog fields", `${(def.title || id).replace(/\.\.\.$/, "")} ▸ ${f.label.replace(/:\s*$/, "")}`, "shown, but nothing reads its value", where, true);
    }
  }
}

// -------------------------------------------------------------- report

const areas = [...new Set(findings.map((f) => f.area))];
const out = [];
out.push(`Fotox UI audit — ${findings.length} controls that do nothing in the app (static; "likely" = unread by name)`);
for (const area of areas) {
  const list = findings.filter((f) => f.area === area);
  out.push("", `${area} (${list.length})`);
  for (const f of list) out.push(`  ${f.likely ? "likely " : ""}${f.what} — ${f.why}  [${f.where}]`);
}
console.log(out.join("\n"));

if (process.argv.includes("--write")) {
  const md = [
    "# UI audit",
    "",
    "Generated by `node ui/tools/audit-ui.mjs --write`. Each row is a control the app shows that does nothing; " +
      "\"likely\" rows are values no code reads by name (a false alarm is possible there).",
    "",
  ];
  for (const area of areas) {
    const list = findings.filter((f) => f.area === area);
    md.push(`## ${area} (${list.length})`, "", "| Control | Problem | Where |", "|---|---|---|");
    for (const f of list) md.push(`| ${f.what} | ${f.likely ? "_likely:_ " : ""}${f.why} | \`${f.where}\` |`);
    md.push("");
  }
  writeFileSync(join(repo, "docs/reports/UI-AUDIT.md"), md.join("\n"));
  console.log("\nwritten: docs/reports/UI-AUDIT.md");
}
