# Instructions for the coding agent — FAST MODE

**From M6-T07 on, Fotox is built in fast mode (D-057): write all the code
first, test and debug later.** Speed matters more than polish. Bugs are
expected; they are found and fixed in one hardening phase at the end
(`docs/tasks/HARDEN.md`), which Claude leads. Your job is to get every card's
feature *written and wired up*, then move straight on to the next card.

Wherever a card, `HOWTO.md` or `REVIEW.md` asks for any of the following,
**skip it now**. It is deferred to HARDEN:

* `**Tests**` paragraphs, spec tests, `#[ignore = "<task>"]` tests to un-ignore;
* performance measurements, exit-criteria tables, `T10`/acceptance cards;
* per-task reports, the review round, clippy, "every `pub` item documented";
* comparisons with Photoshop (write `VERIFY` in a comment and keep going).

## 1. Workflow

1. Read the milestone card file (`docs/tasks/M*.md`) and skim the files it
   names. `docs/ARCHITECTURE.md` and `HOWTO.md` recipes are there to copy
   from, not to study.
2. Work directly on the milestone branch (`m6`, `m7`, …). No task branches.
3. One commit per card, message `M6-T07: text layers` (several commits are
   fine for a big card). Push after each card as a backup.
4. Add a few lines to `docs/reports/LOG.md` (format at the top of that file).
5. Start the next card immediately. Do not wait for review.
6. When the milestone's cards are done, merge `m<N>` into `main`
   (`git merge --no-ff`), push, and start `m<N+1>` from `main`.

**T00 decision cards:** take the card's recommendation for every point,
record it in `docs/DECISIONS.md` marked "fast default — Rob may overturn",
and continue. Stop and ask Rob only when a point has no recommendation, or
involves a licence choice, money, or a download over 1 GB.

**Stuck?** After two failed attempts at a piece, leave it out, write it under
"Skipped" in `LOG.md`, and go on with the rest of the card. Do not stop the
whole run for one piece.

## 2. The only rules

1. **It builds and the app starts.** Before each commit:
   ```bash
   cargo check --workspace --all-targets   # tests must still *compile*
   cargo build -p fx-app                   # when fx-app, vendor/ or ui/ changed
   ```
   If an old test no longer compiles because a signature changed, fix the
   call, the fastest way. You do not need to run `cargo test`.
2. **Keep the architecture** — breaking these means a rewrite later, not a bug fix:
   no buffer proportional to the document size (bands and tiles only), and
   the render thread never blocks on disk, on `TileStore::get` or on the engine.
3. **Do not break or rewrite what already works.** Add; do not refactor earlier
   milestones' code. If you must change a shared API, change it minimally.
4. **Mark every shortcut** with a `// FAST: <what is missing or hacky>` comment
   (e.g. `// FAST: ignores 16-bit`, `// FAST: unwrap, errors not handled`).
   HARDEN greps for them. `unwrap()`, `todo!()` for secondary cases, and
   rough code are all fine as long as they carry this mark.
5. **Vendored code** (`vendor/graphite/`): a change is marked
   `// FOTOX PATCH: <why>`, so the vendor can still be updated.
   `reference/` is read-only; never compile it.
6. Never commit `target/`, `third_party/`, `bench/data/` or scratch files.
   Commit `Cargo.lock`. A new crate is fine: add it to the root
   `[workspace.dependencies]` and put one line in `DECISIONS.md`.
7. **UI** (`ui/`): plain ES modules, no frameworks, no bundler, no npm. Copy
   the existing patterns (`el.js` `h()`, data-driven menus/dialogs). Keep the
   mock engine working enough that the UI still opens in a browser.

Running `cargo fmt` is automatic, so do it before committing. Nothing else
about style is checked now.

## 3. Useful facts

* Build the app: `cargo xtask run`. CEF downloads on the first
  `cargo build -p fx-app` into `third_party/cef/`.
* CMake must be on `PATH` for cargo (`C:\Program Files\CMake\bin`), or
  `cef-dll-sys` fails to build.
* UI in dev builds is read from `./ui` at runtime: UI changes need only an
  app restart, not a rebuild. Browser-only UI: `cd ui && python tools/serve.py`.
* Debug the UI inside the app: `GRAPHITE_BROWSER_DEBUG_PORT=9222`, then open
  `chrome://inspect` in Chrome.
* Logs: `RUST_LOG=fx_engine=debug,fotox=debug` (the shell's binary crate is
  `fotox`, not `fx_app`); CEF logs: `GRAPHITE_BROWSER_LOG=info`.
* Code by Claude (tile store, `fx-render` programs, compositor, planner,
  viewport pass) works; use its APIs rather than writing your own.

## 4. Before fast mode (M0 – M6-T06)

Those cards followed the full process (task branches, reports in
`docs/reports/<ID>.md`, reviews in `docs/reviews/`, checks with clippy and
`cargo test`). The old rules are in git history
(`git show c4c133c:AGENTS.md`) and come back in HARDEN.
