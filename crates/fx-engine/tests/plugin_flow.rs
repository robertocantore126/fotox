//! Brush plugins in the engine (D-096): a `.wasm` dropped into the plugin
//! folder becomes a tool without a restart, paints a History step under its
//! own name, is reloaded when the file is rewritten and removed when it is
//! deleted; a broken file is reported, not fatal.
//!
//! Needs the plugins built (`cargo xtask plugins`); skips without them.

mod common;

use std::path::PathBuf;

use common::{Harness, Seen, gpu, opened, tiff};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

fn built(name: &str) -> Option<PathBuf> {
	let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../plugins/target/wasm32-unknown-unknown/release/{name}.wasm"));
	path.exists().then_some(path)
}

fn pointer(kind: PointerKind, x: f64, y: f64, buttons: u8) -> PointerInput {
	PointerInput {
		kind,
		x,
		y,
		pressure: 0.0,
		tilt_x: 0.0,
		tilt_y: 0.0,
		buttons,
		modifiers: Modifiers::default(),
		time_us: 0,
	}
}

/// The next `plugins` list's tool ids, after any toasts about it.
fn next_tools(harness: &Harness) -> (Vec<String>, Vec<String>) {
	let mut notes = Vec::new();
	loop {
		let seen = harness.wait("a plugins message", |s| match s {
			Seen::Ui(EngineToUi::Plugins { tools }) => Some(Ok(tools.iter().map(|t| t.id.clone()).collect::<Vec<_>>())),
			Seen::Ui(EngineToUi::Toast { text }) if text.starts_with("Plugin") || text.starts_with("Building plugin") => Some(Err(text.clone())),
			Seen::Ui(EngineToUi::Error { text }) if text.contains("plugin") => Some(Err(text.clone())),
			_ => None,
		});
		match seen {
			Ok(tools) => return (tools, notes),
			Err(note) => notes.push(note),
		}
	}
}

fn last_step(harness: &Harness, doc: DocId) -> String {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => labels.get(current.wrapping_sub(1)).cloned(),
		Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
		_ => None,
	})
}

fn stroke(harness: &Harness, y: f64) {
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 100.0, y, 1)));
	for x in [140.0, 200.0, 280.0, 360.0] {
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, x, y + 10.0, 1)));
	}
	harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 360.0, y + 10.0, 0)));
}

#[test]
fn a_plugin_dropped_in_the_folder_becomes_a_tool_and_reloads_live() {
	let (Some(blend), Some(plain)) = (built("blend_eraser"), built("plain_eraser")) else {
		eprintln!("SKIPPED: plugins not built (cargo xtask plugins)");
		return;
	};
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-plugin-flow-{}", std::process::id()));
	let folder = dir.join("plugins");
	std::fs::create_dir_all(&folder).unwrap();
	std::fs::create_dir_all(dir.join("appdata")).unwrap();
	// SAFETY: set before the engine thread starts; this binary runs one test.
	unsafe {
		std::env::set_var("FOTOX_PLUGINS", &folder);
		std::env::set_var("APPDATA", dir.join("appdata"));
		std::env::set_var("FOTOX_PLUGIN_BUILD", dir.join("build"));
	}
	let harness = Harness::start(device, queue, &dir);
	harness.ui(UiToEngine::Hello { ui_version: "test".into() });
	assert_eq!(next_tools(&harness).0, Vec::<String>::new(), "an empty folder, no tools");

	// Dropped in while the app runs: a tool, announced. (Written, not
	// `fs::copy`: Windows' CopyFile keeps the source's modification time, a
	// rebuild does not.)
	let file = folder.join("blend_eraser.wasm");
	let put = |from: &PathBuf| std::fs::write(&file, std::fs::read(from).unwrap()).unwrap();
	put(&blend);
	let (tools, notes) = next_tools(&harness);
	assert_eq!(tools, ["plugin:blend-eraser"]);
	assert_eq!(notes, ["Plugin loaded: Blend Eraser Tool"]);

	// It paints a History step under its own name.
	harness.engine.send(EngineInput::Open(vec![tiff(&dir, "photo.tif", 700, 400)]));
	let doc = opened(&harness);
	harness.ui(UiToEngine::Action {
		id: "tool:plugin:blend-eraser".into(),
		args: serde_json::Value::Null,
	});
	harness.ui(UiToEngine::ToolOptions {
		tool: "plugin:blend-eraser".into(),
		options: serde_json::json!({ "Size": 60, "Opacity": 100, "Flow": 100, "Tone": "Off", "Softness": 40, "Smoothing": 0 }),
	});
	stroke(&harness, 150.0);
	assert_eq!(last_step(&harness, doc), "Blend Eraser Tool");

	// Rebuilt (here: a custom section appended, still valid wasm, so the
	// content changes even within a coarse file-time step): reloaded, still
	// the same tool, still paints.
	let mut rebuilt = std::fs::read(&blend).unwrap();
	rebuilt.extend_from_slice(&[0x00, 0x05, 0x04, b'n', b'o', b't', b'e']);
	std::fs::write(&file, rebuilt).unwrap();
	let (tools, notes) = next_tools(&harness);
	assert_eq!(tools, ["plugin:blend-eraser"]);
	assert_eq!(notes, ["Plugin loaded: Blend Eraser Tool"]);
	stroke(&harness, 220.0);
	assert_eq!(last_step(&harness, doc), "Blend Eraser Tool");

	// The same file now holds another plugin: the old tool goes.
	put(&plain);
	assert_eq!(next_tools(&harness).0, ["plugin:plain-eraser"]);

	// A broken file is an error message, nothing else.
	std::fs::write(folder.join("broken.wasm"), b"not a wasm module").unwrap();
	let (tools, notes) = next_tools(&harness);
	assert_eq!(tools, ["plugin:plain-eraser"]);
	assert!(notes.iter().any(|n| n.contains("does not compile")), "{notes:?}");

	// Deleted: the tool is gone.
	std::fs::remove_file(&file).unwrap();
	let (tools, notes) = next_tools(&harness);
	assert_eq!(tools, Vec::<String>::new());
	assert_eq!(notes, ["Plugin removed: Plain Eraser (plugin)"]);

	// The folder holds the AI prompt.
	assert!(
		std::fs::read_to_string(folder.join("AI-PROMPT.md"))
			.unwrap()
			.contains("Instructions for the AI")
	);
	// A single .rs, as an AI writes it: built, announced, a tool.
	let prompt = std::fs::read_to_string(folder.join("AI-PROMPT.md")).unwrap();
	let example = &prompt[prompt.find("### Complete example").unwrap()..];
	let example = &example[example.find("```rust\n").unwrap() + 8..];
	let example = &example[..example.find("```").unwrap()];
	std::fs::write(folder.join("shadow-eraser.rs"), example).unwrap();
	let (tools, notes) = next_tools(&harness);
	assert_eq!(tools, ["plugin:shadow-eraser"]);
	assert_eq!(notes, ["Building plugin shadow-eraser.rs\u{2026}", "Plugin loaded: Shadow Eraser Tool"]);
	harness.ui(UiToEngine::Action {
		id: "tool:plugin:shadow-eraser".into(),
		args: serde_json::Value::Null,
	});
	stroke(&harness, 300.0);
	assert_eq!(last_step(&harness, doc), "Shadow Eraser Tool");

	// Plugins ▸ Reload Plugins: one summary, the same tools, and the file
	// that still does not load says so again.
	harness.ui(UiToEngine::Action {
		id: "plugins:reload".into(),
		args: serde_json::Value::Null,
	});
	let (tools, notes) = next_tools(&harness);
	assert_eq!(tools, ["plugin:shadow-eraser"]);
	assert_eq!(notes.len(), 2, "{notes:?}");
	assert_eq!(notes[0], "Plugins reloaded (1): Shadow Eraser Tool");
	assert!(notes[1].contains("broken.wasm does not compile"), "{notes:?}");

	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}
