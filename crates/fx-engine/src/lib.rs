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

pub mod b3;
pub mod documents;
pub mod export;
pub mod layers;
pub mod mips;
pub mod thumbs;
pub mod view;

mod engine;
mod render;
mod stats;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use fx_protocol::UiToEngine;
use fx_tiles::{TileStore, TileStoreConfig};

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
	/// Buttons held *after* this event: [`view::BUTTON_LEFT`],
	/// [`view::BUTTON_RIGHT`], [`view::BUTTON_MIDDLE`] bits.
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
	/// Wheel over the viewport at `(x, y)` (viewport pixels). `dx`, `dy` are
	/// in wheel notches, fractional for smooth wheels and touchpads; positive
	/// `dy` = wheel turned away from the user.
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
	/// Open these files (native file dialog, drag and drop, command line).
	Open(Vec<PathBuf>),
	/// Export the active document, flattened, to this file (the shell's save
	/// dialog; the format comes from the extension). M3.
	Export(PathBuf),
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
	/// A freshly rendered viewport texture (`fx_render::VIEWPORT_FORMAT`,
	/// sRGB-encoded values in a non-sRGB format). The shell composites it
	/// until the next one arrives.
	ViewportFrame(wgpu::Texture),
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

/// Where the engine delivers its outputs. The shell wraps its event-loop
/// proxy in one; it is called from the engine and render threads.
pub type OutputSink = Arc<dyn Fn(EngineOutput) + Send + Sync>;

/// Handle to the running engine: the engine thread and the render thread.
///
/// Dropping the handle stops both threads and waits for them.
pub struct EngineHandle {
	input: Sender<EngineInput>,
	threads: Vec<JoinHandle<()>>,
}

impl EngineHandle {
	/// Start the engine and render threads. `queue` must be the shell's
	/// [`wgpu_sync::Queue`]: every submission from the render thread goes
	/// through it (see the crate docs). Tiles spill to a scratch file in
	/// `scratch_dir` (reference-machine budgets, docs/PERFORMANCE.md §2).
	pub fn spawn(
		device: wgpu::Device,
		queue: wgpu_sync::Queue,
		scratch_dir: PathBuf,
		output: impl Fn(EngineOutput) + Send + Sync + 'static,
	) -> std::io::Result<Self> {
		let output: OutputSink = Arc::new(output);
		let store = Arc::new(TileStore::new(TileStoreConfig::reference_machine(scratch_dir)).map_err(std::io::Error::other)?);
		let (input, inputs) = crossbeam_channel::unbounded();
		let (render_requests, render_inbox) = crossbeam_channel::unbounded();
		let (internal, internal_rx) = crossbeam_channel::unbounded();
		let (mips, mips_rx) = crossbeam_channel::unbounded();
		let stats = Arc::new(Mutex::new(stats::RenderStats::default()));

		let context = render::RenderContext {
			device,
			queue,
			store: store.clone(),
			requests: render_inbox,
			wake: render_requests.clone(),
			mips,
			output: output.clone(),
			stats: stats.clone(),
		};
		let render_thread = std::thread::Builder::new().name("fx-render".into()).spawn(move || render::run(context))?;
		let context = engine::EngineContext {
			inputs,
			internal_rx,
			internal,
			mips_rx,
			render: render_requests,
			store,
			stats,
			output,
		};
		let engine_thread = std::thread::Builder::new().name("fx-engine".into()).spawn(move || engine::run(context))?;

		Ok(Self {
			input,
			threads: vec![engine_thread, render_thread],
		})
	}

	/// Queue one input for the engine thread. Never blocks.
	pub fn send(&self, input: EngineInput) {
		// The engine thread only goes away after `Shutdown`; losing input
		// sent after that is fine.
		let _ = self.input.send(input);
	}

	/// Stop both threads and wait for them.
	pub fn shutdown(mut self) {
		self.stop();
	}

	fn stop(&mut self) {
		let _ = self.input.send(EngineInput::Shutdown);
		for thread in self.threads.drain(..) {
			if thread.join().is_err() {
				tracing::error!("an engine thread panicked");
			}
		}
	}
}

impl Drop for EngineHandle {
	fn drop(&mut self) {
		self.stop();
	}
}
