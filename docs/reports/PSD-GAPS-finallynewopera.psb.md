# PSD gap analysis — finallynewopera.psb

*Method: `%USERPROFILE%\Downloads\finallynewopera.psb` read only, once with psd-tools 1.20.0 and once
with ag-psd 31.0.2 (2,684,248,289 bytes, sha256 `c3dce2c0…3ed97e0af`; a **PSB**, header version 2).
Parser coverage and every place the two parsers disagree are in
[PSD-GAPS-PARSERS.md](PSD-GAPS-PARSERS.md); the raw dumps behind every row are in
`XuanZhi9\psd-gap-analysis\` (outside this repo). All Fotox evidence is from `crates/fx-core` (paths
relative to the repo root). `crates/fx-io/src` has no PSD/PSB reader
(`crates/fx-io/src/lib.rs:14` — "PSD/PSB import — M7"; module list `102-114`), so this is what a
future importer would have to map — not a list of bugs. (Tooling note: Node's `fs.readFileSync`
refuses files over 2 GiB, so the ag-psd inventory script now reads in 256 MiB chunks.)*

## Summary

`finallynewopera.psb` is a 3992 × 4990 RGB / 8-bit PSB (3 channels, 72 ppi, **untagged** ICC,
Photoshop 2025) with **479 layers**: 366 pixel, 19 embedded Smart Objects, 14 shape layers, 13
groups (12 pass-through, `Group 1` on Normal; both open and closed folders), and a full set of
adjustments — 14 Gradient Map, 12 Hue/Saturation, 12 Curves, 11 Invert, 6 Posterize, 2 Threshold, 2
Color Balance, 1 each of Levels, Exposure, Selective Color, Channel Mixer and Vibrance — plus 2
solid-colour fill layers and 1 Gradient Fill. 293 layers are hidden, 26 are clipping masks, 15 have
opacity below 100 % (down to 10/255) and 3 have fill opacity 0 (`iOpa`); 29 layers are locked "all".
Layer styles are used heavily: **49 enabled Pattern Overlays** (the file's defining trick), 27 Bevel
& Emboss, 20 Strokes, 15 Drop Shadows, 8 Outer Glows, 6 Inner Glows, 5 Gradient Overlays and 1
Colour Overlay, plus 45 present-but-disabled slots. The document carries a real pattern library (5
patterns, two larger than 2048 px), 18 embedded `lnk2` Smart Object sources totalling ≈121 MB (the
largest `Layer 303.psb` at 54.3 MB), 4 `warpCustom` Smart Object warps (two of them genuinely
deformed), exactly one Smart Filter (`Group 8`, Add Noise, with its mask enabled), 14 vector masks
(3–64 subpaths, none with live-shape data), 82 **empty** layer-mask records, and the usual
`CAI `/`OCIO`/`GenI`/`cinf`/`FMsk` plus a ≈4.9 MB-per-channel `FEid` cached smart-filter render. It
has no text layers, alpha channels, saved paths, layer comps, colour samplers, artboards, guides or
blend-if ranges.

## Feature inventory against Fotox

| Feature | Used in file (count, example layers) | Fotox | Evidence (file:line) | What an importer would lose |
| --- | --- | --- | --- | --- |
| PSB container (header version 2, 8-byte lengths), 3992 × 4990, RGB, 8 bpc, 3 channels | 1 document | yes (model) | `document.rs:10-12`, `color.rs:9` (`BitDepth::U8`), `color.rs:57` | Nothing in the model; there is no PSD/PSB reader to carry it (`crates/fx-io/src/lib.rs:14`). |
| Resolution 72 ppi (unit: ppi) | 1 | yes (metadata) | `document.rs:14` | Nothing (ppi is metadata). |
| ICC profile — **untagged** (`ICC_UNTAGGED_PROFILE = 1`, no ICC resource) | 1 | partial | `color.rs:34` | `ColorProfile` has no "untagged/unknown" state; the importer must assume a working space, so colours can differ from Photoshop's. |
| Layer tree: 479 layers, 13 groups (12 pass-through, `Group 1` Normal; open *and* closed folders) | 479 | yes | `layer.rs:164-220` (`Group` 172, `expanded` 174), `blend.rs:19` (`PassThrough`), `blend.rs:21` (`Normal`) | Nothing: `expanded` takes the `lsct` open/closed kind (both kinds occur here). |
| Layer name and layer id (`luni` 479, `lyid` 479) | all 479 | yes | `layer.rs:319-320` | Nothing. |
| Visibility (293 hidden) | 479 | yes | `layer.rs:321` | Nothing. |
| Layer opacity below 100 % | 15 (`Gradient Map 8` 10/255, `Layer 158` 66, `Layer 86` 94, `Layer 303` 158…) | yes | `layer.rs:323` | Nothing. |
| Fill opacity via the `iOpa` block (BLEND_FILL_OPACITY) | 3 at 0 (2 × `Vector Shape Fill`, `Layer 137`) | yes | `layer.rs:325` | Nothing: a fully transparent fill with visible styles is exactly what `fill` + `styles` express. |
| Blend modes: 24 distinct (`norm` ×407, `pass` ×12, `over` ×8, `colr`/`lite`/`hLit`/`lum`/`scrn` ×6 each, `hMix` ×2, `smud` ×2, `sat` ×2, `pLit` ×2, `div` ×2, `diss`, `diff`, `lbrn`, `mul`, `pLit`, `vLit`, `sLit`, `fdiv`, `fsub`, `lgCl`, `dark`, `lLit` …) | 479 | yes | `blend.rs:17-52` | Nothing: every enum in the file maps to a variant (`hMix` → `HardMix` 42, `diss` → `Dissolve` 22, `lbrn` → `LinearBurn` 27, `lum` → `Luminosity` 52, `sat` → `Saturation` 50, `diff` → `Difference` 44, `lLit` → `LinearLight` 40, `div`/`fdiv` → `Divide` 47, `fsub` → `Subtract` 46). |
| Clipping masks (`clbl` record bit) | 26 (`Curves 9`, `Threshold 2`, `Hue/Saturation 6`, `Layer 295`, `Posterize 1`, `Gradient Map 3`…) | yes | `layer.rs:328` | Nothing. |
| Layer locks (`lspf`) | 29 layers "lock all" (2147483648; also the legacy record flag `transparency_protected`) | partial | `layer.rs:329,332,333` | No single "lock all" flag and no nesting lock: the importer must set the three individual locks, so the saved state is not identical (behaviour matches for editing). |
| Pixel layer masks | 82 layers carry a mask record, **all empty** (bbox 0,0,0,0; `channel_data` size null); no mask pixels anywhere | yes | `layer.rs:16-23`, `layer.rs:334` | Nothing: the masks are Photoshop's default reveal-all records. Mask density/feather have no field (`layer.rs:16-23`) but are not set here. |
| `lmgm` LAYER_MASK_AS_GLOBAL_MASK | 1 (`Hue/Saturation 11`, which also carries `lyvr` 70) | no field | `layer.rs:318-346` | Photoshop's flag that the layer's mask acts as a global mask. psd-tools reads it as a byte; Fotox has no equivalent, so the setting (and whatever it implies for the layers below) is dropped. |
| Vector masks (`vmsk`), 3–64 subpaths each; `vogk` present but with **0 entries** | 14 (`Vector Shape Fill` ×13, `Vector Shape Fill copy`) | yes | `layer.rs:342,360-368`, `path.rs:90`, `vector.rs:35` | Nothing: geometry imports as paths; there is no live-shape/property data to lose. Fotox's `VectorMask` has no "invert" flag — all 14 masks are `inverted=false`, `fill_rule=0` (psd-tools: 0 = fill inside). |
| Shape layers, solid-colour fill (`SoCo` on 16 layers, `fill_opacity` 0 on two) | 14 shape layers + 2 colour-fill layers | yes | `layer.rs:184-192` (`Shape`), `layer.rs:177` (`SolidFill`), `vector.rs:49`, `vector.rs:67` (`Path` for import), `vector.rs:76` (`Paint::Solid`), `fill.rs:7` (`FillSource::Color`) | Nothing: none of the 14 shape layers uses a vector stroke (`layer.stroke` empty; their outlines come from Stroke *layer styles*), and shape paint is solid-only in the model (`vector.rs:76-80`), which is all this file uses. |
| Smart Objects, embedded content and transform (`PlLd`/`SoLd` ×19) | 19 (`Layer 303`/`Layer 303 copy` psb 54.3 MB, `Group 8` psb 46.6 MB, `Untitled-12` png 4.4 MB, `Texture_1` jpg 4.5 MB, `Layer 339` psb 3.7 MB, small pngs/jpgs) | yes | `layer.rs:217-220`, `smart.rs:21-33` (`doc` 23, `composite` 26), `smart.rs:58-64`, `transform.rs:91` (`Affine` 94, `Projective` 97) | Nothing for the pixels: axis-aligned rectangles and three rotated quads (e.g. `Layer 214`, `0_3_inspyrenet (2)`) all fit `Mapping::Affine`; no `nonAffineTransform` is set anywhere. Source resolution (one png is 96 ppi) has no field. |
| Smart Object free-transform warps (`warpCustom`) | 4: `Layer 214` and `Group 8` are identity grids; **`0_3_inspyrenet (2)` and `0_3_birefnet` are genuinely deformed** | partial | `transform.rs:99` (`Mapping::Warp(BezierPatch)`), `transform.rs:649` (`BezierPatch`) | Fotox can hold a warp, but Photoshop's envelope mesh carries per-point interpolation flags and is not literally a bicubic Bézier patch: an importer must fit one, so those two layers can land slightly differently. |
| Smart Object sources are *linked files* with embedded data (`lnk2`, 18 `liFD` buffers, ≈121 MB) | 18 (largest `Layer 303.psb` 54,284,636 B, `Group 8.psb` 46,610,258 B) | partial | `smart.rs:28-30` (`linked` path + `linked_mtime`) | Fotox's linked object is a path on disk plus mtime; the PSD stores the bytes inline with no timestamp. The importer must extract them to disk (keeping the link) or import them as embedded content (losing it). |
| Smart Filters (`SoLd` → `filterFX`) | 1 (`Group 8`: Add Noise, enabled; `maskEnabled` true, `maskLinked`/`extendWithWhite` set) | yes | `smart.rs:38-46` (`SmartFilter`), `ops.rs:24`, `ops.rs:56` (`AddNoise`), `smart.rs:64` (`filters_enabled`) | The filter itself (and its amount/distribution/monochromatic/seed) maps. See the mask row below. |
| Smart-filter mask (`filterMaskEnable` true; document `FMsk` overlay red @50 %) | 1 object, 0 mask pixels | partial | `smart.rs:38-46` has no mask field; `smart.rs:58-64` | No place for a per-filter mask: a painted one would be dropped. In this file the mask is the default white one (no mask channel exists), so nothing visible is lost; the overlay colour/opacity is unrepresentable. |
| Layer effects, enabled — **Pattern Overlay ×49**, Bevel & Emboss ×27, Stroke ×20, Drop Shadow ×15, Outer Glow ×8, Inner Glow ×6, Gradient Overlay ×5, Colour Overlay ×1 | 131 effect instances over 97 layers (examples: Pattern Overlay on `Layer 356`/`Layer 357`/`Group 6`/`Vector Shape Fill`; Bevel on `Layer 303 copy`/`Layer 379`; Stroke on `Group 6`/`Layer 350`; Drop Shadow on `Layer 70`/`Layer 214`; Gradient Overlay on `Layer 379`) | yes | `styles.rs:326` (`LayerStyles`), `PatternOverlay` `styles.rs:194`, `BevelEmboss` `140`, `Stroke` `120`, `DropShadow` `74`, `OuterGlow` `88`, `InnerGlow` `162`, `GradientOverlay` `184`, `ColorOverlay` `112` | All eight types exist. Real gaps: **Pattern Overlay has no angle, alignment or phase** (`styles.rs:194-201` — one `pattern` id + `scale` only) while all 49 enabled overlays are `Algn = true`, `Angl = 0`, `Scl = 100`, `phase (0,0)`; **Bevel texture** has no fields (see below); Gradient Overlay `Algn` (see below); contour (`TrnS`), noise (`Nose`), anti-alias and glow technique/range/jitter have no fields but are at Photoshop defaults here (verified: `Nose` 0 on all 29 effects that carry it, `TrnS` "Linear" on all 56). Effect *multiplicity* is flat (`styles.rs:328-336`, one `Option` per type) — no layer here repeats a type. |
| Pattern Overlay resources: the effect references a document pattern by name + GUID (`Ptrn` `Nm  ` / `Idnt`) | 49 enabled (+8 disabled) | partial | `styles.rs:194-201`, `document.rs:33` (`patterns`), `pattern.rs:16-24`, `pattern.rs:10` (`MAX_SIDE = 2048`) | Fotox can hold the pattern and the scale, but not the GUID (`Idnt`), and two of the five patterns are **larger than 2048 px** — `Layer 8.psb` 2787 × 2219 and `Layer 9.psb` 2700 × 2160 — so they cannot be stored at full size (`pattern.rs:10`: "Edit ▸ Define Pattern refuses bigger"). Seven of the 49 enabled overlays use `Layer 9.psb` (5 on visible layers) and two use `Layer 8.psb` (one visible), i.e. the oversized patterns are really in use, as is the 2048² one. |
| Bevel **texture** (`useTexture` true, `Txtr` = pattern `Tree Tile 4`, `textureDepth` 100, `InvT` false) | 1 (`Layer 303 copy`, enabled, "pillow emboss" style) | no | `styles.rs:140-160` (`BevelEmboss`: style/depth/up/size/soften/angle/light/altitude/highlight/shadow only) | Photoshop stamps a pattern into the bevel (its pattern, scale, depth, invert/"lock texture"); Fotox has none of those fields, so this layer's bevel renders flat. psd-tools' raw descriptor carries everything; ag-psd exposes `useTexture` and the texture `pattern` (name + id) but drops `textureDepth`/`InvT`. |
| Effect blend modes used: `Mltp`, `Nrml`, `Lghn`, `Scrn`, `Ovrl`, `SftL`, `CDdg`, `lighterColor`, `blendDivide`, `H   ` | 131 | yes (one unclear) | `blend.rs:25,21,40,31,36,37,32,34,47` | All map except the gradient overlay on `Layer 379`, whose descriptor carries the enum `H   ` (opacity 69 %) — psd-tools keeps it verbatim and does not map it to a `BlendMode` name; the importer needs a decision for that value (it is the only occurrence in the file). |
| Adjustment layers: **Curves** (composite channel only, 3–5 points) | 12 (`Curves 3`, `Curves 5`, `Curves 6`, `Curves 9`, `Curves 11`, `Curves 12`…) | yes | `layer.rs:44-46` (`Curves { channels: [Vec<(f32, f32)>; 4] }`) | Nothing: the file uses only `channel_id` 0 (composite); Fotox also stores per-channel curves. |
| Adjustment layer: **Levels** (composite: input 40–193, output 0–255, gamma 0.88) | 1 (`Levels 1`, 5 records) | yes | `layer.rs:40-42`, `LevelsChannel` `layer.rs:143` | Nothing: composite plus RGB fit the four `LevelsChannel` slots (the 5th record is the alpha channel, which the file has none of). |
| Adjustment layers: **Gradient Map** (two stops, `Smoo`, undithered, not reversed; one is the "yayy" gradient with 4 stops) | 14 (`Gradient Map 5`, `Gradient Map 7`, `Gradient Map 9`, `Gradient Map 10`, `Gradient Map 12`…) | yes | `layer.rs:70-73` (`GradientMap { stops, reverse }`), `GradientStop` `layer.rs:135` | Dither and interpolation method have no field; both are off/smooth here, so nothing visible is lost. |
| Adjustment layers: **Hue/Saturation** (master values only — all six per-range bands are **zero** in all 12 layers) | 12 (`Hue/Saturation 1`, `6`, `8`, `9`, `11`, `12`…) | yes | `layer.rs:53-58` (`hue`/`saturation`/`lightness`, `colorize` 57) | Nothing in this file (the per-range gap that bites `nuovaera.psd` does not apply here); a future file with per-range edits would lose them. |
| Adjustment layers: Invert ×11, **Posterize** ×6 (level 4), **Threshold** ×2 (137), **Exposure** ×1 (+2.87 stops, offset −0.0068, gamma 1.0), **Selective Color** ×1 (Reds +58/+58/0/0, relative), **Color Balance** ×2 (midtones −12/−67/+1), **Channel Mixer** ×1 (monochrome off, [100, −43, 3, 0]), **Vibrance** ×1 (vibrance 14, saturation 2) | 25 layers | yes | `Invert` `layer.rs:59`, `Posterize` `61`, `Threshold` `65`, `Exposure` `47`, `SelectiveColor` `127` (`ranges: [[f32; 4]; 9]`, `relative`), `ColorBalance` `92`, `ChannelMixer` `78`, `Vibrance` `99` | Nothing: every adjustment in this file has a filled-out variant (Selective Color's nine ranges and the relative/absolute flag included). |
| Fill layers: Gradient Fill ×1 (Linear, −84.06°, "Color to Transparent"), solid-colour fills ×2 (`Color Fill 1`, `Color Fill 2`) | 3 | yes | `layer.rs:211-214`, `fill.rs:18` (`FillLayer::Gradient`) + `fill.rs:7`, `gradient.rs:176,188,274` | The gradient's interpolation method (`Smoo`, `Intr` 4096) has no exact member of `Method` (`gradient.rs:38`) and its name/smoothness are not stored; the solid fills map exactly. |
| Pattern library (`Patt`, PATTERNS1) | 5 patterns: `0_0 (4).png` 2048², `1.psb` 1232 × 928, `Layer 8.psb` **2787 × 2219**, `Layer 9.psb` **2700 × 2160**, `Tree Tile 4` 946² | partial | `pattern.rs:16-24` (`id`/`name`/`width`/`height`/`pixels`), `pattern.rs:10` (`MAX_SIDE = 2048`), `document.rs:33` | The two patterns over 2048 px cannot be stored at native size, so pattern overlays using them (`Layer 9.psb`, `Layer 8.psb`) would be resampled or refused; the PSD's GUID `Idnt` is not kept (Fotox hashes the pixels, so identical patterns collapse into one). |
| Document Global Light angle 90° | 1 | yes | `document.rs:28` | Nothing. |
| Document Global Light altitude 30° | 1 | partial | no document field (`document.rs:9-56`); per-effect `BevelEmboss.altitude` `styles.rs:152` | The 27 bevels use the global light; each can take the value from its own effect, but the document-level value has no home. |
| Guides / grid (`GRID_AND_GUIDES_INFO`) | 0 guides; grid 18 px (576 units) | guides yes (unused), grid no | `document.rs:30,58`; `document.rs:9-56` has no grid field | The grid spacing, a view-only preference (the file has no guides). |
| Slices (`SLICES`) | 1 auto slice covering the whole canvas (named "finallynewopera"), no user slices | yes | `document.rs:43`, `comps.rs:80`, `comps.rs:89` | The slice record's URL/alt/HTML fields have no home but are empty; the auto slice is regenerated. |
| Layer comps, alpha channels, saved paths, colour samplers, text layers, artboards | none: no `LAYER_COMPS` (1065), `ALPHA_NAMES_*` (1006/1045) or `COLOR_SAMPLERS_RESOURCE` (1073) resource, no `tySh` block, no artboard section | yes (unused) | `comps.rs:13,33`, `channel.rs:13` + `document.rs:35`, `path.rs:96` + `document.rs:38`, `annotations.rs:7,17,29,35` + `document.rs:48`, `layer.rs:196` + `text.rs:22`, `layer.rs:345,350` | Nothing: the file uses none of them. (Notes: psd-tools has no notes API at all, so nothing can be said about those either way.) |
| Layer style-blending flags: `clbl` = 1 (470), `infx` = 0 (470), `knko` = 0 (470); blend-if ranges default on all 479 layers | all layers (Photoshop defaults) | no fields | `layer.rs:318-346` | Nothing visually: these are the defaults (styles clipped to the layer, interior effects blended, no knockout, no blend-if), but a file that changed them would lose the setting. |
| Layer metadata blocks: `lnsr` (462), `lyvr` (7: 130 ×4, 160, 110, 70), `shmd` (479 timestamps), `fxrp` (479 reference points), `lclr` (dozens of colour labels) | most layers | no fields | `layer.rs:318-346` | Panel metadata: name source, layer version, edit times, transform origins, colour labels. No pixels lost (`fxrp` only matters if a transform is later re-edited the way Photoshop would). |
| Document metadata: XMP (14,751 B), EXIF (306 B), IPTC (15 B), CAPTION_DIGEST (16 B), empty URL_LIST, VERSION_INFO, IDS seed 982, LAYER_STATE_INFO 490, LAYER_SELECTION_IDS [962], LAYER_GROUP_INFO, LAYER_GROUPS_ENABLED_ID, PIXEL_ASPECT_RATIO 1.0, BACKGROUND_COLOR (white), THUMBNAIL_RESOURCE, UNKNOWN_1092 (16 B), PRINT_INFO_CS5 / PRINT_STYLE / PRINT_FLAGS / PRINT_FLAGS_INFO / PRINT_SCALE, COLOR_HALFTONING_INFO, COLOR_TRANSFER_FUNCTION | 22 resources | no (selection: yes) | `document.rs:9-56` (no metadata field); layer selection maps to `document.rs:18`; `.fxd` persists annotations (`fx-io/src/fxd/manifest.rs:61`) | Metadata only: XMP/EXIF/IPTC captions, print/export settings, halftone/transfer functions, thumbnail, document background colour, layer-panel state. Nothing that affects rendering. |
| Document blocks `CAI ` (77 B, C2PA), `OCIO`, `GenI`, `cinf` (COMPOSITOR_INFO 1.3 / PS 26.5, engine compCore), `FMsk` (red @50 %), `FEid` (≈4.9 MB per channel of cached smart-filter render) | 6 blocks | no | `document.rs:9-56`; `FMsk` relates to `smart.rs:38-46`; Fotox recomputes filters (`ops.rs:56`) | Provenance and engine metadata; the Smart Filter mask overlay colour is unrepresentable (only one object has a filter, and its mask has no pixels); `FEid` is derived data Fotox would recompute — it is the only record of Photoshop's own render, useful to diff against. |
| Composite/flattened preview and thumbnail | 1 composite section | yes | `fx-io/src/fxd/manifest.rs:75,155`, `container.rs:92` | Nothing: Fotox stores its own composite preview. |

## The `no` and `partial` items, ordered by impact on this file

1. **Pattern Overlay is missing angle / alignment / phase — 49 enabled layers** — `partial`
   (`styles.rs:194-201`, `document.rs:33`, `pattern.rs:10,16`). These 49 overlays are what makes the
   file's textures; Fotox stores the pattern id and scale but not `Algn` (true on all 49), `Angl`
   (0 here) or `phase` (0,0 here), so the pattern's registration can shift relative to Photoshop,
   and two of the five patterns (`Layer 8.psb` 2787 × 2219, `Layer 9.psb` 2700 × 2160) exceed
   `pattern::MAX_SIDE = 2048` and cannot be kept at native size — and they are used: 7 of the 49
   overlays reference `Layer 9.psb` (5 visible) and 2 reference `Layer 8.psb`. The pattern GUID
   (`Idnt`) is also dropped in favour of a pixel hash.
2. **Bevel & Emboss *texture*** — `no` (`styles.rs:140-160`). `Layer 303 copy` has an enabled bevel
   ("pillow emboss" style) with `useTexture true`, `textureDepth 100`, `InvT false`, stamping
   pattern `Tree Tile 4` into the bevel with its own scale/depth/invert — and Fotox's `BevelEmboss`
   has no texture fields at all. ag-psd exposes `useTexture` and the texture `pattern` (name + id)
   but has no field for `textureDepth`/`InvT` (it prints `Invalid effect key`, not through the `log`
   callback), so those two values live only in psd-tools' raw descriptor.
3. **Deformed Smart Object warp meshes** — `partial` (`transform.rs:99,649`). `0_3_inspyrenet (2)`
   and one `0_3_birefnet` carry `warpCustom` envelope meshes that are genuinely non-uniform (the
   other two, `Layer 214` and `Group 8`, are identity grids). Fotox's `Mapping::Warp(BezierPatch)`
   is the closest fit, but the per-point mesh flags have to be fitted, so placement/resampling can
   differ slightly.
4. **Gradient Overlay alignment (`Algn = true`) on 5 layers** — `partial`
   (`styles.rs:184-193`, `gradient.rs:274-292`). Photoshop anchors the gradient to each layer's
   bounds, Fotox to the canvas, with no "align" field; one of the five (`Layer 379`, 69 % opacity)
   additionally uses the unmapped blend enum `H   `.
5. **29 layers locked "all"** — `partial` (`layer.rs:329,332,333`). No single "lock all" flag and no
   nesting lock: the importer sets the three it has.
6. **82 layer masks with no density/feather fields** — `partial in principle`
   (`layer.rs:16-23`). Every mask record in the file is empty (bbox 0, no channel data), so nothing
   is lost here, but Photoshop's mask density/feather parameters have nowhere to go.
7. **`lmgm` Layer Mask as Global Mask on `Hue/Saturation 11`** — `no field`
   (`layer.rs:318-346`). The flag that makes the layer's mask global is dropped.
8. **Linked Smart Objects with embedded bytes** — `partial` (`smart.rs:28-30`). 18 buffers totalling
   ≈121 MB (largest `Layer 303.psb` 54.3 MB) are stored inline with no timestamp, while Fotox's
   linked object is a path + mtime: the importer must extract them to disk or import them embedded
   and lose the link.
9. **Untagged ICC profile** — `partial` (`color.rs:34`). Nothing names a profile and Fotox cannot
   record "unspecified".
10. **Smart-filter mask** — `partial` (`smart.rs:38-46`). `Group 8`'s Add Noise filter declares
    `maskEnabled` with `maskLinked`/`extendWithWhite`, and the document carries the `FMsk` overlay;
    no mask pixels exist, so nothing visible is lost, but the mask itself has no home.
11. **Effect detail fields with no home (all at defaults here)** — `no fields`
    (`styles.rs:74-210`): contour, noise, anti-alias, glow technique/range/jitter, layered bevel
    extra options. Nothing is lost in this file (verified `Nose` 0 everywhere, `TrnS` "Linear"
    everywhere), but a file using a custom contour would lose it.
12. **Layer/document metadata without a field** — `no` (`layer.rs:318-346`, `document.rs:9-56`):
    `lnsr`, `lyvr` (130/160/110/70), `shmd` timestamps, `fxrp` (all 479 layers), colour labels,
    XMP/EXIF/**IPTC**/CAPTION_DIGEST/URL_LIST, print blocks, halftone/transfer,
    `BACKGROUND_COLOR`, IDS seed, LAYER_STATE_INFO, LAYER_GROUP_INFO, selection ids, thumbnail,
    `PIXEL_ASPECT_RATIO`, `UNKNOWN_1092`, `CAI `, `OCIO`, `GenI`, `cinf`. Metadata only.
13. **Grid 18 px / Global-light altitude** — `no` / `partial` (`document.rs:9-56`,
    `styles.rs:152`). A view preference and, for the altitude, a value the bevels can carry
    individually.
14. **Pattern GUIDs and per-filter overlay phases** — `partial` (`pattern.rs:16-24`,
    `styles.rs:194-201`). Round-tripping a PSD would lose the pattern identity/phase bookkeeping
    even where the pixels survive.
