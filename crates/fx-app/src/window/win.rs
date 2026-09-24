//! Windows specifics of the main window.
//!
//! Ported from `reference/graphite-desktop/src/window/win.rs`. The reference
//! also draws its own window frame here (via `native_handle.rs`, an invisible
//! helper window that hit-tests the resize borders); Fotox uses the ordinary
//! decorated frame instead, see `docs/reports/M0-T03.md`.

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::core::HSTRING;

use crate::consts::APP_ID;

/// One-time process initialisation that must happen on the main thread before
/// the window exists.
///
/// Attaching to the parent console is what makes `tracing` output show up when
/// the app is started from a terminal; it is a no-op when it was not.
pub(crate) fn init() {
	// SAFETY: `AttachConsole` only associates this process with an existing
	// console. It takes no pointers we own, and a failure (no parent console)
	// is expected and ignored.
	unsafe {
		let _ = AttachConsole(ATTACH_PARENT_PROCESS);
	}

	let app_id = HSTRING::from(APP_ID);
	// SAFETY: `CoInitializeEx` initialises COM for this thread and is documented
	// as safe to call once per thread; `S_FALSE` (already initialised) is
	// returned as a success. `SetCurrentProcessExplicitAppUserModelID` reads
	// only the string we own for the duration of the call.
	unsafe {
		let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
		SetCurrentProcessExplicitAppUserModelID(&app_id).ok();
	}
}

/// Whether the window can currently be drawn to.
///
/// A hidden or minimised window has no surface worth presenting to. winit
/// reports both as `Option` because not every backend can answer: an unknown
/// answer is treated as drawable, since a missed redraw is worse than a
/// wasted one.
pub(crate) fn can_render(window: &dyn winit::window::Window) -> bool {
	window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false)
}
