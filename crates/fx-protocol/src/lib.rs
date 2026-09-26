//! # fx-protocol — UI ↔ engine messages
//!
//! Transport: the binary message channel of the vendored Graphite shell
//! (`window.sendNativeMessage(ArrayBuffer)` / `window.receiveNativeMessage`).
//! Every message is one [`Frame`]:
//!
//! ```text
//! byte 0      kind: 0 = JSON, 1 = JSON header + binary payload
//! kind 0:     bytes 1.. = UTF-8 JSON of UiToEngine / EngineToUi
//! kind 1:     bytes 1..5 = header length N (u32 little endian)
//!             bytes 5..5+N = UTF-8 JSON header (an EngineToUi value)
//!             bytes 5+N..  = raw payload (e.g. RGBA8 thumbnail pixels)
//! ```
//!
//! **Pixels of the document never travel over this channel.** Only small
//! things do: thumbnails, histograms, the navigator preview. The viewport is
//! drawn natively under the UI (docs/ARCHITECTURE.md §2).
//!
//! The JavaScript side of this file is `ui/js/native/protocol.js`. Keep the
//! two in sync; docs/PROTOCOL.md is the human-readable contract.

use fx_core::{Adjustment, BitDepth, BlendMode, Command, FilterParams, LayerId, RenderingIntent};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocId(pub u32);

// ---------------------------------------------------------------------------
// UI → engine
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiToEngine {
	/// First message after the page loads.
	Hello {
		ui_version: String,
	},
	/// Consumed by the shell. `false` while any Fotox popup (menu, drop-down,
	/// flyout, modal dialog) is open or a text field over the viewport has
	/// focus: pointer input over the viewport then goes to the UI instead of
	/// the engine. (Same mechanism as Graphite's `WindowUpdateDirectInput`.)
	DirectInput {
		enabled: bool,
	},
	/// Where the transparent viewport hole is, in *physical* window pixels
	/// (`getBoundingClientRect() × devicePixelRatio`). Sent on every layout change.
	/// Consumed by the shell (composite + input routing), which forwards the
	/// size to the engine as `EngineInput::ViewportResized`.
	ViewportBounds {
		x: f64,
		y: f64,
		width: f64,
		height: f64,
	},
	/// A Fotox menu/shortcut/button action id (`js/data/menus.js` `a:` field),
	/// e.g. `"doc:save"`, `"zoom:in"`, `"dlg:open"`. Unknown ids get a `Toast` back, never an error.
	Action {
		id: String,
		#[serde(default)]
		args: serde_json::Value,
	},
	/// A document command (panels use this directly, e.g. opacity slider).
	Command {
		doc: DocId,
		command: Command,
	},
	Undo {
		doc: DocId,
	},
	Redo {
		doc: DocId,
	},
	/// Make a document tab the active one.
	ActivateDocument {
		doc: DocId,
	},
	CloseDocument {
		doc: DocId,
	},
	/// Zoom from UI controls (status bar field, View menu). 1.0 = 100 %.
	SetZoom {
		doc: DocId,
		zoom: f64,
	},
	/// Ask for layer thumbnails (answered with binary `Thumbnail` frames).
	RequestThumbnails {
		doc: DocId,
		layers: Vec<LayerId>,
		size: u32,
	},
	/// A filter dialog's parameters changed: show `layer` filtered, live, on
	/// the visible area (M4-T05). OK sends the `apply_filter` command; Cancel
	/// sends `filter_preview_cancel`.
	FilterPreview {
		doc: DocId,
		layer: LayerId,
		filter: FilterParams,
	},
	/// Drop the filter preview (Cancel, or the dialog's Preview box off).
	FilterPreviewCancel {
		doc: DocId,
	},
	/// View ▸ Proof Setup (M4-T04): simulate the press of the CMYK profile at
	/// `path` (one of `cmyk_profiles`), and turn Proof Colors on.
	ProofSetup {
		doc: DocId,
		path: String,
		intent: RenderingIntent,
		bpc: bool,
		simulate_paper: bool,
	},
	/// The user answered the "save changes?" prompt of [`EngineToUi::CloseDirtyDocument`].
	CloseDocumentAnswer {
		doc: DocId,
		answer: CloseAnswer,
	},
	/// The active tool's option-bar values (M5-T01). `options` is a JSON object
	/// keyed by the option bar's field text without the trailing colon
	/// (`{"Size": 40, "Hardness": 75, "Mode": "Normal"}`). Sent when a tool
	/// becomes active and on every change.
	ToolOptions {
		tool: String,
		options: serde_json::Value,
	},
	/// The foreground and background colours (M5-T01), 16-bit RGBA. Sent on
	/// every swatch change, on X (swap) and on D (defaults).
	SetColors {
		fg: [u16; 4],
		bg: [u16; 4],
	},
	/// A key the UI's shortcut map did not consume, for the viewport tools
	/// (M5-T04): `"Escape"`, `"Enter"`, `"Backspace"`, `"ArrowLeft"`… named
	/// like the DOM's `KeyboardEvent.key`, the tools match on those names.
	/// The engine forwards it to the active tool, which uses it to finish or
	/// cancel an operation that is under way (the polygonal lasso closes on
	/// Enter, cancels on Escape).
	Key {
		key: String,
	},
	/// The Type tool's textarea changed (M6-T07): the whole text and the
	/// selection, as UTF-8 byte offsets.
	TextEdit {
		text: String,
		selection: (usize, usize),
	},
}

/// Answer to the "save changes before closing?" prompt (M3-T06).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAnswer {
	Save,
	DontSave,
	Cancel,
}

/// Action id prefixes the UI handles entirely by itself (panels, tools, view
/// flags, screen modes, dialogs, zoom display, workspaces). The engine does
/// not toast "not implemented" for these; it may still *read* some of them
/// (`tool:` sets the active tool, `zoom:` drives the view).
/// Mirrored in `ui/js/native/mock-engine.js`.
pub const UI_LOCAL_ACTION_PREFIXES: &[&str] = &["panel:", "panels:", "tool:", "toggle:", "screen:", "dlg:", "zoom:", "ws:", "par:", "debug:"];

// ---------------------------------------------------------------------------
// Engine → UI
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentInfo {
	pub doc: DocId,
	pub name: String,
	pub width: u32,
	pub height: u32,
	pub depth: BitDepth,
	pub profile_name: String,
	pub ppi: f32,
	pub dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerInfoKind {
	Pixel,
	Group,
	Adjustment,
	SolidFill,
	/// A vector shape layer (M6-T06).
	Shape,
	/// A text layer (M6-T07).
	Text,
}

/// Flat, UI-friendly description of one layer. The tree is expressed with
/// `depth` in top → bottom order (the order the Layers panel draws).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerInfo {
	pub id: LayerId,
	pub name: String,
	pub kind: LayerInfoKind,
	pub depth: u32,
	pub visible: bool,
	pub opacity: f32,
	pub fill: f32,
	pub blend: BlendMode,
	pub clipped: bool,
	pub has_mask: bool,
	/// Either lock below is on (the row's lock icon).
	pub locked: bool,
	/// The two locks separately (the panel's lock buttons).
	#[serde(default)]
	pub locked_pixels: bool,
	/// "Lock transparent pixels" (M5, D-049).
	#[serde(default)]
	pub locked_transparency: bool,
	/// Painting goes to this layer's mask (its mask thumbnail was clicked, M5-T09).
	#[serde(default)]
	pub edit_mask: bool,
	#[serde(default)]
	pub locked_position: bool,
	pub expanded: bool,
	pub selected: bool,
	/// Parameters of an adjustment layer (for its dialog / Properties).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub adjustment: Option<Adjustment>,
	/// Colour of a solid fill layer, 16-bit RGBA.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fill_color: Option<[u16; 4]>,
	/// Layer styles (M6-T08), for the style dialogs and the fx marker.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub styles: Option<fx_core::styles::LayerStyles>,
}

/// A font family and its styles (M6-T07).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontFamilyInfo {
	pub name: String,
	pub styles: Vec<String>,
}

/// A CMYK profile the UI can offer (M4-T04).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CmykProfileInfo {
	pub name: String,
	pub path: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryStats {
	pub hot_bytes: u64,
	pub warm_bytes: u64,
	pub scratch_bytes: u64,
	pub gpu_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineToUi {
	DocumentOpened {
		info: DocumentInfo,
	},
	DocumentChanged {
		info: DocumentInfo,
	},
	DocumentClosed {
		doc: DocId,
	},
	ActiveDocument {
		doc: Option<DocId>,
	},
	/// Full layer list. Sent after structure/props changes. Fine up to thousands of layers.
	Layers {
		doc: DocId,
		revision: u64,
		layers: Vec<LayerInfo>,
	},
	History {
		doc: DocId,
		labels: Vec<String>,
		current: usize,
		can_undo: bool,
		can_redo: bool,
	},
	/// View transform, for rulers, status bar and navigator. Throttled to ≤ 60 Hz.
	View {
		doc: DocId,
		zoom: f64,
		center_x: f64,
		center_y: f64,
		rotation_deg: f64,
	},
	/// Sent ~2×/s. The frame statistics cover the render thread's last 2 s
	/// (frames are only drawn when something changes, so an idle view is 0 fps).
	Status {
		memory: MemoryStats,
		fps: f32,
		/// Render-thread time per frame, median and 99th percentile, in ms.
		#[serde(default)]
		frame_ms_p50: f32,
		#[serde(default)]
		frame_ms_p99: f32,
		/// Source tiles uploaded to the GPU by the last frame.
		#[serde(default)]
		uploads: u32,
		/// Tiles being loaded from warm/cold storage right now.
		#[serde(default)]
		pending_loads: u32,
		/// Brush input → pixels on screen, median and 99th percentile over the
		/// last 2 s, in ms (M5-T11; 0 when nothing was painted).
		#[serde(default)]
		input_latency_ms_p50: f32,
		#[serde(default)]
		input_latency_ms_p99: f32,
	},
	Progress {
		task: u64,
		label: String,
		fraction: f32,
	},
	ProgressDone {
		task: u64,
	},
	Toast {
		text: String,
	},
	Error {
		text: String,
	},
	/// The eyedropper sampled a colour (M5-T01). `target` is `"fg"` or `"bg"`;
	/// the UI updates that swatch.
	ColorPicked {
		rgba: [u16; 4],
		target: String,
	},
	/// A tool's status line (M5-T10): the marquee's size while it is dragged.
	/// Empty = clear it.
	ToolInfo {
		text: String,
	},
	/// A Free Transform box went up or down (M6-T04): the UI shows the
	/// transform option bar (interpolation, ✓, ✗) while it is up.
	TransformBox {
		up: bool,
	},
	/// The Type tool's session (M6-T07): `open` shows the hidden textarea with
	/// `text` and `selection` (UTF-8 byte offsets); `false` removes it.
	TextEdit {
		open: bool,
		text: String,
		selection: (usize, usize),
	},
	/// The preferences file's content (M7-T09): after `hello` and on every
	/// change (Open Recent is built from `recent`).
	Preferences {
		prefs: serde_json::Value,
	},
	/// The system's font families (M6-T07), for the Type option bar.
	Fonts {
		families: Vec<FontFamilyInfo>,
	},
	/// The CMYK profiles for proofing and export (M4-T04), sent after `hello`.
	CmykProfiles {
		profiles: Vec<CmykProfileInfo>,
	},
	/// Proof state of a document changed (menus show the check marks).
	ProofState {
		doc: DocId,
		proof_colors: bool,
		gamut_warning: bool,
		profile: Option<String>,
	},
	/// A dirty document was asked to close: the UI shows the save/don't save/
	/// cancel prompt and answers with [`UiToEngine::CloseDocumentAnswer`].
	CloseDirtyDocument {
		doc: DocId,
		name: String,
	},
	/// Binary frame: payload = `width × height × 4` bytes RGBA8, straight alpha.
	Thumbnail {
		doc: DocId,
		layer: LayerId,
		revision: u64,
		width: u32,
		height: u32,
	},
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
	#[error("empty frame")]
	Empty,
	#[error("unknown frame kind {0}")]
	UnknownKind(u8),
	#[error("truncated frame")]
	Truncated,
	#[error("invalid JSON: {0}")]
	Json(#[from] serde_json::Error),
}

pub const KIND_JSON: u8 = 0;
pub const KIND_BINARY: u8 = 1;

pub fn encode_json<T: Serialize>(message: &T) -> Vec<u8> {
	let mut out = vec![KIND_JSON];
	serde_json::to_writer(&mut out, message).expect("protocol messages always serialise");
	out
}

pub fn encode_binary<T: Serialize>(header: &T, payload: &[u8]) -> Vec<u8> {
	let header = serde_json::to_vec(header).expect("protocol messages always serialise");
	let mut out = Vec::with_capacity(5 + header.len() + payload.len());
	out.push(KIND_BINARY);
	out.extend_from_slice(&(header.len() as u32).to_le_bytes());
	out.extend_from_slice(&header);
	out.extend_from_slice(payload);
	out
}

/// Decode a frame into its message and (for binary frames) payload.
pub fn decode<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<(T, &[u8]), FrameError> {
	let (&kind, rest) = frame.split_first().ok_or(FrameError::Empty)?;
	match kind {
		KIND_JSON => Ok((serde_json::from_slice(rest)?, &[])),
		KIND_BINARY => {
			let len_bytes: [u8; 4] = rest.get(..4).ok_or(FrameError::Truncated)?.try_into().unwrap();
			let len = u32::from_le_bytes(len_bytes) as usize;
			let header = rest.get(4..4 + len).ok_or(FrameError::Truncated)?;
			Ok((serde_json::from_slice(header)?, &rest[4 + len..]))
		}
		other => Err(FrameError::UnknownKind(other)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn json_roundtrip() {
		let msg = UiToEngine::ViewportBounds {
			x: 44.0,
			y: 80.0,
			width: 1600.0,
			height: 900.0,
		};
		let frame = encode_json(&msg);
		assert_eq!(frame[0], KIND_JSON);
		assert_eq!(&frame[1..], br#"{"type":"viewport_bounds","x":44.0,"y":80.0,"width":1600.0,"height":900.0}"#);
		let (back, payload): (UiToEngine, _) = decode(&frame).unwrap();
		assert_eq!(back, msg);
		assert!(payload.is_empty());
	}

	#[test]
	fn action_args_default_to_null() {
		let (msg, _): (UiToEngine, _) = decode(&[&[KIND_JSON][..], br#"{"type":"action","id":"dlg:open"}"#].concat()).unwrap();
		assert_eq!(
			msg,
			UiToEngine::Action {
				id: "dlg:open".into(),
				args: serde_json::Value::Null
			}
		);
	}

	#[test]
	fn protocol_md_examples_decode() {
		// Keep in sync with docs/PROTOCOL.md §6.
		let frame = [
			&[KIND_JSON][..],
			br#"{"type":"command","doc":1,"command":{"op":"set_layer_props","layer":{"id":7},"props":{"opacity":0.5}}}"#,
		]
		.concat();
		let (msg, _): (UiToEngine, _) = decode(&frame).unwrap();
		assert!(matches!(msg, UiToEngine::Command { doc: DocId(1), .. }));
		let layers = br#"{"type":"layers","doc":1,"revision":12,"layers":[{"id":7,"name":"Sky","kind":"pixel","depth":0,"visible":true,"opacity":0.5,"fill":1.0,"blend":"normal","clipped":false,"has_mask":false,"locked":false,"expanded":false,"selected":true}]}"#;
		let (msg, _): (EngineToUi, _) = decode(&[&[KIND_JSON][..], layers].concat()).unwrap();
		assert!(matches!(msg, EngineToUi::Layers { revision: 12, .. }));
	}

	#[test]
	fn binary_roundtrip() {
		let header = EngineToUi::Thumbnail {
			doc: DocId(1),
			layer: LayerId(7),
			revision: 3,
			width: 2,
			height: 1,
		};
		let pixels = [1u8, 2, 3, 4, 5, 6, 7, 8];
		let frame = encode_binary(&header, &pixels);
		let (back, payload): (EngineToUi, _) = decode(&frame).unwrap();
		assert_eq!(back, header);
		assert_eq!(payload, &pixels);
	}

	#[test]
	fn tool_options_round_trip() {
		let msg = UiToEngine::ToolOptions {
			tool: "brush".into(),
			options: serde_json::json!({"Size": 40, "Hardness": 75, "Mode": "Normal"}),
		};
		let (back, _): (UiToEngine, _) = decode(&encode_json(&msg)).unwrap();
		assert_eq!(back, msg);
	}

	#[test]
	fn set_colors_round_trip() {
		let msg = UiToEngine::SetColors {
			fg: [0, 65535, 0, 65535],
			bg: [65535, 65535, 65535, 65535],
		};
		let (back, _): (UiToEngine, _) = decode(&encode_json(&msg)).unwrap();
		assert_eq!(back, msg);
	}

	#[test]
	fn key_round_trip() {
		let msg = UiToEngine::Key { key: "Backspace".into() };
		let (back, _): (UiToEngine, _) = decode(&encode_json(&msg)).unwrap();
		assert_eq!(back, msg);
	}

	#[test]
	fn color_picked_round_trip() {
		let msg = EngineToUi::ColorPicked {
			rgba: [1, 2, 3, 65535],
			target: "fg".into(),
		};
		let (back, _): (EngineToUi, _) = decode(&encode_json(&msg)).unwrap();
		assert_eq!(back, msg);
	}
}
