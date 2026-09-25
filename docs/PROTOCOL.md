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
| `filter_preview` | `doc, layer, filter` (`FilterParams`: `{"kind": "gaussian_blur", "radius": 4}` / `unsharp_mask {amount, radius, threshold}`) | a filter dialog changed: the engine shows the layer filtered on the visible area, live (M4-T05). OK sends the `apply_filter` command |
| `filter_preview_cancel` | `doc` | Cancel, or the Preview box off |
| `proof_setup` | `doc, path, intent, bpc, simulate_paper` | View ▸ Proof Setup (M4-T04): the CMYK profile (a `cmyk_profiles` path) to simulate; turns Proof Colors on |
| `close_document_answer` | `doc`, `answer`: `"save"` \| `"dont_save"` \| `"cancel"` | the user's answer to `close_dirty_document` (M3-T06) |
| `set_zoom` | `doc`, `zoom` (1.0 = 100 %) | status-bar zoom field, View menu |
| `request_thumbnails` | `doc`, `layers`, `size` | Layers panel needs thumbnails |
| `tool_options` | `tool`, `options` (a JSON object) | the active tool's option-bar values, keyed by the field text without the colon (`{"Size": 40, "Hardness": 75, "Mode": "Normal"}`); sent when a tool becomes active and on every change (M5-T01) |
| `set_colors` | `fg`, `bg` (16-bit RGBA arrays) | the foreground/background colours: every swatch change, X (swap), D (defaults) (M5-T01) |
| `key` | `key` (a DOM `KeyboardEvent.key`: `"Escape"`, `"Enter"`, `"Backspace"`, `"Delete"`, the arrows; `"Shift+ArrowLeft"` with Shift) | a key the UI's shortcut map did not consume, for the active viewport tool (M5-T04): Escape drops the marquee/lasso being drawn, Enter closes a polygonal lasso, Backspace/Delete drops its last point, the arrows move the selection outline (Shift = 10 px). Delete/Backspace that no tool uses clear the selected pixels (Edit ▸ Clear, M5-T05) |

M5 actions the engine owns (all through `action`): `sel:all` / `sel:none` /
`sel:reselect` / `sel:inverse`; `clip:copy`, `clip:copy-merged`, `clip:cut`,
`clip:paste`, `clip:paste-special` (Paste in Place), `clip:clear`;
`edit:fill` (`args: {use, mode, opacity, preserve}` from the Fill dialog),
`edit:fill-fg` / `edit:fill-bg` (Alt/Ctrl+Backspace) and their `-preserve`
variants; `layer:via-copy` / `layer:via-cut` (with a selection);
`mask:add` (`args: {alt}`: the panel's mask button — from the selection when
there is one), `mask:reveal-sel` / `mask:hide-sel`; `layer:edit-mask`
(`args: {layer, mask}`: a click on a layer's or its mask's thumbnail picks what
painting edits). The shell answers `clip:paste` itself when another program put
an image on the Windows clipboard since Fotox's last copy.

M6 actions the engine owns: `img:rot90cw` / `img:rot90ccw` / `img:rot180` /
`img:flip-h` / `img:flip-v` (M6-T02; the three dialogs send the
`rotate_canvas_arbitrary`, `canvas_size` and `image_size` commands);
`img:crop` (crop to the selection's bounds, M6-T03); `edit:trim`
(`args: {based_on, away}` from the Trim dialog: `based_on` is the dialog's
"Based On" label, `away` the ticked edges `"Top"`, `"Left"`, `"Bottom"`,
`"Right"`); `tool:commit` / `tool:cancel` (the option bar's ✓ and ✗, the
same as Enter and Escape for the active tool). The crop tool's ✓ sends
`{"op":"crop","rect":[x,y,w,h],"angle_deg":a,"delete_cropped":b}`.
Free Transform (M6-T04): `xf:free` (Ctrl+T) puts a box over the active layer
(or the selection); `xf:scale` / `xf:rotate` / `xf:skew` / `xf:distort` /
`xf:perspective` / `xf:warp` do the same with that gesture as the default, or
switch it while the box is up; `xf:rot180` / `xf:rot90cw` / `xf:rot90ccw` /
`xf:flip-h` / `xf:flip-v` turn the box when one is up, else transform the
layer at once. While the box is up it takes the pointer and the `key`s
(Enter, Escape, the arrows); Enter or ✓ sends
`{"op":"transform","layer":{"id":n},"mapping":{"kind":"affine"|"projective"|"warp","value":…},"filter":"bicubic"}`.

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
| `layers` | `doc, revision, layers: LayerInfo[]` | full list, top → bottom, tree via `depth`; each row also carries `locked_pixels`, `locked_transparency`, `locked_position`, `edit_mask` (painting goes to the mask, M5-T09), and `adjustment` (adjustment layers) or `fill_color` (solid fills) |
| `history` | `doc, labels, current, can_undo, can_redo` | History panel, Edit menu state |
| `view` | `doc, zoom, center_x, center_y, rotation_deg` | rulers, status bar, navigator; ≤ 60 Hz |
| `status` | `memory: MemoryStats, fps, frame_ms_p50, frame_ms_p99, uploads, pending_loads, input_latency_ms_p50, input_latency_ms_p99` | status bar memory readout, frame-time overlay (`debug:fps`); ~2 Hz. The input latency covers brush input → pixels on screen over the last 2 s (0 when nothing was painted, M5-T11) |
| `tool_info` | `text` | a tool's status line (M5-T10: the marquee's size while it is dragged) |
| `transform_box` | `up` | a Free Transform box went up or down (M6-T04): the UI shows the `_transform` option bar while it is up and sends its values as `tool_options` with `tool: "_transform"` |
| `progress` / `progress_done` | `task, label, fraction` / `task` | long jobs (import, export, filters) |
| `toast` / `error` | `text` | |
| `cmyk_profiles` | `profiles: [{name, path}]` | after `hello`: the CMYK profiles for Proof Setup and CMYK export (M4-T04) |
| `proof_state` | `doc, proof_colors, gamut_warning, profile` | Proof Colors / Gamut Warning check marks (actions `view:proof-colors` Ctrl+Y, `view:gamut-warning` Shift+Ctrl+Y) |
| `close_dirty_document` | `doc, name` | the document has unsaved changes: show *Save / Don't Save / Cancel*, answer with `close_document_answer` (M3-T06). Also sent while the window is closing, once per unsaved document |
| `color_picked` | `rgba` (16-bit RGBA), `target`: `"fg"` \| `"bg"` | the eyedropper sampled the composite; the UI updates that swatch (M5-T01) |
| `thumbnail` (binary frame) | header: `doc, layer, revision, width, height`; payload: RGBA8 straight | Layers panel |

## 6. Examples

```json
{"type":"viewport_bounds","x":44.0,"y":80.0,"width":1600.0,"height":900.0}
{"type":"action","id":"zoom:in"}
{"type":"command","doc":1,"command":{"op":"set_layer_props","layer":{"id":7},"props":{"opacity":0.5}}}
{"type":"layers","doc":1,"revision":12,"layers":[{"id":7,"name":"Sky","kind":"pixel","depth":0,"visible":true,"opacity":0.5,"fill":1.0,"blend":"normal","clipped":false,"has_mask":false,"locked":false,"expanded":false,"selected":true}]}
{"type":"tool_options","tool":"eyedropper","options":{"Sample Size":"3 by 3 Average","Sample":"All Layers"}}
{"type":"set_colors","fg":[30,30,34,65535],"bg":[65535,65535,65535,65535]}
{"type":"key","key":"Backspace"}
{"type":"color_picked","rgba":[65535,0,0,65535],"target":"fg"}
```

Note the shapes serde produces: `LayerId` is a bare number (`7`), `LayerRef`
is `"active"`, `{"id":7}` or `{"named":"Sky"}`.
