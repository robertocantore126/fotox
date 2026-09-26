# HARDEN — test and debug everything written in fast mode

> Led by Claude when Rob says so (D-057). Can run once at the end, or on one
> milestone at a time. The goal: find the logic bugs, write the tests the cards
> deferred, and bring the code back up to the old rules
> (`git show c4c133c:AGENTS.md`).

Order: H1 → H2 → H3 → H4 → H5 → H6 → H7. Commit each step separately.

## H1 — Green build

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo test`: run the whole suite and fix what fails. Every fix names the
  bug in the commit message (not "fix tests").

## H2 — Read the log

- Go through `docs/reports/LOG.md` card by card:
  - "Skipped" becomes a list of things to finish or to cut (Rob decides);
  - "VERIFY" joins the Photoshop comparison in H6.
- Every T00 point marked "fast default" goes to Rob for a yes or a no.

## H3 — Shortcuts

- `rg "FAST:"`: each hit is fixed, or kept with a reason that Rob accepts.
- Every `unwrap()`, `todo!()` or silent fallback outside tests becomes a real
  error or an `expect("why")`.

## H4 — Deferred tests

- Every `**Tests**` paragraph in the fast-mode cards (M6-T07 onward) becomes
  real tests.
- Every `#[ignore = "<task>"]` spec test is un-ignored with its assertions
  unchanged.
- GPU = CPU checks for every new GPU operation.
- Where a test fails, the code is wrong until proven otherwise.

## H5 — Logic review

- Claude reads each milestone's diff (`git diff <previous milestone>..m<N>`)
  looking for logic bugs:
  - edge tiles, 8 vs 16-bit, empty documents and zero sizes;
  - undo/redo round trips;
  - save → reopen;
  - lock order and threads;
  - buffers proportional to the document size.
- Each bug gets a failing test first, then the fix.

## H6 — Performance and acceptance

- The exit-criteria tables (S16 and later) are measured in release on the
  reference machine, and misses are fixed or reported.
- The `T10`/acceptance cards: Rob compares with Photoshop, VERIFY list in hand.

## H7 — Documentation

- `pub` items get doc comments.
- `ARCHITECTURE.md`, `PROTOCOL.md` and `FILE_FORMAT.md` are updated to match
  the code.
- AGENTS.md goes back to the full process if Rob wants it.
