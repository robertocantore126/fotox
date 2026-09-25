# M3 — review of the DeepSeek work, and completion

Reviewer and finisher: Claude  ·  Branch: `m3-finish` (on top of the agent's chain
`task/M3-T01-fxd-container` → … → `task/M3-T07-export` and its interrupted
`task/M3-T06-wip`)

## What the agent had done

One agent built M3 alone, card by card, on stacked branches, with honest reports
(`M3-T01.md` … `M3-T05.md`, `M3-T07.md`):

* **T00** decisions D-023…D-030 recorded first, as asked.
* **T01** the `.fxd` container: header / chunks / footer exactly as the card,
  positional reads and writes (`seek_read`/`seek_write` loops), backwards footer
  search with overlapping blocks and the `end_offset` check, `sync_data` before
  and after the footer. Good code.
* **T02** the manifest (versioned serde model in `fx-io`, only non-empty slots,
  levels 0 and ≥ 3, unknown fields ignored, version 2 refused) and
  `Document::id_state` / `with_id_state`.
* **T03** backed tiles in `fx-tiles` (`TileSource`, `Backed`, `insert_backed`,
  `attach_backing`, `backing()`, `backed_reads`).
* **T04** incremental Save / Save As, `.part` + rename, `needs_compaction`.
* **T05** lazy open (only footer + manifest read; a test proves no tile is read).
* **T07** JPEG export (`jpeg-encoder`, quality, 4:4:4 / 4:2:0, capped at 16 384 px
  per side because the encoder is not streaming).
* **T06** (interrupted, uncommitted, saved as `task/M3-T06-wip`): engine side of
  Save / Save As / close prompts / `.fxd` open — well structured.

`cargo clippy --workspace -D warnings` was clean; one test failed
(`unknown_extension_is_refused` still expected `.jpg` to be refused).

## Problems found in review, and the fixes

1. **Backed tiles lost their RAM copy at every trim** (`fx-tiles/src/store.rs`).
   `trim()` scanned every live tile and dropped the hot copy of *every* backed
   tile, in use or not: with a `.fxd` open, the tiles on screen would be thrown
   away and re-read from disk over and over. Fixed: backed tiles follow the
   normal LRU (the existing `demote_hot` already drops, never compresses, a tile
   with a backed copy); `attach_backing` releases the now redundant warm and
   cold copies **at once** (scratch freed immediately), so trim needs no scan.
   Test rewritten: under budget the hot copy stays; under pressure it is dropped,
   never compressed or spilled.
2. **Stale or evicted mips were saved** (`fx-io/src/fxd/{save,manifest}.rs`).
   Mip slots ≥ 3 were stored without checking their dirty flag (a reopened file
   would show old pixels at low zoom until the next edit), and an evicted derived
   tile made the whole save fail (`TileError::Evicted`). Fixed: dirty mip slots
   are left out of the file and the manifest (they are empty + dirty after
   opening, so the renderer rebuilds them); evicted derived tiles are skipped.
   New test `clean_mips_are_stored_and_dirty_ones_are_not`.
3. **Sequential compression** (card: rayon, ≤ 64 in flight). Fixed: tiles are
   compressed in parallel 64 at a time and written in order.
4. **A corrupt chunk length could allocate gigabytes** (`read_chunk`). Fixed: the
   header's length must match the reference's.
5. **A torn tail longer than the next save stayed in the file.** Fixed: `commit`
   truncates the file to the new footer.
6. **Save marked the document clean even if it was edited during the save.**
   Fixed: the save carries the document generation of its snapshot.
7. **Save As onto the document's own open file** would fail on Windows (the
   backed tiles keep it open). Fixed: it becomes an incremental save.
8. **Closing the window with several unsaved documents** stopped after the first
   prompt. Fixed: after each answer the engine asks about the next one, Cancel
   stops the close; a cancelled save dialog cancels the pending close
   (`EngineInput::SaveCancelled`).
9. **B3 layers had no mips**: the first frame at fit (and a saved `.fxd`, which
   stores mips ≥ 3) needed every level-0 tile. The B3 job now builds each new
   layer's pyramid, like an import.

## What I completed

* **T06 engine + shell + UI**: `doc:save` (Ctrl+S) and `doc:save-as`
  (Shift+Ctrl+S, new menu item) → incremental save or `EngineOutput::NeedSavePath`
  → the shell's native `.fxd` save dialog → `EngineInput::SaveAs`; the window's
  close button goes through the engine (`CloseRequested` → `MayClose`); the UI's
  *Save / Don't Save / Cancel* prompt (`close_dirty_document` /
  `close_document_answer`, the dialog engine gained custom `buttons`); `.fxd` in
  the Open dialog (and drag & drop / command line via the magic). The old
  Shift+Ctrl+S → Export As shortcut was removed (Photoshop: Save As).
* **T07**: a real *Export As* dialog (File ▸ Export ▸ Export As…): format PNG / JPG
  / TIFF, bit depth (document / 8), transparency (automatic / on / off), JPEG
  quality and chroma → `export:as` → the shell's save dialog for that format →
  `EngineInput::Export { path, choice }`; *File ▸ Export ▸ JPG* direct.
* An **engine integration test** (`crates/fx-engine/tests/save_flow.rs`, real
  engine + render threads, skipped without a GPU): import a TIFF → Save asks for
  a path → Save As writes the `.fxd` and cleans the document → an edit makes it
  dirty → closing asks → the window may not close meanwhile → "Save" saves, closes
  and lets the window close → the `.fxd` reopens clean with the edit.
* `docs/PROTOCOL.md` updated.

## Verification

```text
cargo fmt --check (all crates) / clippy --workspace -D warnings / cargo test --workspace: clean
  (fx-io 45 tests, fx-tiles 32, fx-engine 38 + the save-flow integration test)
node ui/tools/check-data.mjs: ok
In the app (6000 × 4000 TIFF, driven over the DevTools port):
  opacity edit → tab shows "t6000.tif @ 21% (RGB/16)*";
  close → prompt "Save changes to “t6000.tif” before closing?" [Save] [Don't Save] [Cancel];
  Cancel → the tab stays; close again → Don't Save → closed;
  WM_CLOSE on the window → engine answers MayClose → "leaving the event loop", no fotox.exe left.
```

Not tried by hand (native dialogs): Save As dialog, Export As → save dialog,
opening the `.fxd` from the Open dialog. The same paths are covered by the
integration test below the dialogs.

## Still open (M3)

* **Composite preview chunks are not written** (D-026 half): the save passes
  `preview: None`. Per-layer mips ≥ 3 are stored, so the first frame after
  opening composites from them (a few tiles per layer). If S3 (< 1 s on B3) is
  missed, render the preview on the engine side (it has the compositor) and pass
  it to `SaveRequest::preview` — the file side is ready for it.
* **Automatic compaction** is not triggered (`needs_compaction` exists; Save As
  to a new name compacts). Compacting the document's own open file needs the
  handle dance of the card; deferred until files actually grow in practice.
* **Save As of a document opened from another `.fxd` recompresses** its tiles
  (the agent's report explains why: `TileSource` returns pixels, not chunks).
* **GPU readback export** (M3-T07 item 4) is not done: the CPU compositor
  exports B1 + B3 in minutes.
* JPEG export holds the 8-bit image in memory (capped at 16 384 px per side).
* **M3-T08 (Rob)**: S3 and S7 on B3 saved as `.fxd`.
