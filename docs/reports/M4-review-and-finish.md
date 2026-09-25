# M4 — review of the DeepSeek work, and completion

Reviewer and finisher: Claude  ·  Branch: `m4-finish` (on top of `main` with M3,
the agent's `task/M4-T00-decisions` and its interrupted, uncommitted T01/T02
work, saved as the commit "WIP (DeepSeek, interrupted)")

## What the agent had done

* **T00**: the nine decisions recorded — but numbered D-023…D-031, the numbers
  the M3 agent had used for its own decisions at the same time.
* **T01** (`fx-color`): profiles for the named spaces from their primaries and
  curves (correct constants, the ROMM linear segment included), ICC parsing,
  `same_profile`, and the 33³ display LUT with a CPU sampler that matches the
  shader. Good code with meaningful tests.
* **T02** (partly, uncommitted): the LUT step in `viewport.wgsl` on straight
  colour with the right texel-centre coordinates, the identity shortcut, the
  engine's LUT cache. The shell side (reading the monitor's profile) was missing.

## Review fixes

1. **Decision numbers collided with M3's**: M4's are now **D-031…D-039**
   (`DECISIONS.md`, the M4 card, `fx-color` comments).
2. `render()` took the LUT as an eighth argument (clippy); the caller now calls
   `set_display_lut` first. Clippy's loop-index complaint in the LUT sampler fixed.
3. An unreadable monitor profile was retried — and logged — on every frame; now
   it is dropped once and sRGB assumed.
4. **Criterion C1 was not met on a real machine**: Windows reports its sRGB
   display profile as an ICC file, which is not byte-equal to the named sRGB, so
   every sRGB document went through a LUT. A LUT within one 8-bit step of the
   identity is now skipped (Windows' file differs from lcms2's sRGB by at most
   0.54/255), and "no LUT" is cached (before, it would have been rebuilt every
   frame). Test `windows_srgb_profile_counts_as_srgb`.
5. The viewport uniform gained a field: a `vec3` in WGSL aligns to 16 bytes and
   broke the layout (48 vs 64 bytes) — scalars instead.

## What I completed

* **T02 shell**: `fx-app/src/window/win.rs` reads the ICC profile of the monitor
  the window is on (`MonitorFromWindow` → `GetMonitorInfoW` → `CreateDCW` →
  `GetICMProfileW`), sent at start and when the window moves to another monitor
  (checked by monitor id, so window drags stay cheap). Verified in the app:
  `display profile: …\\sRGB Color Space Profile.icm`.
* **T03**: `Command::AssignProfile` (metadata) and `Command::ConvertProfile`
  (every pixel layer and solid fill through an lcms2 transform built without cache
  so rayon workers share it; masks and adjustments untouched; a job with
  progress); *Edit ▸ Assign Profile… / Convert to Profile…*; the document's ICC
  profile embedded in TIFF (tag 34675), PNG (iCCP) and JPEG (APP2) exports;
  embedded profiles shown by their own description in the status bar.
* **T04**: soft proof — `fx_color::proof_lut` (lcms2 proofing transform; paper
  simulation = absolute colorimetric proof→monitor) with the gamut flag in the
  LUT's alpha from lcms2's own gamut check (alarm codes set to pure RGB magenta,
  which no press proof can produce); *View ▸ Proof Setup…, Proof Colors (Ctrl+Y),
  Gamut Warning (Shift+Ctrl+Y)*; the CMYK profiles of Windows' colour folder and
  `%APPDATA%\Fotox\profiles` listed for the UI (D-032); **CMYK TIFF export**
  (flattened onto white, Photometric separated, InkSet CMYK, the CMYK profile
  embedded; *Export As ▸ Print ▸ CMYK*). Verified in the app on a red/cyan
  checkerboard with RSWOP: Proof Colors dulls both, Gamut Warning paints both grey.
* **T05/T06**: the filter framework — `PixelOps` in `fx-core` (recipe R1a:
  commands stay in `fx-core`, algorithms in `fx-ops`/`fx-render`),
  `Command::ApplyFilter`, the `fx-ops` tile driver (layer-local coordinates,
  canvas edge replicate D-036, premultiplied maths, output tiles grown by the
  blur's reach), **Gaussian Blur** (exact separable kernel up to σ = 32; larger σ
  blurs a coarser mip level and upsamples, keeping one tile's neighbourhood under
  ~1.3 MB even at 1000 px) and **Unsharp Mask**; live previews on the visible tiles
  at the view level, latest request wins, never blocking; OK runs a job recorded
  with the new `History::record` (the document is busy meanwhile); *Filter ▸ Last
  Filter*. Verified in the app (radius 25 preview, OK, History "Gaussian Blur").
* **T07**: eight new adjustment layers — Posterize, Threshold, Gradient Map,
  Channel Mixer, Photo Filter, Color Balance, Vibrance, Black & White (tint) —
  CPU reference + GPU (new shader kinds; GPU = CPU with max error ≈ 0.001, only the
  step functions flip the odd pixel on a step), live dialogs, a new adjustment
  layer opens its dialog. Default names ("Posterize 1") need new name counters:
  the `.fxd` manifest now stores them as a list, so files saved before still open
  (test). Verified in the app (Black & White tint, Color Balance live).
* **T08**: Merge Layers (Ctrl+E; one layer = merge down), Merge Visible,
  Stamp Visible (Alt+Shift+Ctrl+E), Flatten Image — jobs through
  `PixelOps::composite` (the export compositor), undoable.
* **Tests**: unit tests per piece; GPU = CPU for every new adjustment; engine
  integration tests (`crates/fx-engine/tests/filter_flow.rs`) for filters,
  merge/stamp/flatten and convert + proof + CMYK export through a running engine.

## Not done (M4) — for the next session or for Rob

* **Hue/Saturation colour ranges** (reds…magentas): 42 parameters need their
  own encoding (a LUT row); not started.
* **Color Settings dialog** (working space, policies): Fotox keeps embedded
  profiles and never asks, like Photoshop's default ("Ask When Opening" off);
  the working space for new documents is sRGB.
* The proof state is per document, but the View menu check marks follow the last
  `proof_state` message, not the active tab.
* Filters run on the CPU (D-034); **S10, S11, S12** are Rob's measurements (the
  first on B1 will tell whether the preview is fast enough at 100 %).
* **C2** (CMYK numbers against Photoshop's conversion) is Rob's, with FOGRA39
  installed.
* Formulas marked VERIFY (Color Balance, Vibrance, Black & White, Photo Filter
  colours, threshold luminance, Unsharp Mask threshold) are checked in M7.

## Verification

```text
cargo fmt --check / clippy --workspace -D warnings / cargo test --workspace: clean
  (fx-color 16, fx-core 72, fx-ops 11, fx-render 47 incl. GPU, fx-io 48, fx-engine 43 + 4 integration)
node ui/tools/check-data.mjs: ok
In the app: monitor profile read; Gaussian Blur preview + OK; Black & White and
Color Balance layers live; Proof Colors and Gamut Warning with RSWOP (screenshots in the session).
```
