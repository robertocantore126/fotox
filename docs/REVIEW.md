# Review process

> **Suspended from M6-T07 (D-057, fast mode).** There is no review per task.
> Cards are committed straight onto the milestone branch, and this checklist
> is used once, in `docs/tasks/HARDEN.md` step H5.

Every task is reviewed before it is merged and before the next one starts.

## Flow

1. The agent pushes `task/<ID>-…` and a filled `docs/reports/<ID>.md`.
2. Rob asks Claude: *"review <ID>"*.
3. Claude reads the report, the task card and the diff
   (`git diff main...task/<ID>-…`), runs the checks from `AGENTS.md` §4 where
   possible, and answers in `docs/reviews/<ID>.md` with one verdict:
   * **Approved** — Rob merges (`git merge --no-ff`), next task starts.
   * **Changes requested** — numbered list, each with `file:line`, severity
     and the expected fix. The agent fixes on the same branch and appends a
     "Round 2" section to its report.
   * **Taken over** — the task is too hard or went in circles; Claude
     implements it (on the same branch) and explains the solution in the review.
4. After two "changes requested" rounds on the same issue, Claude takes over
   that part.

## Severity

* **Blocker** — wrong results, data loss, crash, deadlock, a hard rule from
  `AGENTS.md` §3 broken, performance target missed without explanation.
* **Major** — design drift from `ARCHITECTURE.md`, missing tests for new
  behaviour, API change not justified, error handling missing.
* **Minor** — naming, docs, small simplifications. Fixed in the same round
  but never block a merge on their own.

## Checklist Claude uses

**Correctness**
- [ ] Does exactly what the card asks; acceptance criteria demonstrably met
- [ ] Spec tests un-ignored, assertions unchanged, all green
- [ ] Edge cases: image edges (partial tiles), empty/solid tiles, 8 vs 16-bit,
      zero-size inputs, cancellation, errors propagated not swallowed
- [ ] Concurrency: no lock held across I/O/compression/GPU waits; lock
      order consistent; no busy loops; shutdown path joins threads

**The one rule** (cost ∝ screen + changes)
- [ ] No allocation or loop proportional to document size
- [ ] Render thread never blocks; work is scheduled, not done inline
- [ ] Derived data never written to scratch

**Code quality**
- [ ] Follows existing structure and names; no gratuitous refactors
- [ ] `pub` items documented; comments explain *why*
- [ ] No `unwrap` in non-test code, no `unsafe` without `SAFETY`, no `todo!`
- [ ] Dependencies only from the workspace list; vendored code untouched or
      patched per rules

**Performance tasks**
- [ ] Numbers measured in release on the reference machine, recorded
- [ ] Method is sound (warm vs cold cache stated, median of ≥ 3 runs)

**UI tasks**
- [ ] Works in the app **and** in a plain browser (mock engine)
- [ ] No pixels computed in JS; protocol matches `PROTOCOL.md` and `fx-protocol`
