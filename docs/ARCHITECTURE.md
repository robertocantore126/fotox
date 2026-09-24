# Fotox — Architecture

Status: **v1, 2026-09-24**. This is the contract the code follows. Changing a
rule here is a decision: record it in `DECISIONS.md` in the same commit.

---

## 1. Goal and the one rule

Fotox is a Photoshop-style raster editor for **very large documents**
(reference: 30 000 × 30 000 px, 16-bit, hundreds of layers) on a
**16 GB RAM / RTX 3060 12 GB / NVMe (<100 GB free)** Windows machine. It must
eventually beat Photoshop on that machine for opening, navigating, layer work
and saving, with comparable tools and quality.

Use cases, in priority order: photo retouching, compositing / matte work,
large-format print (RGB editing + soft proof + CMYK export).

**The one rule: the cost of any operation depends on what is on screen and
what changed — never on the size of the document.**

Every design choice below serves that rule. When a change would make some
cost scale with document size, it needs a written justification.

Non-goals (for now): native CMYK/Lab editing modes, 32-bit float documents,
video/3D, cloud, a plugin API, macOS/Linux (code stays portable, only Windows
is tested).

---

## 2. Processes and threads

```
fotox.exe  (main process)
│
├─ main thread ──── winit event loop, window, input routing,
│                   final composite (viewport + UI) → swapchain      [fx-app]
├─ engine thread ── documents, commands, history, protocol           [fx-engine]
├─ render thread ── compositor, GPU tile atlas, viewport texture     [fx-render]
├─ rayon pool ───── import/export, mips, filters, (de)compression
└─ trim thread ──── keeps RAM under budget                           [fx-tiles]

fotox.exe --graphite-browser-host=…   (spawned by the vendored shell)
└─ CEF (Chromium) processes ──── renders the Fotox UI (./ui) off-screen
```

### 2.1 How a frame is made

1. CEF renders the Fotox HTML UI **off-screen** into a GPU texture. On Windows
   the texture is shared with our wgpu device through D3D11 shared handles,
   so no copy is made (`accelerated_paint` feature of the vendored shell).
2. The render thread renders the **viewport texture**: the document at the
   current zoom/pan, same size as the viewport rectangle.
3. The main thread draws one full-screen pass (ported from Graphite's
   `composite_shader.wgsl`): where the UI pixel is opaque, show the UI;
   inside the viewport rectangle, show the viewport with the UI blended on
   top (so rulers, guides overlay, brush cursor, popups over the canvas work).
4. The UI tells the shell where the viewport rectangle is
   (`ViewportBounds`, physical pixels). In the HTML it is simply a
   transparent `<div id="viewport">`.

### 2.2 How input is routed

The shell receives every winit event. For pointer events:

* inside the viewport rectangle **and** direct input enabled → sent straight
  to the engine as `fx_engine::PointerInput` (pressure and tilt from winit
  `TabletToolData`). This bypasses CEF/JS entirely: brush latency is not
  affected by the UI.
* otherwise → forwarded to CEF (`UiCommand::Input`).
* The UI disables direct input (`DirectInput { enabled: false }`) while any
  popup/menu/dialog is open, exactly like Graphite does.
* A stroke that starts in the viewport stays routed to the engine until the
  button is released, even if the pointer leaves the rectangle.

Keyboard events always go to CEF first; the UI's shortcut map (`js/shortcuts.js`)
turns them into `Action`s. Exception: Space (temporary hand tool), which the
shell tracks as a modifier for viewport input.

### 2.3 Life of an edit

```
UI click "Opacity 50%"                        (or a pen stroke in the viewport)
 → fx-protocol frame → shell → engine thread
 → Command::SetLayerProps → History::execute
 → new Document snapshot (Arc) published to the render thread
 → render thread: composite-cache keys of affected tiles changed → re-render only those
 → shell presents; engine sends `Layers` + `History` messages to the UI
```

---

## 3. Pixel storage (`fx-tiles`)

### 3.1 Tiles

* 256 × 256 pixels. Formats: `Rgba8`, `Rgba16`, `Gray8`, `Gray16`.
* RGBA is **straight (non-premultiplied)** alpha. 16-bit uses the full
  0..=65535 range (PSD's 15-bit range is converted at import/export).
* Tiles are **immutable**. Editing produces a new tile; the old one lives as
  long as a handle to it exists (undo history, a render in progress…).
* A `TileHandle` is an `Arc`: clone/drop is one atomic operation.
* Uniform tiles are never stored: `TileSlot::Empty` (transparent) and
  `TileSlot::Solid(value)` cost nothing. Every writer goes through
  `TiledImage::put_buffer`, which does this check.

### 3.2 Residency tiers

| Tier | Where | Cost to read | Used for |
| --- | --- | --- | --- |
| hot | RAM, uncompressed | 0 | tiles in use or recently used |
| warm | RAM, LZ4 | ~0.2 ms/tile | recently used, over hot budget |
| cold | scratch file, LZ4 | ~0.5–1 ms/tile (NVMe) | everything else that was written |
| backed | the opened `.fxd` file | same as cold | untouched tiles of a native document (M3) — **never copied to scratch** |
| evicted | nowhere | recompute | *derived* tiles (mips, caches) under pressure |

* `TileClass::Authoritative` tiles (real content) are never lost.
  `TileClass::Derived` tiles are dropped instead of written to disk:
  scratch space is the scarcest resource on the reference machine.
* The trim thread keeps `hot + warm` under budget, least-recently-used
  first, skipping tiles currently borrowed.
* The render thread only ever calls `try_get_hot`; missing tiles are
  requested from workers, never loaded inline.

Budgets: `PERFORMANCE.md` §2.

### 3.3 Mip pyramid

Every `TiledImage` has levels 0..N; level *n* is downscaled by 2ⁿ; the last
level fits in one tile (30 000 px → 8 levels). Level 0 is authoritative, the
rest are derived:

* writing a level-0 tile marks its ancestors dirty (cheap, stops early);
* dirty mip tiles are recomputed **lazily**: when the viewport needs them,
  or by a low-priority idle job;
* downsampling is a premultiplied 2 × 2 box filter (`downsample_2x2`), exact
  integer arithmetic, so results are reproducible.

Memory cost of mips: +33 % of level 0 at most, and they are evictable.

---

## 4. Rendering (`fx-render`)

### 4.1 Viewport

`ViewTransform { zoom, center }` + viewport size. Level to composite from:
largest *L* with 2ᴸ ≤ 1/zoom (never upsample from a coarser level when
zoomed out). Visible tiles at *L* are bounded by the screen size, not the
document size (checked by a test in `viewport.rs`).

### 4.2 Lazy tile compositing

For each visible tile `(L, tx, ty)`:

1. Build the **composite key**: for each contributing layer, its id, the
   identity of its tile slot at `(L, tx, ty)` (TileId or Solid value), its
   compositing properties, its mask slot, its adjustment parameters. Because
   tiles are immutable, the key changes exactly when the result would.
2. Key in the composite cache → draw it.
3. Otherwise make sure every source tile is in the GPU atlas, then blend
   bottom → top into an accumulator (Rgba16Float, premultiplied), in one
   compute dispatch per tile. Store in the cache, draw.

Empty layer tiles are skipped entirely, so a sparse layer costs almost nothing
where it has no content.

### 4.3 Layer offsets

A pixel layer has an integer `offset`. Its tile grid is aligned to its own
origin. When the offset is not a multiple of the tile size at level *L*, one
output tile reads up to 4 source tiles. Moving a layer therefore **never
rewrites pixels** and is instant at any size. (At levels > 0 the offset can
be fractional: bilinear sampling of that level; exact at level 0.)

### 4.4 Never block a frame

* Source tile not hot / not uploaded / mip dirty → draw the parent tile
  from level L+1 upscaled, schedule the work, redraw when it lands.
* Upload budget per frame (default 48 tiles) keeps fast pans at 60 fps.
* Work is prioritised centre-out, current level first.

### 4.5 Adjustments and filters

* **Adjustment layers** are per-pixel functions evaluated inside the
  composite shader (curves/levels baked into small 1D LUT textures). They
  cost no pixel storage and update live at any document size.
* **Destructive filters** (M4+) run tile by tile. The *preview* is computed
  at the viewed level with parameters scaled by 2⁻ᴸ (blur radius etc.) and
  shown immediately; the level-0 result is computed in the background and
  replaces the preview tile by tile. The UI never waits. Filters that are not
  scale-consistent (e.g. noise, small-radius sharpening) show an approximate
  preview when zoomed out; at ≥ 100 % the preview *is* the final result.

### 4.6 Stack split cache (M2)

While one layer is being edited, cache the composite of everything **below**
it and, when all layers above are Normal mode, of everything **above** it.
Each frame then blends 3 things instead of the whole stack.

### 4.7 Precision

* Display path: Rgba16Float premultiplied. Good enough to look at, never
  written back.
* Commit path (filters/brushes/merge that produce document pixels): f32
  compute, written back to u8/u16 exactly. The CPU reference in
  `fx-render::reference` defines correct results.

---

## 5. Documents, commands, history, automation (`fx-core`)

* `Document` has value semantics: layers are `Arc<Layer>`, pixels are tile
  handles. Cloning a document = cloning a small tree of pointers.
* **Commands are the only way to change a document.** A `Command` is plain
  serialisable data (`{"op": "set_layer_props", …}`), deterministic, all or
  nothing. It names layers with `LayerRef` (`Id`, `Active`, `Named`) so a
  recorded sequence replays on another document.
* **History = snapshots.** Before a command, the document is cloned; undo swaps
  it back. Only layers the command touched are copied, and only their changed
  tiles stay alive. Limit: 50 states (like Photoshop).
* **Automation comes for free:** a macro ("action") is the list of commands
  in the history; batch processing is `fotox-cli batch` replaying it on files
  (M8). Custom workflows = new commands + new UI panels.
* **Brush strokes** (M5) are commands too: they carry input samples
  (position, pressure, tilt, time) and brush parameters, not pixels. The
  engine renders the stroke live into preview tiles and commits at pen-up.

---

## 6. Colour

* Pixels are stored **encoded in the document profile** (not linear), and
  blend modes operate on encoded values — Photoshop's default behaviour,
  required to match its results. Formulas: `BLEND_MODES.md`.
* Display (M4): document → monitor transform baked into a 33³ 3D LUT applied
  in the final viewport shader. Until M4: sRGB assumed end to end.
* Print: RGB editing + soft proof (document → CMYK profile → monitor, same LUT
  mechanism, with gamut warning) + CMYK conversion at TIFF/PDF export (lcms2).
* 8-bit and 16-bit documents; 32-bit float is out of scope for now.

---

## 7. Files (`fx-io`)

* Import and export **stream**: one band of 256 rows at a time, never the
  whole image in memory.
* Native format `.fxd` (M3, `FILE_FORMAT.md`): tile-addressable, compressed
  per tile, opened lazily (instant open at any size), saved incrementally
  (only changed tiles are appended).
* PSD/PSB import in M7 (layers, masks, blend modes, groups, adjustment
  layers where representable); PSD export later.

---

## 8. UI (`ui/`, the Fotox mock)

* Plain HTML/CSS/ES modules, no build step, loaded by CEF from `./ui` in
  development and embedded in release builds.
* The UI **never touches document pixels**. It renders chrome, panels and
  dialogs, and talks to the engine only through `fx-protocol`
  (`ui/js/native/`).
* The mock's `canvas.js` drawing is replaced by the transparent viewport
  element; rulers and the status bar are driven by `View` messages.
* The UI still runs in a normal browser (`python tools/serve.py`) against a
  mock engine (`ui/js/native/mock-engine.js`) for fast UI work.

---

## 9. Crates and dependency rules

```
fx-tiles ◄── fx-core ◄── fx-protocol
               ▲  ▲  ▲
      fx-color ┘  │  └ fx-io, fx-ops
                  │
             fx-render (tiles, core, wgpu)
                  ▲
             fx-engine (tiles, core, protocol, render, io, ops, color)
               ▲     ▲
          fx-app     fx-cli
   (+ vendor/graphite/desktop-ui)
```

* `fx-tiles`, `fx-core`: no GPU, no file formats, no threads of their own
  (except the trim thread in `fx-tiles`).
* `fx-render`: no file I/O.
* Nothing depends on `fx-app`. The engine must run headless (`fx-cli`).
* `vendor/graphite/*` is third-party code: change it only with a
  `// FOTOX PATCH` comment and a line in `vendor/graphite/VENDORED.md`.
* Dependencies: only those listed in the root `Cargo.toml`
  `[workspace.dependencies]`.
