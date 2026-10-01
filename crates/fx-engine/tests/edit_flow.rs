//! Using the selection through a running engine (M5-T05): Fill, Clear on the
//! Delete key, Layer via Copy, the clipboard and a mask from the selection,
//! each one History step with its Photoshop name.

mod common;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
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

#[test]
fn painting_with_the_brush_eraser_and_on_a_mask_records_strokes() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-paint-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	for (tool, label) in [("brush", "Brush Tool"), ("eraser", "Eraser"), ("pencil", "Pencil")] {
		action(&harness, &format!("tool:{tool}"), serde_json::Value::Null);
		harness.ui(UiToEngine::ToolOptions {
			tool: tool.into(),
			options: serde_json::json!({ "Size": 30, "Opacity": 80, "Smoothing": 0 }),
		});
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 100.0, 150.0, 1)));
		for x in [120.0, 160.0, 220.0, 300.0] {
			harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, x, 170.0, 1)));
		}
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 300.0, 170.0, 0)));
		assert_eq!(last_step(&harness, doc), label);
	}

	// A mask, then paint on it: the stroke goes to the mask.
	action(&harness, "mask:add", serde_json::json!({ "alt": false }));
	// The layer list comes before the History message.
	let layer = harness.wait("the layer list", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) => layers.iter().find(|l| l.has_mask).map(|l| l.id),
		_ => None,
	});
	assert_eq!(last_step(&harness, doc), "Add Layer Mask");
	action(&harness, "layer:edit-mask", serde_json::json!({ "layer": layer.0, "mask": true }));
	let marked = harness.wait("the mask marked as the edit target", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) => layers.iter().find(|l| l.id == layer).map(|l| l.edit_mask),
		_ => None,
	});
	assert!(marked);
	action(&harness, "tool:brush", serde_json::Value::Null);
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 200.0, 200.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 260.0, 220.0, 0)));
	assert_eq!(last_step(&harness, doc), "Brush Tool");

	// Undo walks the strokes back.
	harness.ui(UiToEngine::Undo { doc });
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, .. }) if *d == doc => Some(()),
		_ => None,
	});
	harness.engine.shutdown();
}

#[test]
fn switching_tools_mid_stroke_ends_the_gesture_and_hover_does_not_paint() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-tool-switch-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);
	action(&harness, "tool:brush", serde_json::Value::Null);
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 200.0, 200.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 220.0, 210.0, 1)));
	action(&harness, "tool:eyedropper", serde_json::Value::Null);
	let first = harness.wait("the interrupted brush stroke", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc && *current == 1 => Some(labels.clone()),
		_ => None,
	});
	assert_eq!(first.last().map(String::as_str), Some("Brush Tool"));

	action(&harness, "tool:brush", serde_json::Value::Null);
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 280.0, 230.0, 0)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 280.0, 230.0, 0)));
	harness.ui(UiToEngine::Undo { doc });
	let (_, current) = harness.wait("one undo to remove the one real gesture", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc && !labels.is_empty() => Some((labels.clone(), *current)),
		_ => None,
	});
	assert_eq!(current, 0, "hover after switching back did not create a second stroke");
	harness.engine.shutdown();
}

// VERIFY-FIX(P2): Fill now runs as a background job. A command sent while it
// runs (here Deselect, as after Ctrl+D) waits in a queue and runs after it;
// it was refused with "Wait until Fill is finished" and lost.
#[test]
fn commands_sent_during_a_job_run_after_it_in_order() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-edit-queue-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "big.tif", 3000, 3000)]));
	let doc = opened(&harness);
	let command = |command| harness.ui(UiToEngine::Command { doc, command });
	command(fx_core::Command::Select {
		shape: fx_core::SelectionShape::Rect { x: 100.0, y: 100.0, w: 2500.0, h: 2500.0 },
		mode: fx_core::SelectMode::Replace,
		feather: 0.0,
		anti_alias: true,
	});
	command(fx_core::Command::Fill {
		layer: fx_core::LayerRef::Active,
		color: [30000, 20000, 10000, 65535],
		mode: fx_core::BlendMode::Multiply,
		opacity: 0.5,
		preserve_transparency: false,
	});
	command(fx_core::Command::Deselect);
	let steps: Vec<String> = (0..3).map(|_| last_step(&harness, doc)).collect();
	assert_eq!(steps, ["Rectangular Marquee", "Fill", "Deselect"]);
	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}
