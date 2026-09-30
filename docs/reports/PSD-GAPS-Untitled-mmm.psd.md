# PSD gap analysis — Untitled-mmm.psd

*Method: `%USERPROFILE%\Downloads\Untitled-mmm.psd` read only, once with psd-tools 1.20.0 and once
with ag-psd 31.0.2 (181,838,006 bytes, sha256 `dc7d0f44…47da0b5`). Parser coverage and every place
the two parsers disagree are in [PSD-GAPS-PARSERS.md](PSD-GAPS-PARSERS.md); the raw dumps behind
every row are in `XuanZhi9\psd-gap-analysis\` (outside this repo). All Fotox evidence is from
`crates/fx-core` (paths relative to the repo root). `crates/fx-io/src` has no PSD reader
(`crates/fx-io/src/lib.rs:14`, module list `102-114`), so this is what a future importer would have
to map — not a list of bugs.*

## Summary

`Untitled-mmm.psd` is a 2160 × 2700 RGB / 8-bit document (3 channels, 72 ppi, **untagged** ICC,
Photoshop 2025) with 45 layers: 38 pixel, 2 embedded Smart Objects, 1 Gradient Map, 1
Hue/Saturation, 2 Invert adjustments and one pass-through group; 24 layers are hidden, none is
clipping and none is partly transparent. The two Smart Objects are two copies of the same
28.8 MB / 11.2 MB `.psb` source (`Group 1` and `Group 1 copy`), and `Group 1 copy` carries a
**genuinely deformed** free-transform warp mesh (the other is the identity grid); neither has Smart
Filters. Layer styles: Drop Shadow + Outer Glow + Gradient Overlay on `Layer 5` (the Gradient
Overlay blends with Divide), Outer Glow on `Layer 9` and `Layer 10`, and an Outer Glow and Colour
Overlay present-but-disabled on `Layer 21`. `Background` carries the transparency+position lock and
`Layer 1` is "lock all". The Gradient Map has two stops (dark violet → pale mint) and the
Hue/Saturation's per-range bands are all zero. Document level: an empty pattern section, `CAI `
(C2PA), `OCIO`/`GenI`, two embedded `lnk2` link buffers (both named `Group 1.psb`), the red @50 %
filter-mask overlay (`FMsk`), `cinf` (compCoreGPU), an 18 px grid and no guides, one auto slice,
XMP + EXIF + IPTC + print metadata, a white document background colour — and no text, patterns,
alpha channels, saved paths, layer comps, notes, colour samplers, vector masks, Smart Filters or
blend-if ranges.

## Feature inventory against Fotox

| Feature | Used in file (count, example layers) | Fotox | Evidence (file:line) | What an importer would lose |
| --- | --- | --- | --- | --- |
| Document header: 2160 × 2700, RGB, 8 bpc, 3 channels | 1 document | yes | `document.rs:10-12`, `color.rs:9` (`BitDepth::U8`), `color.rs:57` | Nothing. |
| Resolution 72 ppi (unit: ppi) | 1 | yes (metadata) | `document.rs:14` | Nothing (ppi is metadata). |
| ICC profile — **untagged** (`ICC_UNTAGGED_PROFILE = 1`, no ICC resource) | 1 | partial | `color.rs:34` | `ColorProfile` has no "untagged/unknown" state; the importer must assume a working space, so display colours can differ from Photoshop's. |
| Layer tree: pixel / group / smart object / adjustment | 45 layers: 38 pixel, 2 Smart, 1 Gradient Map, 1 Hue/Saturation, 2 Invert, 1 group | yes | `layer.rs:164-220` | Nothing structural. |
| Layer name and layer id (`luni`, `lyid`) | all 45 | yes | `layer.rs:319-320` | Nothing. |
| Visibility (24 hidden) | 45 | yes | `layer.rs:321` | Nothing. |
| Layer opacity / fill opacity | all 100 % | yes (unused) | `layer.rs:323,325` | Nothing to lose here. |
| Blend modes: 5 distinct (`norm` ×40, `hLit`, `mul`, `pass`, `pLit`, `colr`) | 45 | yes | `blend.rs:17-52` | Nothing: `colr` is `Color` (`blend.rs:51`), `hLit` `HardLight` (38), `pLit` `PinLight` (41). |
| Group: one, pass-through and open (`lsct` kind 1 with `blend pass`) | 1 (`Group 1`) | yes | `layer.rs:172-175` (`expanded` 174), `blend.rs:19` | Nothing. |
| Clipping masks | none | yes (unused) | `layer.rs:328` | Nothing. |
| Layer locks (`lspf`) | 2: `Background` 13 (transparency + position); `Layer 1` = 2147483648 (lock all) | partial | `layer.rs:329,332,333` | Fotox has no "lock all" flag and no nesting lock: lock-all must be fanned out to the three locks it does have. Behaviourally equivalent, but the saved distinction is lost. |
| Pixel layer masks | 4 empty default masks, **no mask pixels** | yes | `layer.rs:16-23`, `layer.rs:334` | Nothing. Mask density/feather have no field (`layer.rs:16-23`) but are unset here. |
| Vector masks (`vmsk`/`vogk`) | none | yes (unused) | `layer.rs:342,360`, `path.rs:90` | Nothing in this file. |
| Adjustment layer: Gradient Map (`grdm`) | 1 (`Gradient Map 1`: two stops, 0 → 4096, colours #171616 → #1DB7E3, smooth, not reversed, not dithered) | yes | `layer.rs:70-73` (`GradientMap { stops, reverse }`), `GradientStop` `layer.rs:135` | The dither flag and the interpolation method have no field; both are "off"/"smooth" here, so nothing visible is lost. A noise/dithered gradient map would be approximated. |
| Adjustment layer: Hue/Saturation (`hue2`) | 1 (`Hue/Saturation 1`: master −25 hue / +100 saturation / −14 lightness, **all per-range bands 0**, colourise off) | yes | `layer.rs:53-58` (`hue`/`saturation`/`lightness`, `colorize` 57) | Nothing: with every per-range item zero, the master-only model is exact for this file. |
| Adjustment layers: Invert (`nvrt`) | 2 (`Invert 1`, `Invert 2`) | yes | `layer.rs:59` | Nothing. |
| Smart Objects, embedded content and transform (`PlLd`, `SoLd`) | 2 (`Group 1` psb 28,823,818 B, 2160 × 2700 → (0, 107); `Group 1 copy` psb 11,158,792 B, 1851 × 1918 → (221, 44)) | yes | `layer.rs:217-220`, `smart.rs:21-33,58-64`, `transform.rs:91` (`Affine` 94, `Projective` 97) | The two rectangles are affine, so nothing. The transforms are `warpCustom` with `warpValue 0`; `Group 1`'s 4 × 4 envelope mesh is the identity, but `Group 1 copy`'s is **deformed** (row x-positions 553.5/1207.8/1793.3/2418.7 then 382.2/1246.7/2343.8/…) — see below. |
| Free-transform warp mesh (`customEnvelopeWarp`) | 1 genuinely deformed (`Group 1 copy`) | partial | `transform.rs:99` (`Mapping::Warp(BezierPatch)`), `transform.rs:649` (`BezierPatch`) | Fotox can hold a warp, but Photoshop's envelope mesh carries its own per-point interpolation flags and is not literally a bicubic Bézier patch: an importer has to fit one, so a deformed Smart Object can land slightly differently. |
| Smart Object sources are *linked files* with embedded data (`lnk2` `liFD`: two buffers both named `Group 1.psb`) | 2 | partial | `smart.rs:28-30` (`linked` path + `linked_mtime`) | Fotox's linked object is a path on disk plus mtime; the PSD stores both buffers inline with no timestamp. The importer must extract them to disk (keeping the link) or import them as embedded content (losing it). |
| Smart Filters | none | yes (unused) | `smart.rs:38-46`, `ops.rs:24` | Nothing in this file. |
| Layer effects, enabled: Drop Shadow ×1 (blend Multiply, opacity 89 %) + Outer Glow ×3 + Gradient Overlay ×1 (`Layer 5`; overlays also on `Layer 9`, `Layer 10`) | 5 effect instances on 3 layers | yes | `styles.rs:326` (`LayerStyles`), `DropShadow` `styles.rs:74`, `OuterGlow` `88`, `GradientOverlay` `184` | Effect parameters Photoshop writes but Fotox has no field for — contour (`TrnS`), noise (`Nose`), anti-alias, glow technique/range/jitter (`GlwT`/`Inpr`/`ShdN`), gradient `Algn` (see below) — are all at defaults except `Algn`, so nothing but the alignment is lost. |
| Layer effects, present but disabled: Outer Glow ×1, Colour Overlay ×1 (`Layer 21`) | 2 slots | yes | every effect struct carries `enabled` (e.g. `styles.rs:88-96`), `styles.rs:326-336` | Nothing if the importer keeps disabled effects as `Some(enabled=false)`; dropping them would lose the saved style. |
| Effect blend modes used in effects: `Mltp`, `Nrml`, `blendDivide` (= Divide) | 5 | yes | `blend.rs:25` (`Multiply`), `21` (`Normal`), `47` (`Divide`) | Nothing. |
| Gradient Overlay placement: `Algn = true` (align with layer), scale 55 %, blend Divide, dither/reverse off | 1 (`Layer 5`) | partial | `styles.rs:184-193`, `gradient.rs:274-292` (canvas-centred placement, no `align` field) | Fotox anchors the gradient to the canvas; Photoshop anchors it to `Layer 5`'s pixel bounds. On a layer smaller than the canvas the gradient span/offset differ and the setting cannot be stored. |
| Document Global Light angle 90° | 1 | yes | `document.rs:28` | Nothing. |
| Document Global Light altitude 30° | 1 | partial | no document field (`document.rs:9-56`); per-effect `BevelEmboss.altitude` `styles.rs:152` | No bevel exists in this file, so nothing is lost; a bevel using the global light would need its altitude from elsewhere. |
| Guides (`GRID_AND_GUIDES_INFO`) | 0 (empty guide list) | yes (unused) | `document.rs:30,58` | Nothing (this file has no guides). |
| Grid: 18 px (576 units) | 1 setting | no | `document.rs:9-56` has no grid field | The document's grid spacing (view-only preference). |
| Slices (`SLICES`) | 1 auto slice covering the whole canvas (named "Untitled-mmm"), no user slices | yes | `document.rs:43`, `comps.rs:80`, `comps.rs:89` | The slice record's URL/alt/HTML fields have no home but are empty; the auto slice is regenerated. |
| Layer comps, alpha channels, saved paths, notes/counts/colour samplers, patterns, text layers, artboards, blend-if ranges | none in this file | yes (unused) / no field | `comps.rs:13,33`, `channel.rs:13` + `document.rs:35`, `path.rs:96` + `document.rs:38`, `annotations.rs:7,17,29,35` + `document.rs:48`, `pattern.rs:16` + `document.rs:33`, `layer.rs:196` + `text.rs:22`, `layer.rs:345,350`; blend-if has no field (`layer.rs:318-346`) | Nothing: the file uses none of them (`Patt` is an empty PATTERNS1 section, all blending ranges are defaults). |
| Layer style-blending flags `clbl` = 1, `infx` = 0, `knko` = 0 | all layers (defaults) | no field | `layer.rs:318-346` | Nothing visually (Photoshop defaults). |
| Layer metadata blocks `lnsr` (name source), `shmd` (timestamps), `fxrp` (non-zero on 25 layers), `lclr` (sheet colour, all 0) | all layers | no field | `layer.rs:318-346` | Panel metadata: name source, edit times, transform origins, colour labels. No pixels lost. |
| Document metadata: XMP (14,312 B), EXIF (306 B), IPTC (15 B), CAPTION_DIGEST (16 B), empty URL_LIST, VERSION_INFO, IDS seed 92, LAYER_STATE_INFO 37, LAYER_SELECTION_IDS [90], LAYER_GROUP_INFO, LAYER_GROUPS_ENABLED_ID, PIXEL_ASPECT_RATIO 1.0, BACKGROUND_COLOR (white), THUMBNAIL_RESOURCE, UNKNOWN_1092 (16 B), PRINT_INFO_CS5 / PRINT_STYLE / PRINT_FLAGS / PRINT_FLAGS_INFO / PRINT_SCALE, COLOR_HALFTONING_INFO, COLOR_TRANSFER_FUNCTION | 22 resources | no (selection: yes) | `document.rs:9-56` (no metadata field); layer selection maps to `document.rs:18`; `.fxd` persists annotations (`fx-io/src/fxd/manifest.rs:61`) | Metadata only: XMP/EXIF/IPTC captions, print/export settings, halftone/transfer functions, thumbnail, background colour, layer-panel state. Nothing that affects rendering. |
| Document blocks `CAI ` (77 B, C2PA), `OCIO`, `GenI`, `cinf` (COMPOSITOR_INFO 1.3 / PS 26.5, engine compCoreGPU), `FMsk` (red @50 % filter-mask overlay) | 5 blocks | no | `document.rs:9-56`; `FMsk` relates to `smart.rs:38-46` (no mask field) | Provenance and engine metadata; the Smart Filter mask overlay colour is unrepresentable, but no Smart Filter exists in this file. |
| Composite/flattened preview and thumbnail | 1 composite section | yes | `fx-io/src/fxd/manifest.rs:75,155`, `container.rs:92` | Nothing: Fotox stores its own composite preview. |

## The `no` and `partial` items, ordered by impact on this file

1. **Deformed Smart Object warp mesh on `Group 1 copy`** — `partial` (`transform.rs:99`,
   `transform.rs:649`). The envelope mesh is a real free-transform warp, not the identity grid; Fotox
   has `Mapping::Warp(BezierPatch)`, but Photoshop's per-point mesh flags have to be fitted into a
   Bézier patch, so this layer's placement/resampling can differ slightly. This is the only
   pixel-level gap in the file.
2. **Gradient Overlay `Algn = true` on `Layer 5`** — `partial` (`styles.rs:184-193`,
   `gradient.rs:274-292`). Photoshop anchors the overlay gradient to the layer's bounds, Fotox to
   the canvas, and there is no "align" field: on a layer smaller than the canvas the gradient span
   differs. (Blend Divide, scale 55 % — `styles.rs:184`, `blend.rs:47`.)
3. **Lock-all (`lspf` 2147483648) on `Layer 1`** — `partial` (`layer.rs:329,332,333`). Fotox has
   the three individual locks but no single "lock all" (and no nesting lock); the importer sets all
   three and the exact saved state is not identical.
4. **Linked Smart Objects with embedded bytes** — `partial` (`smart.rs:28-30`). Both `lnk2` `liFD`
   buffers (28.8 MB and 11.2 MB, both named `Group 1.psb`) are the objects' content, but Fotox's
   linked object is a file path plus mtime: the importer must extract them to disk or store them
   embedded and lose the link (no timestamp exists to preserve).
5. **Gradient Map detail fields** — `partial in principle` (`layer.rs:70-73`). The dither flag and
   the interpolation method have no field; both are off/smooth here, so nothing visible is lost.
   (The file also stores `expansion`, `min`/`max` and a random seed for noise gradients, none of
   which is a noise gradient.)
6. **Hue/Saturation per-range data** — `partial in principle` (`layer.rs:52-58`, master-only by
   design); in this file every per-range band is zero, so only a future file would be affected.
7. **Untagged ICC profile** — `partial` (`color.rs:34`). No profile is named in the file and Fotox
   cannot record "unspecified": the importer must choose a working space.
8. **Effect detail fields with no home (at defaults here)** — `no fields` (`styles.rs:74-210`):
   contours, noise, anti-alias, glow technique/range/jitter. Nothing visible is lost (verified
   `Nose` 0, `TrnS` "Linear", `GlwT` "Softer").
9. **Layer/document metadata without a field** — `no` (`layer.rs:318-346`, `document.rs:9-56`):
   `lnsr`, `shmd`, `fxrp` (25 layers), sheet colours, XMP/EXIF/**IPTC**/CAPTION_DIGEST/URL_LIST,
   print blocks, halftone/transfer, `BACKGROUND_COLOR`, IDS seed, LAYER_STATE_INFO,
   LAYER_GROUP_INFO, selection ids, thumbnail, `PIXEL_ASPECT_RATIO`, `UNKNOWN_1092`, `CAI `,
   `OCIO`, `GenI`, `cinf`. Metadata only.
10. **Grid 18 px** — `no` (`document.rs:9-56`). A view preference.
11. **Global-light altitude** — `partial`, but no bevel exists in this file, so nothing is lost
    (`styles.rs:152` holds a per-effect altitude if one is needed).
