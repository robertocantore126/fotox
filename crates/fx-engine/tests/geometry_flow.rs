//! The image-geometry actions through a running engine (M6-T03): Image ▸
//! Crop around the selection, Image ▸ Trim, and the crop tool's box starting
//! over when the canvas changes.

mod common;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::EngineInput;
use fx_protocol::{DocId, EngineToUi, UiToEngine};

fn action(harness: &Harness, id: &str, args: serde_json::Value) {
	harness.ui(UiToEngine::Action { id: id.into(), args });
}

/// The next canvas size the engine reports for `doc`, or the toast or error
/// it answered with instead.
fn canvas(harness: &Harness, doc: DocId) -> Result<(u32, u32), String> {
	harness.wait("a document change", |s| match s {
		Seen::Ui(EngineToUi::DocumentChanged { info }) if info.doc == doc => Some(Ok((info.width, info.height))),
		Seen::Ui(EngineToUi::Error { text }) => Some(Err(format!("error: {text}"))),
		Seen::Ui(EngineToUi::Toast { text }) if text != "Engine connected" => Some(Err(format!("toast: {text}"))),
		_ => None,
	})
}

#[test]
fn crop_to_the_selection_and_trim() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-geometry-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);

	// Nothing selected: Image ▸ Crop has nothing to crop to.
	action(&harness, "img:crop", serde_json::Value::Null);
	assert_eq!(canvas(&harness, doc), Err("toast: Nothing is selected to crop to".into()));

	harness.ui(UiToEngine::Command {
		doc,
		command: fx_core::Command::Select {
			shape: fx_core::SelectionShape::Rect {
				x: 10.0,
				y: 20.0,
				w: 100.0,
				h: 80.0,
			},
			mode: fx_core::SelectMode::Replace,
			feather: 0.0,
			anti_alias: false,
		},
	});
	action(&harness, "img:crop", serde_json::Value::Null);
	assert_eq!(canvas(&harness, doc), Ok((100, 80)), "the canvas is the selection's box");
	harness.ui(UiToEngine::Undo { doc });
	assert_eq!(canvas(&harness, doc), Ok((700, 400)));

	// The photo is opaque everywhere: there is no transparent border to trim.
	action(
		&harness,
		"edit:trim",
		serde_json::json!({ "based_on": "Transparent Pixels", "away": ["Top", "Left", "Bottom", "Right"] }),
	);
	assert_eq!(canvas(&harness, doc), Err("toast: Nothing to trim: the layer fills the canvas".into()));

	harness.engine.shutdown();
}

/// The label of the step a History message reports, or the toast/error the
/// engine answered with instead.
fn last_step(harness: &Harness, doc: DocId) -> String {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => labels.get(current.wrapping_sub(1)).cloned(),
		Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
		Seen::Ui(EngineToUi::Toast { text }) if text != "Engine connected" => Some(format!("toast: {text}")),
		_ => None,
	})
}

#[test]
fn free_transform_commits_one_step_and_escape_none() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-transform-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 600, 400)]));
	let doc = opened(&harness);
	// The Background is locked in place, like Photoshop's: make it a layer.
	harness.ui(UiToEngine::Command {
		doc,
		command: fx_core::Command::SetLayerProps {
			layer: fx_core::LayerRef::Active,
			props: fx_core::command::LayerPropsPatch {
				locked_position: Some(false),
				..Default::default()
			},
		},
	});
	last_step(&harness, doc);

	// Ctrl+T, ten pixels right, Escape: nothing happened.
	action(&harness, "xf:free", serde_json::Value::Null);
	harness.ui(UiToEngine::Key {
		key: "Shift+ArrowRight".into(),
	});
	harness.ui(UiToEngine::Key { key: "Escape".into() });
	// Ctrl+T again, ten pixels right, Enter: one step.
	action(&harness, "xf:free", serde_json::Value::Null);
	harness.ui(UiToEngine::Key {
		key: "Shift+ArrowRight".into(),
	});
	harness.ui(UiToEngine::Key { key: "Enter".into() });
	assert_eq!(last_step(&harness, doc), "Free Transform");
	// Edit ▸ Transform ▸ Rotate 180° with no box up: at once.
	action(&harness, "xf:rot180", serde_json::Value::Null);
	assert_eq!(last_step(&harness, doc), "Free Transform");
	harness.ui(UiToEngine::Undo { doc });
	harness.ui(UiToEngine::Undo { doc });
	let (labels, current) = harness.wait("the history after two undos", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc && *current == 1 => Some((labels.clone(), *current)),
		_ => None,
	});
	assert_eq!(labels.len(), 3, "the lock change and two transforms, no step for the cancelled box: {labels:?}");
	assert_eq!(current, 1);

	harness.engine.shutdown();
}
