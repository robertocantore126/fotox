//! # fx-engine — the application core without any window
//!
//! Threads (docs/ARCHITECTURE.md §2.2):
//! * **engine thread** — owns every open `Document` and its `History`,
//!   processes [`EngineInput`] strictly in order. It never waits on disk or
//!   long GPU work: heavy jobs go to the worker pool and come back as
//!   internal job-done inputs.
//! * **render thread** — owns the `fx-render` compositor/atlas and renders the
//!   viewport texture from the latest document snapshot + view transform.
//!   Rendering is *pull*-based: the shell asks for a frame, the render thread
//!   draws whatever is ready and schedules the rest.
//! * **worker pool** (rayon) — import/export, mip generation, filters, compression.
//!
//! The engine is headless: `fx-cli` drives it without any GUI, which is how
//! most engine features are tested and benchmarked.

use fx_protocol::UiToEngine;

/// Pointer/keyboard input that happened *over the viewport*. The shell routes
/// it here directly (not through the UI) to keep brush latency low.
/// Coordinates are physical pixels relative to the viewport's top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerInput {
	pub kind: PointerKind,
	pub x: f64,
	pub y: f64,
	/// 0..=1; 1.0 for mouse.
	pub pressure: f32,
	/// Pen tilt in degrees, 0 for mouse.
	pub tilt_x: f32,
	pub tilt_y: f32,
	pub buttons: u8,
	pub modifiers: Modifiers,
	/// Monotonic timestamp in microseconds (for stroke smoothing/velocity).
	pub time_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
	Down,
	Move,
	Up,
	/// Pointer left the viewport (hover state must be cleared).
	Leave,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
	pub shift: bool,
	pub ctrl: bool,
	pub alt: bool,
	/// Space held (temporary hand tool, Photoshop behaviour).
	pub space: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineInput {
	/// A decoded message from the UI.
	Ui(UiToEngine),
	Pointer(PointerInput),
	Wheel {
		x: f64,
		y: f64,
		dx: f64,
		dy: f64,
		modifiers: Modifiers,
	},
	/// Viewport size in physical pixels changed.
	ViewportResized {
		width: u32,
		height: u32,
	},
	Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineOutput {
	/// An encoded fx-protocol frame for the UI (`UiCommand::Message`).
	ToUi(Vec<u8>),
	/// The viewport changed; the shell should request a new frame.
	RedrawViewport,
	/// Cursor to show over the viewport (tool-dependent).
	Cursor(CursorShape),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
	Default,
	Crosshair,
	Grab,
	Grabbing,
	ZoomIn,
	ZoomOut,
	/// Brush outline is drawn by the viewport overlay; hide the OS cursor.
	None,
}

/// Handle to the running engine (M0-T04).
pub struct EngineHandle {
	_private: (),
}
