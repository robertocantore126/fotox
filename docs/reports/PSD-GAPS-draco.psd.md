# PSD gap analysis — draco.psd

*Method: `%USERPROFILE%\Downloads\draco.psd` read only, once with psd-tools 1.20.0 and once with
ag-psd 31.0.2 (279,966,323 bytes, sha256 `c5d0bbf4…a9fb7d`). Parser coverage and every place the two
parsers disagree are in [PSD-GAPS-PARSERS.md](PSD-GAPS-PARSERS.md); the raw dumps behind every row
are in `XuanZhi9\psd-gap-analysis\` (outside this repo). All Fotox evidence is from `crates/fx-core`
(paths relative to the repo root). `crates/fx-io/src` has no PSD reader
(`crates/fx-io/src/lib.rs:14`, module list `102-114`), so this is what a future importer would have
to map — not a list of bugs.*

## Summary

`draco.psd` is a 2160 × 2700 RGB / 8-bit-per-channel document (3 channels, 72 ppi, **untagged** ICC,
written by Photoshop 2025) with 39 layers: 31 pixel, 4 embedded Smart Objects, 2 Gradient Fill
layers, 1 Invert adjustment and one pass-through group; 29 layers are hidden and two are clipping
masks. The distinctive content is the Smart Object stack — `fikoffik-15.jpg` (10.0 MB),
`ifdk-1.png` (9.6 MB), `0_3_inspyrenet.png` (4.8 MB) and `Layer 10.psb` (1.6 MB) — three of which
carry **Smart Filters** (seven instances of Gaussian Blur / Add Noise) and all three of which
declare a Smart Filter *mask* (enabled, linked off, extend-with-white on; no mask pixels are stored
in the file). Eight layers carry an enabled Gradient Overlay and one an enabled Stroke;
`love for all living beings` additionally has two Stroke slots, an Outer Glow, a Gradient Overlay
and a Colour Overlay, all present but disabled. Document level: an empty pattern section, the C2PA
provenance block `CAI `, `OCIO`/`GenI` metadata, four embedded linked-file buffers in `lnk2`, the
red @50 % Smart Filter mask overlay (`FMsk`), a ≈5.5 MB-per-channel cached smart-filter render
(`FEid`), one guide off the left edge, an 18 px grid, Photoshop's single auto slice, XMP + EXIF +
print metadata — and no text, patterns, alpha channels, saved paths, layer comps, notes, samplers
or non-default blend-if ranges.

## Feature inventory against Fotox

| Feature | Used in file (count, example layers) | Fotox | Evidence (file:line) | What an importer would lose |
| --- | --- | --- | --- | --- |
| Document header: 2160 × 2700, RGB, 8 bpc, 3 channels | 1 document | yes | `document.rs:10-12`, `color.rs:9` (`BitDepth::U8`), `color.rs:57` | Nothing: RGB/8 is what Fotox stores. |
| Resolution 72 ppi (unit: ppi) | 1 | yes (metadata) | `document.rs:14` | Nothing (ppi is metadata, not rendering). |
| ICC profile — **untagged** (`ICC_UNTAGGED_PROFILE = 1`, no ICC resource) | 1 | partial | `color.rs:34` | `ColorProfile` has no "untagged/unknown" state; the importer must assume a working space, so colours can differ from the user's Photoshop unless that assumption matches. |
| Layer tree: pixel / group / smart object / adjustment / fill kinds | 39 layers: 31 pixel, 4 Smart, 2 Gradient Fill, 1 Invert, 1 group | yes | `layer.rs:164-220` (`Pixel 167`, `Group 172`, `Adjustment 176`, `Shape 184`, `FillLayer 211`, `Smart 217`) | Nothing structural. |
| Layer name and layer id (`luni`, `lyid`) | all 39 | yes | `layer.rs:319-320` | Nothing. |
| Visibility (29 hidden) | 39 | yes | `layer.rs:321` | Nothing. |
| Layer opacity below 100 % (`ifdk-1` 191/255, `Layer 5 copy` 102/255, `Layer 16` 153/255, `Layer 20` 115/255) | 4 | yes | `layer.rs:323` | Nothing. |
| Fill opacity | all 100 % | yes (unused) | `layer.rs:325` | Nothing to lose here. |
| Blend modes: 9 distinct (`norm` ×29, `mul` ×3, `dark`, `lite`, `sLit`, `colr`, `pass`, `scrn`, `hue`) | 39 | yes | `blend.rs:17-52` | Nothing: every mode used exists (`hue` 49, `colr` 51, `scrn` 31, `sLit` 37, `mul` 25, `dark` 24, `lite` 30, `pass` 19). |
| Group with pass-through and open state (`lsct` kind 1, blend `pass`) | 1 (`Group 1`) | yes | `layer.rs:172-175` (`expanded` 174), `blend.rs:19` | Nothing; `expanded` takes the open-folder flag. (A *closed* folder or a bounding section divider would also be representable via `expanded`; not used here.) |
| Clipping masks (`clbl` record bit) | 2 (`Layer 19`, `Layer 20`) | yes | `layer.rs:328` | Nothing. |
| Layer locks (`lspf`) | none (every layer 0) | yes (unused) | `layer.rs:329,332,333` | Nothing in this file. |
| Pixel layer masks | 3 empty default masks, **no mask pixels** | yes | `layer.rs:16-23` (`linked` 20, `outside_value` 22), `layer.rs:334` | Nothing: the masks are the default "reveal all" ones. Mask density/feather have no field (`layer.rs:16-23`) but are unset in this file. |
| Vector masks (`vmsk`/`vogk`) | none | yes (unused) | `layer.rs:342,360`, `path.rs:90` | Nothing in this file. |
| Adjustment layer: Invert (`nvrt`) | 1 (`Invert 1`) | yes | `layer.rs:59` | Nothing. |
| Adjustment layer: Hue/Saturation (`hue2`) | none | partial in general | `layer.rs:53-58` | — (used in `nuovaera`/`Untitled-mmm`; see those reports). |
| Gradient Fill layers (`GdFl`) | 2 (`Gradient Fill 1`: Radial, 90°, scale 200 %, offset (0, 50); `Gradient Fill 2`: Linear, 99.63°) | yes | `layer.rs:211-214`, `fill.rs:18` (`FillLayer::Gradient`), `gradient.rs:176` (`GradientKind`), `gradient.rs:188` (`GradientFill`), `gradient.rs:274` (`GradientLayer`: angle/scale/offset/dither/reverse/mirror) | The gradient's interpolation method (`gradientsInterpolationMethod = Smoo`, `Intr` 4096) has no exact match — `Method` (`gradient.rs:38`) offers Perceptual/Linear/Classic only; the gradient's name (`"Custom"`) and smoothness value are not stored. |
| Smart Objects, embedded content and transform (`PlLd`, `SoLd`) | 4 (`fikoffik-15` jpg 2160×2700, `ifdk-1` png 2160×2700, `0_3_inspyrenet` png 2048×2048, `Layer 10` psb 1907×2652) | yes | `layer.rs:217-220`, `smart.rs:21-33` (`doc` 23, `composite` 26), `smart.rs:58-64`, `transform.rs:91` (`Affine`/`Projective`/`Warp`) | Nothing for the pixels: the four transforms are axis-aligned rectangles (`Mapping::Affine` fits) and the free-transform warps are identity meshes. Photoshop's `customEnvelopeWarp` point flags have no field, but the mesh is uniform here. |
| Smart Object source is a *linked file* with embedded data (`lnk2`, four `liFD` buffers: `fikoffik-15.jpg`, `ifdk-1.png`, `0_3_inspyrenet.png`, `Layer 10.psb`) | 4 | partial | `smart.rs:28-30` (`linked` path + `linked_mtime`) | Fotox's linked object is a path on disk plus its mtime; the PSD stores the bytes inline and no timestamp (`timestamp: null`, `assetModTime 0`). The importer must either extract the buffers to disk (keeping the link) or import them as embedded content (losing the link), and there is no mtime to carry. |
| Smart Filters (`SoLd` → `filterFX.filterFXList`) on 3 objects: Gaussian Blur (radius 10 / 6.2 / 107.1 px), Add Noise (amount 0.6445 / 0.5066 / 1.2191, distribution uniform/uniform/gaussian, monochromatic false/true/false, seed 40397693) — 7 instances, all enabled | 3 smart objects | yes | `smart.rs:38-46` (`SmartFilter`: filter/enabled/mode/opacity), `ops.rs:24` (`FilterParams`), `ops.rs:27` (`GaussianBlur`), `ops.rs:56` (`AddNoise`: amount/gaussian/monochromatic/seed), `ops.rs:105` (`label`) | Nothing: both filters and all their parameters have a home (Fotox's `AddNoise.amount` is the 0.1–400 % scale Photoshop writes, `gaussian` is the distribution, `seed` the random seed). |
| Smart-filter **mask** (`filterMaskEnable true`, `filterMaskLinked false`, `filterMaskExtendWithWhite true` on all three; document `FMsk` overlay red @50 %) | 3 objects, 0 mask pixels | partial | `smart.rs:38-46` (no mask field), `smart.rs:58-64` | No place for a per-filter mask, so a painted smart-filter mask would be dropped. In *this* file the mask is the default white one (no mask channel exists), so nothing visible is lost; the overlay colour/opacity (`FMsk`) is unrepresentable. |
| "Smart Filters" master enable | on for all three | yes | `smart.rs:64` (`filters_enabled`) | Nothing. |
| Layer effects, enabled: Gradient Overlay ×8 (0_3_inspyrenet, `Layer 5`, `Layer 5 copy`, `Layer 5 copy 2` ×2, `Layer 5 copy 3/4/5`), Stroke ×1 (`Layer 7`) | 9 layers | yes | `styles.rs:326` (`LayerStyles`), `GradientOverlay` `styles.rs:184`, `Stroke` `styles.rs:120` | Multiplicity: `LayerStyles` holds one `Option` per effect type (`styles.rs:328-336`). Effect parameters Photoshop writes but Fotox has no field for — contour (`TrnS`), noise (`Nose`), anti-alias (`AntA`), glow technique/range/jitter (`GlwT`/`Inpr`/`ShdN`), thickness/shape/texture on bevel — are all at Photoshop defaults in this file. A stroke fill that is a gradient or pattern could not be stored (`styles.rs:120-127` is one solid colour); all strokes here are solid. |
| Layer effects, present but disabled: Colour Overlay ×2 (`Layer 7`, `love for all living beings`), Outer Glow ×1, Gradient Overlay ×1, Stroke ×2 (all `love for all living beings`) | 6 slots | yes | every effect struct carries `enabled` (e.g. `styles.rs:74-85`), `styles.rs:326-336` | Nothing if the importer keeps disabled effects as `Some(enabled=false)`; dropping them silently would lose the saved style. The two Stroke slots collapse to one (see below). |
| Effect blend modes used: `Lghn` (Linear Light, ×8), `Mltp`, `Nrml` | 11 effects | yes | `blend.rs:40` (`LinearLight`), `blend.rs:25` (`Multiply`), `blend.rs:21` (`Normal`) | Nothing. |
| Gradient Overlay placement: `Algn = true` (align with layer), `Scl 55`, `Dthr` off, `Rvrs` off, type Linear | all 8 overlays | partial | `styles.rs:184-193`, `gradient.rs:274-292` (`angle`/`scale`/`offset` are canvas-centred; no `align` flag) | Fotox's gradient overlay is placed on the **canvas**; Photoshop's `Algn=true` places it over each layer's pixel bounds. On layers smaller than the canvas the gradient's span and offset differ, and the model has no field to say "align with layer". |
| Document Global Light angle 90° | 1 | yes | `document.rs:28` | Nothing. |
| Document Global Light altitude 30° | 1 | partial | no document field (`document.rs:9-56`); per-effect `BevelEmboss.altitude` `styles.rs:152` | No bevel exists in this file so nothing is lost; a bevel using the global light would have to take its altitude from elsewhere. |
| Guides (`GRID_AND_GUIDES_INFO`) | 1 vertical guide at x = −510.5625 px (left of the canvas) | yes | `document.rs:30`, `document.rs:58` | Nothing (guide position is a document pixel value; the guide is off-canvas but representable). Note: psd-tools returns the raw unsigned 1/32-px value and ag-psd's `readUint32()/32` reports it as 134,217,217.4 px — the importer must read it as signed. |
| Grid: 18 px (576 units) | 1 setting | no | `document.rs:9-56` has no grid field | The document's grid spacing (view-only preference). |
| Slices (`SLICES`) | 1 auto slice covering the whole canvas (named after the doc, "fikoffik-1"), no user slices | yes | `document.rs:43`, `comps.rs:80` (`Slice`), `comps.rs:89` (`auto_slices`) | The record's URL/alt/HTML/alignment fields have no home, but they are empty here; the auto slice is regenerated by `auto_slices`. |
| Layer comps, alpha channels, saved paths, notes/counts/colour samplers, patterns, text layers, artboards | none in this file | yes (unused) | `comps.rs:13,33`, `channel.rs:13` + `document.rs:35`, `path.rs:96` + `document.rs:38`, `annotations.rs:7,17,29,35` + `document.rs:48`, `pattern.rs:16` + `document.rs:33`, `layer.rs:196` + `text.rs:22`, `layer.rs:345,350` | Nothing: the file uses none of them (`Patt` is an empty PATTERNS1 section). |
| Layer style-blending flags `clbl` = 1, `infx` = 0, `knko` = 0 | all layers (Photoshop defaults) | no field | `layer.rs:318-346`, `styles.rs:326` | Nothing visually: these are Photoshop's defaults (styles clipped to the layer, interior effects blended, no knockout). A file that changed them would lose the setting. |
| Blend-if ranges (`blendingRanges`) | all layers default | no field | `layer.rs:318-346` | Nothing in this file; blend-if is not modelled at all. |
| Layer metadata blocks `lnsr` (name source), `lyvr` (LAYER_VERSION 130 on `Group 1`, 160 on `love for all living beings`), `shmd` (layer timestamps), `fxrp` (non-zero reference point on 23 layers), `lclr` (sheet colour, all 0) | all layers | no field | `layer.rs:318-346` | The panel metadata: which Photoshop wrote the layer, the layer's edit time, the transform origin, the colour label. No pixels are lost; `fxrp` only matters if a transform is later re-edited the way Photoshop would. |
| Document metadata: XMP (14,562 B), EXIF (306 B), CAPTION_DIGEST (16 B), empty URL_LIST, VERSION_INFO, IDS seed 54, LAYER_STATE_INFO 14, LAYER_SELECTION_IDS [50], LAYER_GROUP_INFO, LAYER_GROUPS_ENABLED_ID, PIXEL_ASPECT_RATIO 1.0, THUMBNAIL_RESOURCE, UNKNOWN_1092 (25 B), PRINT_INFO_CS5 / PRINT_STYLE / PRINT_FLAGS / PRINT_FLAGS_INFO / PRINT_SCALE, COLOR_HALFTONING_INFO, COLOR_TRANSFER_FUNCTION | 20 resources | no (selection: yes) | `document.rs:9-56` (no metadata field); the layer selection maps to `document.rs:18`; `.fxd` only persists annotations (`fx-io/src/fxd/manifest.rs:61`) | Metadata only: XMP/EXIF/IPTC-style captions, print and export settings, halftone/transfer functions, the thumbnail, the layer-panel state. Nothing that affects rendering. |
| Document blocks `CAI ` (77 B, C2PA Content Credentials), `OCIO` (display/view config), `GenI` (GenTech marker), `cinf` (COMPOSITOR_INFO 1.3 / PS 26.5, compCoreGPU) | 4 blocks | no | `document.rs:9-56` | Provenance and "which engine rendered this" metadata; the C2PA credentials cannot be re-serialised. |
| `FEid` / `FILTER_EFFECTS2`: cached smart-filter render, ≈5.5 MB per channel | 1 block | no | `document.rs:9-56`; Fotox recomputes filters (`smart.rs:38`, `ops.rs:27,56`) | Nothing in principle (derived data), but it is the only place the file records what Photoshop actually drew — useful to diff against an importer's own render. |
| Composite/flattened preview and thumbnail | 1 composite section | yes | `fx-io/src/fxd/manifest.rs:75,155`, `container.rs:92` | Nothing: Fotox stores its own composite preview. |

## The `no` and `partial` items, ordered by impact on this file

1. **Gradient Overlay `Algn = true` on 8 layers** — `partial` (`styles.rs:184-193`,
   `gradient.rs:274-292`). Photoshop anchors the overlay's gradient to each layer's pixel bounds;
   Fotox's gradient is canvas-centred, with no "align with layer" field. The eight
   `Layer 5 copy…`/`0_3_inspyrenet` overlays (Linear, scale 55 %, blend Linear Light) are the
   largest visible gap in this file: on any layer smaller than the canvas the gradient's span
   differs. An importer can only approximate by pre-computing scale/offset for the canvas.
2. **Smart Filter masks are declared but not representable** — `partial` (`smart.rs:38-46`).
   All three filtered Smart Objects set `filterMaskEnable true`, `filterMaskLinked false`,
   `filterMaskExtendWithWhite true`, and the document carries the red @50 % overlay (`FMsk`).
   Fotox's `SmartFilter` has no mask field. No mask *pixels* exist in this file, so the current
   render loses nothing; as soon as a mask is painted, it would be dropped.
3. **Gradient interpolation method `Smoo`** — `partial` (`gradient.rs:38`, `Method` =
   Perceptual/Linear/Classic). Both Gradient Fill layers and all eight overlays use Photoshop's
   "Smooth" interpolation (`Intr 4096`), which has no exact member; the importer must pick the
   nearest, so gradient ramps can band slightly differently.
4. **Two Stroke slots on one layer** — `partial` (`styles.rs:328-336`, one `Option` per effect
   type). `love for all living beings` (`lmfx`, LAYER_VERSION 160) stores two Stroke entries;
   Fotox keeps one. Both are disabled here, so only the saved style loses a variant.
5. **Linked Smart Objects with embedded bytes** — `partial` (`smart.rs:28-30`). The four `lnk2`
   `liFD` buffers (10.0 / 9.6 / 4.8 / 1.6 MB) are the object's content, but Fotox's linked object is
   a file path plus mtime. Importing them as embedded content is lossless for pixels and loses the
   link; importing them "as linked" means writing the buffers to disk and inventing a path.
6. **Untagged ICC profile** — `partial` (`color.rs:34`). Nothing in the file names a profile, and
   Fotox cannot record "unspecified": the importer must choose sRGB (or the user's working space),
   so a Photoshop user in Adobe RGB would see different colours.
7. **Effect detail fields that have no home but are at defaults** — `no` fields
   (`styles.rs:74-210`): contour, noise, anti-alias, glow technique/range/jitter, bevel
   texture/shape. Nothing is lost in this file (verified: `Nose` 0, `TrnS` "Linear" everywhere).
8. **Layer/document metadata without a field** — `no` (`layer.rs:318-346`, `document.rs:9-56`):
   `lyvr` 130/160, `lnsr`, `shmd` timestamps, `fxrp` on 23 layers, sheet colours, XMP/EXIF/
   CAPTION_DIGEST/URL_LIST, print blocks, halftone/transfer, IDS seed, layer state and selection
   resources, `PIXEL_ASPECT_RATIO`, thumbnail, `UNKNOWN_1092`, `CAI `, `OCIO`, `GenI`, `cinf`.
   Metadata only — no rendering difference.
9. **Grid 18 px** — `no` (`document.rs:9-56`). A view preference.
10. **`FEid` cached render** — `no`. Derived data; Fotox recomputes it.
11. **Global-light altitude** — `partial`, but no bevel exists here, so nothing is lost
    (`document.rs:28` holds the angle; `styles.rs:152` holds a per-effect altitude).
