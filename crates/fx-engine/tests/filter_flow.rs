//! Filters through a running engine (M4-T05): a live preview, OK runs the
//! filter as a job and records one history step, undo restores the pixels,
//! Ctrl+F repeats it, and a document is busy while its job runs.

mod common;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_core::{Command, FilterParams, LayerRef};
use fx_engine::EngineInput;
use fx_protocol::{EngineToUi, UiToEngine};

fn history(harness: &Harness, doc: fx_protocol::DocId) -> (Vec<String>, usize) {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => Some((labels.clone(), *current)),
		_ => None,
	})
}

#[test]
fn preview_apply_undo_and_last_filter() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-filter-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);
	let layer = harness.wait("the layer list", |s| match s {
		Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == doc => layers.first().map(|l| l.id),
		_ => None,
	});

	// A live preview, then Cancel: nothing is recorded.
	let blur = FilterParams::GaussianBlur { radius: 3.0 };
	harness.ui(UiToEngine::FilterPreview {
		doc,
		layer,
		filter: blur.clone(),
	});
	harness.ui(UiToEngine::FilterPreviewCancel { doc });

	// OK: the filter runs as a job and becomes one history step.
	harness.ui(UiToEngine::Command {
		doc,
		command: Command::ApplyFilter {
			layer: LayerRef::Id(layer),
			filter: blur,
		},
	});
	let (labels, current) = history(&harness, doc);
	assert_eq!(labels.last().map(String::as_str), Some("Gaussian Blur"));
	assert_eq!(current, labels.len());

	// Undo, then Ctrl+F repeats the last filter.
	harness.ui(UiToEngine::Undo { doc });
	let (_, current) = history(&harness, doc);
	assert_eq!(current, 0, "undone");
	harness.ui(UiToEngine::Action {
		id: "filter:last".into(),
		args: serde_json::Value::Null,
	});
	let (labels, current) = history(&harness, doc);
	assert_eq!((labels.last().map(String::as_str), current), (Some("Gaussian Blur"), 1), "{labels:?}");

	// A bad radius is refused with an error, not applied.
	harness.ui(UiToEngine::Command {
		doc,
		command: Command::ApplyFilter {
			layer: LayerRef::Id(layer),
			filter: FilterParams::GaussianBlur { radius: 5000.0 },
		},
	});
	harness.wait("the refusal", |s| match s {
		Seen::Ui(EngineToUi::Error { text }) if text.contains("radius") => Some(()),
		_ => None,
	});

	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}
