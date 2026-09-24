//! winit events → the UI's [`InputEvent`]s.
//!
//! A partial port of `reference/graphite-desktop/src/input.rs`. The reference
//! splits every event between the UI and the editor, using the viewport
//! rectangle and a `direct_input` flag the UI sets while a popup is open.
//! M0-T03 has no editor and no viewport rectangle yet, so `direct_input` would
//! be false for every event and every event would take the UI route anyway.
//! What remains is therefore the conversion table, modifier tracking and
//! multi-click counting; M0-T06 puts the routing back on top of it.

use std::time::Instant;

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::ModifiersState;

use crate::ui::{InputEvent, MULTICLICK_ALLOWED_TRAVEL, MULTICLICK_TIMEOUT};

/// The input state an event needs in order to be interpreted.
pub(crate) struct InputState {
	modifiers: ModifiersState,
	clicks: ClickTracker,
}

impl InputState {
	/// A state with no modifiers held and no clicks counted yet.
	pub(crate) fn new() -> Self {
		Self {
			modifiers: ModifiersState::default(),
			clicks: ClickTracker::default(),
		}
	}

	/// Convert one window event and hand the result to `ui`.
	pub(crate) fn process(&mut self, event: &WindowEvent, mut ui: impl FnMut(InputEvent)) {
		match event {
			WindowEvent::PointerMoved { position, .. } => ui(InputEvent::pointer().position(*position).moved().modifiers(self.modifiers).build()),
			WindowEvent::PointerEntered { position, .. } => ui(InputEvent::pointer().position(*position).entered().modifiers(self.modifiers).build()),
			WindowEvent::PointerLeft { position: Some(position), .. } => {
				ui(InputEvent::pointer().position(*position).exited().modifiers(self.modifiers).build())
			}
			WindowEvent::PointerLeft { position: None, .. } => ui(InputEvent::pointer().exited().modifiers(self.modifiers).build()),
			WindowEvent::PointerButton { state, button, position, .. } => {
				let count = button.clone().mouse_button().map_or(1, |button| self.clicks.input(*position, button, *state));
				let pointer = InputEvent::pointer().position(*position);
				let input = match state {
					ElementState::Pressed => pointer.pressed(button.clone(), count),
					ElementState::Released => pointer.released(button.clone(), count),
				};
				ui(input.modifiers(self.modifiers).build());
			}
			WindowEvent::MouseWheel { delta, .. } => {
				let input = match delta {
					MouseScrollDelta::LineDelta(x, y) => Some(InputEvent::pointer().scrolled_lines(f64::from(*x), f64::from(*y))),
					MouseScrollDelta::PixelDelta(position) => Some(InputEvent::pointer().scrolled_pixels(position.x, position.y)),
					// `MouseScrollDelta` is non-exhaustive; a kind we do not know
					// is not reported rather than guessed at.
					_ => None,
				};
				if let Some(input) = input {
					ui(input.modifiers(self.modifiers).build());
				}
			}
			WindowEvent::PinchGesture { delta, .. } => ui(InputEvent::pointer().zoomed(*delta).modifiers(self.modifiers).build()),
			WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
			// Keyboard events go to CEF: the UI's shortcut map turns them into
			// actions (`docs/ARCHITECTURE.md` §2.2).
			WindowEvent::KeyboardInput { event, .. } => ui(InputEvent::key(event).modifiers(self.modifiers).build()),
			_ => {}
		}
	}
}

/// Counts how many clicks in a row landed close enough together to be a
/// double- or triple-click, per button.
#[derive(Default)]
struct ClickTracker {
	left: ClickChains,
	right: ClickChains,
	middle: ClickChains,
	back: ClickChains,
	forward: ClickChains,
}

impl ClickTracker {
	/// Record one press or release and return the resulting click count.
	fn input(&mut self, position: PhysicalPosition<f64>, button: MouseButton, state: ElementState) -> u32 {
		let position = (position.x as i32, position.y as i32);
		let clicks = match button {
			MouseButton::Left => &mut self.left,
			MouseButton::Right => &mut self.right,
			MouseButton::Middle => &mut self.middle,
			MouseButton::Back => &mut self.back,
			MouseButton::Forward => &mut self.forward,
			_ => return 1,
		};
		let chain = match state {
			ElementState::Pressed => &mut clicks.down,
			ElementState::Released => &mut clicks.up,
		};

		let now = Instant::now();
		let count = match chain {
			Some(previous) => {
				let close_in_time = now.saturating_duration_since(previous.time) <= MULTICLICK_TIMEOUT;
				let dx = position.0.abs_diff(previous.position.0) as usize;
				let dy = position.1.abs_diff(previous.position.1) as usize;
				let close_enough = dx <= MULTICLICK_ALLOWED_TRAVEL && dy <= MULTICLICK_ALLOWED_TRAVEL;
				if close_in_time && close_enough { previous.count.saturating_add(1) } else { 1 }
			}
			None => 1,
		};
		*chain = Some(Click { time: now, position, count });
		count
	}
}

/// The most recent press and the most recent release of one button.
#[derive(Default)]
struct ClickChains {
	down: Option<Click>,
	up: Option<Click>,
}

/// One recorded press or release.
struct Click {
	time: Instant,
	position: (i32, i32),
	count: u32,
}
