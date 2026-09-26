// Fotox — controllo di coerenza dei dati.
//
// Verifica statica (nessun browser) che i dati dei menu puntino solo a cose che
// esistono davvero: finestre, pannelli, strumenti, icone. Un'azione `dlg:` senza
// definizione, o un'icona `i-xxx` assente dallo sprite, è un pulsante che non fa
// nulla o che resta vuoto: errori silenziosi in un'interfaccia disegnata a runtime.
//
// Uso:  node tools/check-data.mjs      (exit code 1 se trova errori)

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const { menus } = await import(new URL("../js/data/menus.js", import.meta.url));
const { dialogs, dialogDef } = await import(new URL("../js/data/dialogs.js", import.meta.url));
const { toolSlots, allTools } = await import(new URL("../js/data/tools.js", import.meta.url));
const { panelDefs, dockGroups, initialActiveTab } = await import(new URL("../js/data/panels.js", import.meta.url));
const { optionBars } = await import(new URL("../js/data/options.js", import.meta.url));

const problems = [];
// Menu honesty (M7-T09): the list of implemented actions must be current.
{
  const { scan, body } = await import(new URL("./gen-implemented.mjs", import.meta.url));
  const current = readFileSync(join(root, "js/data/implemented.js"), "utf8").replace(/\r\n/g, "\n");
  if (current !== body(scan())) problems.push("js/data/implemented.js is stale: run node ui/tools/gen-implemented.mjs");
}
const warnings = [];
const say = (ok, msg) => (ok ? "" : problems.push(msg));

/* ------------------------------------------------- raccolta voci di menu */

const allItems = [];
function walk(items, path) {
  for (const item of items) {
    if (item.sep) continue;
    const here = path ? `${path} > ${item.label}` : item.label;
    allItems.push({ ...item, path: here });
    if (item.sub) walk(item.sub, here);
  }
}
for (const menu of menus) walk(menu.items, menu.label);

const depth = (p) => (p.match(/ > /g) || []).length;
const stats = {
  menus: menus.length,
  topLevel: allItems.filter((i) => depth(i.path) === 1).length,
  commands: allItems.filter((i) => !i.sub).length,
  submenus: allItems.filter((i) => i.sub).length,
  disabled: allItems.filter((i) => i.dis).length,
  checkable: allItems.filter((i) => i.chk).length,
  withShortcut: allItems.filter((i) => i.short).length,
  deepest: Math.max(...allItems.map((i) => depth(i.path))),
};

/* ------------------------------------------------------- azioni -> bersagli */

const byPrefix = new Map();
for (const item of allItems) {
  if (!item.a) continue;
  const [prefix] = item.a.split(":");
  if (!byPrefix.has(prefix)) byPrefix.set(prefix, []);
  byPrefix.get(prefix).push(item);
}

const actionsSrc = readFileSync(join(root, "js/actions.js"), "utf8");

for (const item of allItems) {
  if (!item.a) continue;
  const [prefix, ...rest] = item.a.split(":");
  const id = rest.join(":");

  if (prefix === "dlg") {
    say(dialogs[id] !== undefined, `finestra mancante: dlg:${id}  (${item.path})`);
  } else if (prefix === "panel") {
    say(panelDefs[id] !== undefined, `pannello mancante: panel:${id}  (${item.path})`);
  } else if (prefix === "tool") {
    say(allTools.some((t) => t.id === id), `strumento mancante: tool:${id}  (${item.path})`);
  }
}

// actions.js chiude con un fallback universale (toast + stato), quindi nessun
// comando può restare muto: qui distinguiamo solo i gestori dedicati dai
// comandi che rispondono con il fallback, a titolo informativo.
const dedicated = new Set([...actionsSrc.matchAll(/startsWith\("([a-z-]+):/g)].map((m) => m[1]));
for (const key of ["dlg", "panel", "tool", "zoom", "toggle"]) dedicated.add(key);
const viaFallback = [...byPrefix.keys()].filter((p) => !dedicated.has(p));
const fallbackCount = viaFallback.reduce((n, p) => n + byPrefix.get(p).length, 0);
stats.dedicated = allItems.filter((i) => i.a && dedicated.has(i.a.split(":")[0])).length;
stats.fallback = fallbackCount;
stats.fallbackPrefixes = viaFallback.sort().join(" ");

/* ------------------------------------------- finestre raggiungibili davvero */

// Una finestra può esistere ed essere comunque irraggiungibile, se l'unica voce
// che la apre è disabilitata: è il caso da tenere d'occhio, perché l'utente la
// cerca nei menu e non la trova.
const reachable = new Set();
const onlyDisabled = new Map();
for (const item of allItems) {
  if (!item.a || !item.a.startsWith("dlg:")) continue;
  const id = item.a.slice(4);
  if (item.dis) {
    if (!onlyDisabled.has(id)) onlyDisabled.set(id, []);
    onlyDisabled.get(id).push(item.path);
  } else {
    reachable.add(id);
  }
}
// Anche gli strumenti e i pannelli aprono finestre dal proprio codice.
for (const rel of ["js/actions.js", "js/canvas.js", "js/panels.js", "js/main.js", "js/shortcuts.js"]) {
  const src = readFileSync(join(root, rel), "utf8");
  for (const m of src.matchAll(/openDialog\(\s*["']([a-z0-9-]+)["']/g)) reachable.add(m[1]);
}
const unreachable = Object.keys(dialogs).filter((id) => !reachable.has(id));
stats.reachable = reachable.size;
if (unreachable.length) {
  warnings.push(`${unreachable.length} finestre definite ma raggiungibili solo da voci disabilitate (o da nessuna voce): ${unreachable.slice(0, 12).join(", ")}${unreachable.length > 12 ? ", …" : ""}`);
}

/* ------------------------------------------------------------ pannelli/dock */

for (const group of dockGroups) {
  say(Array.isArray(group.tabs) && group.tabs.length > 0, `dock: gruppo "${group.id}" senza schede`);
  for (const id of group.tabs || []) {
    say(panelDefs[id] !== undefined, `dock: gruppo "${group.id}" elenca il pannello inesistente "${id}"`);
  }
}
for (const [groupId, id] of Object.entries(initialActiveTab)) {
  const group = dockGroups.find((g) => g.id === groupId);
  say(!!group, `scheda attiva iniziale per il gruppo inesistente "${groupId}"`);
  if (group) say((group.tabs || []).includes(id), `scheda attiva "${id}" non appartiene al gruppo "${groupId}"`);
  say(panelDefs[id] !== undefined, `scheda attiva iniziale inesistente: "${id}" (gruppo ${groupId})`);
}

const docked = new Set(dockGroups.flatMap((g) => g.tabs || []));
const missingFromDock = Object.keys(panelDefs).filter((id) => !docked.has(id));
if (missingFromDock.length) warnings.push(`pannelli definiti ma in nessun gruppo del dock: ${missingFromDock.join(", ")}`);
const duplicateTab = [...docked].filter((id) => dockGroups.filter((g) => (g.tabs || []).includes(id)).length > 1);
if (duplicateTab.length) warnings.push(`pannelli elencati in più gruppi: ${duplicateTab.join(", ")}`);

/* ------------------------------------------------ barre opzioni e strumenti */

for (const id of Object.keys(optionBars)) {
  if (id.startsWith("_")) continue;
  say(allTools.some((t) => t.id === id), `barra opzioni orfana: "${id}" non è uno strumento`);
}
const noBar = toolSlots.filter((s) => optionBars[s.id] === undefined).map((s) => s.id);
if (noBar.length) warnings.push(`slot senza barra opzioni dedicata (usano _default): ${noBar.join(", ")}`);

/* ------------------------------------------------------------------ icone */

const sprite = readFileSync(join(root, "assets/icons.svg"), "utf8");
const symbolIds = new Set([...sprite.matchAll(/<symbol[^>]*\bid="([^"]+)"/g)].map((m) => m[1]));
const inlineSrc = readFileSync(join(root, "js/icons.js"), "utf8");

// Si leggono TUTTI i file js/ invece di una lista scritta a mano: una lista
// dimentica sempre il file appena aggiunto (è già successo con i dati dei
// dialoghi, che usavano due icone assenti dallo sprite).
const files = readdirSync(join(root, "js"), { recursive: true })
  .map((p) => String(p).replace(/\\/g, "/"))
  .filter((p) => p.endsWith(".js"))
  .map((p) => "js/" + p);
const used = new Set();
for (const rel of files) {
  const src = readFileSync(join(root, rel), "utf8");
  for (const m of src.matchAll(/["'](i-[a-z0-9-]+)["']/g)) used.add(m[1]);
  for (const m of src.matchAll(/icon\(\s*["'](i-[a-z0-9-]+)["']/g)) used.add(m[1]);
}
const missingIcons = [...used].filter((name) => !symbolIds.has(name) && !inlineSrc.includes(name));
for (const name of missingIcons) problems.push(`icona mancante: ${name} (né in assets/icons.svg né in js/icons.js)`);

/* ------------------------------------------------------------------ report */

const pad = (s, n) => String(s).padEnd(n);
console.log("");
console.log("  Controllo dati Fotox");
console.log("  " + "-".repeat(46));
console.log("  " + pad("menu", 26) + pad(stats.menus, 6));
console.log("  " + pad("voci di primo livello", 26) + pad(stats.topLevel, 6));
console.log("  " + pad("profondità massima", 26) + pad(stats.deepest, 6));
console.log("  " + pad("comandi (foglie)", 26) + pad(stats.commands, 6));
console.log("  " + pad("sottomenu", 26) + pad(stats.submenus, 6));
console.log("  " + pad("voci disabilitate", 26) + pad(stats.disabled, 6));
console.log("  " + pad("voci con spunta", 26) + pad(stats.checkable, 6));
console.log("  " + pad("voci con scorciatoia", 26) + pad(stats.withShortcut, 6));
console.log("  " + pad("finestre definite", 26) + pad(Object.keys(dialogs).length, 6));
console.log("  " + pad("di cui raggiungibili", 26) + pad(stats.reachable, 6));
console.log("  " + pad("pannelli definiti", 26) + pad(Object.keys(panelDefs).length, 6));
console.log("  " + pad("gruppi del dock", 26) + pad(dockGroups.length, 6));
console.log("  " + pad("schede visibili all'avvio", 26) + pad(Object.keys(initialActiveTab).length * 3, 6));
console.log("  " + pad("slot strumento", 26) + pad(toolSlots.length, 6));
console.log("  " + pad("strumenti totali", 26) + pad(allTools.length, 6));
console.log("  " + pad("simboli nello sprite", 26) + pad(symbolIds.size, 6));
console.log("  " + pad("icone usate", 26) + pad(used.size, 6));
console.log("  " + pad("comandi con gestore", 26) + pad(stats.dedicated, 6));
console.log("  " + pad("comandi su fallback", 26) + pad(stats.fallback, 6));
console.log("  " + pad("prefissi su fallback", 26) + pad(stats.fallbackPrefixes || "—", 6));
console.log("");

for (const w of warnings) console.log("  ~ " + w);
if (warnings.length) console.log("");

if (problems.length) {
  console.log(`  ✗ ${problems.length} problemi:`);
  for (const p of problems) console.log("    - " + p);
  console.log("");
  process.exit(1);
}
console.log("  ✓ nessun problema: ogni azione, pannello, strumento e icona esiste.");
console.log("");
