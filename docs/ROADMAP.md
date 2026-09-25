# Roadmap

Milestones are done in order. Each ends with an **acceptance run** on the
reference machine. Task cards: `docs/tasks/M0.md` … `M2.md` (done),
`M3.md` … `M6.md` (**drafts**, 25 Sep 2026 — each starts with a `T00` card of
decisions for Rob, and is refined when its milestone starts). How the
recurring pieces are built (commands, jobs, live previews, tools, derived
tiles, filters, measurements): `docs/tasks/HOWTO.md`.

| | Milestone | Result you can see | Exit criteria |
| --- | --- | --- | --- |
| **M0** | Shell | `fotox.exe` opens with the full Fotox UI; the canvas area shows a native GPU test pattern you can pan/zoom at 60 fps; rulers and zoom readout follow | M0 checklist in `tasks/M0.md` |
| **M1** | Open and navigate | Open a 30k × 30k 16-bit TIFF, fly around it smoothly, RAM stays under budget | S1, S2, S8 (`PERFORMANCE.md`) |
| **M2** | Layers and blend modes | Layer stack with groups, masks, clipping, all blend modes, 6 adjustment layers, live Layers/History panels, undo | S4, S5, S6, S8 |
| M3 | Native file + export | `.fxd` save/open (lazy, incremental), TIFF/PNG/JPEG export | S3, S7 |
| M4 | Colour + first filters | Colour management, soft proof, CMYK export, more adjustments, merge/flatten, filter framework (Gaussian blur, Unsharp mask) with live preview | S10–S12, C1, C2 (proposed) |
| M5 | Selections and painting | Marquee/lasso/magic wand, brush engine with pen pressure, eraser, mask painting, clone stamp, healing | S9, S13–S15 (proposed) |
| M6 | Transform and vector | Free transform/warp, crop, image/canvas size, view rotation, shapes and text (renderer: M6-T00), basic layer styles | S16–S20 (proposed) |
| M7 | PSD/PSB import | Open layered PSD/PSB; verify blend modes against Photoshop | TBD |
| M8 | Automation and custom workflows | Actions panel (record/replay), batch processing, Rob's custom tools | TBD |

Current milestone: **M3** (M0–M2 coded, acceptance measurements pending — `docs/reports/STATUS-2026-09-25.md`).
