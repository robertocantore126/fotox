# Coverage — every Photoshop tool and feature Rob asked for

> **Draft by Claude (26 Sep 2026)** from Rob's list of 26 Sep. Status checked
> in the code at `c4c133c` (branch `m6`, M6-T06 done), not read from the
> menus. Milestones M7–M13 are the new drafts in this folder; PSD import
> moves to M14, automation to M15 (`docs/ROADMAP.md`).
>
> ✅ done · 🔜 in M6, still being built · a milestone/card = planned there.

## Toolbox

| Category | Tool | Status | Where |
| --- | --- | --- | --- |
| Move & transform | Move Tool | 🔴 the default tool, does nothing | **M7-T02** |
| | Artboard Tool | — | **M12-T07** |
| Selection | Rectangular / Elliptical / Single Row / Single Column Marquee | ✅ M5 | — |
| Lasso | Lasso, Polygonal Lasso | ✅ M5 | — |
| | Magnetic Lasso | placeholder | **M9-T07** |
| Object selection | Magic Wand | ✅ M5 | — |
| | Quick Selection | placeholder | **M9-T06** |
| | Object Selection | placeholder | **M13-T04** (a segmentation model) |
| Crop & slice | Crop | ✅ M6-T03 (with straighten) | — |
| | Perspective Crop | — | **M9-T09** |
| | Slice, Slice Select | — | **M12-T08** |
| Eyedropper / measurement | Eyedropper | ✅ M5 | — |
| | Color Sampler, Ruler, Note, Count | — | **M9-T08** |
| Retouching | Spot Healing Brush, Healing Brush | ✅ M5 (Proximity Match; Content-Aware type in **M11-T02**) | — |
| | Patch | — | **M11-T03** |
| | Content-Aware Move | — | **M11-T04** |
| | Red Eye | — | **M8-T08** |
| Painting | Brush, Pencil | ✅ M5 (sampled tips, Brush Settings panel, presets: **M8-T01**) | — |
| | Color Replacement | — | **M8-T08** |
| | Mixer Brush | — | **M8-T09** |
| Clone | Clone Stamp | ✅ M5 | — |
| | Pattern Stamp | — | **M8-T06** |
| History painting | History Brush, Art History Brush | — | **M8-T07** |
| Eraser | Eraser | ✅ M5 | — |
| | Background Eraser, Magic Eraser | — | **M8-T02** |
| Gradient / fill | Gradient, Paint Bucket | — | **M8-T03**, **M8-T02** |
| Blur / sharpen / smudge | Blur, Sharpen, Smudge | — | **M8-T05** |
| Dodge / burn | Dodge, Burn, Sponge | — | **M8-T04** |
| Pen / paths | Pen, Freeform Pen, Curvature Pen, Add / Delete Anchor Point, Convert Point | — (the `Path` shape exists, unused) | **M10-T02…T05** |
| Path selection | Path Selection | ✅ M6-T06 for shape layers; paths in **M10-T05** | — |
| | Direct Selection | — | **M10-T05** |
| Shapes | Rectangle (and rounded), Ellipse, Polygon, Line | ✅ M6-T06 | — |
| | Triangle, Custom Shape | — | **M10-T07** |
| Text | Horizontal Type | 🔜 M6-T07 | — |
| | Vertical Type, Horizontal / Vertical Type Mask | — | **M10-T08** |
| Navigation | Hand, Rotate View | ✅ M0 / M6-T05 | — |
| | Zoom Tool (click, Alt, scrubby drag) | 🟡 zoom works from menu, keys and wheel; the tool itself does nothing | **M7-T07** |

## Transform

| Feature | Status | Where |
| --- | --- | --- |
| Free Transform, Scale, Rotate, Skew, Distort, Perspective, Warp | ✅ M6-T04 | — |
| Content-Aware Scale | — | **M11-T05** |
| Puppet Warp | — | **M11-T07** |

## Selection-related

| Feature | Status | Where |
| --- | --- | --- |
| Modify Selection (Border, Smooth, Expand, Contract, Feather) | ✅ M5 | — |
| Grow, Similar, Transform Selection | — (menu items greyed) | **M9-T02** |
| Color Range | — | **M9-T03** |
| Focus Area | — | **M9-T04** |
| Select and Mask | — | **M9-T05** |
| Select Subject, Remove Background, Sky Select | — | **M13-T02**, **M13-T03** (segmentation models) |

## Retouching / image manipulation

| Feature | Status | Where |
| --- | --- | --- |
| Content-Aware Fill | — | **M11-T01/T02** (PatchMatch) |
| Liquify | — | **M11-T06** |
| Vanishing Point | — | **M11-T09** |
| Perspective Warp | — | **M11-T08** |
| Puppet Warp | — | **M11-T07** |
| Generative Fill, Generative Expand | — | **M13-T06** (through a local ComfyUI, M13-T00) |
| Neural Filters | — | **M13-T05** (a subset: Super Zoom, JPEG artefacts, Colorize) |

## Colour / tonal

| Feature | Status | Where |
| --- | --- | --- |
| Brightness/Contrast, Levels, Curves, Exposure, Vibrance, Hue/Saturation, Color Balance, Black & White, Photo Filter, Channel Mixer, Invert, Posterize, Threshold, Gradient Map | ✅ M2/M4 (as adjustment layers and as Image ▸ Adjustments) | — |
| Color Lookup | — | **M12-T05** |
| Selective Color | — | **M12-T05** |

## Layer-related

| Feature | Status | Where |
| --- | --- | --- |
| Layer Mask, Clipping Mask, Adjustment Layers, Blend Modes (27 + Pass Through), Fill / Opacity | ✅ M2 | — |
| Layer Styles: Drop Shadow, Stroke, Outer Glow, Color Overlay, Inner Shadow | 🔜 M6-T08 | — |
| Layer Styles: Bevel & Emboss, Satin, Inner Glow, Gradient Overlay, Pattern Overlay, Blend If, Knockout | — | **M12-T04** |
| Smart Objects | — (excluded by D-055 for M6) | **M12-T01/T02** (M12-T00 reverses D-055) |
| Smart Filters | — | **M12-T03** |
| Layer Comps | — | **M12-T06** |

## Not in the list, needed by it

These come up because an item above depends on them:

| What | Needed by | Where |
| --- | --- | --- |
| File ▸ New (a blank document) | everything: today a document can only be opened | **M7-T01** |
| Place Embedded, drop a file onto an open document as a layer | Smart Objects, compositing | **M7-T03** |
| Guides, grid, snapping | Move, Crop, shapes, Artboards, Slices | **M7-T06** |
| Arrange / Align / Distribute, the rest of the Mask menu | Move, Artboards | **M7-T04/T05** |
| Tool kinds (extension points in the brush engine and the tools) | every M8 tool | **M7-T08** |
| Channels, Save / Load Selection, Quick Mask | Select and Mask, Color Range, Quick Selection | **M9-T01** |
| Paths and the Paths panel | Pen, vector masks, Custom Shape, type on a path | **M10-T01** |
| Patterns (define, library) | Pattern Stamp, Pattern Overlay, Pattern Fill | **M8-T06** |
| More filters (Blur / Noise / Sharpen / Distort / Stylize / Render families) | Smart Filters are worth little with two filters | proposed as **M12-T03b** or its own milestone — Rob's call in M12-T00 |
