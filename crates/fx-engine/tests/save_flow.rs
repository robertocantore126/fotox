//! The whole M3 save flow through a running engine (engine + render threads
//! on a real GPU device; skipped without an adapter): import a TIFF, Save asks
//! the shell for a path, Save As writes a `.fxd` and cleans the document, an
//! edit makes it dirty again, closing a dirty document asks first, and the
//! `.fxd` reopens lazily with the edited layer.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fx_core::{Command, LayerRef, command::LayerPropsPatch};
use fx_engine::{EngineHandle, EngineInput, EngineOutput};
use fx_protocol::{CloseAnswer, DocId, EngineToUi, UiToEngine};

fn gpu() -> Option<(wgpu::Device, wgpu_sync::Queue)> {
	let instance = wgpu_sync::Instance::new(wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()));
	let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
	let limits = adapter.limits();
	pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
		label: Some("fx-engine-save-flow"),
		required_limits: limits,
		..Default::default()
	}))
	.ok()
}

/// What the engine said, decoded.
#[derive(Debug)]
enum Seen {
	Ui(EngineToUi),
	NeedSavePath(DocId),
	MayClose(bool),
}

struct Harness {
	engine: EngineHandle,
	seen: Arc<Mutex<Vec<Seen>>>,
}

impl Harness {
	fn start(device: wgpu::Device, queue: wgpu_sync::Queue, dir: &std::path::Path) -> Self {
		let seen = Arc::new(Mutex::new(Vec::new()));
		let sink = seen.clone();
		let engine = EngineHandle::spawn(device, queue, dir.join("scratch"), move |output| {
			let item = match output {
				EngineOutput::ToUi(frame) => match fx_protocol::decode::<EngineToUi>(&frame) {
					Ok((message, _)) => Seen::Ui(message),
					Err(_) => return,
				},
				EngineOutput::NeedSavePath { doc, .. } => Seen::NeedSavePath(doc),
				EngineOutput::MayClose(may) => Seen::MayClose(may),
				_ => return,
			};
			sink.lock().unwrap().push(item);
		})
		.unwrap();
		engine.send(EngineInput::ViewportResized { width: 800, height: 600 });
		Harness { engine, seen }
	}

	/// Wait until `pick` finds something in what the engine said (removing
	/// everything up to and including it).
	fn wait<T>(&self, what: &str, pick: impl Fn(&Seen) -> Option<T>) -> T {
		let deadline = Instant::now() + Duration::from_secs(30);
		loop {
			{
				let mut seen = self.seen.lock().unwrap();
				if let Some(i) = seen.iter().position(|s| pick(s).is_some()) {
					let found = pick(&seen[i]).unwrap();
					seen.drain(..=i);
					return found;
				}
			}
			assert!(Instant::now() < deadline, "timed out waiting for {what}");
			std::thread::sleep(Duration::from_millis(10));
		}
	}

	fn ui(&self, message: UiToEngine) {
		self.engine.send(EngineInput::Ui(message));
	}
}

fn dirty_of(seen: &Seen, doc: DocId) -> Option<bool> {
	match seen {
		Seen::Ui(EngineToUi::DocumentChanged { info }) if info.doc == doc => Some(info.dirty),
		_ => None,
	}
}

#[test]
fn import_save_edit_close_reopen() {
	let Some((device, queue)) = gpu() else {
		eprintln!("no GPU adapter: test skipped");
		return;
	};
	let dir = std::env::temp_dir().join(format!("fx-engine-save-flow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();

	// A small 16-bit TIFF to import.
	let tif = dir.join("photo.tif");
	let (w, h) = (600u32, 300u32);
	let mut writer = fx_io::tiff_write::TiffWriter::create(&tif, w, h, 16, h).unwrap();
	let strip: Vec<u8> = (0..w * h)
		.flat_map(|i| [(i % 65535) as u16, 1000, 2000].into_iter().flat_map(u16::to_le_bytes))
		.collect();
	writer.write_strip(&strip).unwrap();
	writer.finish().unwrap();

	let harness = Harness::start(device, queue, &dir);
	harness.engine.send(EngineInput::Open(vec![tif.clone()]));
	let doc = harness.wait("the imported document", |s| match s {
		Seen::Ui(EngineToUi::DocumentOpened { info }) => Some(info.doc),
		_ => None,
	});

	// Save of an imported document: the engine asks the shell for a path.
	harness.ui(UiToEngine::Action {
		id: "doc:save".into(),
		args: serde_json::Value::Null,
	});
	let asked = harness.wait("NeedSavePath", |s| match s {
		Seen::NeedSavePath(d) => Some(*d),
		_ => None,
	});
	assert_eq!(asked, doc);

	// Save As → clean.
	let fxd: PathBuf = dir.join("photo.fxd");
	harness.engine.send(EngineInput::SaveAs { doc, path: fxd.clone() });
	assert!(!harness.wait("the saved document", |s| dirty_of(s, doc)), "clean after Save As");
	assert!(fxd.exists());

	// An edit makes it dirty; closing it then asks instead of closing.
	harness.ui(UiToEngine::Command {
		doc,
		command: Command::SetLayerProps {
			layer: LayerRef::Active,
			props: LayerPropsPatch {
				opacity: Some(0.5),
				..Default::default()
			},
		},
	});
	harness.ui(UiToEngine::CloseDocument { doc });
	harness.wait("the save-changes prompt", |s| match s {
		Seen::Ui(EngineToUi::CloseDirtyDocument { doc: d, .. }) if *d == doc => Some(()),
		_ => None,
	});
	// The window cannot close either while the answer is pending.
	harness.engine.send(EngineInput::CloseRequested);
	assert!(!harness.wait("MayClose", |s| match s {
		Seen::MayClose(may) => Some(*may),
		_ => None,
	}));

	// "Save": saved incrementally, then closed, then the window may close.
	harness.ui(UiToEngine::CloseDocumentAnswer {
		doc,
		answer: CloseAnswer::Save,
	});
	harness.wait("the document closed", |s| match s {
		Seen::Ui(EngineToUi::DocumentClosed { doc: d }) if *d == doc => Some(()),
		_ => None,
	});
	assert!(harness.wait("MayClose after the last answer", |s| match s {
		Seen::MayClose(may) => Some(*may),
		_ => None,
	}));

	// Reopen: the `.fxd` opens clean, and the edit is there.
	harness.engine.send(EngineInput::Open(vec![fxd.clone()]));
	let reopened = harness.wait("the reopened .fxd", |s| match s {
		Seen::Ui(EngineToUi::DocumentOpened { info }) => Some(info.clone()),
		_ => None,
	});
	assert!(!reopened.dirty);
	assert_eq!((reopened.width, reopened.height), (w, h));
	let opacity = harness.wait("its layers", |s| match s {
		Seen::Ui(EngineToUi::Layers { doc: d, layers, .. }) if *d == reopened.doc => layers.first().map(|l| l.opacity),
		_ => None,
	});
	assert_eq!(opacity, 0.5, "the edit saved on close is in the file");

	harness.engine.shutdown();
	let _ = std::fs::remove_dir_all(&dir);
}
