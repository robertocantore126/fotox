//! Command line of the Fotox desktop app.

use std::path::PathBuf;

use clap::Parser;

/// The Fotox desktop app.
#[derive(Parser)]
#[command(name = "fotox", version, about = "Fotox — an image editor for huge documents")]
pub(crate) struct Cli {
	/// Render the UI with CEF's software paint path instead of the GPU
	/// sharing path. Persisted: the app also sets this itself when the
	/// accelerated path fails to present a frame.
	#[arg(long)]
	pub(crate) disable_ui_acceleration: bool,

	/// Started by Fotox itself after process `PID` crashed: wait for it to
	/// let go of the instance lock, and do not restart again on a crash
	/// right after starting (a loop).
	#[arg(long, hide = true, value_name = "PID")]
	pub(crate) after_crash: Option<u32>,

	/// Images to open at startup.
	pub(crate) files: Vec<PathBuf>,
}
