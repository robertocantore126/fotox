//! Using the selection through a running engine (M5-T05): Fill, Clear on the
//! Delete key, Layer via Copy, the clipboard and a mask from the selection,
//! each one History step with its Photoshop name.

mod common;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::EngineInput;
use fx_protocol::{DocId, EngineToUi, UiToEngine};

/// The label of the step a History message reports, or the error/toast the
/// engine answered with instead (so a failure says why).
fn last_step(harness: &Harness, doc: DocId) -> String {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => labels.get(current.wrapping_sub(1)).cloned(),
		Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
		Seen::Ui(EngineToUi::Toast { text }) if text != "Engine connected" => Some(format!("toast: {text}")),
		_ => None,
	})
}

fn action(harness: &Harness, id: &str, args: serde_json::Value) {
	harness.ui(UiToEngine::Action { id: id.into(), args });
}

#[test]
fn fill_clear_copy_paste_and_masks_are_history_steps() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-edit-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	harness.ui(UiToEngine::Command {
		doc,
		command: fx_core::Command::Select {
			shape: fx_core::SelectionShape::Rect {
				x: 10.0,
				y: 10.0,
				w: 100.0,
				h: 80.0,
			},
			mode: fx_core::SelectMode::Replace,
			feather: 0.0,
			anti_alias: true,
		},
	});
	assert_eq!(last_step(&harness, doc), "Rectangular Marquee");

	// Alt+Backspace: fill with the foreground colour.
	action(&harness, "edit:fill-fg", serde_json::Value::Null);
	assert_eq!(last_step(&harness, doc), "Fill");
	// The Fill dialog's values.
	action(
		&harness,
		"edit:fill",
		serde_json::json!({ "use": "50% Grey", "mode": "Multiply", "opacity": 50, "preserve": true }),
	);
	assert_eq!(last_step(&harness, doc), "Fill");

	// Copy, then Paste: a new layer.
	action(&harness, "clip:copy", serde_json::Value::Null);
	action(&harness, "clip:paste", serde_json::Value::Null);
	assert_eq!(last_step(&harness, doc), "Paste");
	action(&harness, "clip:paste-special", serde_json::Value::Null);
	assert_eq!(last_step(&harness, doc), "Paste in Place");

	// Delete with the selection and no tool busy: Edit ▸ Clear.
	harness.ui(UiToEngine::Key { key: "Delete".into() });
	assert_eq!(last_step(&harness, doc), "Clear");

	// Layer via Copy with the selection, then a mask from it.
	action(&harness, "layer:via-copy", serde_json::Value::Null);
	assert_eq!(last_step(&harness, doc), "Layer Via Copy");
	action(&harness, "mask:add", serde_json::json!({ "alt": false }));
	assert_eq!(last_step(&harness, doc), "Add Layer Mask");

	// Copy Merged runs on a helper thread; a paste after it gets its pixels.
	action(&harness, "clip:copy-merged", serde_json::Value::Null);
	harness.wait("the copy-merged job", |s| match s {
		Seen::Ui(EngineToUi::ProgressDone { .. }) => Some(()),
		_ => None,
	});
	action(&harness, "clip:paste", serde_json::Value::Null);
	assert_eq!(last_step(&harness, doc), "Paste");

	harness.engine.shutdown();
}
