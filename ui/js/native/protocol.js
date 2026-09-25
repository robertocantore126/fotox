// Fotox — UI ↔ engine message framing and message type names.
//
// JavaScript mirror of crates/fx-protocol/src/lib.rs; docs/PROTOCOL.md is the
// contract. Any change touches all three in the same commit.
//
// Frame layout (byte 0 = kind):
//   KIND_JSON    bytes 1..        UTF-8 JSON object with a "type" field
//   KIND_BINARY  bytes 1..5       header length N, u32 little endian
//                bytes 5..5+N     UTF-8 JSON header
//                bytes 5+N..      raw payload

export const KIND_JSON = 0;
export const KIND_BINARY = 1;

/** UI → engine message types (`UiToEngine`). */
export const UI = Object.freeze({
  HELLO: "hello",
  DIRECT_INPUT: "direct_input",
  VIEWPORT_BOUNDS: "viewport_bounds",
  ACTION: "action",
  COMMAND: "command",
  UNDO: "undo",
  REDO: "redo",
  ACTIVATE_DOCUMENT: "activate_document",
  CLOSE_DOCUMENT: "close_document",
  CLOSE_DOCUMENT_ANSWER: "close_document_answer",
  FILTER_PREVIEW: "filter_preview",
  FILTER_PREVIEW_CANCEL: "filter_preview_cancel",
  SET_ZOOM: "set_zoom",
  REQUEST_THUMBNAILS: "request_thumbnails",
});

/** Engine → UI message types (`EngineToUi`). */
export const ENGINE = Object.freeze({
  DOCUMENT_OPENED: "document_opened",
  DOCUMENT_CHANGED: "document_changed",
  CLOSE_DIRTY_DOCUMENT: "close_dirty_document",
  DOCUMENT_CLOSED: "document_closed",
  ACTIVE_DOCUMENT: "active_document",
  LAYERS: "layers",
  HISTORY: "history",
  VIEW: "view",
  STATUS: "status",
  PROGRESS: "progress",
  PROGRESS_DONE: "progress_done",
  TOAST: "toast",
  ERROR: "error",
  THUMBNAIL: "thumbnail",
});

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** A JSON frame for `message` (an object with a `type` field). */
export function encodeJson(message) {
  const json = encoder.encode(JSON.stringify(message));
  const frame = new Uint8Array(1 + json.length);
  frame[0] = KIND_JSON;
  frame.set(json, 1);
  return frame.buffer;
}

/** A binary frame: JSON `header` followed by the raw `payload` bytes. */
export function encodeBinary(header, payload) {
  const json = encoder.encode(JSON.stringify(header));
  const bytes = payload instanceof Uint8Array ? payload : new Uint8Array(payload);
  const frame = new Uint8Array(5 + json.length + bytes.length);
  frame[0] = KIND_BINARY;
  new DataView(frame.buffer).setUint32(1, json.length, true);
  frame.set(json, 5);
  frame.set(bytes, 5 + json.length);
  return frame.buffer;
}

/**
 * Decode a frame into `{ message, payload }`. `payload` is an empty
 * `Uint8Array` for JSON frames. Throws on a malformed frame.
 */
export function decode(buffer) {
  const bytes = new Uint8Array(buffer);
  if (bytes.length === 0) throw new Error("empty frame");
  switch (bytes[0]) {
    case KIND_JSON:
      return { message: JSON.parse(decoder.decode(bytes.subarray(1))), payload: new Uint8Array(0) };
    case KIND_BINARY: {
      if (bytes.length < 5) throw new Error("truncated frame");
      const length = new DataView(bytes.buffer, bytes.byteOffset).getUint32(1, true);
      if (bytes.length < 5 + length) throw new Error("truncated frame");
      return {
        message: JSON.parse(decoder.decode(bytes.subarray(5, 5 + length))),
        payload: bytes.subarray(5 + length),
      };
    }
    default:
      throw new Error(`unknown frame kind ${bytes[0]}`);
  }
}
