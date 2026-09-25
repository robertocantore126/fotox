//! The main application window.
//!
//! Ported from `reference/graphite-desktop/src/window.rs`, keeping its
//! `cfg`-selected `native` module so another platform is a sibling file. Only
//! the Windows module exists (decision D-014: Windows is the only tested
//! platform).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use winit::cursor::{CustomCursor, CustomCursorSource};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window as WinitWindow, WindowAttributes};

use crate::consts::APP_NAME;
use crate::ui::Cursor;

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
use win as native;

#[cfg(not(target_os = "windows"))]
compile_error!("crates/fx-app is implemented for Windows only (decision D-014); port reference/graphite-desktop/src/window/{{linux,mac}}.rs to add a platform");

/// The window, plus the custom cursors the UI has asked for so far.
///
/// `winit::window::Window` is a trait, so the window is held as a trait object
/// — the same shape as the reference, and what `wgpu`'s surface target wants.
pub(crate) struct Window {
	winit_window: Arc<dyn WinitWindow>,
	custom_cursors: HashMap<CustomCursorSource, CustomCursor>,
}

impl Window {
	/// Process-wide initialisation that has to happen before the window exists.
	pub(crate) fn init() {
		native::init();
	}

	/// Create the window for the given event loop.
	pub(crate) fn new(event_loop: &dyn ActiveEventLoop) -> Result<Self> {
		let attributes = WindowAttributes::default()
			.with_title(APP_NAME)
			.with_min_surface_size(LogicalSize::new(1024, 700))
			.with_surface_size(LogicalSize::new(1600, 1000))
			.with_resizable(true)
			.with_visible(false)
			.with_theme(Some(winit::window::Theme::Dark))
			// Native title bar and resize border (decision D-022). The Fotox UI
			// has no window buttons, and in off-screen mode CEF ignores its
			// `app-region: drag`, so a frameless window could not be moved,
			// resized, minimised or maximised.
			.with_decorations(true);

		let winit_window = event_loop.create_window(attributes).context("failed to create the Fotox window")?;
		Ok(Self {
			winit_window: winit_window.into(),
			custom_cursors: HashMap::new(),
		})
	}

	/// Raw `HWND` of the window, for the Windows-only helpers in `win.rs`.
	fn hwnd(&self) -> Option<isize> {
		use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
		match self.winit_window.window_handle().ok()?.as_raw() {
			RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
			_ => None,
		}
	}

	/// The monitor the window is on (an id that changes when it moves to
	/// another monitor), and that monitor's ICC profile (M4-T02).
	pub(crate) fn monitor(&self) -> Option<(isize, Option<Vec<u8>>)> {
		let hwnd = self.hwnd()?;
		Some((native::monitor_id(hwnd), native::monitor_icc_profile(hwnd)))
	}

	/// The id of the monitor the window is on (cheap; no profile read).
	pub(crate) fn monitor_id(&self) -> Option<isize> {
		self.hwnd().map(native::monitor_id)
	}

	/// Make the window visible and give it focus.
	pub(crate) fn show(&self) {
		self.winit_window.set_visible(true);
		self.winit_window.focus_window();
	}

	/// Ask for a redraw.
	pub(crate) fn request_redraw(&self) {
		self.winit_window.request_redraw();
	}

	/// Create the wgpu surface presenting into this window.
	pub(crate) fn create_surface(&self, instance: &wgpu_sync::Instance) -> Result<wgpu_sync::Surface> {
		// No display handle: the instance is created without one (DX12 does not
		// need it), so the surface is bound to the window alone.
		let target = wgpu::SurfaceTarget::from_window_without_display(self.winit_window.clone());
		instance.create_surface(target).context("failed to create the window surface")
	}

	/// Tell the compositor a frame is about to be presented.
	pub(crate) fn pre_present_notify(&self) {
		self.winit_window.pre_present_notify();
	}

	/// Whether the window is currently drawable.
	pub(crate) fn can_render(&self) -> bool {
		native::can_render(self.winit_window.as_ref())
	}

	/// Size of the drawable area, in physical pixels.
	pub(crate) fn surface_size(&self) -> PhysicalSize<u32> {
		self.winit_window.surface_size()
	}

	/// Dots per logical pixel of the monitor the window is on.
	pub(crate) fn scale_factor(&self) -> f64 {
		self.winit_window.scale_factor()
	}

	/// Apply a cursor requested by the UI.
	///
	/// Custom cursors are cached: CEF re-sends the same bitmap for every frame
	/// of a hover, and creating one each time leaks.
	pub(crate) fn set_cursor(&mut self, event_loop: &dyn ActiveEventLoop, cursor: Cursor) {
		let cursor = match cursor {
			Cursor::Icon(icon) => icon.into(),
			Cursor::Custom {
				rgba,
				width,
				height,
				hotspot_x,
				hotspot_y,
			} => {
				let Ok(source) = CustomCursorSource::from_rgba(rgba, width, height, hotspot_x, hotspot_y) else {
					tracing::error!("ignoring an invalid custom cursor image");
					return;
				};
				let custom = match self.custom_cursors.get(&source).cloned() {
					Some(cached) => cached,
					None => {
						let Ok(created) = event_loop.create_custom_cursor(source.clone()) else {
							tracing::error!("failed to create a custom cursor");
							return;
						};
						self.custom_cursors.insert(source, created.clone());
						created
					}
				};
				custom.into()
			}
			Cursor::None => {
				self.winit_window.set_cursor_visible(false);
				return;
			}
		};
		self.winit_window.set_cursor_visible(true);
		self.winit_window.set_cursor(cursor);
	}
}
