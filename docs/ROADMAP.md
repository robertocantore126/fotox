# Roadmap

Milestones are done in order. Each ends with an **acceptance run** on the
reference machine. Detailed task cards exist for the current and next
milestone only (`docs/tasks/`); later ones are refined when we get there.

| | Milestone | Result you can see | Exit criteria |
| --- | --- | --- | --- |
| **M0** | Shell | `fotox.exe` opens with the full Fotox UI; the canvas area shows a native GPU test pattern you can pan/zoom at 60 fps; rulers and zoom readout follow | M0 checklist in `tasks/M0.md` |
| **M1** | Open and navigate | Open a 30k × 30k 16-bit TIFF, fly around it smoothly, RAM stays under budget | S1, S2, S8 (`PERFORMANCE.md`) |
| **M2** | Layers and blend modes | Layer stack with groups, masks, clipping, all blend modes, 6 adjustment layers, live Layers/History panels, undo | S4, S5, S6, S8 |
| M3 | Native file + export | `.fxd` save/open (lazy, incremental), TIFF/PNG/JPEG export | S3, S7 |
| M4 | Colour + first filters | Colour management, soft proof, CMYK export, more adjustments, filter framework (Gaussian blur, Unsharp mask) with live preview | TBD |
| M5 | Selections and painting | Marquee/lasso/magic wand, brush engine with pen pressure, eraser, mask painting, clone stamp, healing | S9 |
| M6 | Transform and vector | Free transform/warp, crop, image/canvas size, shapes and text (Vello), basic layer styles | TBD |
| M7 | PSD/PSB import | Open layered PSD/PSB; verify blend modes against Photoshop | TBD |
| M8 | Automation and custom workflows | Actions panel (record/replay), batch processing, Rob's custom tools | TBD |

Current milestone: **M3** (M0–M2 coded, acceptance measurements pending — `docs/reports/STATUS-2026-09-25.md`).
