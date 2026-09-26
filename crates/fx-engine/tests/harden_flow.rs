//! Regression tests for bugs found in HARDEN, through a running engine
//! (skipped without a GPU adapter).

mod common;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

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

/// Press the brush and drag, without releasing: a stroke in flight.
fn stroke_in_flight(harness: &Harness) {
	harness.ui(UiToEngine::Action {
		id: "tool:brush".into(),
		args: serde_json::Value::Null,
	});
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 200.0, 200.0, 1)));
	for x in [220.0, 240.0, 260.0] {
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, x, 220.0, 1)));
	}
}

fn history_label(harness: &Harness, doc: DocId) -> String {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => labels.get(current.wrapping_sub(1)).cloned(),
		_ => None,
	})
}

/// BUG-1: closing while a stroke is in flight used to close without asking
/// (the stroke was in the pixels but not in `dirty`) and lose it.
#[test]
fn closing_during_a_stroke_commits_it_and_asks_first() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-harden-close-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	stroke_in_flight(&harness);
	harness.ui(UiToEngine::CloseDocument { doc });
	assert_eq!(history_label(&harness, doc), "Brush Tool", "the stroke became a step");
	let answer = harness.wait("the prompt or the close", |s| match s {
		Seen::Ui(EngineToUi::CloseDirtyDocument { doc: d, .. }) if *d == doc => Some("asked"),
		Seen::Ui(EngineToUi::DocumentClosed { doc: d }) if *d == doc => Some("closed"),
		_ => None,
	});
	assert_eq!(answer, "asked");
	harness.engine.shutdown();
}

/// BUG-1: Save As while a stroke is in flight saved half a stroke with no
/// history step; now the stroke is committed first and the save cleans it.
#[test]
fn saving_during_a_stroke_commits_it_first() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-harden-save-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	stroke_in_flight(&harness);
	let path = dir.join("stroke.fxd");
	harness.engine.send(EngineInput::SaveAs { doc, path: path.clone() });
	assert_eq!(history_label(&harness, doc), "Brush Tool");
	harness.wait("the saved document", |s| match s {
		Seen::Ui(EngineToUi::DocumentChanged { info }) if info.doc == doc && !info.dirty => Some(()),
		_ => None,
	});
	assert!(path.exists());
	// The release after the save paints nothing more and adds no step.
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 260.0, 220.0, 0)));
	harness.engine.shutdown();
}
