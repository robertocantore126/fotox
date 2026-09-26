# HARDEN H2 — what fast mode left for Rob

> Extracted from `docs/reports/LOG.md` and `docs/DECISIONS.md` on 2026-09-26 (HARDEN H2).
> Nothing here is decided yet: each item needs a yes / no (decisions), or finish / cut / later (skipped work).
> The VERIFY list goes to the Photoshop comparison in H6. "Tests" in the skipped
> lines are not for Rob: H4 writes them.

Counts: **35** fast-default decisions, **64** cards with skipped work, **38** cards with VERIFY notes.

## 1. Fast-default decisions to confirm

Write **yes** (keep) or **no** (and what instead) in the last column.

| Id | Decision | From | Rob |
| --- | --- | --- | --- |
| D-057 | **Fast mode** from M6-T07: all milestone code is written first, and tests, reviews, reports, clippy and measurements are deferred to one hardening phase (`docs/tasks/HARDEN.md`). Agents commit card after card on the milestone branch, mark shortcuts `// FAST:`, log each card in `docs/reports/LOG.md` and take T00 recommendations as fast defaults. Only rules kept: it builds, no document-sized buffers, a render thread that never blocks, no rewriting working code (`AGENTS.md` §2). | AI-written code has bugs whatever the process; a review and test round per card made each card slow without preventing them. Finding logic bugs in one pass at the end, led by Claude, is faster. Rob, 2026-09-26. | |
| D-058 | File ▸ New presets: Clipboard (when it holds an image), Last Used, 1920 × 1080 px 72 ppi, A4 300 ppi, 4K UHD, 30 000² (B3 test). | M7-T00-1, the card's recommendation. | |
| D-059 | The Move tool's Auto-Select is **off** by default (Ctrl+drag auto-selects once), as in Photoshop. | M7-T00-2. | |
| D-060 | Until Smart Objects exist (M12), Place Embedded makes a **pixel layer** with a Free Transform box to confirm. | M7-T00-3. | |
| D-061 | Menu honesty: an item the engine does not implement is greyed out with a "Planned for Mx" tooltip, enforced by `check-data.mjs` from a list the engine exports. | M7-T00-4. | |
| D-062 | Preferences live in `%APPDATA%\Fotox\preferences.json` (JSON, versioned, unknown keys kept): units, guide and grid colours, grid spacing, memory budget, scratch folder, recent files. | M7-T00-5. | |
| D-063 | Brush presets are Fotox JSON; `.abr` import (v6+) covers **sampled tips** (the tip image) and the basic dynamics. Computed tips and the dual brush / texture sections come later. | M8-T00-1, the card's recommendation. | |
| D-064 | Tool formulas Adobe does not document (Dodge / Burn ranges, Sponge, Smudge strength, Color Replacement's modes) use published approximations marked `VERIFY` in the code; they are compared with Photoshop in M14 (PSD import gives the test files). | M8-T00-2. | |
| D-065 | Gradient and Pattern fill layers are derived-tile layers like shapes (R6), rendered at every level from their parameters. | M8-T00-3. | |
| D-066 | Mixer Brush model (an **approximation**, Photoshop's is undocumented): a reservoir colour buffer of the tip's size and a pickup buffer; Wet = how much canvas is picked up, Load = reservoir amount, Mix = canvas vs reservoir ratio, Flow as usual; clean / load after each stroke. | M8-T00-4. | |
| D-067 | The History Brush's source is a **history state** of the same document (the snapshots History already keeps, D-009); the History panel gets Photoshop's source column. | M8-T00-5. | |
| D-068 | Alpha channels are document data: `Document::channels` (name, grey image at the document depth, colour, opacity), saved in `.fxd`; a saved selection is a channel; R, G, B are views of the composite, not stored. | M9-T00-1. | |
| D-069 | Quick Selection (non-AI): seeded region growing on colour + edge strength at a coarse level, refined at level 0 on the boundary tiles; Enhance Edge = the T05 refine. | M9-T00-2. | |
| D-070 | Select and Mask refines with a **guided filter** (He et al.) over the edge band, the image as guide; Decontaminate Colors = a local weighted average of the confident foreground; no closed-form matting. | M9-T00-3. | |
| D-071 | Magnetic Lasso = live-wire (Dijkstra on a gradient-magnitude cost) in a window around the pointer; anchors placed by Frequency. | M9-T00-4. | |
| D-072 | Notes and counts are document data (saved in `.fxd`, never printed or exported). | M9-T00-5. | |
| D-073 | One path model (`fx_core::path::Path`: subpaths of anchors with in / out handles and a smooth flag, a `PathOp` per subpath) for document paths, shape outlines, vector masks and type on a path; `PathEl` converts losslessly. | M10-T00-1. | |
| D-074 | Boolean path operations go through **`i_overlay`** (pure Rust, MIT) on flattened curves.  In fast mode the crate is not added yet: the ops are approximated with winding and fill rules and Merge Shape Components waits (HARDEN adds the crate). | M10-T00-2. | |
| D-075 | Vertical type is laid out as horizontal runs rotated −90° (CJK upright, Latin rotated, Standard Vertical Roman Alignment as the option) — an **approximation** of full vertical OpenType layout. | M10-T00-3. | |
| D-076 | The custom shapes library is Fotox JSON (paths) in M10; `.csh` import later. | M10-T00-4. | |
| D-077 | PatchMatch (Barnes et al. 2009) with Wexler-style multi-scale EM voting, Fotox's own in `fx-ops`, on a region of interest (the hole plus the sampling area) read tile by tile under a pixel budget; beyond the budget it runs on a reduced copy and upsamples. | M11-T00-1. | |
| D-078 | Liquify is a displacement field on a coarse grid (4-px cells, coarser for big images so the field stays bounded), applied through a remapping sampler; no Face-Aware Liquify. | M11-T00-2. | |
| D-079 | Puppet Warp: as-rigid-as-possible deformation on a mesh from the layer's alpha (Igarashi et al. 2005), Density, Expansion, Pin Depth, Rotate.  In fast mode the solver is rigid moving-least-squares (Schaefer et al. 2006) on a grid, not ARAP on a triangulation. | M11-T00-3. | |
| D-080 | Content-Aware Scale is seam carving (Avidan–Shamir) with a Protect channel and Protect Skin Tones, run at a reduced level for big images with the seams mapped back up. | M11-T00-4. | |
| D-081 | The content-aware and deformation tools resample the layer's pixels (no Smart Object until M12): repeated use loses quality, as in Photoshop on a normal layer. | M11-T00-5. | |
| D-082 | Smart Objects (reverses D-055 from M12 on): `LayerKind::Smart { source, transform, filters, cache }`; an embedded source is a nested document stored inside the same `.fxd`, a linked source is a file path; the rendered pixels are derived tiles resampled from the source composite's mips through the transform, so transforms are lossless. | M12-T00-1. | |
| D-083 | Smart Filters are re-evaluated on the Smart Object's rendered pixels per requested tile and level (the M4 filter framework and its level scaling), with one filter mask for the stack and per-filter blending (mode, opacity). | M12-T00-2. | |
| D-085 | An artboard is a top-level group with bounds and a background colour; the canvas becomes the union of the artboards; export writes one image per artboard. | M12-T00-4. | |
| D-086 | Color Lookup reads `.cube` (1D / 3D) and `.3dl` (`.look` / `.csp` later); the LUT is applied in the document's encoding and its table is stored in the document. | M12-T00-5. | |
| D-087 | Slice export writes PNG / JPEG (WebP when the formats milestone has it) per slice, named after the slice; no HTML output. | M12-T00-6. | |
| D-088 | Inference runtime: `ort` (ONNX Runtime bindings) with the DirectML execution provider on Windows and the CPU fallback, in a new `fx-ai` crate. The runtime is loaded dynamically (`load-dynamic`): no build-time download, the ONNX Runtime library (MIT) ships next to the app (`onnxruntime.dll`) or is named by `FOTOX_ORT_DYLIB`. | M13-T00-1. | |
| D-090 | Models live in one folder, `%LOCALAPPDATA%\Fotox\models` (`$XDG_DATA_HOME/fotox/models` elsewhere), downloaded on first use with the size shown first and a checksum; the installer and the runtime call the same function (`fx_ai::models::models_dir`). | M13-T00-3. | |
| D-091 | Generative Fill / Expand go through a bridge to a local ComfyUI server (its HTTP API: `/upload/image`, `/prompt`, `/history`, `/view`) with Fotox's bundled inpaint / outpaint workflow templates; no diffusion model is embedded. | M13-T00-4. | |
| D-092 | Honesty in the UI: without a model or a reachable ComfyUI the items say so and offer the download or the address; nothing is faked. | M13-T00-5. | |
| D-094 | Photoshop's Generative Layer is a group named after the prompt holding one pixel layer per variation (the first visible), each with the selection as its mask: Fotox groups cannot carry a pixel mask. | M13-T06. | |

## 2. Skipped work — finish, cut, or later

One line per card, as the log wrote it. Mark each **finish** / **cut** / **later**.


### M6

- [ ] **M6-T07** Text layers: vertical type, type mask, per-selection formatting (the option bar formats the whole layer), Faux Bold/Italic, tracking/leading UI, text thumbnails (the row shows "T"), the card's Tests.
- [ ] **M6-T08** Layer styles: styles on groups, per-effect eye toggles under the row, Global Light dialog, noise/contour/knockout options, effects on clipped layers, the card's Tests.
- [ ] **M6-T09** UI: the transform fields are not updated from the box while dragging (the status bar shows the numbers); `[`/`]` for type size; shape/text real thumbnails.

### M7

- [ ] **M7-T00** Decisions: none. FAST: none. VERIFY: none.
- [ ] **M7-T01** File ▸ New: the Clipboard preset (shell does not report the clipboard size), name field, colour profile choice (always sRGB), units other than px.
- [ ] **M7-T02** Move tool: Show Transform Controls (box + handle drag), live preview of a selection move.
- [ ] **M7-T03** Place Embedded: placing a `.fxd` (it opens instead), Escape removing the placed layer, one "Place" history step (it is Paste + Rename + Transform).
- [ ] **M7-T04** Arrange, Align, Distribute: `dist:hspace|vspace` (equal gaps), groups and fill layers in align.
- [ ] **M7-T05** Mask and Layer menus: Background from Layer.
- [ ] **M7-T06** Guides, grid and snapping: dragging or deleting a single guide, guide/grid colours from prefs, pixel grid in the app, snapping to layer bounds and selection edges.
- [ ] **M7-T07** Zoom tool: rectangle zoom (Scrubby off).
- [ ] **M7-T08** Tool kinds: DragTool live preview, a toy op, the stored-hash test.
- [ ] **M7-T09** Menu honesty and Preferences: scratch folder editing, units, guide/grid colours, the other preference pages.

### M8

- [ ] **M8-T00** Decisions: none. FAST: none. VERIFY: none.
- [ ] **M8-T01** Brush tips, dynamics and presets: Photoshop's Smoothing modes other than the pulled string (Catch-up / Adjust for Zoom); the ABR `desc` section (names, spacing, dynamics of the file); computed tips' Flip X/Y; texture / dual brush / wet edges / noise; Tests (should check: a sampled tip stamps its image; live = replay with jitter; smoothing keeps a straight line straight; an ABR fixture imports).
- [ ] **M8-T02** Paint Bucket, Magic Eraser, Background Eraser: Background Eraser limits Contiguous / Find Edges (all act as Discontiguous); continuous sampling (samples once at the press); Magic Eraser opacity; Tests (should check: a bucket fill on a two-colour image stops at the edge; non-contiguous fills both regions; the magic eraser leaves transparency and un-Backgrounds; the background eraser keeps the protected colour).
- [ ] **M8-T03** Gradient tool and Gradient Fill layers: live preview of the drag (line overlay only); the Properties panel for fill layers (the dialog is used instead); gradient presets saved by the user; noise gradients; "Align with layer"; Tests (should check: linear endpoints exact; radial symmetric; dithered 8-bit has no bands wider than the period; fill layer at level 2 ≈ level 0 downsampled within 2/255; undo).
- [ ] **M8-T04** Dodge, Burn, Sponge: Sponge's skin-tone protection; Tests (should check: midtone dodge lifts 50 % grey more than black/white; Protect Tones keeps hue; desaturate reaches grey; vibrance protects a saturated pixel).
- [ ] **M8-T05** Blur, Sharpen, Smudge: accumulation of Blur/Sharpen within one stroke (one pass per stroke), Smudge's Sample All Layers, Tests (should check: blur flattens a step monotonically with strength; sharpen raises edge contrast, flat stays flat; smudge drags a colour and fades with strength < 1; live = replay for the three).
- [ ] **M8-T06** Patterns and the Pattern Stamp: `.pat` import, pattern export, Tests (should check: a pattern tiles seamlessly across tiles; aligned stamping continues between strokes; the fill layer round-trips through `.fxd`).
- [ ] **M8-T07** History Brush, Art History Brush: History snapshots (Photoshop's named snapshots); painting a mask with them; Tests (should check: History Brush from the first state after a Levels restores the pixels; the size-mismatch refusal; an Art History stroke is deterministic for a seed).
- [ ] **M8-T08** Color Replacement and Red Eye: Color Replacement Limits Contiguous / Find Edges and continuous sampling; Tests (should check: replacing a red area's hue with blue keeps luminosity; Red Eye turns a synthetic red pupil dark and leaves the iris).
- [ ] **M8-T09** Mixer Brush: Load / Clean Brush menus and the "after each stroke" toggles (always load + clean per stroke), Sample All Layers, the current-brush-load swatch, Tests (should check: Dry with a loaded reservoir paints the reservoir colour; Very Wet mixes a two-colour field along the path; clean after each stroke resets).
- [ ] **M8-T10** UI: a Properties panel for fill layers (their dialog instead); the Tool Presets panel stays a mock.

### M9

- [ ] **M9-T00** Decisions: none. FAST: none. VERIFY: none.
- [ ] **M9-T01** Channels, Save / Load Selection, Quick Mask: showing R / G / B (or an alpha channel) as grey in the viewport and the channel eye toggles; drag rows onto the buttons; Quick Mask Options (colour, opacity, masked vs selected); a new empty channel; Tests (should check: save then load = same coverage; the four modes; channels round-trip through `.fxd`; Quick Mask paint then exit = the painted selection).
- [ ] **M9-T02** Grow, Similar, Transform Selection: the live preview of the transformed ants (the box only); Tests (should check: Grow stops at an edge; Similar selects a separate same-coloured region; Transform Selection rotates the coverage and leaves the pixels).
- [ ] **M9-T03** Color Range: the dialog's live preview modes (Selection / Grayscale / Black / White Matte / Quick Mask — the result shows after OK), the image eyedropper with +/− samples (the swatches are the samples), Localized Color Clusters' centre, Out of Gamut, Detect Faces (M13); Tests (should check: sampled red on a hue ramp selects a band growing with fuzziness; Highlights pick the bright end; localized clusters drop the far region).
- [ ] **M9-T04** Focus Area: the dialog's add / subtract brushes, preview and view modes, output to mask / new layer (use Select and Mask's output or Layer ▸ Layer Mask afterwards); Tests (should check: sharp-left / blurred-right synthetic selects the left half within a few pixels; the noise floor keeps a noisy flat area out).
- [ ] **M9-T05** Select and Mask: Photoshop's modal workspace (view modes, Show Edge / Original, its Quick Selection / Refine Edge / Brush / Lasso tools), Decontaminate Colors, output to New Layer / New Document; Tests (should check: a soft-haired synthetic's band error halves; Shift Edge +50 % grows the selection).
- [ ] **M9-T06** Quick Selection tool: the live outline during the stroke (it updates at release); the coarse-level pass + boundary refinement of D-069; Select Subject (M13); Tests (should check: a stroke in a flat region bounded by a strong edge stops at the edge; Alt subtracts; live = replay).
- [ ] **M9-T07** Magnetic Lasso: Alt switching to the freehand / polygonal lasso, pen pressure = width, a magnetic closing segment; Tests (should check: tracing near a synthetic disc closes on its edge within 1.5 px).
- [ ] **M9-T08** Color Sampler, Ruler, Note, Count: the protractor (Alt-drag from a ruler end), Straighten's crop, the sampler's second readout mode (HSB / Lab…), dragging samplers / notes, count labels and group colours / sizes, showing annotations while another tool is active, Tests (should check: a sampler's average; the ruler's angle; Straighten levels a tilted line; notes and counts round-trip through `.fxd`).
- [ ] **M9-T09** Perspective Crop: Resolution, Front Image, Show Grid toggle; Tests (should check: a synthetic rectangle in perspective comes out axis-aligned within a pixel).

### M10

- [ ] **M10-T00** Decisions: adding `i_overlay` itself (D-074 says why). FAST: none. VERIFY: none.
- [ ] **M10-T01** Paths and the Paths panel: Schneider's curve fit (a traced grid + RDP + Catmull-Rom instead); Fill / Stroke Path dialogs (the panel buttons use the foreground and the Brush); path thumbnails; Tests (should check: a path round-trips through `.fxd`; selection → path → selection within a pixel; Stroke Path with the Brush equals painting the same points).
- [ ] **M10-T02..T05** Pen, Curvature Pen, Freeform Pen, anchor tools, Direct Selection: Ctrl switching to Direct Selection, the Freeform Pen's Magnetic option, marquee / Shift multi-selection of anchors, Path Selection on document paths (select / move / align subpaths, Merge Shape Components — needs D-074's crate); Tests (scripted clicks build the expected anchors; closing; Shape mode adds a layer; Alt breaks the symmetry; three curvature clicks give a smooth curve; a freehand circle fits within Curve Fit; adding an anchor keeps the curve; direct selection moves one anchor).
- [ ] **M10-T06** Vector masks: multiplying with a pixel mask (with both, only the pixel mask applies); Feather; Rasterize Vector Mask; Ctrl+click = load as selection; vector masks on groups; Properties' Density / Feather fields; Tests (should check: level 2 ≈ level 0 downsampled within 2/255; multiplication with the pixel mask; `.fxd` round trip).
- [ ] **M10-T07** Triangle, Custom Shape: Live Shape Properties in Properties (W / H / X / Y, radii, sides, star ratio), stroke options UI (caps, joins, dashes — the renderer has dashes), Line arrowheads, gradient / pattern paint for shapes, Tests (a triangle's corners; a custom shape round-trips; dash lengths; a gradient fill equals the renderer clipped).
- [ ] **M10-T08** Type completion: vertical type (D-075) — the Vertical Type tool stays planned and the vertical mask tool types horizontally; type on a path and area text; live Character / Paragraph panels (tracking, kerning, baseline shift, faux styles, caps, indents, spacing); Warp's Horizontal / Vertical and the distortions; Tests (vertical glyph positions; a type mask's coverage; text on a circle; warp preset hashes; Convert to Shape = the rendered text).

### M11

- [ ] **M11-T00** Decisions: none. FAST: none. VERIFY: none.
- [ ] **M11-T01** PatchMatch core: the card's file split (one file `patchmatch.rs`), colour adaptation, rows-parallel NNF search, Tests (periodic texture fill, same seed ⇒ same result, memory budget on a 20 000² image).
- [ ] **M11-T02** Content-Aware Fill, Spot Healing Content-Aware: the workspace (painted sampling area, live preview, Colour / Rotation / Scale adaptation, Mirror), Sample All Layers, grey layers, Tests.
- [ ] **M11-T03** Patch tool: live preview while dragging, Transparent, Structure / Color (Content-Aware), Diffusion, Tests.
- [ ] **M11-T04** Content-Aware Move: Transform on Drop, Structure / Color (edge blending), Sample All Layers, Duplicate mode, Tests.
- [ ] **M11-T05** Content-Aware Scale: the transform box UI and its view-level preview, scaling a selection only, Tests.
- [ ] **M11-T06** Liquify: the modal workspace, Freeze / Thaw Mask, Hand / Zoom inside, Show Mesh / Backdrop, Reconstruct All (partial), stylus pressure, mesh load / save, Face-Aware (D-078), Tests.
- [ ] **M11-T07** Puppet Warp: ARAP (D-079 fast default), pin rotation, Pin Depth, Tests.
- [ ] **M11-T08** Perspective Warp: Shift+click one edge, deleting a quad, snapping along whole edges, Tests (no crack along a shared edge).
- [ ] **M11-T09** Vanishing Point: the whole card (a modal workspace with planes, perspective marquee / stamp / brush, paste into a plane). The pieces it needs exist (homographies in `Mapping::from_quad`, `Mapping::Custom` meshes, the Clone Stamp); left for HARDEN or a later milestone after two cards' worth of warp sessions.

### M12

- [ ] **M12-T00** Decisions: none. FAST: none. VERIFY: none.
- [ ] **M12-T01** Smart Object core: the nested canvas as the layers' union (it is the parent's canvas), lazy opening of nested tiles checked, Tests.
- [ ] **M12-T02** Edit Contents: Linked Smart Objects (Place Linked, mtime watch, Update Modified Content, Relink, Embed Linked), Replace Contents, Export Contents; Tests.
- [ ] **M12-T03** Smart Filters: the filter mask; per-filter blend modes; filter rows under the layer with drag-reorder; double-click to re-edit a filter's parameters with the live preview; Tests (a Smart Filter equals the destructive filter: checked by hand, max difference 0 at level 0).
- [ ] **M12-T03b** More filters: Lens Blur, Average, Smart Sharpen, the Distort / Pixelate / other Render / Stylize / Video entries, Custom, Difference Clouds, Tests.
- [ ] **M12-T04** Layer styles: the other five: Contour and Texture sub-sections, Chisel techniques, gloss contours, Inner Glow's Center source and noise, Stroke Emboss (acts as Emboss), advanced blending (Blend If, Knockout, Blend Interior Effects as Group, Blend Clipped Layers as Group, Transparency Shapes Layer, Layer Mask Hides Effects), GPU = CPU check, Tests.
- [ ] **M12-T05** Color Lookup and Selective Color: Color Lookup as a destructive image adjustment, Abstract / Device Link profiles, `.look` / `.csp`, dithering for 8-bit, Export Color Lookup, Tests (identity LUT exact, 1D cube = Curves, Reds −100 % cyan on pure red).
- [ ] **M12-T06** Layer Comps: File ▸ Export ▸ Layer Comps to Files, the Smart Object source state, comments UI, the "Last Document State" row, Tests.
- [ ] **M12-T07** Artboards: export per artboard (File ▸ Export ▸ Artboards to Files), side handles, "+" adjacent buttons, presets, guides / snapping knowing artboards, artboards left of / above the origin (the canvas only grows right / down), an artboard marker in the Layers panel, Tests.
- [ ] **M12-T08** Slices and slice export: Divide Slice, the Slice Options dialog, layer-based slices, slice numbers drawn on the canvas, JPEG / WebP choice for slices (PNG only), Tests.

### M13

- [ ] **M13-T00** Decisions: M13-T03 (Sky Select) and M13-T05 (Neural Filters), by Rob's choice.
- [ ] **M13-T01** Inference host: the Preferences page for models (download size prompt, progress, delete button), the M9-T05 edge refinement of the upsampled mask, shipping `onnxruntime.dll` with the app (xtask / installer), Tests (bounded read on a 30k² document, crisp-edge upsample).
- [ ] **M13-T01** : M13-T03 (Sky) and T05 (Neural Filters) by Rob's choice; Object Finder (hover highlight); Select Subject inside Select and Mask; shipping `onnxruntime.dll` (xtask / installer); DirectML untested (no DirectML build here); S38 on a 6000 × 4000 portrait with GPU; a real photo IoU fixture.
- [ ] **M13-T06** Generative Fill and Generative Expand: a real run through ComfyUI (S40), the mocked-ComfyUI test, Properties' variation switcher (toggle the layers' eyes), websocket progress (it is guessed from elapsed time).

## 3. VERIFY — for the Photoshop comparison (H6)


### M6

- **M6-T07** Text layers: anti-alias labels → renderer; history label "Type Tool" for a new layer (Photoshop may name it differently); default font when the bar's font is not installed (parley fallback).
- **M6-T08** Layer styles: Photoshop's order and blend of effects, spread/choke semantics (percent of size = dilation part), blur σ = size/2, default global light 120°, stroke anti-aliasing.

### M7

- **M7-T02** Move tool: nudge history label.
- **M7-T04** Arrange, Align, Distribute: MoveLayer's index semantics for "forward/backward" (assumed final index).
- **M7-T06** Guides, grid and snapping: snap radius.

### M8

- **M8-T01** Brush tips, dynamics and presets: jitter distributions (uniform), hue jitter range (±180° at 100 %), the ABR skip sizes (47/301 bytes, from GIMP).
- **M8-T02** Paint Bucket, Magic Eraser, Background Eraser: the match edge softness; Photoshop's bucket anti-alias on the flood edge.
- **M8-T03** Gradient tool and Gradient Fill layers: midpoint curve, Perceptual space, dither pattern, Angle direction, the fill layer's Scale meaning (fraction of the canvas extent along the angle).
- **M8-T04** Dodge, Burn, Sponge: every formula (D-064), Photoshop's default Protect Tones (on), Exposure as flow.
- **M8-T05** Blur, Sharpen, Smudge: kernel size, sharpen amount, smudge strength curve.
- **M8-T06** Patterns and the Pattern Stamp: Impressionist (Photoshop's is a painterly blotch, not a jitter).
- **M8-T07** History Brush, Art History Brush: Art History stroke shapes and counts; the refusal wording.
- **M8-T08** Color Replacement and Red Eye: the red-eye formula and threshold; Color Replacement's match edge.
- **M8-T09** Mixer Brush: the whole model and the preset values (D-066).

### M9

- **M9-T02** Grow, Similar, Transform Selection: Photoshop's Grow / Similar distance.
- **M9-T03** Color Range: every curve (D-064).
- **M9-T04** Focus Area: normalisation constant, threshold curve, Photoshop's slider meaning.
- **M9-T05** Select and Mask: guided-filter ε and window, the global refinements' curves.
- **M9-T06** Quick Selection tool: constants against Photoshop on Rob's photos (M9-T10).
- **M9-T07** Magnetic Lasso: cost function, Frequency spacing.
- **M9-T08** Color Sampler, Ruler, Note, Count: Straighten's sign convention against Photoshop.

### M10

- **M10-T02..T05** Pen, Curvature Pen, Freeform Pen, anchor tools, Direct Selection: handle behaviour against Photoshop (M10-T09, the logo test).
- **M10-T07** Triangle, Custom Shape: Photoshop's triangle corner rounding (arcs, not quadratics).
- **M10-T08** Type completion: the warp shapes against Photoshop's presets.

### M11

- **M11-T02** Content-Aware Fill, Spot Healing Content-Aware: Photoshop's Auto sampling area; its default seedless behaviour (Fotox is deterministic per seed).
- **M11-T03** Patch tool: Photoshop's selection after a patch.
- **M11-T05** Content-Aware Scale: Photoshop's skin detector and how Amount mixes the two.
- **M11-T06** Liquify: brush falloff and twirl / pucker speeds against Photoshop.
- **M11-T08** Perspective Warp: Photoshop keeps the content outside the planes (it does, via the mesh's extension) — Fotox drops it.

### M12

- **M12-T01** Smart Object core: Photoshop's name for a converted group of layers.
- **M12-T03b** More filters: every formula against Photoshop (Radial Blur's amounts, Sharpen's strength, Surface Blur's weights, Emboss grey level, Despeckle's edge test).
- **M12-T04** Layer styles: the other five: all five effects against Photoshop at identical parameters (M12-T09), the composite order.
- **M12-T05** Color Lookup and Selective Color: Selective Color's range weights and Relative / Absolute formulas (published approximations, not Photoshop's).
- **M12-T06** Layer Comps: Photoshop's behaviour for layers the comp does not know.
- **M12-T08** Slices and slice export: Photoshop's auto-slice layout and numbering.

### M13

- **M13-T01** Inference host: the rembg mirror of BiRefNet's ONNX weights (licence of the export).
- **M13-T01** : Photoshop's Select Subject edge quality and its refinement radius.
- **M13-T06** Generative Fill and Generative Expand: Photoshop's context margin around the selection; the grow-mask values in the workflows.
