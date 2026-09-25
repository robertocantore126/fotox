//! The pixel selection through a running engine (M5-T03): a selection command
//! is a real history step that never dirties the document, the ants are
//! redrawn without recompositing, and undo walks back through them.

mod common;

use std::time::Duration;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::EngineInput;
use fx_protocol::{DocId, EngineToUi, UiToEngine};

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
