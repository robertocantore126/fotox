//! The pixel selection through a running engine (M5-T03): a selection command
//! is a real history step that never dirties the document, the ants are
//! redrawn without recompositing, and undo walks back through them.

mod common;

use std::time::Duration;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

/// One pointer event, in physical viewport pixels (the engine maps it to the
/// document through the view).
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

fn history(harness: &Harness, doc: DocId) -> (Vec<String>, usize) {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => Some((labels.clone(), *current)),
		_ => None,
	})
}

/// Everything the engine said once it has gone quiet, as debug text.
fn settled(harness: &Harness) -> String {
	std::thread::sleep(Duration::from_millis(200));
	let mut seen = harness.seen.lock().unwrap();
	let text = seen.iter().map(|s| format!("{s:?}\n")).collect();
	seen.clear();
	text
}

#[test]
fn a_selection_is_a_history_step_that_does_not_dirty_the_document() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-selection-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	// Every command lands in the History panel under its Photoshop name.
	for (step, (command, label)) in [
		(fx_core::Command::SelectAll, "Select All"),
		(fx_core::Command::Deselect, "Deselect"),
		(fx_core::Command::Reselect, "Reselect"),
		(fx_core::Command::InvertSelection, "Inverse"),
	]
	.into_iter()
	.enumerate()
	{
		harness.ui(UiToEngine::Command { doc, command });
		let (labels, current) = history(&harness, doc);
		assert_eq!(labels.last().map(String::as_str), Some(label), "{labels:?}");
		assert_eq!(current, step + 1, "{labels:?}");
		// The document is not saved for a selection (D-028), so the tab keeps
		// no asterisk: the engine must not report it as changed.
		let after = settled(&harness);
		assert!(!after.contains("DocumentChanged"), "{label} said: {after}");
	}

	// A real marquee through the engine's rasteriser.
	harness.ui(UiToEngine::Command {
		doc,
		command: fx_core::Command::Select {
			shape: fx_core::SelectionShape::Rect {
				x: 10.0,
				y: 10.0,
				w: 100.0,
				h: 100.0,
			},
			mode: fx_core::SelectMode::Replace,
			feather: 0.0,
			anti_alias: true,
		},
	});
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Rectangular Marquee"), 5), "{labels:?}");
	assert!(!settled(&harness).contains("DocumentChanged"));

	// Undo takes the marquee away (the ants are redrawn from the new state).
	harness.ui(UiToEngine::Undo { doc });
	let (labels, current) = history(&harness, doc);
	assert_eq!(current, 4, "{labels:?}");
	harness.ui(UiToEngine::Redo { doc });
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Rectangular Marquee"), 5), "{labels:?}");

	harness.engine.shutdown();
}

#[test]
fn a_marquee_drag_and_a_polygonal_lasso_reach_the_document() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-selection-tools-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	// The 700 × 400 document fits the 800 × 600 viewport at 100 %, centred,
	// so document (0, 0) is at viewport (50, 100).
	harness.ui(UiToEngine::Action {
		id: "tool:marquee".into(),
		args: serde_json::Value::Null,
	});
	// A hover takes the marquee's crosshair over from the view's default arrow.
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 100.0, 100.0, 0)));
	harness.wait("the crosshair", |s| match s {
		Seen::Cursor(fx_engine::CursorShape::Crosshair) => Some(()),
		_ => None,
	});
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 100.0, 100.0, 1)));
	for x in [120.0, 200.0, 300.0] {
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, x, 250.0, 1)));
	}
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 300.0, 250.0, 0)));
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Rectangular Marquee"), 1), "{labels:?}");
	assert!(!settled(&harness).contains("DocumentChanged"), "a selection is not a document change");

	// Escape in the middle of a drag cancels it: nothing is recorded.
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 100.0, 100.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 300.0, 250.0, 1)));
	harness.ui(UiToEngine::Key { key: "Escape".into() });
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 300.0, 250.0, 0)));
	let after = settled(&harness);
	assert!(!after.contains("History"), "a cancelled drag is not a step: {after}");

	// The polygonal lasso: three clicks and Enter make one selection.
	harness.ui(UiToEngine::Action {
		id: "tool:lasso-poly".into(),
		args: serde_json::Value::Null,
	});
	for (x, y) in [(100.0, 100.0), (400.0, 100.0), (400.0, 300.0)] {
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, x, y, 1)));
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, x, y, 0)));
	}
	harness.ui(UiToEngine::Key { key: "Enter".into() });
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Lasso"), 2), "{labels:?}");

	harness.engine.shutdown();
}

#[test]
fn the_magic_wand_runs_as_a_job_and_its_outline_can_be_dragged() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-selection-wand-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	// Tolerance 255 matches every pixel: the whole canvas.
	harness.ui(UiToEngine::Command {
		doc,
		command: fx_core::Command::MagicWand {
			params: fx_core::WandParams {
				x: 5.0,
				y: 5.0,
				tolerance: 255.0,
				contiguous: true,
				anti_alias: false,
				sample_all_layers: true,
			},
			mode: fx_core::SelectMode::Replace,
		},
	});
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Magic Wand"), 1), "{labels:?}");
	assert!(!settled(&harness).contains("DocumentChanged"), "a selection is not a document change");

	// With the wand active, a drag inside the selection moves the outline.
	harness.ui(UiToEngine::Action {
		id: "tool:magic-wand".into(),
		args: serde_json::Value::Null,
	});
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 300.0, 300.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 320.0, 290.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 320.0, 290.0, 0)));
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Move Selection"), 2), "{labels:?}");
	// And an arrow key nudges it.
	harness.ui(UiToEngine::Key { key: "Shift+ArrowLeft".into() });
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Move Selection"), 3), "{labels:?}");

	harness.engine.shutdown();
}
