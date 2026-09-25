# UI ↔ engine protocol

Rust source of truth: `crates/fx-protocol/src/lib.rs`.
JavaScript mirror: `ui/js/native/protocol.js` (created in M0-T05).
Any change touches both files and this document in the same commit.

## 1. Transport

The vendored Graphite shell carries opaque byte messages (see `GRAPHITE.md` §1):

* JS → native: `window.sendNativeMessage(ArrayBuffer)`
* native → JS: `window.receiveNativeMessage(ArrayBuffer)`
* JS must call `window.initializeNativeCommunication()` once, after
  installing `receiveNativeMessage`. Native queues outgoing messages until then.

## 2. Framing

```
byte 0 = kind
  0  JSON          bytes 1..        UTF-8 JSON object
  1  JSON+binary   bytes 1..5       header length N, u32 little-endian
                   bytes 5..5+N     UTF-8 JSON header
                   bytes 5+N..      raw payload
```

Every JSON object has a `"type"` field (snake_case variant name).
Numbers follow serde defaults (floats always carry a decimal point when
serialised from Rust; JS accepts both).

## 3. Who consumes what

Some UI messages are for the **shell** (`fx-app`), not the engine:

| Message | Consumer |
| --- | --- |
| `viewport_bounds` | shell (composite + input routing) → forwards size to engine |
| `direct_input` | shell (input routing) |
| everything else | engine |

## 4. UI → engine

| type | fields | when |
| --- | --- | --- |
| `hello` | `ui_version` | page loaded and receiver installed |
| `direct_input` | `enabled` | a popup/menu/dialog opens (`false`) or all close (`true`) |
| `viewport_bounds` | `x, y, width, height` (physical px) | layout change (`ResizeObserver` on `#viewport`, window resize, panel dock resize, screen mode change) |
| `action` | `id`, `args?` | any Fotox action id from `js/data/menus.js` (`"doc:save"`, `"zoom:in"`, `"dlg:open"`, `"tool:brush"`…). Unknown ids → engine answers `toast`. The shell also answers `dlg:open` (native open dialog), `export:png` and `export:tiff` (native save dialog, then the engine exports the flattened document with `progress` and a `toast`). `doc:save` / `doc:save-as` (M3-T06): the engine saves the `.fxd` incrementally, or — no file yet, or Save As — tells the shell (`EngineOutput::NeedSavePath`, not a protocol message) to show its save dialog. |
| `command` | `doc`, `command` (an `fx_core::Command` JSON) | panels acting directly on the document (Layers panel opacity, visibility eye, rename…) |
| `undo` / `redo` | `doc` | |
| `activate_document` / `close_document` | `doc` | document tabs; closing an unsaved document is answered with `close_dirty_document` instead |
| `close_document_answer` | `doc`, `answer`: `"save"` \| `"dont_save"` \| `"cancel"` | the user's answer to `close_dirty_document` (M3-T06) |
| `set_zoom` | `doc`, `zoom` (1.0 = 100 %) | status-bar zoom field, View menu |
| `request_thumbnails` | `doc`, `layers`, `size` | Layers panel needs thumbnails |

Action routing rule for the UI (`ui/js/actions.js`): actions that only change
UI state (panels, screen modes, tool selection display) stay in JS as today;
**every** action is also sent to the engine, which ignores the ones it does
not handle. The engine is the authority for anything that touches a document.

## 5. Engine → UI

| type | fields | notes |
| --- | --- | --- |
| `document_opened` / `document_changed` | `info: DocumentInfo` | create/update a document tab |
| `document_closed` | `doc` | |
| `active_document` | `doc \| null` | |
| `layers` | `doc, revision, layers: LayerInfo[]` | full list, top → bottom, tree via `depth`; each row also carries `locked_pixels`, `locked_position`, and `adjustment` (adjustment layers) or `fill_color` (solid fills) |
| `history` | `doc, labels, current, can_undo, can_redo` | History panel, Edit menu state |
| `view` | `doc, zoom, center_x, center_y, rotation_deg` | rulers, status bar, navigator; ≤ 60 Hz |
| `status` | `memory: MemoryStats, fps, frame_ms_p50, frame_ms_p99, uploads, pending_loads` | status bar memory readout, frame-time overlay (`debug:fps`); ~2 Hz |
| `progress` / `progress_done` | `task, label, fraction` / `task` | long jobs (import, export, filters) |
| `toast` / `error` | `text` | |
| `close_dirty_document` | `doc, name` | the document has unsaved changes: show *Save / Don't Save / Cancel*, answer with `close_document_answer` (M3-T06). Also sent while the window is closing, once per unsaved document |
| `thumbnail` (binary frame) | header: `doc, layer, revision, width, height`; payload: RGBA8 straight | Layers panel |

## 6. Examples

```json
{"type":"viewport_bounds","x":44.0,"y":80.0,"width":1600.0,"height":900.0}
{"type":"action","id":"zoom:in"}
{"type":"command","doc":1,"command":{"op":"set_layer_props","layer":{"id":7},"props":{"opacity":0.5}}}
{"type":"layers","doc":1,"revision":12,"layers":[{"id":7,"name":"Sky","kind":"pixel","depth":0,"visible":true,"opacity":0.5,"fill":1.0,"blend":"normal","clipped":false,"has_mask":false,"locked":false,"expanded":false,"selected":true}]}
```

Note the shapes serde produces: `LayerId` is a bare number (`7`), `LayerRef`
is `"active"`, `{"id":7}` or `{"named":"Sky"}`.
