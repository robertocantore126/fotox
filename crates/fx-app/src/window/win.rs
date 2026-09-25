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

/// The monitor the window is mostly on, as an opaque id (changes when the
/// window is moved to another monitor).
pub(crate) fn monitor_id(hwnd: isize) -> isize {
	use windows::Win32::Foundation::HWND;
	use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
	// SAFETY: `MonitorFromWindow` only reads the window handle, which is the
	// live main window; with DEFAULTTONEAREST it always returns a monitor.
	unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST).0 as isize }
}

/// The ICC profile bytes of the monitor the window is on (M4-T02, D-031):
/// the monitor's device name → a device context for it → its colour profile
/// path (`GetICMProfileW`) → the file. `None` when Windows reports no profile
/// or the file cannot be read; the engine then assumes sRGB.
pub(crate) fn monitor_icc_profile(hwnd: isize) -> Option<Vec<u8>> {
	use windows::Win32::Foundation::HWND;
	use windows::Win32::Graphics::Gdi::{CreateDCW, DeleteDC, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow};
	use windows::Win32::UI::ColorSystem::GetICMProfileW;
	use windows::core::{PCWSTR, PWSTR};

	let mut info = MONITORINFOEXW::default();
	info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
	// SAFETY: `info` is a properly sized MONITORINFOEXW (cbSize set), which
	// GetMonitorInfoW fills; the monitor handle comes from MonitorFromWindow.
	let ok = unsafe {
		let monitor = MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST);
		GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>()).as_bool()
	};
	if !ok {
		return None;
	}
	let device = PCWSTR(info.szDevice.as_ptr());
	let mut path = [0u16; 1024];
	let mut len = path.len() as u32;
	// SAFETY: `device` points into `info.szDevice`, a NUL-terminated device
	// name that outlives the call; the DC is deleted before returning;
	// `path`/`len` describe a writable buffer of `len` UTF-16 units.
	let found = unsafe {
		let dc = CreateDCW(device, device, PCWSTR::null(), None);
		if dc.is_invalid() {
			return None;
		}
		let found = GetICMProfileW(dc, &mut len, Some(PWSTR(path.as_mut_ptr()))).as_bool();
		let _ = DeleteDC(dc);
		found
	};
	if !found {
		return None;
	}
	let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
	let path = String::from_utf16_lossy(&path[..end]);
	match std::fs::read(&path) {
		Ok(bytes) => {
			tracing::info!("display profile: {path}");
			Some(bytes)
		}
		Err(error) => {
			tracing::warn!("cannot read the display profile {path}: {error}");
			None
		}
	}
}
