//! Zooming out after a paste (flight recorder, 2026-09-26): the render thread
//! panicked with "image smaller than the document" and the view froze. A
//! pasted image smaller than the canvas has a shorter mip pyramid than the
//! canvas, and the compositor reads every layer at the canvas's level.

mod common;

use common::{Harness, Seen, gpu};
use fx_engine::{EngineInput, Modifiers};
use fx_protocol::{EngineToUi, UiToEngine};

#[test]
fn zooming_out_past_a_small_paste_keeps_drawing() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-paste-zoom-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.ui(UiToEngine::Action {
		id: "doc:new".into(),
		args: serde_json::json!({ "width": 1080, "height": 1080, "ppi": 72, "depth": 8, "background": "white" }),
	});
	harness.wait("a document", |s| match s {
		Seen::Ui(EngineToUi::DocumentOpened { info }) => Some(info.doc),
		_ => None,
	});
	// The trace's paste: 600 × 917 (3 levels of its own, the canvas has 4).
	let (w, h) = (600u32, 917u32);
	let rgba8: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 239) as u8, 90, 255]).collect();
	harness.engine.send(EngineInput::PasteImage { width: w, height: h, rgba8 });
	harness.wait("the paste", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.len() >= 2 => Some(()),
		_ => None,
	});

	// Ctrl + wheel down to ~6 % (level 4 is clamped to the canvas's 3).
	let ctrl = Modifiers {
		ctrl: true,
		..Default::default()
	};
	for _ in 0..40 {
		harness.engine.send(EngineInput::Wheel {
			x: 400.0,
			y: 300.0,
			dx: 0.0,
			dy: -1.0,
			modifiers: ctrl,
		});
	}
	let zoom = harness.wait("the zoomed-out view", |s| match s {
		Seen::Ui(EngineToUi::View { zoom, .. }) if *zoom < 0.125 => Some(*zoom),
		_ => None,
	});
	assert!(zoom < 0.125, "zoomed to level 3: {zoom}");
	// A dead render thread draws nothing more: frames keep coming after a
	// nudge of the view (a pan, same zoom).
	for _ in 0..3 {
		harness.engine.send(EngineInput::Wheel {
			x: 400.0,
			y: 300.0,
			dx: 0.0,
			dy: 0.2,
			modifiers: Modifiers::default(),
		});
		harness.wait("a frame at the zoomed-out level", |s| matches!(s, Seen::Frame).then_some(()));
	}
	harness.engine.shutdown();
}
