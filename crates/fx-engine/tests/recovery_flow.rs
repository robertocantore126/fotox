//! Crash recovery across engine sessions (audit D2, verified 2026-10-01):
//! a session that ends with an unsaved document leaves a recovery file; the
//! next session offers it, reopens it with the same content, and once that
//! session holds its own copy the old file is not offered again. "Discard"
//! does the same without reopening, and a clean exit leaves no folder.
//! `LOCALAPPDATA` points at a temporary folder for the whole test.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Harness, Seen, gpu};
use fx_core::Command;
use fx_core::command::NewLayer;
use fx_engine::EngineInput;
use fx_protocol::{CloseAnswer, DocId, EngineToUi, UiToEngine};

fn session(dir: &Path, name: &str) -> Option<Harness> {
	let (device, queue) = gpu()?;
	let work = dir.join(name);
	std::fs::create_dir_all(&work).unwrap();
	let harness = Harness::start(device, queue, &work);
	harness.ui(UiToEngine::Hello { ui_version: "test".into() });
	Some(harness)
}

fn sessions(root: &Path) -> Vec<PathBuf> {
	std::fs::read_dir(root)
		.map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect())
		.unwrap_or_default()
}

fn recovery_files(root: &Path) -> Vec<PathBuf> {
	sessions(root)
		.into_iter()
		.flat_map(|dir| std::fs::read_dir(dir).unwrap().filter_map(Result::ok).map(|e| e.path()))
		.filter(|p| p.extension().is_some_and(|e| e == "fxd"))
		.collect()
}

/// A new document with `layers` extra layers, left unsaved, then a forced
/// recovery snapshot; the session then ends without saving.
fn crash_with_unsaved_work(dir: &Path, name: &str, root: &Path, layers: usize) -> Option<()> {
	let harness = session(dir, name)?;
	harness.ui(UiToEngine::Action {
		id: "doc:new".into(),
		args: serde_json::json!({ "width": 512, "height": 512, "ppi": 72, "depth": 8, "background": "white" }),
	});
	let doc = common::opened(&harness);
	for _ in 0..layers {
		harness.ui(UiToEngine::Command {
			doc,
			command: Command::AddLayer {
				layer: NewLayer::Pixel,
				name: None,
			},
		});
	}
	harness.wait("the layers", |s| match s {
		Seen::Ui(EngineToUi::Layers { doc: d, layers: l, .. }) if *d == doc && l.len() == layers + 1 => Some(()),
		_ => None,
	});
	let before = recovery_files(root).len();
	harness.engine.send(EngineInput::EmergencyRecovery);
	let deadline = Instant::now() + Duration::from_secs(20);
	while recovery_files(root).len() == before {
		assert!(Instant::now() < deadline, "no recovery snapshot was written");
		std::thread::sleep(Duration::from_millis(50));
	}
	// The document is still dirty: a "shutdown" here keeps the snapshot,
	// exactly as a crash would.
	harness.engine.shutdown();
	Some(())
}

fn offered(harness: &Harness) -> Vec<String> {
	harness.wait("the recovery offer", |s| match s {
		Seen::Ui(EngineToUi::RecoveryAvailable { paths }) => Some(paths.clone()),
		_ => None,
	})
}

fn nothing_offered(harness: &Harness) -> bool {
	std::thread::sleep(Duration::from_secs(2));
	!harness
		.seen
		.lock()
		.unwrap()
		.iter()
		.any(|s| matches!(s, Seen::Ui(EngineToUi::RecoveryAvailable { .. })))
}

fn close_discarding(harness: &Harness, doc: DocId) {
	harness.ui(UiToEngine::CloseDocument { doc });
	harness.ui(UiToEngine::CloseDocumentAnswer {
		doc,
		answer: CloseAnswer::DontSave,
	});
	harness.wait("the document closed", |s| match s {
		Seen::Ui(EngineToUi::DocumentClosed { doc: d }) if *d == doc => Some(()),
		_ => None,
	});
}

#[test]
fn unsaved_work_is_recovered_once_and_discard_forgets_it() {
	let dir = std::env::temp_dir().join(format!("fx-engine-recovery-flow-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	// SAFETY: set before any engine thread of this test binary starts.
	unsafe { std::env::set_var("LOCALAPPDATA", dir.join("localappdata")) };
	let root = dir.join("localappdata").join("Fotox").join("recovery");

	// 1. A session dies with three unsaved layers.
	if crash_with_unsaved_work(&dir, "a", &root, 3).is_none() {
		eprintln!("no GPU adapter: test skipped");
		return;
	}

	// Each session in its own block: a Harness holds the one-engine lock until
	// it is dropped, and a shadowed `harness` would live to the end of the test.

	// 2. The next session offers it; reopening gives the same layers.
	{
		let harness = session(&dir, "b").unwrap();
		let paths = offered(&harness);
		assert_eq!(paths.len(), 1, "one recovery document offered: {paths:?}");
		harness.ui(UiToEngine::RecoverDocument { path: paths[0].clone() });
		let doc = common::opened(&harness);
		harness.ui(UiToEngine::RequestLayers { doc });
		let layers = harness.wait("the recovered layers", |s| match s {
			Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == doc => Some(layers.len()),
			_ => None,
		});
		assert_eq!(layers, 4, "the recovered document has the unsaved layers");
		// This session takes its own snapshot; then the old file is consumed.
		harness.engine.send(EngineInput::EmergencyRecovery);
		let old = PathBuf::from(&paths[0]);
		let deadline = Instant::now() + Duration::from_secs(20);
		while old.exists() {
			assert!(Instant::now() < deadline, "the recovered file was not consumed after the new snapshot");
			std::thread::sleep(Duration::from_millis(50));
		}
		close_discarding(&harness, doc);
		harness.engine.shutdown();
	}

	// 3. Nothing is offered again, and no session folder is left over.
	{
		let harness = session(&dir, "c").unwrap();
		assert!(nothing_offered(&harness), "a recovered document was offered again");
		harness.engine.shutdown();
	}
	assert_eq!(sessions(&root), Vec::<PathBuf>::new(), "clean sessions left recovery folders behind");

	// 4. Discard: offered once, then never again.
	crash_with_unsaved_work(&dir, "d", &root, 2).unwrap();
	{
		let harness = session(&dir, "e").unwrap();
		let paths = offered(&harness);
		assert_eq!(paths.len(), 1);
		harness.ui(UiToEngine::DiscardRecovery { paths });
		std::thread::sleep(Duration::from_millis(500));
		harness.engine.shutdown();
	}
	{
		let harness = session(&dir, "f").unwrap();
		assert!(nothing_offered(&harness), "a discarded document was offered again");
		harness.engine.shutdown();
	}
	assert_eq!(sessions(&root), Vec::<PathBuf>::new(), "discarded or clean sessions left folders behind");
	let _ = std::fs::remove_dir_all(&dir);
}
