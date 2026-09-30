//! What reaches the screen after an edit (2026-09-27). The derived-tile job
//! compared the render thread's `render_generation` with the document's
//! content generation, which differ after the first edit, so no shape, text
//! or layer style was ever drawn until the document was saved and reopened.
//! These tests read the frames the render thread delivers: what the user sees.

mod common;

use common::{Harness, Seen, gpu};
use fx_protocol::{EngineToUi, UiToEngine};

fn command(harness: &Harness, doc: fx_protocol::DocId, json: serde_json::Value) {
	harness.ui(UiToEngine::Command {
		doc,
		command: serde_json::from_value(json).expect("a valid command"),
	});
}

/// Wait (up to 10 s) until the latest frame shows `want` at document point
/// `(dx, dy)`; returns the last colour seen when it never does.
fn frame_color_at(harness: &Harness, view: (f64, f64, f64), (dx, dy): (f64, f64), want: impl Fn([u8; 4]) -> bool) -> Result<[u8; 4], [u8; 4]> {
	let (zoom, cx, cy) = view;
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
	let mut last = [0; 4];
	loop {
		if let Some((pixels, (w, h))) = harness.frame_pixels() {
			let x = (f64::from(w) / 2.0 + (dx - cx) * zoom).round() as u32;
			let y = (f64::from(h) / 2.0 + (dy - cy) * zoom).round() as u32;
			last = pixels[(y.min(h - 1) * w + x.min(w - 1)) as usize];
			if want(last) {
				return Ok(last);
			}
		}
		if std::time::Instant::now() > deadline {
			return Err(last);
		}
		std::thread::sleep(std::time::Duration::from_millis(100));
	}
}

#[test]
fn shapes_and_styles_added_after_an_edit_reach_the_screen() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-render-flow-{}", std::process::id()));
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
	let view = harness.wait("the document's view", |s| match s {
		Seen::Ui(EngineToUi::View {
			doc: of,
			zoom,
			center_x,
			center_y,
			..
		}) if *of == doc => Some((*zoom, *center_x, *center_y)),
		_ => None,
	});
	let layers = |n: usize| {
		harness.wait(&format!("{n} layers"), |s| match s {
			Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.len() == n => Some(()),
			_ => None,
		})
	};
	// An edit first: the content generation moves away from 0.
	command(&harness, doc, serde_json::json!({ "op": "add_layer", "layer": "pixel", "name": null }));
	layers(2);
	command(
		&harness,
		doc,
		serde_json::json!({ "op": "add_layer", "name": null, "layer": { "shape": {
			"shape": { "kind": "rect", "w": 200.0, "h": 200.0, "radii": [0.0, 0.0, 0.0, 0.0] },
			"fill": { "kind": "solid", "rgba": [0, 0, 65535, 65535] },
			"stroke": null,
			"transform": [1.0, 0.0, 0.0, 1.0, 50.0, 50.0]
		} } }),
	);
	layers(3);
	let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 40 && p[1] < 40;
	let shown = frame_color_at(&harness, view, (150.0, 150.0), blue);
	assert!(shown.is_ok(), "the shape never reached the screen: {:?}", shown.unwrap_err());
	let white = |p: [u8; 4]| p[0] > 240 && p[1] > 240 && p[2] > 240;
	assert!(
		frame_color_at(&harness, view, (500.0, 400.0), white).is_ok(),
		"the canvas around it stays white"
	);

	// A layer style on it (the fx menu's Color Overlay).
	command(
		&harness,
		doc,
		serde_json::json!({ "op": "set_layer_style", "layer": "active", "styles": {
			"color_overlay": { "enabled": true, "blend": "normal", "color": [65535, 0, 0, 65535], "opacity": 1.0 }
		} }),
	);
	let red = |p: [u8; 4]| p[0] > 200 && p[1] < 40 && p[2] < 40;
	let styled = frame_color_at(&harness, view, (150.0, 150.0), red);
	assert!(styled.is_ok(), "the layer style never reached the screen: {:?}", styled.unwrap_err());
}

fn pointer(kind: fx_engine::PointerKind, x: f64, y: f64, buttons: u8) -> fx_engine::EngineInput {
	fx_engine::EngineInput::Pointer(fx_engine::PointerInput {
		kind,
		x,
		y,
		pressure: 1.0,
		tilt_x: 0.0,
		tilt_y: 0.0,
		buttons,
		modifiers: fx_engine::Modifiers::default(),
		time_us: 0,
	})
}

/// Free Transform on a layer: a drag inside moves the box, a click away from
/// it commits (the box used to rotate on any press outside it and stay up).
#[test]
fn a_free_transform_drag_moves_the_layer_and_a_click_away_commits_it() {
	use fx_engine::PointerKind::{Down, Move, Up};
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-render-flow-xf-{}", std::process::id()));
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
	let view = harness.wait("the document's view", |s| match s {
		Seen::Ui(EngineToUi::View {
			doc: of,
			zoom,
			center_x,
			center_y,
			..
		}) if *of == doc => Some((*zoom, *center_x, *center_y)),
		_ => None,
	});
	assert_eq!(view, (1.0, 400.0, 300.0), "the viewport shows the document 1:1");
	command(
		&harness,
		doc,
		serde_json::json!({ "op": "add_layer", "name": null, "layer": { "shape": {
			"shape": { "kind": "rect", "w": 200.0, "h": 200.0, "radii": [0.0, 0.0, 0.0, 0.0] },
			"fill": { "kind": "solid", "rgba": [0, 0, 65535, 65535] },
			"stroke": null,
			"transform": [1.0, 0.0, 0.0, 1.0, 100.0, 100.0]
		} } }),
	);
	command(&harness, doc, serde_json::json!({ "op": "rasterize", "layers": ["active"] }));
	harness.wait("the rasterised layer", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.len() == 2 && layers.iter().all(|l| l.kind == fx_protocol::LayerInfoKind::Pixel) => Some(()),
		_ => None,
	});
	harness.ui(UiToEngine::Action {
		id: "xf:free".into(),
		args: serde_json::Value::Null,
	});
	harness.wait("the transform box", |s| match s {
		Seen::Ui(EngineToUi::TransformBox { up: true, .. }) => Some(()),
		_ => None,
	});
	// 1:1 and centred: viewport pixels are document pixels. Drag the box by
	// (+100, +50) from inside.
	for event in [
		pointer(Move, 200.0, 200.0, 0),
		pointer(Down, 200.0, 200.0, 1),
		pointer(Move, 250.0, 225.0, 1),
		pointer(Move, 300.0, 250.0, 1),
		pointer(Up, 300.0, 250.0, 0),
		// Far from the box: commits.
		pointer(Move, 700.0, 550.0, 0),
		pointer(Down, 700.0, 550.0, 1),
		pointer(Up, 700.0, 550.0, 0),
	] {
		harness.engine.send(event);
	}
	harness.wait("the box to close", |s| match s {
		Seen::Ui(EngineToUi::TransformBox { up: false, .. }) => Some(()),
		_ => None,
	});
	let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 40 && p[1] < 40;
	let white = |p: [u8; 4]| p[0] > 240 && p[1] > 240 && p[2] > 240;
	let moved = frame_color_at(&harness, view, (350.0, 300.0), blue);
	assert!(moved.is_ok(), "the layer did not move: {:?} at its new place", moved.unwrap_err());
	assert!(
		frame_color_at(&harness, view, (150.0, 150.0), white).is_ok(),
		"nothing is left at its old place"
	);
}

/// Code review 2026-09-27 R11: an adjustment dialog's value is on screen while
/// the dialog is open (outside the history), and gone when it closes.
#[test]
fn an_adjustment_preview_reaches_the_screen_and_leaves_it() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-render-flow-adj-{}", std::process::id()));
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
	let view = harness.wait("the document's view", |s| match s {
		Seen::Ui(EngineToUi::View {
			doc: of,
			zoom,
			center_x,
			center_y,
			..
		}) if *of == doc => Some((*zoom, *center_x, *center_y)),
		_ => None,
	});
	let levels = |out_white: f64| {
		let channels: Vec<serde_json::Value> = (0..4)
			.map(|i| {
				let out = if i == 0 { out_white } else { 1.0 };
				serde_json::json!({ "in_black": 0.0, "in_white": 1.0, "gamma": 1.0, "out_black": 0.0, "out_white": out })
			})
			.collect();
		serde_json::json!({ "kind": "levels", "channels": channels })
	};
	command(
		&harness,
		doc,
		serde_json::json!({ "op": "add_layer", "name": null, "layer": { "adjustment": levels(1.0) } }),
	);
	let layer = harness.wait("the Levels layer", |s| match s {
		Seen::Ui(EngineToUi::Layers { layers, .. }) if layers.len() == 2 => Some(layers[0].id),
		_ => None,
	});
	let white = |p: [u8; 4]| p[0] > 240 && p[1] > 240 && p[2] > 240;
	let black = |p: [u8; 4]| p[0] < 16 && p[1] < 16 && p[2] < 16;
	assert!(frame_color_at(&harness, view, (400.0, 300.0), white).is_ok(), "the neutral Levels leaves white");
	harness.ui(UiToEngine::AdjustmentPreview {
		doc,
		layer,
		adjustment: serde_json::from_value(levels(0.0)).unwrap(),
	});
	let shown = frame_color_at(&harness, view, (400.0, 300.0), black);
	assert!(shown.is_ok(), "the preview never reached the screen: {:?}", shown.unwrap_err());
	harness.ui(UiToEngine::AdjustmentPreviewEnd { doc });
	let gone = frame_color_at(&harness, view, (400.0, 300.0), white);
	assert!(gone.is_ok(), "the preview stayed on screen: {:?}", gone.unwrap_err());
}
