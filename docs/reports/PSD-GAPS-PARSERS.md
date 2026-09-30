# PSD/PSB parser coverage and disagreements — psd-tools 1.20.0 vs ag-psd 31.0.2

Appendix to the gap analyses of `draco.psd`, `nuovaera.psd`, `Untitled-1weird.psd`,
`Untitled-mmm.psd` and `finallynewopera.psb` (all in `%USERPROFILE%\Downloads`) — per-file reports:
[PSD-GAPS-draco.psd.md](PSD-GAPS-draco.psd.md), [PSD-GAPS-nuovaera.psd.md](PSD-GAPS-nuovaera.psd.md),
[PSD-GAPS-Untitled-1weird.psd.md](PSD-GAPS-Untitled-1weird.psd.md),
[PSD-GAPS-Untitled-mmm.psd.md](PSD-GAPS-Untitled-mmm.psd.md),
[PSD-GAPS-finallynewopera.psb.md](PSD-GAPS-finallynewopera.psb.md). Rob asked for the part no single
parser tells you: **which blocks a file uses that nobody decodes**, and **which features one
parser reports and the other misses or reads differently**. Fotox is not involved here — this
page only talks about the two reference parsers.

Method: each file was read once with psd-tools (Python 3.14, `psd-tools` 1.20.0, high-level API
*and* `layer.tagged_blocks` / `psd.image_resources`) and once with ag-psd (Node 25, `ag-psd`
31.0.2, `readPsd` with `skipLayerImageData`, `skipCompositeImageData`, `skipThumbnail`,
`skipLinkedFilesData`, `logMissingFeatures: true` and a capturing `log` callback). Both parsers
were asked for their own "unknown" reporting: psd-tools' `Unknown tagged block` / `Unknown image
resource` warnings and ag-psd's `Unhandled additional info: XXXX` / `Unread N bytes left`
messages. Full JSON dumps and the scripts are in `XuanZhi9\psd-gap-analysis\` (outside the repo).

## 1. Static coverage — who has a decoder for what

| | psd-tools | ag-psd |
| --- | --- | --- |
| layer/document blocks it has a *name* for | 94 keys (`Tag`) | 73 keys (`infoHandlersMap`) |
| blocks it decodes into a structure | 89 keys | 73 keys |
| image resources it decodes | 50 ids | 35 ids |

**Blocks ag-psd has no handler for, but psd-tools names:** none — ag-psd's 73 block keys are a
subset of psd-tools' 94. There is no block key in any of the five files that one parser knows as
a *key* and the other has never heard of, except through the two lists below.

**Keys psd-tools names but keeps raw (no structure):** `Alph` (alpha-channel names), `CAI `
(Content Credentials / C2PA), `Layr` (the merged-image layer records, PSB only), `shpa`
(section-divider sub-settings), `tySh` (the *old* type-tool block, pre-CS6 text). None of these
five files contains `Layr`, `shpa` or `tySh`.

**Blocks in these files that neither parser decodes — the "nobody decoded it" list:**

| Key | Level | Bytes | psd-tools | ag-psd | Present in |
| --- | --- | --- | --- | --- | --- |
| `CAI ` | document | 77 | key recognised, payload left raw | no handler, logs `Unhandled additional info: CAI ` | all five files |
| resource **1092** | document | 16 (`Untitled-mmm`, `Untitled-1weird`, `finallynewopera`), 25 (`nuovaera`, `draco`) | `Unknown image resource 1092` (psd-tools keeps the bytes) | no handler — its skip path has no log line at all | all five files |

`CAI ` is Photoshop's *Content Credentials* block (C2PA provenance); resource 1092 is an
undocumented 16/25-byte resource (CAPTION_DIGEST is 1091; 1092 sits in the same family). Both are
dropped silently by anything built on these parsers.

**Image resources only one side reads** (all absent or harmless in these five files, listed for
completeness): ag-psd reads ids 1035 (`urlsList`), 1060/1061 (counts), 4000/7000/7001 — psd-tools
has no typed reader for those; psd-tools types ids 1008, 1012–1017, 1033, 1040, 1042, 1046, 1047,
1051, 1074, 1077, 1083, 1086, 1087, 2999, 3000, 10000 — ag-psd has no handler for those.
`EXIF` (1059), `IPTC` (1028), `PRINT_STYLE` (1082), halftone (1013) and transfer functions (1014)
are read by **neither** as structured data: psd-tools keeps the bytes raw, ag-psd skips them
without a log line (its resource loop's "unhandled resource" log is commented out in 31.0.2).

## 2. Every block key the five files use, and who decodes it

`decoded` = the parser turns it into a structure; `recognised, raw` = psd-tools knows the key and
keeps bytes; `UNHANDLED` = ag-psd logs it as unhandled. Occurrences are block counts (the four
PSDs only; `finallynewopera.psb` is written separately below so the columns stay readable).

| Key | Name (psd-tools) | Level | psd-tools | ag-psd | Occurrences |
| --- | --- | --- | --- | --- | --- |
| `brit` | BRIGHTNESS_AND_CONTRAST | layer | decoded | decoded | nuovaera×1 |
| `CgEd` | CONTENT_GENERATOR_EXTRA_DATA | layer | decoded | decoded | nuovaera×1 |
| `cinf` | COMPOSITOR_INFO | document | decoded | decoded | all four×1 |
| `clbl` | BLEND_CLIPPING_ELEMENTS | layer | decoded | decoded | 1weird×38, mmm×44, draco×39, nuovaera×127 |
| `FEid` | FILTER_EFFECTS2 (smart-filter render) | document | decoded | decoded | draco×1 |
| `FMsk` | FILTER_MASK | document | decoded | decoded | all four×1 |
| `fxrp` | REFERENCE_POINT | layer | decoded | decoded | all four, every layer |
| `GdFl` | GRADIENT_FILL_SETTING | layer | decoded | decoded | 1weird×1, draco×2 |
| `grdm` | GRADIENT_MAP | layer | decoded | decoded | mmm×1 |
| `hue2` | HUE_SATURATION | layer | decoded | decoded | mmm×1, nuovaera×3 |
| `infx` | BLEND_INTERIOR_ELEMENTS | layer | decoded | decoded | 1weird×38, mmm×44, draco×39, nuovaera×127 |
| `knko` | KNOCKOUT_SETTING | layer | decoded | decoded | 1weird×38, mmm×44, draco×39, nuovaera×127 |
| `lclr` | SHEET_COLOR_SETTING | layer | decoded | decoded | all four, every layer |
| `lfx2` | OBJECT_BASED_EFFECTS_LAYER_INFO | layer | decoded | decoded | 1weird×1, mmm×4, draco×9, nuovaera×6 |
| `lfxs` | OBJECT_BASED_EFFECTS_LAYER_INFO_V1 | layer | decoded | decoded | 1weird×1 |
| `lmfx` | OBJECT_BASED_EFFECTS_LAYER_INFO_V0 | layer | decoded | decoded | draco×1 |
| `lnk2` | LINKED_LAYER2 | document | decoded | decoded | all four×1 |
| `lnkE` | LINKED_LAYER_EXTERNAL | document | decoded | decoded | all four×1 |
| `lnsr` | LAYER_NAME_SOURCE_SETTING | layer | decoded | decoded | all four, every layer |
| `lrFX` | EFFECTS_LAYER | layer | decoded | decoded | 1weird×2, mmm×4, draco×10, nuovaera×6 |
| `lsct` | SECTION_DIVIDER_SETTING | layer | decoded | decoded | 1weird×2, mmm×1, draco×1 |
| `lspf` | PROTECTED_SETTING | layer | decoded | decoded | all four, every layer |
| `luni` | UNICODE_LAYER_NAME | layer | decoded | decoded | all four, every layer |
| `lyid` | LAYER_ID | layer | decoded | decoded | all four, every layer |
| `lyvr` | LAYER_VERSION | layer | decoded | decoded | 1weird×1, draco×2 |
| `nvrt` | INVERT | layer | decoded | decoded | 1weird×1, mmm×2, draco×1, nuovaera×1 |
| `Patt` | PATTERNS1 | document | decoded (empty in the four PSDs) | decoded (empty in the four PSDs) | all four×1 (+1 non-empty PSB, see below) |
| `PlLd` | PLACED_LAYER2 | layer | decoded | decoded | 1weird×3, mmm×2, draco×4, nuovaera×1 |
| `shmd` | METADATA_SETTING | layer | decoded | decoded | all four, every layer |
| `SoCo` | SOLID_COLOR_SHEET_SETTING | layer | decoded | decoded | 1weird×4 |
| `SoLd` | SMART_OBJECT_LAYER_DATA1 | layer | decoded | decoded | 1weird×3, mmm×2, draco×4, nuovaera×1 |
| `thrs` | THRESHOLD | layer | decoded | decoded | nuovaera×1 |
| `vmsk` | VECTOR_MASK_SETTING1 | layer | decoded | decoded | 1weird×4 |
| `vogk` | VECTOR_ORIGINATION_DATA | layer | decoded | decoded | 1weird×4 |
| `GenI` | GENI | document | decoded | **UNHANDLED** | all four×1 |
| `OCIO` | OCIO | document | decoded | **UNHANDLED** | all four×1 |
| `CAI ` | CAI | document | **recognised, raw** | **UNHANDLED** | all four×1 |

Two of the parser-visible disagreements are visible right here: `GenI` and `OCIO` (photoshop's
"GenTech"/model list marker and the OCIO display/view config) are decoded by psd-tools and
*not logged at all* by ag-psd's handler map — ag-psd skips them with a log line, psd-tools keeps a
descriptor.

### `finallynewopera.psb` (a 2.68 GB PSB) — keys the four PSDs never exercise

The PSB's document side is the same nine blocks as the PSDs (`Patt`, `CAI `, `OCIO`, `GenI`,
`lnk2`, `lnkE`, `FEid`, `FMsk`, `cinf`), but its layer side adds ten keys, all of them **decoded by
both** parsers: `iOpa` (BLEND_FILL_OPACITY), `lmgm` (LAYER_MASK_AS_GLOBAL_MASK), `mixr`
(CHANNEL_MIXER), `blnc` (COLOR_BALANCE), `curv` (CURVES), `expA` (EXPOSURE), `levl` (LEVELS),
`post` (POSTERIZE), `selc` (SELECTIVE_COLOR) and `vibA` (VIBRANCE). It has no `Alph`, `Layr`,
`shpa` or `tySh` either, and it is the first file of the five whose `Patt` section is not empty.

## 3. Reading differences that are not a missing block

These are cases where both parsers "see" a feature but expose it differently enough that an
importer written against one of them would silently lose something.

1. **Layer effects: `present` vs every slot.** psd-tools' `layer.effects.items` yields only effects
   whose descriptor carries `present: true`, and drops an effect whose `classID` it does not know
   (`logger.debug`, silently). ag-psd lists *every* slot the descriptor declares, with `present`
   and `enabled` flags. In these five files Photoshop writes all ten slots on most styled layers
   with `present: false`; a tool that counts "effects in the file" off ag-psd over-counts by ~4×
   (e.g. `Untitled-mmm` layer `Layer 5`: ag-psd reports drop shadow, inner shadow, inner glow,
   bevel, satin, stroke, pattern overlay and colour overlay slots; only the drop shadow, outer glow
   and gradient overlay are `present`). psd-tools' view is the useful one; ag-psd's is the complete
   one. Also `BevelEmboss` is one effect in psd-tools and one `bevel` in ag-psd (both folds
   highlight+shadow), so that pair agrees.
2. **Effect *multiplicity*.** Photoshop stores each effect as an array; `draco`'s layer
   `love for all living beings` has two stroke slots (one enabled, one disabled). ag-psd exposes
   the array, psd-tools' `items` also returns both, but psd-tools' typed accessors
   (`layer.effects.find('Stroke')`) are the only way to tell which is which. Anyone modelling one
   effect per type (Fotox does) must decide which instance wins.
3. **Smart filters.** ag-psd *enumerates* the smart filters (`placedLayer.filter.list`, with a name
   per filter — `Gaussian Blur...`, `Add Noise...` — plus each filter's own opacity, blend mode,
   `enabled`, and its parameters: radius, amount, distribution, monochromatic, random seed; it
   understands ~60 filter `filterID`s). psd-tools has **no smart-filter API**: the same data exists
   in its dump only as the raw `SoLd` descriptor key `filterFX` → `filterFXList`
   (`Nm  `, `Fltr`, `filterID`, `blendOptions`), and `psd_tools.api.smart_object.SmartObject`
   exposes `kind/filename/data/warp/transform_box` but no filters. An importer written on
   psd-tools' API alone cannot see which smart filters a layer has.
4. **Smart-filter masks.** The `SoLd` descriptor declares them (`filterFX.filterMaskEnable`,
   `filterMaskLinked`, `filterMaskExtendWithWhite`) and the document carries an `FMsk` block with
   the mask's overlay colour/opacity. ag-psd exposes both (`placedLayer.filter.maskEnabled`,
   `maskLinked`, `maskExtendWithWhite`, `psd.filterMask`); psd-tools exposes neither through an
   API (both are raw descriptors). No layer in these files carries a mask *channel* for the smart
   filter (only `CHANNEL_0..2` and `TRANSPARENCY_MASK` exist), so the mask content here is the
   default white.
5. **Vector masks and shape geometry.** psd-tools builds real path objects
   (`VectorMaskSetting.path` → knots with `preceding`/`anchor`/`leaving`, plus the fill rule and the
   `vmsk` flags). ag-psd gives `vectorMask.paths` as flat point arrays. Both give the geometry;
   neither tells you whether the shape is still a *live* Photoshop primitive.
6. **Patterns.** Both parse the `Patt` section (psd-tools: `Patterns`, each with a name and
   thumbnail; ag-psd: `patterns`). The four PSDs carry an **empty** pattern section and ag-psd's own
   source still marks `LayerEffectsInfo.patternOverlay` as "not supported yet because of `Patt`
   section not implemented" — so for pattern overlays the two differ in intent, not in output
   there. **`finallynewopera.psb` is the exception:** its `Patt` holds five real patterns and the
   two parsers disagree. psd-tools decodes all five (`0_0 (4).png` 2048², `1.psb` 1232×928,
   `Layer 8.psb` 2787×2219, `Layer 9.psb` 2700×2160, `Tree Tile 4` 946²) with their names and
   GUIDs; ag-psd returns only **four** — it logs `Unread 101 bytes left for additional info: Patt`
   and silently drops the last one, `Tree Tile 4` (the pattern the bevel texture on
   `Layer 303 copy` stamps). An importer built on ag-psd loses that pattern (and any overlay using
   it) without an error.
7. **Adjustment layers: same data, different shape.** psd-tools reports `INVERT`/`GRADIENT_MAP` as raw *blocks* with
   typed accessors, ag-psd as a typed `adjustment` object with the descriptor's key names
   (`type`, `master`, `reds`, …). For hue/saturation, psd-tools' `HueSaturation` gives
   `master`/`items` (six hue ranges); ag-psd's `adjustment` gives `master`, `reds`, `yellows`,
   `greens`, `cyans`, `blues`, `magentas` — the same data, named differently.
8. **Layer mask richness.** Both read the mask channels; psd-tools adds
   `Mask.parameters` (`user_mask_density`, `user_mask_feather`, `vector_mask_density`,
   `vector_mask_feather`) and `real_flags` when the file has them; ag-psd exposes
   `userMaskDensity`/`userMaskFeather`/`vectorMaskDensity`/`vectorMaskFeather` on the mask object.
   Neither hides anything here — but note that **none** of the five files sets density/feather, so
   this row is untested by this file set.
9. **Composite/rendered data.** ag-psd reports the smart-filter *render* (`lfxs`/`FEid`) as
   `filterEffectsMasks` (channels + rectangle + depth) while psd-tools parses the same `FEid` block
   into `FilterEffects` objects (`uuid`, `rectangle`, `channels`, `extra`). `FEid` in `draco.psd` is
   ~5.5 MB per channel of *cached render output*: derived data, safe to drop, but it is the only
   place the file states what Photoshop last drew.

10. **Effect detail keys ag-psd does not model.** For `Layer 303 copy`'s bevel texture, psd-tools'
    raw descriptor carries `useTexture true`, `textureDepth 100`, `InvT false` and the texture
    pattern (`Tree Tile 4`). ag-psd exposes `useTexture` and the texture `pattern` (name + id) but
    has no field for `textureDepth` or `InvT`: it prints `Invalid effect key: 'textureDepth'` /
    `'InvT'` from `parseEffectObject`'s default branch via `console.log` — **not** through the `log`
    callback, which is why the message is absent from the captured `logSummary` — and drops the two
    values. So the bevel's texture depth/invert exist only on psd-tools' side.
11. **An effect blend enum with no name.** The gradient overlay on `Layer 379` carries
    `Md  ` with `enum = "H   "` (4 chars, trailing spaces). psd-tools' `BlendMode` enum has no such member, so it
    keeps the raw string; ag-psd's effect-mode parser (which reads `H   ` elsewhere as the *hue*
    channel of an HSBC colour) has no blend name for it either. It is the only unmapped effect blend
    mode in the five files.
12. **Guide positions are signed.** `GRID_AND_GUIDES_INFO` stores each guide as a position in
    1/32 px plus a direction byte (0 = vertical, 1 = horizontal). psd-tools reads the position as
    *unsigned* (`read_fmt("IB")`) and hands back the raw value; ag-psd divides the unsigned value by
    32 (`readUint32() / 32`), so a guide left of the canvas comes back enormous: draco's guide
    reads `4294950958` (psd-tools) and `134,217,217.4375 px` (ag-psd), where the real position is
    `−16338 / 32 = −510.5625 px` (`−5811 / 32 = −181.59375 px` in nuovaera). Read it as a signed
    32-bit value and divide by 32; the grid cycle (`576` here, i.e. 18 px) uses the same unit.

## 4. Reproducing this

```
python psdtools_inventory.py <file> pt-<stem>.json         # psd-tools 1.20.0
node   agpsd_inventory.js   <file> ag-<stem>.json          # ag-psd 31.0.2, logMissingFeatures
python coverage.py                                         # who decodes which block
python features.py <stem>                                  # per-file feature inventory
```

All five files were read only. Nothing in `fotox/` was executed against them: `crates/fx-io/src`
has no PSD/PSB reader (`crates/fx-io/src/lib.rs:14` still lists "PSD/PSB import — M7" as future
work; the module list is `crates/fx-io/src/lib.rs:102-114`). Note that Node's `fs.readFileSync`
refuses files over 2 GiB, so the PSB was read with a chunked reader (`agpsd_inventory.js`).
