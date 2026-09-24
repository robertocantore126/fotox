# What we take from Graphite, and how

Source: <https://github.com/GraphiteEditor/Graphite>, commit
`ddafaaa7575a1a7f771399a693ab2e53c0900061` (2026-09-23), dual licensed
**MIT OR Apache-2.0**. Both licence texts are kept next to every copied file.

Graphite is a node-based vector/raster editor. Its core (node graph, document
format, editor) does not fit a tiled raster engine, so we do **not** fork it.
We take the one part that solves a hard, generic problem: running a web UI
inside a native wgpu window.

## 1. Vendored (compiled, lightly patched)

| Our path | Graphite path | What it does |
| --- | --- | --- |
| `vendor/graphite/desktop-ui` | `desktop/ui` | Runs CEF in a separate host process, renders the web UI off-screen into a `wgpu::Texture` (zero-copy D3D11 sharing on Windows), forwards input, carries binary messages both ways. Public API: `UiContext`, `UiInstance`, `UiCommand`, `UiEvent`, `InputEvent`. |
| `vendor/graphite/embedded-resources` | `desktop/embedded-resources` | Bakes the UI folder into the binary for release builds. |
| `vendor/graphite/wgpu-sync` | `libraries/wgpu-sync` | Locks that keep surface reconfiguration and queue submissions from racing (needed because CEF frames arrive on another thread). |

Rules:

* Do not refactor vendored code. Every change is marked `// FOTOX PATCH` and
  listed in `vendor/graphite/VENDORED.md`.
* Graphite's forks of `winit` and `cef-rs` are required and pinned by
  revision in the root `Cargo.toml` `[patch.crates-io]`.
* Environment variables keep Graphite's names: `GRAPHITE_RESOURCES` (UI folder
  in dev builds, set by `.cargo/config.toml`), `GRAPHITE_BROWSER_LOG`
  (`debug|info|warn|error`), `GRAPHITE_BROWSER_DEBUG_PORT` (Chrome DevTools
  remote debugging of the Fotox UI running inside the app — open
  `chrome://inspect` in Chrome and add `localhost:<port>`).

### The UI-side contract (JavaScript)

The vendored shell injects two functions into the page and calls one:

```js
// 1. install the receiver first
window.receiveNativeMessage = (buffer /* ArrayBuffer */) => { … };
// 2. then tell native we are ready  → shell receives UiEvent::Ready
window.initializeNativeCommunication();
// 3. send
window.sendNativeMessage(arrayBuffer);   // → UiEvent::Message(Vec<u8>)
```

Native → JS: `UiCommand::Message(Vec<u8>)` → `window.receiveNativeMessage(ArrayBuffer)`.
The byte format inside is ours: `PROTOCOL.md`.

## 2. Reference only (not compiled): `reference/graphite-desktop/`

Graphite's desktop app around the shell, copied for study and porting into
`crates/fx-app`. It depends on Graphite's editor (`graphite-desktop-wrapper`),
so it cannot compile as-is; port the parts listed here and drop the rest.

| File | Port to | Keep | Drop / replace |
| --- | --- | --- | --- |
| `src/lib.rs` | `fx-app/src/main.rs` | startup order: `UiContext::setup()` **first** (helper processes re-enter `main`), single-instance lock, wgpu context, event loop, UI start, `ui-events` bridge thread, restart-on-acceleration-failure | `wrapper::*` editor messages → our fx-protocol bridge |
| `src/app.rs` | `fx-app/src/app.rs` | `ApplicationHandler` structure, resize handling (`UiCommand::Resized/ScaleChanged/Refresh`), UI frame binding, redraw scheduling, `UpdateViewportPhysicalBounds` handling → our `ViewportBounds`, `WindowUpdateDirectInput` → our `DirectInput` | all `DesktopFrontendMessage`/editor handling, file dialogs (redo with `rfd` later), persistence of Graphite docs |
| `src/render/state.rs` + `composite_shader.wgsl` | `fx-app/src/render.rs` + `composite.wgsl` | surface config, the viewport + UI composite pass, viewport offset/scale uniforms, background colour | Vello overlay texture (we render overlays in the engine's viewport texture) |
| `src/input.rs` | `fx-app/src/input.rs` | pointer state machine, click/multi-click tracking, `Route::Ui` vs `Route::Editor` with `direct_input` + viewport rect, stroke capture until release, tablet data | Graphite `InputMessage` → `fx_engine::PointerInput` |
| `src/window.rs`, `src/window/win.rs`, `win/native_handle.rs` | `fx-app/src/window*.rs` | Windows init (AppUserModelID, COM, console attach), native handle helpers | mac/linux files (keep the `cfg` structure, no need to port) |
| `src/gpu_context.rs` | `fx-app/src/gpu.rs` | adapter selection override env var, adapter listing | Graphite `WgpuContextBuilder` → plain wgpu + `wgpu_sync` |
| `src/event.rs`, `src/consts.rs`, `src/dirs.rs`, `src/cli.rs`, `src/preferences.rs` | same names | small helpers | Graphite names/paths → Fotox |
| `bundle/src/win.rs`, `common.rs` | `tools/xtask` (M0-T06) | copying the CEF runtime next to the exe, trimming unneeded CEF files | mac bundle |
| `platform-win/` | `fx-app/build.rs` | Windows resource/icon embedding via `winres` | — |

## 3. Not taken, and why

* **Editor, node graph (`graphene`), document format** — built for procedural
  vector work; our core is a tiled raster store with lazy compositing.
* **Svelte frontend** — we use Fotox.
* **Message system** — ours is smaller: `fx-protocol` + `fx-core::Command`.
* **Vello** — not now. When vector layers/shapes/text arrive (M6), use the
  `vello` crate directly, not Graphite's wrappers.

## 4. Updating from upstream

Only when there is a reason (a bug fixed in the shell, a CEF security update).
Diff `desktop/ui`, `desktop/embedded-resources`, `libraries/wgpu-sync` between
the pinned commit and the new one, re-apply the patches listed in
`VENDORED.md`, bump the fork revisions in `[patch.crates-io]` to the ones in
upstream's `Cargo.lock`, update the commit hash here.
