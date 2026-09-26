//! M13 through a running engine, with the real models (ignored by default:
//! they need ONNX Runtime via `FOTOX_ORT_DYLIB` and the models installed,
//! e.g. by `cargo test -p fx-ai --test real_models -- --ignored`).
//!
//! `cargo test -p fx-engine --test ai_flow -- --ignored --nocapture`

mod common;

use std::time::Instant;

use common::{Harness, Seen, gpu, opened};
use fx_engine::{EngineInput, Modifiers, PointerInput, PointerKind};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

/// A 700 × 400 picture: a warm disc (centre (400, 200), radius 120) on a
/// cool textured background.
fn picture(dir: &std::path::Path) -> std::path::PathBuf {
	let (w, h) = (700u32, 400u32);
	let path = dir.join("subject.tif");
	let mut writer = fx_io::tiff_write::TiffWriter::create(&path, w, h, 16, h).unwrap();
	let mut strip = Vec::with_capacity((w * h * 6) as usize);
	for y in 0..h {
		for x in 0..w {
			let d = ((x as f32 - 400.0).powi(2) + (y as f32 - 200.0).powi(2)).sqrt();
			let n = ((x * 7 + y * 13) % 17) as f32 / 170.0;
			let rgb = if d < 120.0 {
				[0.9 - d / 120.0 * 0.3, 0.45, 0.2]
			} else {
				[0.25 + n, 0.35 + n, 0.45 + n]
			};
			for c in rgb {
				strip.extend(((c * 65535.0) as u16).to_le_bytes());
			}
		}
	}
	writer.write_strip(&strip).unwrap();
	writer.finish().unwrap();
	path
}

fn in_disc(x: f32, y: f32) -> bool {
	(x - 400.0).hypot(y - 200.0) < 120.0
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

/// The label of the newest History step, or the error the engine gave.
fn last_step(harness: &Harness, doc: DocId) -> String {
	harness.wait("a history message", |s| match s {
		Seen::Ui(EngineToUi::History { doc: d, labels, current, .. }) if *d == doc => labels.get(current.wrapping_sub(1)).cloned(),
		Seen::Ui(EngineToUi::Error { text }) => Some(format!("error: {text}")),
		_ => None,
	})
}

fn b64(text: &str) -> Vec<u8> {
	let value = |c: u8| match c {
		b'A'..=b'Z' => c - b'A',
		b'a'..=b'z' => c - b'a' + 26,
		b'0'..=b'9' => c - b'0' + 52,
		b'+' => 62,
		_ => 63,
	};
	let bytes: Vec<u8> = text.bytes().filter(|c| *c != b'=').collect();
	let mut out = Vec::new();
	for chunk in bytes.chunks(4) {
		let v: Vec<u32> = chunk.iter().map(|c| u32::from(value(*c))).collect();
		let n = v.iter().enumerate().fold(0u32, |acc, (i, x)| acc | (x << (18 - 6 * i)));
		out.extend(n.to_be_bytes()[1..chunk.len()].iter());
	}
	out
}

/// IoU of the selection (saved as a channel, its 48² thumbnail) with the disc.
fn selection_iou(harness: &Harness, doc: DocId) -> f64 {
	harness.ui(UiToEngine::Command {
		doc,
		command: fx_core::Command::SaveSelection {
			channel: None,
			name: Some("probe".into()),
			mode: fx_core::SelectMode::Replace,
		},
	});
	let thumb = harness.wait("the channels", |s| match s {
		Seen::Ui(EngineToUi::Channels { channels, .. }) => channels.last().and_then(|c| c["thumb"].as_str()).map(b64),
		_ => None,
	});
	let (mut both, mut either) = (0, 0);
	for j in 0..48 {
		for i in 0..48 {
			let (x, y) = (i as f32 * 700.0 / 48.0, j as f32 * 400.0 / 48.0);
			let (s, d) = (thumb[j * 48 + i] > 127, in_disc(x, y));
			both += usize::from(s && d);
			either += usize::from(s || d);
		}
	}
	both as f64 / either.max(1) as f64
}

#[test]
#[ignore = "real models and ONNX Runtime"]
fn select_subject_and_object_selection_follow_the_disc() {
	if fx_ai::runtime::find_library().is_none() || !fx_ai::models::BIREFNET.installed() || !fx_ai::models::EFFICIENT_SAM.installed() {
		eprintln!("skipped: ONNX Runtime or the models are missing");
		return;
	}
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-ai-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![picture(&dir)]));
	let doc = opened(&harness);

	// Select ▸ Subject.
	let t = Instant::now();
	harness.ui(UiToEngine::Action {
		id: "ai:subject".into(),
		args: serde_json::Value::Null,
	});
	assert_eq!(last_step(&harness, doc), "Select Subject");
	let iou = selection_iou(&harness, doc);
	eprintln!("Select Subject: {:?}, IoU {iou:.3}", t.elapsed());
	assert!(iou > 0.85, "IoU {iou}");

	// The Object Selection tool: a box around the disc. The 700 × 400
	// document is at 100 %, centred in 800 × 600: doc (0, 0) = view (50, 100).
	harness.ui(UiToEngine::Action {
		id: "sel:none".into(),
		args: serde_json::Value::Null,
	});
	assert_eq!(last_step(&harness, doc), "Deselect");
	harness.ui(UiToEngine::Action {
		id: "tool:object-select".into(),
		args: serde_json::Value::Null,
	});
	for round in 0..2 {
		let t = Instant::now();
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 310.0, 180.0, 0)));
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Down, 310.0, 180.0, 1)));
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 450.0, 300.0, 1)));
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Move, 590.0, 420.0, 1)));
		harness.engine.send(EngineInput::Pointer(pointer(PointerKind::Up, 590.0, 420.0, 0)));
		assert_eq!(last_step(&harness, doc), "Object Selection");
		let elapsed = t.elapsed();
		let iou = selection_iou(&harness, doc);
		// The second run reuses the embedding (S39).
		eprintln!("Object Selection #{round}: {elapsed:?}, IoU {iou:.3}");
		assert!(iou > 0.85, "IoU {iou}");
	}
}
