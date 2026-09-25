//! Using the selection through a running engine (M5-T05): Fill, Clear on the
//! Delete key, Layer via Copy, the clipboard and a mask from the selection,
//! each one History step with its Photoshop name.

mod common;

use std::time::{Duration, Instant};

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

/// Wait until `pred` holds over everything the engine has said so far (the
/// queue is not drained), or panic after `ms`.
fn wait_until(harness: &Harness, what: &str, ms: u64, pred: impl Fn(&[Seen]) -> bool) {
	let deadline = Instant::now() + Duration::from_millis(ms);
	loop {
		if pred(harness.seen.lock().unwrap().as_slice()) {
			return;
		}
		assert!(
			Instant::now() < deadline,
			"timed out waiting for {what}; seen {:?}",
			harness
				.seen
				.lock()
				.unwrap()
				.iter()
				.filter_map(|s| match s {
					Seen::Ui(EngineToUi::Toast { text }) => Some(format!("toast: {text}")),
					Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
					Seen::Ui(EngineToUi::Progress { task, label, fraction }) => Some(format!("progress #{task} {label} {fraction}")),
					Seen::Ui(EngineToUi::ProgressDone { task }) => Some(format!("progress done #{task}")),
					_ => None,
				})
				.collect::<Vec<_>>()
		);
		std::thread::sleep(Duration::from_millis(10));
	}
}

/// Wait up to `ms` for something `pick` accepts, without failing when it does
/// not come. For "nothing else happened" assertions (the harness `wait`
/// panics on timeout instead).
fn wait_briefly<T>(harness: &Harness, ms: u64, pick: impl Fn(&Seen) -> Option<T>) -> Option<T> {
	let deadline = Instant::now() + Duration::from_millis(ms);
	loop {
		{
			let mut seen = harness.seen.lock().unwrap();
			if let Some(i) = seen.iter().position(|s| pick(s).is_some()) {
				let found = pick(&seen[i]);
				seen.drain(..=i);
				return found;
			}
		}
		if Instant::now() >= deadline {
			return None;
		}
		std::thread::sleep(Duration::from_millis(10));
	}
}

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
fn a_second_save_while_one_runs_is_refused_or_queued_behind_it() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-save-guard-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	// Give the document a file first (Save As), so Save below is incremental
	// rather than another "where do I write?" dialog.
	let fxd = dir.join("out.fxd");
	let saved = |s: &Seen| matches!(s, Seen::Ui(EngineToUi::Toast { text }) if text.starts_with("Saved "));
	let refused = |s: &Seen| matches!(s, Seen::Ui(EngineToUi::Toast { text }) if text == "Wait until the save is finished");
	harness.engine.send(EngineInput::SaveAs { doc, path: fxd.clone() });
	harness.wait("the Save As", |s| saved(s).then_some(()));

	// Two Saves right behind each other: the second must not run a second
	// writer over the file while the first is still appending (S1-02).
	action(&harness, "doc:save", serde_json::Value::Null);
	action(&harness, "doc:save", serde_json::Value::Null);
	// Either the second was refused, or it ran after the first finished.
	wait_until(&harness, "the saves to settle", 30_000, |seen| {
		seen.iter().any(refused) || seen.iter().filter(|s| saved(s)).count() >= 2
	});
	// No save may still be writing when the file is read.
	wait_until(&harness, "a finished save", 30_000, |seen| seen.iter().any(saved));

	// Either way the file must be a readable .fxd.
	let store = fx_tiles::TileStore::new(fx_tiles::TileStoreConfig::for_tests(dir.join("reopen"))).unwrap();
	let reopened = fx_io::fxd::open(&fxd, &store);
	assert!(reopened.is_ok(), "reopening {fxd:?} failed: {:?}", reopened.err());

	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switching_tool_mid_stroke_records_one_step_and_a_hover_records_none() {
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
	harness.ui(UiToEngine::ToolOptions {
		tool: "brush".into(),
		options: serde_json::json!({ "Size": 30, "Smoothing": 0 }),
	});
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 100.0, 150.0, 1)));
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 160.0, 170.0, 1)));
	// The tool changes mid-stroke: the stroke is ended, the release goes to
	// the marquee (which never saw a press, so it stays quiet).
	action(&harness, "tool:marquee", serde_json::Value::Null);
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 160.0, 170.0, 0)));

	let (labels, current) = harness.wait("the stroke step", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => Some((labels.clone(), *current)),
		_ => None,
	});
	assert_eq!(labels, vec!["Brush Tool".to_string()], "the gesture is one step");
	assert_eq!(current, 1);

	// Back to the brush; a plain hover (no button) must not paint or record.
	action(&harness, "tool:brush", serde_json::Value::Null);
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 300.0, 300.0, 0)));
	let more = wait_briefly(&harness, 300, |s| match s {
		Seen::Ui(EngineToUi::History { .. }) => Some(()),
		_ => None,
	});
	assert!(more.is_none(), "a hover after the cancelled stroke records nothing");

	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zoom_fill_shows_more_of_the_document_than_fit() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-zoom-fill-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	action(&harness, "zoom:fit", serde_json::Value::Null);
	let fit = harness.wait("the fitted view", |s| match s {
		Seen::Ui(EngineToUi::View { doc: d, zoom, .. }) if *d == doc => Some(*zoom),
		_ => None,
	});
	action(&harness, "zoom:fill", serde_json::Value::Null);
	let fill = harness.wait("the filled view", |s| match s {
		Seen::Ui(EngineToUi::View { doc: d, zoom, .. }) if *d == doc => Some(*zoom),
		_ => None,
	});
	assert!(fill > fit, "Fill Screen ({fill}) covers the viewport more than Fit ({fit})");

	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}
