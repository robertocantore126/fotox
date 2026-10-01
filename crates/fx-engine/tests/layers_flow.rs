//! The layer list the engine sends after each edit (2026-09-27, stress report
//! O2): a property edit sends only the rows it changed (`layers_patch`), an
//! edit that changes the tree a `layers_structure_patch`, and `request_layers`
//! gets the whole list again. The harness applies the patches to its own
//! copy, as the Layers panel does, and fails on one made against another list.

mod common;

use common::{Harness, Seen, gpu};
use fx_core::command::{LayerPropsPatch, NewLayer};
use fx_core::{Command, LayerRef};
use fx_protocol::{EngineToUi, LayerInfo, UiToEngine};

#[test]
fn property_edits_send_only_the_changed_rows() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-layers-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.ui(UiToEngine::Action {
		id: "doc:new".into(),
		args: serde_json::json!({ "width": 800, "height": 600, "ppi": 72, "depth": 8, "background": "white" }),
	});
	let doc = harness.wait("the document", |s| match s {
		Seen::Ui(EngineToUi::DocumentOpened { info }) => Some(info.doc),
		_ => None,
	});
	let layers = |what: &str, n: usize| -> Vec<LayerInfo> {
		harness.wait(what, |s| match s {
			Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == doc && layers.len() == n => Some(layers.clone()),
			Seen::Ui(EngineToUi::Error { text }) => panic!("{what}: {text}"),
			_ => None,
		})
	};
	let command = |command: Command| harness.ui(UiToEngine::Command { doc, command });
	let last_frame = || *harness.layer_frames.lock().unwrap().last().expect("a list frame");

	for _ in 0..200 {
		command(Command::AddLayer {
			layer: NewLayer::Pixel,
			name: None,
		});
	}
	let list = layers("201 layers", 201);
	// VERIFY-FIX(4.2): a structural edit is now a structure patch of a few rows, not the whole list.
	let frame = last_frame();
	assert!(frame.patch && frame.rows <= 3, "adding a layer is a small structure patch: {frame:?}");
	harness.ui(UiToEngine::RequestLayers { doc });
	layers("the full list", 201);
	assert!(!last_frame().patch, "request_layers sends the whole list");
	let full_bytes = last_frame().bytes;
	assert!(frame.bytes * 20 < full_bytes, "{} bytes against {full_bytes} for the full list", frame.bytes);

	// Hide one layer: one row, a fraction of the full list.
	let target = list[100].id;
	command(Command::SetLayerProps {
		layer: LayerRef::Id(target),
		props: LayerPropsPatch {
			visible: Some(false),
			..Default::default()
		},
	});
	let list = harness.wait("the hidden layer", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.iter().any(|l| l.id == target && !l.visible) => Some(layers.clone()),
		_ => None,
	});
	assert_eq!(list.len(), 201);
	let frame = last_frame();
	assert!(frame.patch && frame.rows == 1, "a visibility change is a one-row patch: {frame:?}");
	assert!(frame.bytes * 20 < full_bytes, "{} bytes against {full_bytes} for the full list", frame.bytes);

	// Rename it: again one row.
	command(Command::SetLayerProps {
		layer: LayerRef::Id(target),
		props: LayerPropsPatch {
			name: Some("renamed".into()),
			..Default::default()
		},
	});
	harness.wait("the renamed layer", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.iter().any(|l| l.id == target && l.name == "renamed" && !l.visible) => Some(()),
		_ => None,
	});
	assert!(last_frame().patch);

	// Undo the rename: the tree is the same, so a patch too.
	harness.ui(UiToEngine::Undo { doc });
	harness.wait("the name back", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.iter().any(|l| l.id == target && l.name != "renamed") => Some(()),
		_ => None,
	});
	assert!(last_frame().patch);

	// Delete it: the tree changed.
	command(Command::DeleteLayers {
		layers: vec![LayerRef::Id(target)],
	});
	layers("200 layers", 200);
	// VERIFY-FIX(4.2): deleting is a one-op structure patch too.
	let frame = last_frame();
	assert!(frame.patch && frame.rows <= 3, "deleting is a small structure patch: {frame:?}");

	// The UI lost track: the whole list again.
	harness.ui(UiToEngine::RequestLayers { doc });
	layers("the list again", 200);
	assert!(!last_frame().patch);
	harness.engine.shutdown();
}
