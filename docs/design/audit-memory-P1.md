# P1: memory admission and GPU ceiling (implemented incrementally, untested)

Current code: fx-tiles/src/store.rs:184 fixes hot/warm at 5/3 GiB; engine/lib.rs:334 applies only hot preference; store.rs:726 snapshots/sorts all weak entries and skips held buffers. render workers use shared rayon for misses, so blanket insert waits would freeze render. gpu/atlas.rs grows pages lazily after the first prompt; gpu/compositor.rs still reserves its separate fixed output slots.

A1: query physical RAM once via GlobalMemoryStatusEx; hot 25%, warm 10%, explicit preferences win. FOTOX_OLD_BUDGETS read once restores fixed 5/3 defaults (existing hot preference still wins, matching original path). Send runtime-only effective budgets/total to Preferences, avoiding stale UI defaults.

A2: opt-in RAII thread-local worker scope, fresh timeout-log flag each job. Condvar notified after trim. Over 125% combined hot+warm ceiling waits only on opted-in thread; at most two seconds/insert then continues. Shared rayon stays unopted. FOTOX_NO_BACKPRESSURE preserves immediate insert. Scope only dedicated pixel/derived/open/save/export and recovery workers; never engine/render/thumbnail.

A3: weak candidate clock queue with access epoch; skip recently touched once, held tiles as before. Process a bounded pass and stop at targets. No registry-wide sort; preserve authoritative copies and skip lock contention. Off-switch FOTOX_OLD_BUDGETS also selects original sorted trim for exact old comparison.

A4: half VRAM ceiling includes output texture cache; allocate output at min(old 1024 slots, ceiling/4), atlas remainder. Both texture overhead and placeholder accounted; report configured effective ceiling. OLD_BUDGETS restores first-prompt GPU path with separate 0.5GiB cache.

Invariants: no disk or engine wait on render; no authoritative loss; backpressure cannot wait holding store registry/copy locks; bound metadata by live handles; 8/16-bit accounting unchanged. Shutdown/trim must wake waiters. No RAM-sized pixel buffers.

Risks: pinned inputs/undo/derived held buffers cannot be evicted; 2s escape deliberately permits overshoot. Dedicated-thread opt-in does not propagate into shared rayon inner work, so many algorithms still cannot be throttled safely at every insert. RAM detection fallback 16GiB; VRAM vendor/device match ambiguity retained. Clock recency is approximate. Total memory also contains decoder, caches and application overhead. Compile-only validation cannot establish latency or memory bounds.
