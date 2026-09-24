# Instructions for the coding agent

You implement Fotox **one task card at a time** (`docs/tasks/M*.md`). Claude
reviews every task before the next one starts; Rob owns the decisions. You do
not need to design the architecture — it is written down. You need to follow
it precisely, write solid code, and report honestly.

## 1. Before you start a task

1. Read, in this order: this file, `docs/ARCHITECTURE.md`, the task card, and
   every file the card mentions (including doc comments in the code — they
   are part of the spec).
2. Check `docs/DECISIONS.md`. Decisions are not re-opened inside a task.
3. Create a branch: `task/<ID>-<short-name>` (e.g. `task/M0-T03-shell`) from
   the latest `main`.
4. Write a short plan (5–15 lines) at the top of your report file
   `docs/reports/<ID>.md` (copy `docs/reports/_TEMPLATE.md`) **before** coding.

## 2. While you work

* Code marked "Written by Claude" (tile store, `fx-render` programs,
  compositor, planner, viewport pass — see `docs/reports/CLAUDE-core.md`)
  is finished and tested. Use its APIs; do not restructure it. A bug there:
  write a failing test, make the smallest fix, explain it in your report.

* Implement **only** the task. Useful ideas outside it go in the report under
  "Suggestions", not in the code.
* Keep the public APIs already defined in the skeleton. If one must change,
  change it minimally and explain why in the report.
* Commit in small logical steps. Message format: `M0-T03: port winit app handler`.
* Run the checks (§4) before every commit you consider "done".

## 3. Hard rules

1. **No buffer proportional to the document size.** Bands and tiles only.
2. **The render thread never blocks** on disk, on `TileStore::get`, or on the engine.
3. **Spec tests are the spec.** Tests marked `#[ignore = "<task>"]` must be
   un-ignored and pass *without changing their assertions*. If one looks
   wrong, stop and ask (§6).
4. **Dependencies:** only crates listed in the root `Cargo.toml`
   `[workspace.dependencies]`, used as `{ workspace = true }`. A new crate
   needs a line in `docs/DECISIONS.md` in the same commit.
5. **Vendored code** (`vendor/graphite/`) is not refactored. A necessary
   change is marked `// FOTOX PATCH: <why>` and listed in
   `vendor/graphite/VENDORED.md`.
6. **`reference/`** is read-only study material. Never compile it, never edit it.
7. **No `unsafe`** without a `// SAFETY:` comment explaining why it is sound,
   and a mention in the report.
8. **No `unwrap()`/`expect()` in non-test code** unless it is a true
   invariant; then use `expect("why this cannot fail")`.
9. **No `todo!()`, `unimplemented!()`, commented-out code or silent
   fallbacks** left inside the task's scope. A case you cannot handle returns
   a clear error.
10. **No `println!`** outside `fx-cli` output; use `tracing`.
11. **Docs:** every `pub` item has a doc comment. If behaviour differs from a
    doc in `docs/`, the code is wrong — or stop and ask.
12. **UI code** (`ui/`): plain ES modules, no frameworks, no bundler, no npm
    dependencies. Follow the existing patterns (`el.js` `h()`, `state.js`
    emitter, data-driven menus/dialogs). The UI must keep working in a plain
    browser with the mock engine. New comments in English.
13. Never commit `target/`, `third_party/`, `bench/data/`, scratch files.
    Always commit `Cargo.lock`.

## 4. Checks (all must pass)

```bash
cargo fmt -p fx-tiles -p fx-core -p fx-protocol -p fx-color -p fx-io -p fx-render -p fx-ops -p fx-engine -p fx-cli -p fx-app -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test                     # GPU tests skip themselves if no adapter exists
cargo build -p fx-app          # when the task touches fx-app, vendor/ or ui/
node ui/tools/check-data.mjs   # when the task touches ui/js/data
```

Performance tasks: measure in `--release` on the reference machine and put
the numbers in the report.

## 5. When you finish

1. Fill in the report (`docs/reports/<ID>.md`): what you did, files changed,
   how you verified it (commands + results), measurements, deviations from
   the card, open problems, suggestions.
2. Tick the task's checkboxes if the card has any.
3. Push the branch. **Do not merge and do not start the next task.** Tell Rob
   the task is ready for review.

## 6. When you are stuck or something seems wrong

Stop — do not improvise around the spec — when:

* the spec (card, docs, spec test) seems wrong or contradictory;
* two different approaches have failed;
* the task needs an architectural change or a new dependency not obviously
  covered by the card;
* a performance target looks unreachable after measuring.

Write the problem in the report under **Blocked**: what you tried, what
happened (exact errors), what you think the options are. Push, and tell Rob.
Claude will answer or take the task over.

## 7. Useful facts

* Build the app: `cargo xtask run` (after M0-T02). CEF downloads on the first
  `cargo build -p fx-app` into `third_party/cef/`.
* UI in dev builds is read from `./ui` at runtime: UI changes need only an
  app restart, not a rebuild.
* Debug the UI inside the app: set `GRAPHITE_BROWSER_DEBUG_PORT=9222`, open
  `chrome://inspect` in Chrome.
* Logs: `RUST_LOG=fx_engine=debug,fx_app=debug`; CEF logs: `GRAPHITE_BROWSER_LOG=info`.
* Browser-only UI work: `cd ui && python tools/serve.py`.
