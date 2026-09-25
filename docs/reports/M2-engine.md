# M2 engine side — commands, undo/redo, split cache (M2-T05), thumbnails (M2-T07)

Agent: Claude  ·  Branch: `task/M2-engine`  ·  Status: done; S5 measurement with/without the split cache is Rob's

## Commands and history (engine side of M2-T06)

* `command {doc, command}` → `History::execute` with the engine's tile store. On success: `layers`, `history`, `document_changed` (dirty) and a new frame; selection-only commands send `layers` + `history` only. Refused commands → `error` with the reason.
* `undo` / `redo {doc}`, and the actions `hist:undo` (Ctrl+Z), `hist:redo` (Ctrl+Shift+Z), `hist:toggle` (Ctrl+Alt+Z: redo if something was just undone, else undo).
* `history` message: `labels` = undo steps followed by the undone (redoable) steps, `current` = number of undo steps. `fx-core::History` gained `redo_labels()` (read-only accessor, needed for that list).
* **Finding:** undo swaps the whole document back, `revision` included, so after undo + a new command two different states can carry the same revision. Each open document now has a `generation` bumped on every change; the render thread keys its ready-map and program cache on (document, generation), and snapshots are rebuilt on every change.

## M2-T05 — split cache (engine side)

A layer that receives ≥ 2 edits within 1 s (props or pixels, e.g. an opacity slider drag) becomes the document's hot layer; 2 s without edits cools it down (engine timer). Every frame passes it to `GpuCompositor::set_hot_layer`. Tests: `documents::tests::a_layer_edited_twice_within_a_second_becomes_hot_then_cools`.

Not done: S5 with and without (needs B3, M2-T08, and the app).

## M2-T07 — thumbnails

* `request_thumbnails {doc, layers, size}` → per layer a rayon job (`thumbs::render`): the deepest mip level with ≥ `size` px on the document's longer side, a few tiles stitched, box-filtered into a box with the document's aspect, the layer's offset respected, RGBA8 straight → binary `thumbnail` frame. Solid fills → a flat colour; groups and adjustment layers get none (the panel shows icons, like Photoshop).
* Throttle: a layer whose pixels change is re-rendered at most every 500 ms (later changes are coalesced into one due refresh); after undo/redo every requested thumbnail of the document is refreshed.
* Tests: aspect fitting, half-red layer, offset layer, solid fill.

## Verification

```text
cargo fmt --check / clippy --workspace -D warnings / cargo test: clean
fx-engine 33 tests, fx-core 67
```
