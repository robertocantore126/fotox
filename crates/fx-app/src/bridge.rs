//! The shell's end of the UI ↔ engine protocol (docs/PROTOCOL.md).
//!
//! Every frame from the UI is decoded here and sorted: `viewport_bounds` and
//! `direct_input` belong to the shell (§3), everything else to the engine.
//! Engine → UI frames arrive already encoded (`EngineOutput::ToUi`).

use fx_protocol::{FrameError, UiToEngine};

use crate::render::ViewportBounds;

/// A decoded UI message, sorted by who consumes it.
#[derive(Debug, PartialEq)]
pub(crate) enum Routed {
	/// Where the transparent viewport hole is (physical window pixels).
	ViewportBounds(ViewportBounds),
	/// Whether pointer input over the viewport goes to the engine (`true`) or
	/// to the UI because a popup, menu or dialog is open (`false`).
	DirectInput(bool),
	/// Everything else is the engine's.
	Engine(UiToEngine),
}

/// Decode one frame from the UI and decide who consumes it.
pub(crate) fn route(frame: &[u8]) -> Result<Routed, FrameError> {
	let (message, _payload) = fx_protocol::decode::<UiToEngine>(frame)?;
	Ok(match message {
		UiToEngine::ViewportBounds { x, y, width, height } => Routed::ViewportBounds(ViewportBounds::from_physical(x, y, width, height)),
		UiToEngine::DirectInput { enabled } => Routed::DirectInput(enabled),
		other => Routed::Engine(other),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_protocol::KIND_JSON;

	fn frame(json: &str) -> Vec<u8> {
		[&[KIND_JSON][..], json.as_bytes()].concat()
	}

	#[test]
	fn shell_messages_are_routed_to_the_shell() {
		assert_eq!(
			route(&frame(r#"{"type":"viewport_bounds","x":44.0,"y":84.5,"width":1078.0,"height":753.0}"#)).unwrap(),
			Routed::ViewportBounds(ViewportBounds {
				x: 44,
				y: 85,
				width: 1078,
				height: 753
			})
		);
		assert_eq!(route(&frame(r#"{"type":"direct_input","enabled":false}"#)).unwrap(), Routed::DirectInput(false));
	}

	#[test]
	fn everything_else_goes_to_the_engine() {
		let routed = route(&frame(r#"{"type":"action","id":"zoom:in"}"#)).unwrap();
		assert!(matches!(routed, Routed::Engine(UiToEngine::Action { ref id, .. }) if id == "zoom:in"));
	}

	#[test]
	fn a_malformed_frame_is_an_error_not_a_panic() {
		assert!(route(&[]).is_err());
		assert!(route(&frame("{not json")).is_err());
		assert!(route(&frame(r#"{"type":"no_such_message"}"#)).is_err());
	}
}
