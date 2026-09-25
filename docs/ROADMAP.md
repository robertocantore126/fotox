# Roadmap

Milestones are done in order. Each ends with an **acceptance run** on the
reference machine. Task cards: `docs/tasks/M0.md` … `M5.md` (done), `M6.md`
(in progress), `M7.md` … `M13.md` (**drafts**, 26 Sep 2026). Each draft starts
with a `T00` card of decisions for Rob and is refined when its milestone
starts. How the recurring pieces are built (commands, jobs, live previews,
tools, derived tiles, filters, measurements): `docs/tasks/HOWTO.md`.
`docs/tasks/COVERAGE.md` maps every Photoshop tool and feature Rob asked for
to the milestone and card that builds it.

**Renumbered on 26 Sep 2026:** M7–M13 were inserted before PSD import, which
moves from M7 to **M14**, and automation from M8 to **M15**. Decisions and
reports written before that date say "M7" for PSD import and "M8" for
automation.

| | Milestone | Result you can see | Exit criteria |
| --- | --- | --- | --- |
| **M0** | Shell | `fotox.exe` opens with the full Fotox UI; the canvas area shows a native GPU test pattern you can pan/zoom at 60 fps; rulers and zoom readout follow | M0 checklist in `tasks/M0.md` |
| **M1** | Open and navigate | Open a 30k × 30k 16-bit TIFF, fly around it smoothly, RAM stays under budget | S1, S2, S8 (`PERFORMANCE.md`) |
| **M2** | Layers and blend modes | Layer stack with groups, masks, clipping, all blend modes, 6 adjustment layers, live Layers/History panels, undo | S4, S5, S6, S8 |
| M3 | Native file + export | `.fxd` save/open (lazy, incremental), TIFF/PNG/JPEG export | S3, S7 |
| M4 | Colour + first filters | Colour management, soft proof, CMYK export, more adjustments, merge/flatten, filter framework (Gaussian blur, Unsharp mask) with live preview | S10–S12, C1, C2 (proposed) |
| M5 | Selections and painting | Marquee/lasso/magic wand, brush engine with pen pressure, eraser, mask painting, clone stamp, healing | S9, S13–S15 (proposed) |
| M6 | Transform and vector | Free transform/warp, crop, image/canvas size, view rotation, shapes and text (renderer: M6-T00), basic layer styles | S16–S20 (proposed) |
| M7 | Everyday basics and tool kinds | File ▸ New, Move tool, Place / drop as a layer, Arrange / Align / Distribute, the Mask menu, guides / grid / snapping, Zoom tool, no dead menu items; the tool extension points M8 builds on | S21–S23 (proposed) |
| M8 | Painting and retouching tools | Paint Bucket, Gradient (+ fill layers), Blur / Sharpen / Smudge, Dodge / Burn / Sponge, Background / Magic Eraser, Pattern Stamp, History / Art History Brush, Color Replacement, Red Eye, Mixer Brush; sampled tips, Brush Settings, presets, ABR import | S24–S26 (proposed) |
| M9 | Channels, selections and measurement | Channels panel, Save / Load Selection, Quick Mask, Grow / Similar, Transform Selection, Color Range, Focus Area, Select and Mask, Quick Selection, Magnetic Lasso, Color Sampler / Ruler / Note / Count, Perspective Crop | S27–S29 (proposed) |
| M10 | Paths, pen tools, vector masks, shapes and type | Paths panel, Pen / Freeform / Curvature Pen, anchor tools, Direct Selection, vector masks, Triangle / Custom Shape, vertical type, type masks, type on a path, Warp Text | S30–S31 (proposed) |
| M11 | Content-aware tools and deformations | PatchMatch: Content-Aware Fill, Patch, Content-Aware Move, Content-Aware Scale; Liquify, Puppet Warp, Perspective Warp, Vanishing Point | S32–S34 (proposed) |
| M12 | Smart Objects, styles, comps, artboards, slices | Smart Objects (embedded / linked) and Smart Filters, the other five layer styles + Blend If / Knockout, Color Lookup, Selective Color, Layer Comps, Artboards, Slices | S35–S37 (proposed) |
| M13 | AI-assisted features (local) | Select Subject, Remove Background, Sky, Object Selection (local ONNX models), a subset of Neural Filters, Generative Fill / Expand through a local ComfyUI | S38–S40 (proposed) |
| M14 | PSD/PSB import | Open layered PSD/PSB; verify blend modes and every `VERIFY` formula against Photoshop | TBD |
| M15 | Automation, custom workflows and plugins | Actions panel (record/replay), batch processing, Rob's custom tools; a WebAssembly plugin interface over M7-T08's tool kinds and the filter kernels, and a scripting layer over the serialisable commands (D-010) | TBD |

Current milestone: **M6** (M0–M5 coded and merged; acceptance measurements for M1–M5 are Rob's, see `docs/reports/STATUS-2026-09-25.md` and the per-task reports).
