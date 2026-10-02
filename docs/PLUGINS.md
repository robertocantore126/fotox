# Brush plugins (D-096)

A brush plugin is a small Rust crate compiled to WebAssembly. Fotox loads it
at start and **reloads it while running** whenever the `.wasm` changes: edit,
save, and the next stroke uses the new code. The open document, the undo
history and the option-bar values stay where they are.

## The loop

```text
cargo xtask plugins --watch
```

Leave it running next to Fotox. Every save under `plugins/` rebuilds (about
1–3 s) and copies the changed `.wasm` files to `%APPDATA%\Fotox\plugins`
(`FOTOX_PLUGINS` overrides the folder, for the app and the xtask alike). Fotox
notices within half a second, shows "Plugin loaded: …", and refreshes the
tool's option bar if it is the active tool. A build error stays in the xtask
console; the app keeps the last plugin that built.

`cargo xtask plugins` without `--watch` builds and copies once.

If cargo answers "failed to remove file …xtask.exe: Access is denied", a
running `cargo xtask run` holds that exe and cargo wanted to rebuild it (seen
on the exFAT worktree, where coarse timestamps make it look stale). Build the
watcher into its own folder instead:

```text
cargo run -p xtask --target-dir target/xtask-plugins -- plugins --watch
```

## What a plugin decides

The stroke engine keeps everything a brush has in common: the tip, spacing,
pressure, smoothing, the selection, the coverage buffer (opacity is a ceiling,
flow builds up), undo, and "live = replay". The plugin only decides what
happens to a pixel given how much the stroke has built up there:

```rust
fn rect(ctx: &Ctx, pixels: &mut [[f32; 4]], k: &[f32])
```

* `pixels`: the layer **as it was when the stroke started**, premultiplied
  RGBA `0..=1`, row-major, `ctx.w × ctx.h`; overwrite them with the result.
* `k`: opacity × coverage `0..=1` per pixel.
* `ctx.params`: the option-bar fields named in the manifest's `params`, in
  order. A number is the value shown (a 40 % slider is `40.0`), a toggle `0`/`1`,
  a drop-down the index of the chosen entry.
* `ctx.color`: the foreground or background swatch (manifest `color`),
  straight RGBA; `ctx.lock_alpha`: the layer's Lock Transparent Pixels.
* `ctx.pos(i)`: the canvas position of pixel `i`, for noise that must be the
  same every time ([`fotox_plugin::noise`]).

The result must depend only on these: no state kept between calls, no
randomness that is not a function of the position. Then the live stroke and
its replay paint the same pixels, whatever the tiling.

An optional `gray(ctx, values, k)` does the same for a layer mask (grey
`0..=1`). Without it, the tool paints nothing on a mask.

The engine calls `rect` **once per dirty rectangle** (part of one 256 × 256
tile), in parallel on the rayon threads, each with its own instance of the
plugin. So a plugin pays the call cost per rectangle, not per pixel.

## A new plugin

1. Copy `plugins/plain-eraser/` to `plugins/<name>/`; rename the package in
   its `Cargo.toml`; add it to `members` in `plugins/Cargo.toml`.
2. Edit `src/manifest.json`:
   * `id`: stable, unique; the UI tool id becomes `plugin:<id>`;
   * `name`: the toolbar and History name;
   * `slot`: the toolbar slot whose flyout shows it (`eraser`, `brush`,
     `dodge`…); `icon`: an icon of the UI sprite;
   * `color`: `foreground` or `background`;
   * `options`: the option bar, in `ui/js/data/options.js`'s format.
     `Size`, `Hardness`, `Opacity`, `Flow`, `Smoothing`, `Pressure for size`,
     `Pressure for opacity` and `Spacing` drive the brush engine as for the
     native brushes;
   * `params`: the keys of the fields the plugin reads (the field text without
     the colon), at most 16;
   * `brush` (optional): how the stroke engine paints for this tool —
     `profile` (`classic`, `gaussian`, `feather`; forces the round tip),
     `accumulate` (`build_up` like every native brush, or `max`: the round
     tip swept along the path, each pixel keeps the strongest coverage, so
     scrubbing never builds up), `spacing` (fraction of the diameter, unless
     the bar has `Spacing`), `feather_from` (the key of a px field: the tip
     becomes `Size` of solid core plus that much fade on each side).
3. Write `rect` (and `gray`) in `src/lib.rs`.
4. `cargo xtask plugins --watch`, pick the tool in its slot's flyout.

## When it goes wrong

* A plugin that does not compile, has no manifest or imports anything (only
  `wasm32-unknown-unknown` with the SDK, no wasm-bindgen, no WASI) is not
  loaded; the app shows the reason.
* A plugin that traps (a panic, an out-of-bounds index) or runs more than 2 s
  in one call fails that call: those pixels stay as they were, the instance is
  thrown away, and the reason shows when the stroke ends.
* `RUST_LOG=fx_plugin=info` logs every load.

## Measuring

`plugins/plain-eraser` is the native Eraser as a plugin. Run in release:

```text
cargo test --release -p fx-ops --test plugin_boundary -- --ignored --nocapture
```

It checks that both paint the same pixels and prints both times. On
2026-10-02 (Rob's PC) the plugin cost +2–10 %, and +23 % for a 1200 px brush
painted live. The Feather Eraser (Size 100 + Feather 300, 700 px across)
costs 2.4 ms per pen event live against 0.85 ms for the native soft eraser of
the same outline: its fade reaches further, so it touches more pixels.

## Files

| What | Where |
| --- | --- |
| Host: load, reload, call | `crates/fx-plugin/src/lib.rs` |
| Stroke engine call | `fx_ops::brush::stroke::Stroke::recompute_plugin` |
| Engine: folder, watcher, params, UI list | `crates/fx-engine/src/plugins.rs` |
| Tool | `tools/paint.rs` `Kind::Plugin`, `tools/mod.rs` `new_tool` |
| UI | `ENGINE.PLUGINS` in `ui/js/main.js`; `setPluginTools`, `setPluginBars` |
| SDK (plugin side, the ABI) | `plugins/sdk/src/lib.rs` |
| Build and watch | `tools/xtask/src/plugins.rs` |
| Tests | `fx-ops/tests/plugin_boundary.rs`, `fx-engine/tests/plugin_flow.rs` |
