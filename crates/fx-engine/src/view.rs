//! The engine thread's view of the (for now virtual) document: pan, zoom and
//! the viewport-input gestures that drive them (M0-T06).
//!
//! Pure state + math, no threads and no GPU, so every gesture is unit-tested.

use fx_render::{ViewTransform, ViewportSize};

use crate::{CursorShape, Modifiers, PointerInput, PointerKind};

/// Size of the virtual document M0 navigates.
pub const VIRTUAL_DOC: (u32, u32) = (30_000, 30_000);

/// Zoom factor per wheel notch (Ctrl/Alt + wheel).
pub const ZOOM_PER_NOTCH: f64 = 1.1;

/// Screen pixels panned per wheel notch.
pub const PAN_PER_NOTCH: f64 = 60.0;

/// Pointer button bits in [`PointerInput::buttons`].
pub const BUTTON_LEFT: u8 = 1;
pub const BUTTON_RIGHT: u8 = 2;
pub const BUTTON_MIDDLE: u8 = 4;

/// View state plus the gesture in progress.
#[derive(Clone, Debug)]
pub struct ViewState {
	pub doc: (u32, u32),
	pub view: ViewTransform,
	/// `None` until the shell reports the viewport size.
	pub viewport: Option<ViewportSize>,
	/// The active tool id from the UI (`"move"`, `"hand"`, …).
	pub tool: String,
	/// Last pointer position of a pan drag in progress.
	drag: Option<(f64, f64)>,
	/// Whether the view has been fitted once (the first viewport size fits).
	fitted: bool,
}

/// What a handled input changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
	/// Zoom or centre changed: re-render and tell the UI.
	pub view: bool,
	/// The cursor over the viewport should change to this.
	pub cursor: Option<CursorShape>,
}

impl ViewState {
	pub fn new(doc: (u32, u32)) -> Self {
		Self {
			doc,
			view: ViewTransform {
				zoom: 1.0,
				center_x: doc.0 as f64 / 2.0,
				center_y: doc.1 as f64 / 2.0,
			},
			viewport: None,
			tool: "move".into(),
			drag: None,
			fitted: false,
		}
	}

	/// The viewport changed size. The first size fits the document; later
	/// ones keep zoom and centre (Photoshop keeps the centre on resize).
	pub fn resize(&mut self, width: u32, height: u32) -> Changed {
		let viewport = ViewportSize { width, height };
		if self.viewport == Some(viewport) {
			return Changed::default();
		}
		self.viewport = Some(viewport);
		if !self.fitted && width > 0 && height > 0 {
			self.view = ViewTransform::fit(viewport, self.doc.0, self.doc.1);
			self.fitted = true;
		}
		Changed { view: true, cursor: None }
	}

	/// Handle a view action id; `None` if the id is not a view action.
	pub fn action(&mut self, id: &str) -> Option<Changed> {
		if let Some(tool) = id.strip_prefix("tool:") {
			self.tool = tool.to_owned();
			return Some(Changed::default());
		}
		let zoom = match id {
			"zoom:in" => self.view.step_zoom(1),
			"zoom:out" => self.view.step_zoom(-1),
			"zoom:100" => 1.0,
			"zoom:fit" => {
				let viewport = self.viewport?;
				self.view = ViewTransform::fit(viewport, self.doc.0, self.doc.1);
				return Some(Changed { view: true, cursor: None });
			}
			_ => return None,
		};
		Some(self.set_zoom(zoom))
	}

	/// Zoom to `zoom` (1.0 = 100 %), keeping the viewport centre fixed.
	pub fn set_zoom(&mut self, zoom: f64) -> Changed {
		if !zoom.is_finite() || zoom <= 0.0 {
			return Changed::default();
		}
		let zoom = zoom.clamp(fx_render::viewport::MIN_ZOOM, fx_render::viewport::MAX_ZOOM);
		if zoom == self.view.zoom {
			return Changed::default();
		}
		self.view.zoom = zoom;
		Changed { view: true, cursor: None }
	}

	/// Wheel over the viewport at `(x, y)`, deltas in notches (positive `dy`
	/// = wheel away from the user). Ctrl/Alt zoom around the cursor, Shift
	/// pans horizontally, plain wheel pans vertically.
	pub fn wheel(&mut self, x: f64, y: f64, dx: f64, dy: f64, modifiers: Modifiers) -> Changed {
		let Some(viewport) = self.viewport else {
			return Changed::default();
		};
		if modifiers.ctrl || modifiers.alt {
			let factor = ZOOM_PER_NOTCH.powf(dy + dx);
			let before = self.view;
			self.view.zoom_at(viewport, x, y, self.view.zoom * factor);
			return Changed {
				view: self.view != before,
				cursor: None,
			};
		}
		let (px, py) = if modifiers.shift { (dy + dx, 0.0) } else { (dx, dy) };
		if px == 0.0 && py == 0.0 {
			return Changed::default();
		}
		self.view.pan_screen(px * PAN_PER_NOTCH, py * PAN_PER_NOTCH);
		Changed { view: true, cursor: None }
	}

	/// Pointer input over the viewport. Panning starts with the middle button,
	/// Space + left button, or the left button with the hand tool, and lasts
	/// until every button is released.
	pub fn pointer(&mut self, input: &PointerInput) -> Changed {
		match input.kind {
			PointerKind::Down => {
				let pan = input.buttons & BUTTON_MIDDLE != 0 || (input.buttons & BUTTON_LEFT != 0 && self.hand_active(input.modifiers));
				if pan && self.drag.is_none() {
					self.drag = Some((input.x, input.y));
					return Changed {
						view: false,
						cursor: Some(CursorShape::Grabbing),
					};
				}
				Changed::default()
			}
			PointerKind::Move => {
				if let Some((lx, ly)) = self.drag {
					self.drag = Some((input.x, input.y));
					let (dx, dy) = (input.x - lx, input.y - ly);
					if dx != 0.0 || dy != 0.0 {
						self.view.pan_screen(dx, dy);
						return Changed { view: true, cursor: None };
					}
					return Changed::default();
				}
				Changed {
					view: false,
					cursor: Some(self.hover_cursor(input.modifiers)),
				}
			}
			PointerKind::Up => {
				if self.drag.is_some() && input.buttons == 0 {
					self.drag = None;
					return Changed {
						view: false,
						cursor: Some(self.hover_cursor(input.modifiers)),
					};
				}
				Changed::default()
			}
			PointerKind::Leave => Changed::default(),
		}
	}

	/// Whether a pan drag is in progress.
	pub fn dragging(&self) -> bool {
		self.drag.is_some()
	}

	fn hand_active(&self, modifiers: Modifiers) -> bool {
		modifiers.space || self.tool == "hand"
	}

	fn hover_cursor(&self, modifiers: Modifiers) -> CursorShape {
		if self.hand_active(modifiers) {
			CursorShape::Grab
		} else {
			CursorShape::Default
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn state() -> ViewState {
		let mut s = ViewState::new(VIRTUAL_DOC);
		s.resize(1600, 1000);
		s
	}

	fn pointer(kind: PointerKind, x: f64, y: f64, buttons: u8, modifiers: Modifiers) -> PointerInput {
		PointerInput {
			kind,
			x,
			y,
			pressure: 1.0,
			tilt_x: 0.0,
			tilt_y: 0.0,
			buttons,
			modifiers,
			time_us: 0,
		}
	}

	#[test]
	fn first_viewport_fits_the_document_later_ones_keep_the_view() {
		let mut s = ViewState::new(VIRTUAL_DOC);
		assert!(s.resize(1600, 1000).view);
		assert!((s.view.zoom - 1000.0 / 30_000.0).abs() < 1e-12);
		s.view.zoom = 0.5;
		s.resize(1200, 800);
		assert_eq!(s.view.zoom, 0.5, "resize keeps the zoom");
		assert!(!s.resize(1200, 800).view, "same size is not a change");
	}

	#[test]
	fn ctrl_wheel_keeps_the_point_under_the_cursor() {
		let mut s = state();
		let viewport = s.viewport.unwrap();
		let (x, y) = (300.0, 700.0);
		let before = s.view.screen_to_doc(viewport, x, y);
		let ctrl = Modifiers {
			ctrl: true,
			..Default::default()
		};
		assert!(s.wheel(x, y, 0.0, 3.0, ctrl).view);
		let after = s.view.screen_to_doc(viewport, x, y);
		assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6);
		assert!((s.view.zoom / (1000.0 / 30_000.0) - 1.1f64.powi(3)).abs() < 1e-9);
	}

	#[test]
	fn plain_wheel_pans_vertically_shift_wheel_horizontally() {
		let mut s = state();
		let start = s.view;
		s.wheel(0.0, 0.0, 0.0, 1.0, Modifiers::default());
		assert_eq!(s.view.center_x, start.center_x);
		assert!(s.view.center_y < start.center_y, "wheel up shows what is above");
		let mid = s.view;
		s.wheel(
			0.0,
			0.0,
			0.0,
			1.0,
			Modifiers {
				shift: true,
				..Default::default()
			},
		);
		assert_eq!(s.view.center_y, mid.center_y);
		assert!(s.view.center_x < mid.center_x);
	}

	#[test]
	fn space_drag_pans_and_ends_on_release() {
		let mut s = state();
		let space = Modifiers {
			space: true,
			..Default::default()
		};
		let down = s.pointer(&pointer(PointerKind::Down, 100.0, 100.0, BUTTON_LEFT, space));
		assert_eq!(down.cursor, Some(CursorShape::Grabbing));
		let before = s.view;
		assert!(s.pointer(&pointer(PointerKind::Move, 150.0, 80.0, BUTTON_LEFT, space)).view);
		let zoom = s.view.zoom;
		assert!((s.view.center_x - (before.center_x - 50.0 / zoom)).abs() < 1e-9);
		assert!((s.view.center_y - (before.center_y + 20.0 / zoom)).abs() < 1e-9);
		s.pointer(&pointer(PointerKind::Up, 150.0, 80.0, 0, space));
		assert!(!s.dragging());
	}

	#[test]
	fn left_drag_without_space_or_hand_does_not_pan() {
		let mut s = state();
		s.pointer(&pointer(PointerKind::Down, 100.0, 100.0, BUTTON_LEFT, Modifiers::default()));
		assert!(!s.dragging());
		s.action("tool:hand");
		s.pointer(&pointer(PointerKind::Down, 100.0, 100.0, BUTTON_LEFT, Modifiers::default()));
		assert!(s.dragging(), "the hand tool pans with the left button");
	}

	#[test]
	fn middle_button_pans_with_any_tool() {
		let mut s = state();
		s.pointer(&pointer(PointerKind::Down, 10.0, 10.0, BUTTON_MIDDLE, Modifiers::default()));
		assert!(s.dragging());
	}

	#[test]
	fn zoom_actions_step_and_keep_the_centre() {
		let mut s = state();
		let centre = (s.view.center_x, s.view.center_y);
		s.action("zoom:100").unwrap();
		assert_eq!(s.view.zoom, 1.0);
		s.action("zoom:in").unwrap();
		assert_eq!(s.view.zoom, 2.0);
		s.action("zoom:out").unwrap();
		assert_eq!(s.view.zoom, 1.0);
		assert_eq!((s.view.center_x, s.view.center_y), centre);
		s.action("zoom:fit").unwrap();
		assert!((s.view.zoom - 1000.0 / 30_000.0).abs() < 1e-12);
		assert!(s.action("doc:save").is_none());
	}

	#[test]
	fn set_zoom_clamps_and_rejects_nonsense() {
		let mut s = state();
		s.set_zoom(1000.0);
		assert_eq!(s.view.zoom, fx_render::viewport::MAX_ZOOM);
		let z = s.view.zoom;
		assert!(!s.set_zoom(f64::NAN).view);
		assert!(!s.set_zoom(-1.0).view);
		assert_eq!(s.view.zoom, z);
	}
}
