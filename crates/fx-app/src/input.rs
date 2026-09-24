//! winit events â†’ the UI's [`InputEvent`]s or the engine's [`EngineInput`]s.
//!
//! A port of `reference/graphite-desktop/src/input.rs`'s routing
//! (docs/ARCHITECTURE.md Â§2.2):
//! * A pointer over the viewport rectangle goes to the **engine** while
//!   `direct_input` is on (no popup, menu or dialog open); anywhere else, or
//!   with a popup open, it goes to the **UI**.
//! * A drag keeps the route it started with until every button is released,
//!   so a pan started in the viewport keeps working over the panels.
//! * Keyboard input always goes to the UI (its shortcut map owns keys); Space
//!   is also tracked here as the temporary-hand modifier for the engine.
//! * Pointer moves over the engine still reach the UI, so hover states in the
//!   chrome clear when the pointer leaves a button for the viewport.

use std::time::Instant;

use fx_engine::view::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use winit::dpi::PhysicalPosition;
use winit::event::{ButtonSource, ElementState, MouseButton, MouseScrollDelta, PointerSource, TabletToolData, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};

use crate::render::ViewportBounds;
use crate::ui::{InputEvent, MULTICLICK_ALLOWED_TRAVEL, MULTICLICK_TIMEOUT};

/// Pixel wheel deltas (precision touchpads) per notch-equivalent.
const PIXELS_PER_NOTCH: f64 = 120.0;

/// Who a pointer event belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
	Ui,
	Engine,
}

/// The input state an event needs in order to be interpreted and routed.
pub(crate) struct InputState {
	start: Instant,
	modifiers: ModifiersState,
	space: bool,
	clicks: ClickTracker,
	viewport: Option<ViewportBounds>,
	direct_input: bool,
	position: PhysicalPosition<f64>,
	/// Route of the pointer; while `buttons` is non-zero, the drag's route.
	route: Route,
	/// Buttons held, as `fx_engine::view::BUTTON_*` bits.
	buttons: u8,
	/// Pressure and tilt of the last tablet event (mouse: 1.0 and 0).
	pressure: f32,
	tilt: (f32, f32),
}

impl InputState {
	/// No modifiers, no buttons, no viewport yet; direct input on (the UI only
	/// reports changes, and starts with nothing open).
	pub(crate) fn new() -> Self {
		Self {
			start: Instant::now(),
			modifiers: ModifiersState::default(),
			space: false,
			clicks: ClickTracker::default(),
			viewport: None,
			direct_input: true,
			position: PhysicalPosition::new(0.0, 0.0),
			route: Route::Ui,
			buttons: 0,
			pressure: 1.0,
			tilt: (0.0, 0.0),
		}
	}

	/// Where the viewport hole is (from the UI's `viewport_bounds`).
	pub(crate) fn set_viewport(&mut self, bounds: ViewportBounds) {
		self.viewport = Some(bounds);
	}

	/// `false` while a UI popup, menu or dialog is open.
	pub(crate) fn set_direct_input(&mut self, enabled: bool) {
		self.direct_input = enabled;
	}

	/// Whether the pointer currently belongs to the engine (its cursor wins).
	pub(crate) fn pointer_on_engine(&self) -> bool {
		self.route == Route::Engine
	}

	/// Convert one window event and hand the result to `ui` and/or `engine`.
	pub(crate) fn process(&mut self, event: &WindowEvent, mut ui: impl FnMut(InputEvent), mut engine: impl FnMut(EngineInput)) {
		match event {
			WindowEvent::PointerMoved { position, source, .. } => {
				self.position = *position;
				if let PointerSource::TabletTool { data, .. } = source {
					self.tablet(data);
				}
				if self.buttons == 0 {
					let next = self.route_at(*position);
					if self.route == Route::Engine && next == Route::Ui {
						engine(EngineInput::Pointer(self.pointer(PointerKind::Leave)));
					}
					self.route = next;
				}
				ui(InputEvent::pointer().position(*position).moved().modifiers(self.modifiers).build());
				if self.route == Route::Engine {
					engine(EngineInput::Pointer(self.pointer(PointerKind::Move)));
				}
			}
			WindowEvent::PointerEntered { position, .. } => {
				self.position = *position;
				ui(InputEvent::pointer().position(*position).entered().modifiers(self.modifiers).build());
			}
			WindowEvent::PointerLeft { position, .. } => {
				if let Some(position) = position {
					self.position = *position;
				}
				let input = match position {
					Some(position) => InputEvent::pointer().position(*position).exited(),
					None => InputEvent::pointer().exited(),
				};
				ui(input.modifiers(self.modifiers).build());
				if self.route == Route::Engine && self.buttons == 0 {
					engine(EngineInput::Pointer(self.pointer(PointerKind::Leave)));
					self.route = Route::Ui;
				}
			}
			WindowEvent::PointerButton { state, button, position, .. } => {
				self.position = *position;
				if let ButtonSource::TabletTool { data, .. } = button {
					self.tablet(data);
				} else {
					self.pressure = 1.0;
					self.tilt = (0.0, 0.0);
				}
				let mouse_button = button.clone().mouse_button();
				let bit = match mouse_button {
					Some(MouseButton::Left) => BUTTON_LEFT,
					Some(MouseButton::Right) => BUTTON_RIGHT,
					Some(MouseButton::Middle) => BUTTON_MIDDLE,
					_ => 0,
				};
				// A press with nothing held decides the route of the whole drag.
				if state.is_pressed() && self.buttons == 0 {
					self.route = self.route_at(*position);
				}
				match state {
					ElementState::Pressed => self.buttons |= bit,
					ElementState::Released => self.buttons &= !bit,
				}

				if self.route == Route::Engine && bit != 0 {
					let kind = if state.is_pressed() { PointerKind::Down } else { PointerKind::Up };
					engine(EngineInput::Pointer(self.pointer(kind)));
				} else {
					let count = mouse_button.map_or(1, |button| self.clicks.input(*position, button, *state));
					let pointer = InputEvent::pointer().position(*position);
					let input = match state {
						ElementState::Pressed => pointer.pressed(button.clone(), count),
						ElementState::Released => pointer.released(button.clone(), count),
					};
					ui(input.modifiers(self.modifiers).build());
				}
				if self.buttons == 0 {
					self.route = self.route_at(*position);
				}
			}
			WindowEvent::MouseWheel { delta, .. } => {
				if self.buttons == 0 && self.route_at(self.position) == Route::Engine {
					let (dx, dy) = match delta {
						MouseScrollDelta::LineDelta(x, y) => (f64::from(*x), f64::from(*y)),
						MouseScrollDelta::PixelDelta(p) => (p.x / PIXELS_PER_NOTCH, p.y / PIXELS_PER_NOTCH),
						_ => return,
					};
					let (x, y) = self.viewport_position();
					engine(EngineInput::Wheel {
						x,
						y,
						dx,
						dy,
						modifiers: self.engine_modifiers(),
					});
					return;
				}
				let input = match delta {
					MouseScrollDelta::LineDelta(x, y) => InputEvent::pointer().scrolled_lines(f64::from(*x), f64::from(*y)),
					MouseScrollDelta::PixelDelta(position) => InputEvent::pointer().scrolled_pixels(position.x, position.y),
					// `MouseScrollDelta` is non-exhaustive; a kind we do not know
					// is not reported rather than guessed at.
					_ => return,
				};
				ui(input.modifiers(self.modifiers).build());
			}
			WindowEvent::PinchGesture { delta, .. } => ui(InputEvent::pointer().zoomed(*delta).modifiers(self.modifiers).build()),
			WindowEvent::ModifiersChanged(modifiers) => {
				self.modifiers = modifiers.state();
				self.notify_hover(&mut engine);
			}
			WindowEvent::KeyboardInput { event, .. } => {
				if event.physical_key == PhysicalKey::Code(KeyCode::Space) && !event.repeat {
					self.space = event.state.is_pressed();
					self.notify_hover(&mut engine);
				}
				// Keyboard events go to CEF: the UI's shortcut map turns them into
				// actions (`docs/ARCHITECTURE.md` Â§2.2).
				ui(InputEvent::key(event).modifiers(self.modifiers).build());
			}
			WindowEvent::Focused(false) => {
				self.space = false;
				self.modifiers = ModifiersState::default();
			}
			_ => {}
		}
	}

	/// Tell the engine about a modifier change while it owns the pointer, so
	/// the hover cursor (hand while Space is held) follows at once.
	fn notify_hover(&self, engine: &mut impl FnMut(EngineInput)) {
		if self.route == Route::Engine {
			engine(EngineInput::Pointer(self.pointer(PointerKind::Move)));
		}
	}

	fn route_at(&self, position: PhysicalPosition<f64>) -> Route {
		let inside = self.viewport.is_some_and(|v| {
			position.x >= f64::from(v.x) && position.y >= f64::from(v.y) && position.x < f64::from(v.x + v.width) && position.y < f64::from(v.y + v.height)
		});
		if self.direct_input && inside { Route::Engine } else { Route::Ui }
	}

	fn viewport_position(&self) -> (f64, f64) {
		let (ox, oy) = self.viewport.map_or((0.0, 0.0), |v| (f64::from(v.x), f64::from(v.y)));
		(self.position.x - ox, self.position.y - oy)
	}

	fn engine_modifiers(&self) -> Modifiers {
		Modifiers {
			shift: self.modifiers.shift_key(),
			ctrl: self.modifiers.control_key(),
			alt: self.modifiers.alt_key(),
			space: self.space,
		}
	}

	fn pointer(&self, kind: PointerKind) -> PointerInput {
		let (x, y) = self.viewport_position();
		PointerInput {
			kind,
			x,
			y,
			pressure: self.pressure,
			tilt_x: self.tilt.0,
			tilt_y: self.tilt.1,
			buttons: self.buttons,
			modifiers: self.engine_modifiers(),
			time_us: u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX),
		}
	}

	fn tablet(&mut self, data: &TabletToolData) {
		self.pressure = data.force.map_or(1.0, |force| force.normalized(None).clamp(0.0, 1.0) as f32);
		self.tilt = data.clone().tilt().map_or((0.0, 0.0), |tilt| (f32::from(tilt.x), f32::from(tilt.y)));
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
