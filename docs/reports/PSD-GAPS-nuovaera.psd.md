# PSD gap analysis — nuovaera.psd

*Method: `%USERPROFILE%\Downloads\nuovaera.psd` read only, once with psd-tools 1.20.0 and once with
ag-psd 31.0.2 (188,540,428 bytes, sha256 `edb4990d…f69076`). Parser coverage and every place the two
parsers disagree are in [PSD-GAPS-PARSERS.md](PSD-GAPS-PARSERS.md); the raw dumps behind every row
are in `XuanZhi9\psd-gap-analysis\` (outside this repo). All Fotox evidence is from `crates/fx-core`
(paths relative to the repo root). `crates/fx-io/src` has no PSD reader
(`crates/fx-io/src/lib.rs:14`, module list `102-114`), so this is what a future importer would have
to map — not a list of bugs.*

## Summary

`nuovaera.psd` is a 1080 × 1080 RGB / 8-bit document (3 channels, 72 ppi, **untagged** ICC,
Photoshop 2025) with 127 flat layers and **no groups**: 120 pixel layers, three Hue/Saturation
adjustments, one Invert, one Threshold (level 128), one Brightness/Contrast (0/0) and one embedded
Smart Object (`IMG_2065-.jpg`, 800 × 533, no Smart Filters). 73 layers are hidden; three are
clipping masks (`Layer 106`, `Layer 105`, `Layer 46`); two layers are partly transparent
(`Layer 46` 77/255, `Layer 89` 97/255); two carry a colour label (`Layer 45`, `Layer 24`), and six
carry locks — `Background` (transparency + position) plus five "lock all" layers (`Layer 45`,
`Layer 84`, `Layer 36`, `Layer 89`, `Layer 24`). Six layers carry layer styles: four Drop Shadows,
four Gradient Overlays, three Outer Glows, three Strokes and one Bevel & Emboss (`Layer 76`), all
enabled. Document level: an empty pattern section, `CAI ` (C2PA), `OCIO`/`GenI`, the embedded
`IMG_2065-.jpg` link buffer, the red @50 % Smart Filter mask overlay (`FMsk`), `cinf` (compCore),
one horizontal guide above the canvas, an 18 px grid, one auto slice, XMP + EXIF + IPTC + print
metadata, a white document background colour — and no text, patterns, alpha channels, saved paths,
layer comps, notes, colour samplers, vector masks, Smart Filters or blend-if ranges.

## Feature inventory against Fotox

| Feature | Used in file (count, example layers) | Fotox | Evidence (file:line) | What an importer would lose |
| --- | --- | --- | --- | --- |
| Document header: 1080 × 1080, RGB, 8 bpc, 3 channels | 1 document | yes | `document.rs:10-12`, `color.rs:9` (`BitDepth::U8`), `color.rs:57` | Nothing. |
| Resolution 72 ppi (unit: ppi) | 1 | yes (metadata) | `document.rs:14` | Nothing (ppi is metadata). |
| ICC profile — **untagged** (`ICC_UNTAGGED_PROFILE = 1`, no ICC resource) | 1 | partial | `color.rs:34` | `ColorProfile` has no "untagged/unknown" state; the importer must assume a working space, so display colours can differ from Photoshop's unless that assumption matches the user's. |
| Layer tree: 127 flat layers, no groups | 127 | yes | `layer.rs:164-220` | Nothing structurally. |
| Layer name and layer id (`luni`, `lyid`) | all 127 | yes | `layer.rs:319-320` | Nothing. |
| Visibility (73 hidden) | 127 | yes | `layer.rs:321` | Nothing. |
| Layer opacity below 100 % (`Layer 46` 77/255 ≈ 30 %, `Layer 89` 97/255 ≈ 38 %) | 2 | yes | `layer.rs:323` | Nothing. |
| Fill opacity | all 100 % | yes (unused) | `layer.rs:325` | Nothing. |
| Blend modes: 10 distinct (`norm` ×110, `lite` ×4, `mul` ×3, `lgCl` ×3, `hLit` ×2, `vLit`, `pLit`, `hue`, `fsub`, `dkCl`) | 127 | yes | `blend.rs:17-52` | Nothing: `fsub` is `Subtract` (`blend.rs:46`), `dkCl` `DarkerColor` (28), `lgCl` `LighterColor` (34), `pLit` `PinLight` (41), `vLit` `VividLight` (39), the rest as named. |
| Clipping masks (record flag) | 3 (`Layer 106`, `Layer 105`, `Layer 46`) | yes | `layer.rs:328` | Nothing. |
| Layer locks (`lspf`) | 6: `Background` 13 (transparency + position), `Layer 45` / `Layer 84` / `Layer 36` / `Layer 89` / `Layer 24` = 2147483648 (lock all) | partial | `layer.rs:329,332,333` | Fotox has no "lock all" flag and no nesting lock: lock-all must be fanned out to the three locks it does have. The result behaves the same for editing, but the saved distinction is lost. |
| Layer colour label (`lclr` sheet colour 1) | 2 (`Layer 45`, `Layer 24`) | no field | `layer.rs:318-346` | The Layers-panel colour label only. |
| Pixel layer masks | 6 empty default masks, **no mask pixels** | yes | `layer.rs:16-23`, `layer.rs:334` | Nothing: the masks are the default ones. Mask density/feather have no field (`layer.rs:16-23`) but are unset here. |
| Vector masks (`vmsk`/`vogk`) | none | yes (unused) | `layer.rs:342,360`, `path.rs:90` | Nothing in this file. |
| Adjustment layer: Invert (`nvrt`) | 1 (`Invert 1`) | yes | `layer.rs:59` | Nothing. |
| Adjustment layer: Threshold (`thrs`) | 1 (`Threshold 1`, level 128) | yes | `layer.rs:65` | Nothing. |
| Adjustment layer: Brightness/Contrast (`brit`) | 1 (`Brightness/Contrast 1`, brightness 0 / contrast 0, not legacy, not Lab-only) | yes | `layer.rs:34-38` | Nothing for these values; Photoshop's "Use Legacy" toggle has a field (`legacy`), the Lab-only mode does not (unused here). |
| Adjustment layers: Hue/Saturation (`hue2`) | 3 (`Hue/Saturation 1`: master +115 hue; `Hue/Saturation 2`: master +21/+11 and **cyans band hue 166**; `Hue/Saturation 3`: master +3/+6; colourise off in all three) | partial | `layer.rs:52-58` — master-only by design ("Master only for M2; per-range editing … in M4"); `colorize` 57 | `Hue/Saturation 2`'s cyan-range hue shift (166) is dropped: Fotox applies the master hue/saturation/lightness only. That is one real colour difference in the composite. The colourise checkbox itself is representable. |
| Smart Object, embedded content and transform (`PlLd`, `SoLd`) | 1 (`IMG_2065-`, jpg 230,620 B, 800 × 533, transform a 800 × 533 rectangle at (140, 273.5)) | yes | `layer.rs:217-220`, `smart.rs:21-33,58-64`, `transform.rs:91` | Nothing: the transform is an axis-aligned rectangle (`Mapping::Affine`), the warp mesh is the identity grid. |
| Smart Object source is a *linked file* with embedded data (`lnk2` `liFD`, `IMG_2065-.jpg`) | 1 | partial | `smart.rs:28-30` (`linked` path + `linked_mtime`) | Fotox's linked object is a path on disk plus mtime; the PSD stores the JPEG inline and has no timestamp. The importer must extract the bytes to disk or import them as embedded content, losing the link. |
| Smart Filters | none | yes (unused) | `smart.rs:38-46`, `ops.rs:24` | Nothing in this file. |
| Layer effects, all enabled: Drop Shadow ×4 (`Layer 117`, `Layer 60`, `Layer 115`, `Layer 116`), Gradient Overlay ×4 (`Layer 117`, `Layer 51`, `Layer 115`, `Layer 116`), Outer Glow ×3 (`Layer 117`, `Layer 115`, `Layer 116`), Stroke ×3 (`Layer 117`, `Layer 115`, `Layer 116`), Bevel & Emboss ×1 (`Layer 76`) | 15 effect instances on 6 layers | yes | `styles.rs:326` (`LayerStyles`), `DropShadow` `styles.rs:74`, `OuterGlow` `88`, `ColorOverlay` `112`, `Stroke` `120`, `BevelEmboss` `140`, `GradientOverlay` `184` | Effect parameters Photoshop writes but Fotox has no field for — contour (`TrnS`), noise (`Nose`), anti-alias (`AntA`), glow technique/range/jitter (`GlwT`/`Inpr`/`ShdN`), bevel texture/shape — are all at Photoshop defaults in this file, so nothing visible is lost. Multiplicity is also flat (`styles.rs:328-336`, one `Option` per type) but no layer here repeats an effect. |
| Effect blend modes used: `Mltp`, `Nrml` | 15 | yes | `blend.rs:25,21` | Nothing. |
| Stroke effects: all solid-colour (`Styl`/`PntT`/`Sz`/`Clr`) | 3 | yes | `styles.rs:120-127`, `StrokePosition` `styles.rs:67` | Nothing: Fotox's stroke is one solid colour; a gradient/pattern stroke (not used here) would have no home. |
| Gradient Overlay placement: `Algn = true` (align with layer), scale 55 %, dither/reverse off | all 4 overlays | partial | `styles.rs:184-193`, `gradient.rs:274-292` (canvas-centred placement, no `align` field) | Fotox anchors the gradient to the canvas; Photoshop anchors it to the layer's bounds. On layers smaller than the canvas the gradient's span/offset differ and the setting itself cannot be stored. |
| Document Global Light angle 90° | 1 | yes | `document.rs:28` | Nothing. |
| Document Global Light altitude 30° | 1 | partial | no document field (`document.rs:9-56`); per-effect `BevelEmboss.altitude` `styles.rs:152` | `Layer 76`'s bevel uses the global light; its altitude can be taken from the effect, but the document-level value has no home. |
| Guides (`GRID_AND_GUIDES_INFO`) | 1 horizontal guide at y = −181.59375 px (above the canvas) | yes | `document.rs:30`, `document.rs:58` | Nothing (representable; the guide is off-canvas). Note psd-tools returns the raw unsigned 1/32-px value and ag-psd's `readUint32()/32` reports 134,217,546.4 px — read it as signed. |
| Grid: 18 px (576 units) | 1 setting | no | `document.rs:9-56` has no grid field | The document's grid spacing (view-only preference). |
| Slices (`SLICES`) | 1 auto slice covering the whole canvas (named "nuovaera"), no user slices | yes | `document.rs:43`, `comps.rs:80`, `comps.rs:89` | The slice record's URL/alt/HTML fields have no home but are empty; the auto slice is regenerated. |
| Layer comps, alpha channels, saved paths, notes/counts/colour samplers, patterns, text layers, artboards, blend-if ranges | none in this file | yes (unused) / no field | `comps.rs:13,33`, `channel.rs:13` + `document.rs:35`, `path.rs:96` + `document.rs:38`, `annotations.rs:7,17,29,35` + `document.rs:48`, `pattern.rs:16` + `document.rs:33`, `layer.rs:196` + `text.rs:22`, `layer.rs:345,350`; blend-if has no field (`layer.rs:318-346`) | Nothing: the file uses none of them (`Patt` is empty, all blending ranges are Photoshop's defaults). |
| Layer style-blending flags `clbl` = 1, `infx` = 0, `knko` = 0 | all layers (defaults) | no field | `layer.rs:318-346` | Nothing visually (Photoshop defaults). |
| `CgEd` CONTENT_GENERATOR_EXTRA_DATA | 1 layer | no field | `layer.rs:318-346` | The record that a layer was produced by Content Generator (pixels are baked in the file). Metadata only. |
| Layer metadata blocks `lnsr` (name source), `shmd` (timestamps), `fxrp` (non-zero on 77 layers), `lclr`, `clbl`/`infx`/`knko` | all layers | no field | `layer.rs:318-346` | Panel metadata: name source, edit times, transform origins, colour labels. No pixels lost (`fxrp` matters only for later transform edits in Photoshop's sense). |
| Document metadata: XMP (14,751 B), EXIF (306 B), IPTC (15 B), CAPTION_DIGEST (16 B), empty URL_LIST, VERSION_INFO, IDS seed 132, LAYER_STATE_INFO 126, LAYER_SELECTION_IDS [131], LAYER_GROUP_INFO, LAYER_GROUPS_ENABLED_ID, PIXEL_ASPECT_RATIO 1.0, BACKGROUND_COLOR (white), THUMBNAIL_RESOURCE, UNKNOWN_1092 (25 B), PRINT_INFO_CS5 / PRINT_STYLE / PRINT_FLAGS / PRINT_FLAGS_INFO / PRINT_SCALE, COLOR_HALFTONING_INFO, COLOR_TRANSFER_FUNCTION | 22 resources | no (selection: yes) | `document.rs:9-56` (no metadata field); layer selection maps to `document.rs:18`; `.fxd` persists annotations (`fx-io/src/fxd/manifest.rs:61`) | Metadata only: XMP/EXIF/IPTC captions, print/export settings, halftone/transfer functions, thumbnail, background colour, layer-panel state. Nothing that affects rendering. |
| Document blocks `CAI ` (77 B, C2PA), `OCIO`, `GenI`, `cinf` (COMPOSITOR_INFO 1.3 / PS 26.5, engine **compCore**), `FMsk` (red @50 % filter-mask overlay) | 5 blocks | no | `document.rs:9-56`; `FMsk` relates to `smart.rs:38-46` (no mask field) | Provenance and engine metadata; the Smart Filter mask overlay colour is unrepresentable, but no Smart Filter exists in this file. |
| Composite/flattened preview and thumbnail | 1 composite section | yes | `fx-io/src/fxd/manifest.rs:75,155`, `container.rs:92` | Nothing: Fotox stores its own composite preview. |

## The `no` and `partial` items, ordered by impact on this file

1. **Hue/Saturation per-range edits** — `partial` (`layer.rs:52-58`, master-only by design).
   `Hue/Saturation 2` stores a hue shift of 166 for the **cyans** range on top of its master
   +21/+11; Fotox has no per-range fields, so that band's shift is lost — a real colour difference
   in the composite. (`Hue/Saturation 1` and `3` have all bands at 0, so only this one layer is
   affected; the master hue/saturation/lightness and the colourise checkbox survive.)
2. **Gradient Overlay `Algn = true` on 4 layers** — `partial` (`styles.rs:184-193`,
   `gradient.rs:274-292`). Photoshop anchors the overlay gradient to the layer's bounds, Fotox to
   the canvas, and there is no "align" field: on layers smaller than 1080 × 1080 the gradient span
   differs. Affects `Layer 117`, `Layer 51` (blend Colour), `Layer 115`, `Layer 116`.
3. **Lock-all (`lspf` 2147483648) on five layers** — `partial` (`layer.rs:329,332,333`). Fotox has
   transparency/pixels/position locks but no single "lock all" (and no nesting lock): the importer
   must set all three, and the saved state is not identical.
4. **Layer colour labels on 2 layers** — `no field` (`layer.rs:318-346`). `Layer 45` and `Layer 24`
   lose their panel colour.
5. **Linked Smart Object with embedded bytes** — `partial` (`smart.rs:28-30`). `IMG_2065-.jpg`
   (230,620 B) is stored in `lnk2`; Fotox's linked object is a file path plus mtime, so the importer
   must extract it to disk or store it as embedded content and lose the link (no timestamp exists
   to preserve).
6. **Untagged ICC profile** — `partial` (`color.rs:34`). No profile is named in the file and Fotox
   cannot record "unspecified": the importer must choose a working space.
7. **Effect detail fields with no home (all at defaults here)** — `no fields`
   (`styles.rs:74-210`): contour, noise, anti-alias, glow technique/range/jitter, bevel
   texture/shape. Nothing is lost in this file (verified `Nose` 0, `TrnS` "Linear").
8. **`CgEd` Content Generator provenance** — `no field` (`layer.rs:318-346`). Which layer was
   generated; no pixels lost.
9. **Layer/document metadata without a field** — `no` (`layer.rs:318-346`, `document.rs:9-56`):
   `lnsr`, `shmd`, `fxrp` (77 layers), XMP/EXIF/**IPTC**/CAPTION_DIGEST/URL_LIST, print blocks,
   halftone/transfer, `BACKGROUND_COLOR`, IDS seed, LAYER_STATE_INFO, LAYER_GROUP_INFO, selection
   ids, thumbnail, `PIXEL_ASPECT_RATIO`, `UNKNOWN_1092`, `CAI `, `OCIO`, `GenI`, `cinf`.
   Metadata only.
10. **Grid 18 px** — `no` (`document.rs:9-56`). A view preference.
11. **Global-light altitude** — `partial`, but only `Layer 76`'s bevel is affected and its own
    altitude is storable (`styles.rs:152`).
