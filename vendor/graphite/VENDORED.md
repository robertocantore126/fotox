# Vendored from Graphite

* Upstream: https://github.com/GraphiteEditor/Graphite
* Commit: `ddafaaa7575a1a7f771399a693ab2e53c0900061` (2026-09-23)
* Licence: MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE` in this folder)
* Copyright: Graphite Authors <contact@graphite.art>

| Folder | Upstream path |
| --- | --- |
| `desktop-ui/` | `desktop/ui/` |
| `embedded-resources/` | `desktop/embedded-resources/` |
| `wgpu-sync/` | `libraries/wgpu-sync/` |

## Patches (search for `FOTOX PATCH`)

| File | Change | Why |
| --- | --- | --- |
| `*/Cargo.toml` | `license.workspace = true` → explicit `"MIT OR Apache-2.0"` | our workspace has no licence field; keep theirs accurate |
| `embedded-resources/build.rs` | `DEFAULT_RESOURCES_DIR` → `../../../ui` | our UI lives in `<repo>/ui` |
| `desktop-ui/src/dirs.rs` | temp dir name `Graphite`/`graphite` → `Fotox`/`fotox` | never touch a real Graphite install's temp files |
| `desktop-ui/src/remote/spawn.rs` | shared-memory name prefix → `art.fotox.Fotox.cef-frames` | same |
| `desktop-ui/src/consts.rs` | `IPC_BOOTSTRAP_PREFIX` → `art.fotox.Fotox.ipc.` (macOS only) | same |

Verified: `cargo check -p fx-app` succeeds against these crates (Linux, with
`cef-dll-sys/dox` to skip the CEF download) on 2026-09-24. The Windows build
is verified in task M0-T01.
