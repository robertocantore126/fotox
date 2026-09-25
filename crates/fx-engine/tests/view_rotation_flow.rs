//! The Rotate View tool through a running engine (M6-T05): a drag turns the
//! view, Escape and Rotate View ▸ Reset View put it back to 0°, and the View
//! message the UI reads carries the angle.

mod common;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

fn action(harness: &Harness, id: &str) {
	harness.ui(UiToEngine::Action {
		id: id.into(),
		args: serde_json::Value::Null,
	});
}

fn pointer(kind: PointerKind, x: f64, y: f64, buttons: u8) -> PointerInput {
	PointerInput {
		kind,
		x,
		y,
		pressure: 1.0,
		tilt_x: 0.0,
		tilt_y: 0.0,
		buttons,
		modifiers: Modifiers::default(),
		time_us: 0,
	}
}

/// The next view angle the engine reports for `doc`, in degrees.
fn rotation(harness: &Harness, doc: DocId) -> f64 {
	harness.wait("a view message", |s| match s {
		Seen::Ui(EngineToUi::View { doc: d, rotation_deg, .. }) if *d == doc => Some(*rotation_deg),
		_ => None,
	})
}

#[test]
fn the_rotate_view_tool_turns_the_view_and_escape_resets_it() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-view-rotation-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);
	assert_eq!(rotation(&harness, doc), 0.0, "a fresh view is straight");

	action(&harness, "tool:rotate-view");
	// The harness opened an 800 × 600 viewport: press to the right of the
	// centre and drag below it for a quarter turn (the viewport centre is
	// (400, 300) and the tool turns about it).
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 500.0, 300.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 400.0, 400.0, 1)));
	let turned = rotation(&harness, doc);
	assert!((turned - 90.0).abs() < 1e-6, "a quarter turn clockwise: {turned}");
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 400.0, 400.0, 0)));

	// Escape with the tool active: Reset View.
	harness.ui(UiToEngine::Key { key: "Escape".into() });
	assert_eq!(rotation(&harness, doc), 0.0);

	// And the option bar's Reset View, after another turn.
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 500.0, 300.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 400.0, 400.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 400.0, 400.0, 0)));
	assert!((rotation(&harness, doc) - 90.0).abs() < 1e-6);
	action(&harness, "view:reset-rotation");
	assert_eq!(rotation(&harness, doc), 0.0);

	harness.engine.shutdown();
}
