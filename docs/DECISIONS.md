# Decision log

One entry per decision. Do not re-open a decision inside a task: if a task
seems to need a different choice, stop and ask (see `AGENTS.md` §6).
New entries go at the bottom with the next number.

| # | Decision | Why | Date |
| --- | --- | --- | --- |
| D-001 | Write our own engine; do not fork an existing editor. Vendor only Graphite's CEF/wgpu shell. | The winning part (tiled, out-of-core, lazy compositing) is the core of an editor and cannot be retrofitted. The shell is generic and hard to write. | 2026-09-24 |
| D-002 | UI = the Fotox HTML mock running in CEF off-screen, composited over a native wgpu viewport. Not Tauri/WebView2, not a native widget toolkit. | Keeps the finished Fotox UI; pixels never cross a process boundary; transparent WebView2 over a GPU surface is fragile on Windows. CEF adds ~150–200 MB, acceptable for a personal tool. | 2026-09-24 |
| D-003 | Rust (edition 2024) + wgpu (DX12 on Windows). | Memory safety with manual control of memory, threads and GPU; same stack as the vendored shell. | 2026-09-24 |
| D-004 | 256 × 256 tiles, immutable, copy-on-write, `Arc` handles; uniform tiles stored as values. | Cheap snapshots/undo, safe background rendering, zero cost for empty areas. | 2026-09-24 |
| D-005 | Storage in straight alpha; GPU works premultiplied. | Editing tools and PSD expect straight alpha; compositing needs premultiplied. Convert at upload. | 2026-09-24 |
| D-006 | Blend on encoded values (document profile), Photoshop formulas (`BLEND_MODES.md`). | Matching Photoshop's look is a requirement. | 2026-09-24 |
| D-007 | 8-bit and 16-bit (full 0..65535 range) only. No 32-bit float for now. | Covers retouch/compositing/print; 32f doubles memory for little benefit in these use cases. | 2026-09-24 |
| D-008 | Derived tiles (mips, caches) are never written to scratch. | Scratch space is the scarcest resource on the reference machine. | 2026-09-24 |
| D-009 | Undo history = snapshots of the (cheap-to-clone) document. Limit 50. | Simple, always correct, memory proportional to what changed. | 2026-09-24 |
| D-010 | Every document change is a serialisable `Command`. | Undo, UI protocol, macros and batch processing share one mechanism. | 2026-09-24 |
| D-011 | Protocol: JSON frames + JSON-header/binary frames over the shell's message channel. | Debuggable; small messages only; pixels never cross. | 2026-09-24 |
| D-012 | Print = RGB editing + soft proof + CMYK conversion on export. No native CMYK mode. | Rob's choice; much simpler, covers large-format print. | 2026-09-24 |
| D-013 | Native `.fxd` is the working format; PSD/PSB import in M7, export later. | Lazy open and incremental save are impossible with PSD; PSD compatibility still matters for existing files. | 2026-09-24 |
| D-014 | Windows is the only tested platform. Keep `cfg` structure portable. | Reference machine. | 2026-09-24 |
| D-015 | Layer moves change an integer offset; pixels are not rewritten. | Instant moves at any size (`ARCHITECTURE.md` §4.3). | 2026-09-24 |
| D-016 | LZ4 (`lz4_flex`) for warm and cold tiers; zstd only for `.fxd` files. | Tier transitions are on the interactive path: speed over ratio. | 2026-09-24 |
