# Fotox

A Photoshop-style image editor built for **huge documents**: 30 000 × 30 000
px, 16-bit, hundreds of layers, on a 16 GB machine — and faster than
Photoshop at opening, navigating, layer work and saving.

The one rule: **the cost of any operation depends on what is on screen and
what changed, never on the size of the document.** How: `docs/ARCHITECTURE.md`.

Status: **M0 (shell) not started.** The hard engine core is implemented and
tested (tile store with RAM/compressed/disk tiers, GPU compositor with all
blend modes, groups, clipping, masks, adjustments, caches, frame planner —
`docs/reports/CLAUDE-core.md`); the rest has API skeletons and spec tests;
the UI is the complete Fotox interface mock. See `docs/ROADMAP.md`.

## Layout

```
AGENTS.md              rules for the coding agent — read first when coding
Cargo.toml             workspace + the approved dependency list
crates/
  fx-tiles/            tile store: immutable tiles, RAM/LZ4/scratch tiers, mip pyramid
  fx-core/             document model, layers, blend modes, commands, undo history
  fx-protocol/         UI ↔ engine messages
  fx-render/           GPU: viewport math, tile atlas, lazy compositor, blend shaders
  fx-io/               TIFF/PNG/JPEG import/export, native .fxd, later PSD
  fx-color/            colour management, soft proof, CMYK export
  fx-ops/              brushes, retouching, filters
  fx-engine/           the headless application core (threads, jobs, documents)
  fx-app/              the desktop app: window, CEF UI, viewport composite, input
  fx-cli/              headless tool: test images, benchmarks, batch
vendor/graphite/       CEF-in-wgpu shell from Graphite (MIT/Apache-2.0), lightly patched
reference/             Graphite desktop code for study/porting — not compiled
ui/                    the Fotox interface (HTML/CSS/JS, no build step)
docs/
  ARCHITECTURE.md      how it works and why
  DECISIONS.md         decision log
  ROADMAP.md           milestones
  tasks/M*.md          task cards for the coding agent
  PERFORMANCE.md       targets, budgets, benchmark method
  BLEND_MODES.md       compositing spec
  PROTOCOL.md          UI ↔ engine messages
  FILE_FORMAT.md       native .fxd format (draft)
  GRAPHITE.md          what we took from Graphite and how to port it
  REVIEW.md            review process
  reports/ reviews/    per-task reports (agent) and reviews (Claude)
bench/                 benchmark results (data files are git-ignored)
```

## Build and run (Windows)

Prerequisites: Rust (rustup, MSVC toolchain), Visual Studio 2022 Build
Tools with "Desktop development with C++", CMake, Ninja, Git.

```bash
cargo test                 # engine crates, no CEF needed
cargo build -p fx-app      # first run downloads CEF (~300 MB) into third_party/cef
cargo xtask run            # bundle + launch (available after task M0-T02)
```

UI only, in a browser: `cd ui && python tools/serve.py`.

## Licence

Fotox code: personal project, no licence chosen yet. Vendored Graphite code:
MIT OR Apache-2.0, see `vendor/graphite/`.
