# How to build a milestone — recipes

Companion to the task cards of M3–M6. The cards say **what** to build; this
file says **how** the recurring pieces are built in this code base, so every
card does them the same way. Read `AGENTS.md` and `docs/ARCHITECTURE.md`
first; this file does not repeat them.

The small details that are easy to get wrong (rounding, premultiplied
averaging, negative tile coordinates, spacing, footers, LUT coordinates…)
have reference sketches in `SNIPPETS.md` — read it before the first card.

Each recipe names the files to touch, in order, and the tests that prove it.
A card that says "recipe R3" means: follow R3, then do the card's specific part.

---

## R0 — The order of work inside a milestone

1. **Decisions first.** Every card file starts with a `T00` card listing the
   choices only Rob can make (new crates, formats, behaviour that differs from
   Photoshop). Nothing that depends on an open decision is coded before the
   decision is written in `docs/DECISIONS.md` (next free number, same commit as
   the first code that uses it).
2. **Core before UI.** Data model + command + CPU reference + tests (headless,
   `cargo test`) → engine wiring (job, protocol) → UI → GPU/performance.
   A feature is usable from `fotox-cli` or a test before it has a button.
3. **One card = one branch = one report** (`AGENTS.md` §1, §5).
4. **Acceptance card last**: the scenarios of `PERFORMANCE.md` for the
   milestone, measured in `--release` on the reference machine, rows in
   `bench/results.csv`, plus the Photoshop comparison where the card asks.

## R1 — A new `Command`

Every document change is a `Command` (`crates/fx-core/src/command.rs`, D-010).

1. Add the variant to `enum Command` with a doc comment ending in the
   milestone (`… M4`). Fields are plain serde data; name layers with
   `LayerRef`, never with indices. **Variant names are part of saved macros:
   never rename one.**
2. Write `fn <name>(doc: &mut Document, …) -> Result<CommandEffect, CommandError>`
   below the others: **validate everything first, then mutate** (all or
   nothing). Locks: `locked_pixels` refuses pixel changes, `locked_position`
   refuses moves/transforms → `CommandError::Locked`.
3. Fill `CommandEffect` exactly: `label` (the History panel text — use
   Photoshop's wording: "Gaussian Blur", "Free Transform", "Brush Tool"),
   `pixels_changed`, `props_changed`, `structure_changed`.
4. Pixel work inside `apply` runs **tile by tile on rayon** and is committed
   with one slot write per tile (`TiledImage::put_buffer`, which stores
   uniform tiles as `Empty`/`Solid`). No buffer larger than a tile + apron.
5. Add the arm in `Command::apply`. (The `other => todo!()` arm disappears when
   the last variant is implemented — before that, a variant listed there must
   not be reachable from the UI.)
6. Mirror the JSON in `docs/PROTOCOL.md` §4 if the UI sends it.
7. Tests in `command.rs`'s test module: success, every error, and an
   undo/redo round trip through `History` (`execute`, `undo`, `redo`, compare
   documents — tile tables compared by `TileSlot::same_as` or by pixels).
   Plus: `serde_json` round trip of the command itself.

### R1a — Commands that need `fx-ops` or `fx-render` (filters, strokes, transforms, merge)

`Command::apply` lives in `fx-core`, but the pixel algorithms live in crates
that depend on `fx-core` (`fx-ops` for filters/brushes/resampling,
`fx-render` for compositing). The dependency is inverted with a trait that
`fx-core` defines and the engine implements:

```rust
// fx-core/src/ops.rs — added by the first card that needs it (M4-T05)
pub trait PixelOps: Send + Sync {
    /// The layer image after `filter`, limited to `selection` (M5) if given.
    fn filter(&self, image: &TiledImage, filter: &FilterParams, selection: Option<&TiledImage>,
              canvas: (u32, u32), store: &TileStore) -> Result<TiledImage, CommandError>;
    /// The composite of `layers` (isolated, level 0) as one pixel image (merge, flatten, stamp).
    fn composite(&self, doc: &Document, layers: &[LayerId], background: Option<[u16; 4]>,
                 store: &TileStore) -> Result<TiledImage, CommandError>;
    // M5: fn stroke(…), fn fill(…); M6: fn resample(…) — one method per family.
}
pub struct CommandContext<'a> {
    pub tiles: &'a TileStore,
    /// `None` in `fx-core`'s own tests: commands that need it return `NotAllowed`.
    pub ops: Option<&'a dyn PixelOps>,
}
```

`fx-engine/src/ops.rs` implements it with `fx-ops` and the compositor. So
every command — including a replayed macro — goes through `Command::apply`;
the engine never bypasses it for correctness, only for *scheduling* (R1b).
Tests of these commands live in `fx-engine` (where an implementation exists)
or use a small fake `PixelOps` in `fx-core`.

### R1b — Commands whose result is computed live (strokes, filters, transforms)

A brush stroke is rendered while the pen moves; a filter's level-0 result is
computed in the background after OK. The command still exists (for undo and
macros) but the engine already has the result:

* Add to `fx-core::History`:
  `pub fn record(&mut self, before: Document, label: String)` — pushes the
  given *before* snapshot as an undo step without applying anything (the
  caller has already put the new state in `doc`). Same 50-step limit, clears
  redo.
* The engine keeps `before = doc.clone()` when the live operation starts,
  swaps in the computed layer when it ends, then calls `record`.
* **Spec test (mandatory):** replaying the command with `Command::apply` on
  `before` gives exactly the same pixels as the live path. This is what makes
  macros (M15) trustworthy. Live and replay must therefore call the **same**
  pixel function (e.g. `fx_ops::brush::render_dabs`) with the same inputs;
  only the scheduling differs.

## R2 — An engine job with progress (import, export, save, filter final)

Pattern already used by `open`, `export` and `load_b3` in
`crates/fx-engine/src/engine.rs`:

1. On the engine thread: take what the job needs as **snapshots** (`Document`
   clone, `Arc<TileStore>`), allocate `task = self.next_task += 1`, send
   `EngineToUi::Progress { task, label, fraction: 0.0 }`.
2. `std::thread::Builder::new().name(format!("<job>-{task}")).spawn(…)`; inside,
   use rayon for the parallel part. Report progress through
   `Internal::Progress` (throttle to ≥ 1 % steps). Cancellation: a
   `progress` callback returning `false` (see `fx_io::Progress`).
3. Finish with an `Internal::<JobDone> { task, …, result }` variant. The
   engine thread handles it in `fn internal`: `ProgressDone`, then apply the
   result (as a command via R1b, or a new document), `Toast` on success,
   `Error { text }` on failure (never a panic, never silence).
4. If spawning fails: `ProgressDone` + `Error` immediately.
5. A job that changes a layer holds that layer "busy": further commands that
   touch the layer wait in a per-document queue (add
   `OpenDoc::pending: VecDeque<Command>` with the first job that needs it) and
   run when the job commits. Commands on other layers run immediately.

## R3 — A new protocol message

1. Add the variant to `UiToEngine` or `EngineToUi` in
   `crates/fx-protocol/src/lib.rs` (serde `type` tag = snake_case name). New
   fields on existing messages are `#[serde(default)]` so old UIs still parse.
2. Mirror the name in `ui/js/native/protocol.js` (`UI.*` / `ENGINE.*`).
3. Answer it in the mock engine `ui/js/native/mock-engine.js` (a plausible
   fake), so the UI keeps working in a browser (`AGENTS.md` §3.12).
4. Document it in `docs/PROTOCOL.md` §4/§5 with one example in §6.
5. Test: a serde round trip in `fx-protocol`'s tests.
6. Pixels never travel over the channel. Small binary things (thumbnails,
   histograms, brush-tip previews) use a binary frame (`encode_binary`).

## R4 — A dialog that edits live (adjustments, filters, image size…)

The dialog engine supports live editing since M2 (`ui/js/dialogs.js`):

```js
openDialog("gaussian-blur", {
  title, values: { "Radius:": 4 },
  onChange: (values, dialog) => …,   // every slider / menu / checkbox / curve change
  onOk: (values) => …,
  onCancel: () => …,                 // Cancel, ×, Escape, click outside
});
```

* Definitions stay pure data in `ui/js/data/dialogs.js`. Use `rng()`
  (slider + number box) for every value the engine reads: `readValues` reads
  sliders, menus (`sel`), checkboxes and the curve editor, by label. Plain
  `num()` boxes are not read.
* `onChange` sends the **preview** message (e.g. `filter_preview`), `onOk` the
  command, `onCancel` a `preview_cancel`. The *Preview* checkbox off = send the
  cancel message but keep the dialog open.
* Throttle nothing in the UI: the engine keeps only the latest preview request
  per document (it drops stale ones), so dragging a slider cannot queue work.

## R5 — A tool that uses the pointer in the viewport

Pointer events over the viewport reach the engine directly
(`EngineInput::Pointer(PointerInput)`, `ARCHITECTURE.md` §2.2) with pressure,
tilt and a microsecond timestamp.

1. The active tool id reaches the engine as the action `tool:<id>`
   (`ViewState::tool`). **Every** tool change in the UI must send it — the
   toolbar, the flyouts and the single-key shortcuts (see M5-T01).
2. Tool options (size, hardness, mode, opacity, tolerance…) reach the engine
   as `UiToEngine::ToolOptions { tool, options }` (JSON object keyed like the
   option bar's field `text`, M5-T01) and colours as `SetColors { fg, bg }`.
3. In `fx-engine`, a tool is a struct implementing
   ```rust
   pub trait Tool {
       /// Pointer event in document coordinates (already inverse-transformed).
       fn pointer(&mut self, ctx: &mut ToolContext, event: &DocPointer) -> ToolResult;
       /// Overlay to draw (marching ants, brush outline, handles), in document space.
       fn overlay(&self) -> Option<Overlay>;
       fn cursor(&self, modifiers: Modifiers) -> CursorShape;
       /// Enter/Escape while the tool has a pending operation (crop box, transform).
       fn key(&mut self, ctx: &mut ToolContext, key: ToolKey) -> ToolResult { ToolResult::default() }
   }
   ```
   in `crates/fx-engine/src/tools/<tool>.rs`; `tools/mod.rs` maps ids to tools.
   The view's pan/zoom handling (`view.rs`) keeps priority: Space, the hand
   tool and the middle button never reach a tool.
4. `ToolResult` says what changed: a command to execute (R1), a live-preview
   update, an overlay change, a cursor change.
5. Overlays are drawn by the render thread, not by the UI (M5-T02): the UI
   never knows document coordinates at 60 fps.
6. Tests: drive the tool with synthetic `DocPointer` sequences
   (down/move/up with modifiers) and assert the command it produces.

## R6 — Lazily rendered derived tiles (mips, vector layers, layer styles, filter previews)

The model is the mip pyramid (`fx-engine/src/mips.rs`,
`fx_render::build_program` → `MipRequest`):

* The tiles live in a `TiledImage` as `TileClass::Derived` (evictable, never
  written to scratch, D-008).
* `build_program` finds a missing/dirty tile → returns a request instead of a
  program; the render thread draws the parent level meanwhile and sends the
  request to the engine; the engine computes it on rayon and commits it with
  `set_derived_slot`; the next frame uses it.
* A new kind of derived tile (vector render, style effect, preview) adds a
  request variant next to `MipRequest` and a compute function next to
  `ensure_mip`; everything else — never blocking a frame, eviction, retries —
  works the same way.
* The key of the program (`hash_ops`) must include whatever identifies the
  derived content (e.g. the vector layer's content hash + level), so cached
  composites refresh by themselves.

## R7 — A new adjustment kind (CPU reference + GPU)

1. Variant in `fx_core::Adjustment` (serde, M-number in the doc comment),
   default parameters = identity.
2. Per-channel function of the value only → bake it as a LUT in
   `fx_render::adjust::bake`. Otherwise add an `AdjustKind` variant, a CPU
   implementation in `reference.rs`, and a `K_ADJUST_*` kind in
   `gpu/composite.wgsl` + its params encoding in `gpu/compositor.rs`.
3. Formulas that copy Photoshop's are marked `VERIFY` (checked in M14, PSD import) with the
   source of the formula in a comment.
4. Tests: identity parameters leave pixels unchanged; a few hand-computed
   values; **GPU = CPU within 1e-3** (`gpu::tests`, skipped without adapter).
5. UI: an entry in `NEW_ADJUSTMENTS` and a dialog mapping in
   `ADJUSTMENT_DIALOGS` / `PER_CHANNEL_DIALOGS` (`ui/js/native/layers-panel.js`);
   `LayerInfo.adjustment` already carries the parameters.

## R8 — A pixel filter (`fx-ops`)

1. `crates/fx-ops/src/<filter>.rs`: a pure function on a *neighbourhood*:
   ```rust
   /// `input`: the tile and its apron, straight RGBA f32, row-major,
   /// (256 + 2·apron)² pixels. Writes the 256² result into `out`.
   pub fn run(params: &Params, level_scale: f32, input: &Neighbourhood, out: &mut [[f32; 4]]);
   pub fn apron(params: &Params, level_scale: f32) -> u32;
   ```
   `level_scale = 2^-level` scales every distance parameter (preview at a mip
   level, `ARCHITECTURE.md` §4.5). Colour math on premultiplied values where
   pixels are averaged (blur), straight where they are not.
2. The tile driver (`fx_ops::filter::apply_tiles`, M4-T05) gathers the
   neighbourhood from the store (empty tiles = transparent, no fetch), runs
   the function on rayon, converts back to u8/u16 with rounding, and writes
   with `put_buffer`. The set of output tiles = non-empty source tiles grown
   by the apron (a blur spreads into empty tiles next to content).
3. Tests: a CPU reference computed naively on a small whole image equals the
   tiled result (tile seams are the classic bug: test an image of 3×3 tiles
   with content across the seams); identity parameters are exact; 8- and
   16-bit.
4. Register it in `FilterParams` (M4-T05), give its dialog `rng()` fields.

## R9 — Measuring

* Engine-side numbers: a `fotox-cli bench <scenario>` that appends to
  `bench/results.csv` (`crates/fx-cli/src/bench.rs`), release build.
* In-app numbers: the frame-time overlay (Ctrl+Alt+F) and the `status`
  statistics; for latency (S9) the engine stamps input time → the render
  thread reports input-to-present in `Status` (M5-T09).
* Every performance card states its target and puts the measured number and
  the command used in its report. A target missed after measuring → stop and
  report (`AGENTS.md` §6), do not ship a silent regression.

## R10 — Things that are easy to get wrong here

* **Straight vs premultiplied.** Storage is straight (D-005). Anything that
  averages pixels (blur, resample, mips, dab accumulation) works on
  premultiplied values, then un-premultiplies. Test with a half-transparent
  red next to transparent black: no dark fringe.
* **Tile seams.** Every neighbourhood operation needs the apron; test content
  that crosses tile edges and the right/bottom partial tiles.
* **Offsets.** Pixel layers have an integer `offset`; a layer's tile (tx, ty)
  covers document pixels `offset + (tx·256, ty·256)`. Tools work in document
  coordinates and convert per layer.
* **Undo keeps tiles alive.** Never mutate a `TileBuffer` that is in a
  `TiledImage`: copy (`(*store.get(h)?).clone()`), modify, `put_buffer`.
* **The render thread never waits.** No `TileStore::get` on it, no locks held
  across I/O, only `try_get_hot` (`ARCHITECTURE.md` §3.2).
* **Document-sized buffers are forbidden** — also for masks, selections,
  flood-fill "visited" sets and undo data: use tiles.
