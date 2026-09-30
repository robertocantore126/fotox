# PSD gap analysis — Untitled-1weird.psd

*Method: `%USERPROFILE%\Downloads\Untitled-1weird.psd` read only, once with psd-tools 1.20.0 and
once with ag-psd 31.0.2 (207,118,363 bytes, sha256 `e38bdf82…a2378214`). Parser coverage and every
place the two parsers disagree are in [PSD-GAPS-PARSERS.md](PSD-GAPS-PARSERS.md); the raw dumps
behind every row are in `XuanZhi9\psd-gap-analysis\` (outside this repo). All Fotox evidence is from
`crates/fx-core` (paths relative to the repo root). `crates/fx-io/src` has no PSD reader
(`crates/fx-io/src/lib.rs:14`, module list `102-114`), so this is what a future importer would have
to map — not a list of bugs.*

## Summary

`Untitled-1weird.psd` is a 2160 × 2700 RGB / 8-bit document (3 channels, 72 ppi, **untagged** ICC,
Photoshop 2025) with 39 layers: 28 pixel, 4 **shape layers** (a solid-colour fill each, defined by a
`vmsk` vector mask — three copies of `Color Fill 3` with 449 subpaths and `Color Fill 2` with 5),
3 embedded Smart Objects (`0_0 (2).png` 1024 × 1024, `Color Fill 3.psb` 8282 × 6146,
`Color Fill 1.psb` 2276 × 2134, no Smart Filters), 2 pass-through groups, 1 Gradient Fill (Radial,
angle −35.37°, scale 134 %, 4 colour + 2 opacity stops, named "yayy") and 1 Invert; 22 layers are
hidden, none is clipping and none is partly transparent. `Background` carries the
transparency+position lock and `Layer 7`, `Layer 1`, `Layer 10` are "lock all". Layer styles: Satin
on `Group 1`, Outer Glow + Bevel & Emboss + Stroke on `Color Fill 2`, all enabled. Document level:
an empty pattern section, `CAI ` (C2PA), `OCIO`/`GenI`, three embedded `lnk2` link buffers, the red
@50 % filter-mask overlay (`FMsk`), `cinf` (compCore), an 18 px grid and **no guides**, one auto
slice, XMP + EXIF + IPTC + print metadata — and no text, patterns, alpha channels, saved paths,
layer comps, notes, colour samplers, Smart Filters, pixel-mask pixels or blend-if ranges.

## Feature inventory against Fotox

| Feature | Used in file (count, example layers) | Fotox | Evidence (file:line) | What an importer would lose |
| --- | --- | --- | --- | --- |
| Document header: 2160 × 2700, RGB, 8 bpc, 3 channels | 1 document | yes | `document.rs:10-12`, `color.rs:9` (`BitDepth::U8`), `color.rs:57` | Nothing. |
| Resolution 72 ppi (unit: ppi) | 1 | yes (metadata) | `document.rs:14` | Nothing (ppi is metadata). |
| ICC profile — **untagged** (`ICC_UNTAGGED_PROFILE = 1`, no ICC resource) | 1 | partial | `color.rs:34` | `ColorProfile` has no "untagged/unknown" state; the importer must assume a working space, so display colours can differ from Photoshop's. |
| Layer tree: pixel / shape / group / smart object / fill / adjustment | 39 layers: 28 pixel, 4 shape, 3 Smart, 2 groups, 1 Gradient Fill, 1 Invert | yes | `layer.rs:164-220` (`Pixel` 167, `Group` 172, `Shape` 184, `Adjustment` 176, `FillLayer` 211, `Smart` 217) | Nothing structural. |
| Layer name and layer id (`luni`, `lyid`) | all 39 | yes | `layer.rs:319-320` | Nothing. |
| Visibility (22 hidden) | 39 | yes | `layer.rs:321` | Nothing. |
| Layer opacity / fill opacity | all 100 % | yes (unused) | `layer.rs:323,325` | Nothing to lose here. |
| Blend modes: 8 distinct (`norm` ×29, `smud` ×3 = Exclusion, `vLit`, `pass` ×2, `scrn`, `dark`, `fdiv` = Divide, `mul`) | 39 | yes | `blend.rs:17-52` | Nothing: `smud` is `Exclusion` (`blend.rs:45`), `fdiv` `Divide` (47), `vLit` `VividLight` (39). |
| Groups: two, both pass-through and open (`lsct` kind 1 with `blend pass`) | 2 (`Group 1`, `Group 2`) | yes | `layer.rs:172-175` (`expanded` 174), `blend.rs:19` | Nothing. |
| Clipping masks | none | yes (unused) | `layer.rs:328` | Nothing. |
| Layer locks (`lspf`) | 4: `Background` 13 (transparency + position); `Layer 7`, `Layer 1`, `Layer 10` = 2147483648 (lock all) | partial | `layer.rs:329,332,333` | Fotox has no "lock all" flag and no nesting lock: lock-all must be fanned out to the three locks it does have. Behaviourally equivalent, but the saved distinction is lost. |
| Pixel layer masks | 1 empty default mask, **no mask pixels** | yes | `layer.rs:16-23`, `layer.rs:334` | Nothing. Mask density/feather have no field (`layer.rs:16-23`) but are unset here. |
| Vector masks (`vmsk`) | 4 (`Color Fill 3` ×3, 449 subpaths each; `Color Fill 2`, 5 subpaths; psd-tools reports `initial_fill_rule` 0 for all four — its docs: 0 = fill inside, 1 = fill outside — plus `inverted=False`, `disabled=False`) | yes | `layer.rs:342,360-368` (`VectorMask`: path/enabled/feather/density), `path.rs:90` (`Path`), `vector.rs:35` (`PathEl`) | Fotox's `VectorMask` has no "invert" flag (none is used here); the paths themselves import as real geometry. Mask density/feather are unset in the file. |
| Vector origination data (`vogk`) | 4 masks, **0 entries each** | yes (unused) | `layer.rs:360` | Nothing: there is no live-shape/property data in this file to carry. |
| Shape layers: solid fill (`SoCo`) + vector mask geometry | 4 (`Color Fill 3` ×3, `Color Fill 2`) | yes | `layer.rs:184-192` (`Shape`), `vector.rs:49` (`VectorShape`), `vector.rs:67` (`Path` variant, "free path (PSD import…)"), `vector.rs:76` (`Paint::Solid`), `layer.rs:177` (`SolidFill`) | Nothing for these files; note the model's shape paint is solid only (`vector.rs:76-80`), so a gradient/pattern shape fill (not used here) would have no home, and the live-shape tool parameters (radius/corner data) live in `vogk`, which is empty. |
| Smart Objects, embedded content and transform (`PlLd`, `SoLd`) | 3 (`0_0 (2)` png 1024 × 1024, `Color Fill 3` psb 8282 × 6146, `Color Fill 1` psb 2276 × 2134) | yes | `layer.rs:217-220`, `smart.rs:21-33,58-64`, `transform.rs:91` | Nothing for the pixels: all three transforms are axis-aligned rectangles and the warp meshes are identity grids (identical x/y rows), so `Mapping::Affine` is exact. |
| Smart Object sources are *linked files* with embedded data (`lnk2` `liFD`: `Color Fill 1.psb`, `Color Fill 3.psb`, `0_0 (2).png`) | 3 | partial | `smart.rs:28-30` (`linked` path + `linked_mtime`) | Fotox's linked object is a path on disk plus mtime; the PSD stores the bytes inline with no timestamp. The importer must extract them to disk (keeping the link) or import them as embedded content (losing it). |
| Smart Filters | none | yes (unused) | `smart.rs:38-46`, `ops.rs:24` | Nothing in this file. |
| Layer effects, enabled: Satin ×1 (`Group 1`), Outer Glow ×1 + Bevel & Emboss ×1 + Stroke ×1 (`Color Fill 2`) | 4 effect instances on 2 layers | yes | `styles.rs:326` (`LayerStyles`), `Satin` `styles.rs:172`, `OuterGlow` `88`, `BevelEmboss` `140`, `Stroke` `120` | Effect parameters Photoshop writes but Fotox has no field for — the satin's anti-alias flag (`AntA true`), contours (`TrnS`), noise (`Nose`), glow technique/range/jitter, bevel texture/shape — have no home. All the ones that change pixels at the values this file uses are covered (`Nose` 0, contour "Linear", glow technique "Softer"), so only the satin's anti-alias flag is dropped outright. Effects on a *group* layer (`Group 1`) are fine: `Layer.styles` is on every layer (`layer.rs:337`). |
| Effect blend modes used: `Mltp` (satin), `Nrml` | 4 | yes | `blend.rs:25,21` | Nothing. |
| Stroke effects: solid-colour only (`Styl`/`PntT`/`Sz`/`Clr`) | 1 | yes | `styles.rs:120-127`, `StrokePosition` `styles.rs:67` | Nothing: a gradient/pattern stroke (not used here) would have no home. |
| Gradient Fill layer (`GdFl`) | 1 (`Gradient Fill 1`: Radial, angle −35.3726°, scale 134.074 %, `Algn false`, offset (−50.185, −10.519), 4 colour stops + 2 opacity stops with midpoints, gradient named "yayy") | yes | `layer.rs:211-214`, `fill.rs:18` (`FillLayer::Gradient`), `gradient.rs:176` (`GradientKind::Radial`), `gradient.rs:188` (`GradientFill`), `gradient.rs:274` (`GradientLayer`: angle/scale/offset/dither/reverse), `gradient.rs:15,24` (stops + midpoints) | The interpolation method (`Smoo`, `Intr` 4096) has no exact member of `Method` (`gradient.rs:38`: Perceptual/Linear/Classic) and the gradient's name/smoothness value are not stored. `Algn false` matches Fotox's canvas-anchored fill. |
| Adjustment layer: Invert (`nvrt`) | 1 (`Invert 1`) | yes | `layer.rs:59` | Nothing. |
| Document Global Light angle 90° | 1 | yes | `document.rs:28` | Nothing. |
| Document Global Light altitude 30° | 1 | partial | no document field (`document.rs:9-56`); per-effect `BevelEmboss.altitude` `styles.rs:152` | `Color Fill 2`'s bevel uses the global light; its altitude can come from the effect, but the document value has no home. |
| Guides (`GRID_AND_GUIDES_INFO`) | 0 (empty guide list) | yes (unused) | `document.rs:30,58` | Nothing (this file has no guides). |
| Grid: 18 px (576 units) | 1 setting | no | `document.rs:9-56` has no grid field | The document's grid spacing (view-only preference). |
| Slices (`SLICES`) | 1 auto slice covering the whole canvas (named "Untitled-1weird"), no user slices | yes | `document.rs:43`, `comps.rs:80`, `comps.rs:89` | The slice record's URL/alt/HTML fields have no home but are empty; the auto slice is regenerated. |
| Layer comps, alpha channels, saved paths, notes/counts/colour samplers, patterns, text layers, artboards, blend-if ranges | none in this file | yes (unused) / no field | `comps.rs:13,33`, `channel.rs:13` + `document.rs:35`, `path.rs:96` + `document.rs:38`, `annotations.rs:7,17,29,35` + `document.rs:48`, `pattern.rs:16` + `document.rs:33`, `layer.rs:196` + `text.rs:22`, `layer.rs:345,350`; blend-if has no field (`layer.rs:318-346`) | Nothing: the file uses none of them (`Patt` is an empty PATTERNS1 section, all blending ranges are defaults). |
| Layer style-blending flags `clbl` = 1, `infx` = 0, `knko` = 0 | all layers (defaults) | no field | `layer.rs:318-346` | Nothing visually (Photoshop defaults). |
| Layer metadata blocks `lnsr` (name source), `lyvr` (LAYER_VERSION 130 on `Group 1`), `shmd` (timestamps), `fxrp` (non-zero on 31 layers), `lclr` (sheet colour, all 0) | all layers | no field | `layer.rs:318-346` | Panel metadata: name source, layer version, edit times, transform origins, colour labels. No pixels lost. |
| Document metadata: XMP (14,751 B), EXIF (306 B), IPTC (15 B), CAPTION_DIGEST (16 B), empty URL_LIST, VERSION_INFO, IDS seed 216, LAYER_STATE_INFO 40, LAYER_SELECTION_IDS (empty), LAYER_GROUP_INFO, LAYER_GROUPS_ENABLED_ID, PIXEL_ASPECT_RATIO 1.0, THUMBNAIL_RESOURCE, UNKNOWN_1092 (16 B), PRINT_INFO_CS5 / PRINT_STYLE / PRINT_FLAGS / PRINT_FLAGS_INFO / PRINT_SCALE, COLOR_HALFTONING_INFO, COLOR_TRANSFER_FUNCTION | 21 resources | no | `document.rs:9-56`; `.fxd` persists annotations (`fx-io/src/fxd/manifest.rs:61`) | Metadata only: XMP/EXIF/IPTC captions, print/export settings, halftone/transfer functions, thumbnail, layer-panel state. Nothing that affects rendering. |
| Document blocks `CAI ` (77 B, C2PA), `OCIO`, `GenI`, `cinf` (COMPOSITOR_INFO 1.3 / PS 26.5, engine compCore), `FMsk` (red @50 % filter-mask overlay) | 5 blocks | no | `document.rs:9-56`; `FMsk` relates to `smart.rs:38-46` (no mask field) | Provenance and engine metadata; the Smart Filter mask overlay colour is unrepresentable, but no Smart Filter exists in this file. |
| Composite/flattened preview and thumbnail | 1 composite section | yes | `fx-io/src/fxd/manifest.rs:75,155`, `container.rs:92` | Nothing: Fotox stores its own composite preview. |

## The `no` and `partial` items, ordered by impact on this file

1. **Lock-all (`lspf` 2147483648) on three layers** — `partial` (`layer.rs:329,332,333`).
   `Layer 7`, `Layer 1` and `Layer 10` are locked completely; Fotox has the three individual locks
   but no "lock all" (and no nesting lock), so the importer must set all three and the exact saved
   state is not preserved.
2. **Gradient interpolation method `Smoo`** — `partial` (`gradient.rs:38`). `Gradient Fill 1`'s
   four-stop "yayy" gradient uses Photoshop's Smooth interpolation; `Method` offers
   Perceptual/Linear/Classic only, so the importer picks the nearest and the ramp can band slightly
   differently. The `Intr 4096` smoothness value and the gradient's name have no field either.
3. **Linked Smart Objects with embedded bytes** — `partial` (`smart.rs:28-30`). The three `lnk2`
   `liFD` buffers (`Color Fill 3.psb` 7.3 MB, `Color Fill 1.psb` 1.7 MB, `0_0 (2).png` 677 KB) are
   the objects' content, but Fotox's linked object is a file path plus mtime: the importer must
   extract them to disk or store them embedded and lose the link (no timestamp exists to preserve).
4. **Satin anti-alias flag** — `no field` (`styles.rs:172-182`). `Group 1`'s Satin has `AntA true`
   and Fotox's `Satin` has no anti-alias switch. Everything else in it
   (blend/colour/opacity/angle/distance/size/invert) has a field; a renderer that always
   anti-aliases loses only the off case, not this one.
5. **Vector masks cannot be inverted** — `partial` in principle (`layer.rs:360-368` has `enabled`,
   `feather`, `density`, no `inverted`). All four masks in this file are normal
   (`fill_rule 0` = fill inside, not inverted, not disabled), so nothing is lost here; density and
   feather are unset as well.
6. **Untagged ICC profile** — `partial` (`color.rs:34`). No profile is named in the file and Fotox
   cannot record "unspecified": the importer must choose a working space.
7. **Effect detail fields with no home (at defaults here)** — `no fields` (`styles.rs:74-210`):
   contours, noise, glow technique/range/jitter, bevel texture/shape. Nothing visible is lost
   (verified `Nose` 0, `TrnS` "Linear", `GlwT` "Softer").
8. **Shape-layer paint is solid-only** — `partial` in principle (`vector.rs:76-80`). The four shape
   layers are solid-colour fills from `SoCo`, fully representable; a gradient- or pattern-filled
   shape layer would have no home.
9. **Layer/document metadata without a field** — `no` (`layer.rs:318-346`, `document.rs:9-56`):
   `lyvr` 130 on `Group 1`, `lnsr`, `shmd`, `fxrp` (31 layers), sheet colours, XMP/EXIF/**IPTC**/
   CAPTION_DIGEST/URL_LIST, print blocks, halftone/transfer, IDS seed, LAYER_STATE_INFO,
   LAYER_GROUP_INFO, empty selection ids, thumbnail, `PIXEL_ASPECT_RATIO`, `UNKNOWN_1092`, `CAI `,
   `OCIO`, `GenI`, `cinf`. Metadata only.
10. **Grid 18 px** — `no` (`document.rs:9-56`). A view preference.
11. **Global-light altitude** — `partial`, but only `Color Fill 2`'s bevel is affected and its own
    altitude is storable (`styles.rs:152`).
