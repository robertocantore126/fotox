//! Shared harness of the engine integration tests: a real engine (engine +
//! render threads on a GPU device) whose outputs are collected and decoded.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fx_engine::{EngineHandle, EngineInput, EngineOutput};
use fx_protocol::{DocId, EngineToUi, UiToEngine};

pub fn gpu() -> Option<(wgpu::Device, wgpu_sync::Queue)> {
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
pub enum Seen {
	Ui(EngineToUi),
	NeedSavePath(DocId),
	MayClose(bool),
	/// The cursor the engine asked the shell for (M5-T04: the marquee tools'
	/// crosshair, taken over from the view's plain-hover default).
	Cursor(fx_engine::CursorShape),
}

pub struct Harness {
	pub engine: EngineHandle,
	pub seen: Arc<Mutex<Vec<Seen>>>,
	/// One engine at a time per test binary: each allocates the reference
	/// machine's GPU caches, and several in parallel run the device out of
	/// memory. Declared last, so the engine stops before the next one starts.
	_one_at_a_time: std::sync::MutexGuard<'static, ()>,
}

/// Serialises the engines of one test binary (see `Harness`).
static ONE_ENGINE: Mutex<()> = Mutex::new(());

impl Harness {
	pub fn start(device: wgpu::Device, queue: wgpu_sync::Queue, dir: &std::path::Path) -> Self {
		let one_at_a_time = ONE_ENGINE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
				EngineOutput::Cursor(shape) => Seen::Cursor(shape),
				_ => return,
			};
			sink.lock().unwrap().push(item);
		})
		.unwrap();
		engine.send(EngineInput::ViewportResized { width: 800, height: 600 });
		Harness {
			engine,
			seen,
			_one_at_a_time: one_at_a_time,
		}
	}

	/// Wait until `pick` finds something in what the engine said (removing
	/// everything up to and including it).
	pub fn wait<T>(&self, what: &str, pick: impl Fn(&Seen) -> Option<T>) -> T {
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

	pub fn ui(&self, message: UiToEngine) {
		self.engine.send(EngineInput::Ui(message));
	}
}

/// Write a small 16-bit RGB TIFF (`w × h`, one strip) and return its path.
pub fn tiff(dir: &std::path::Path, name: &str, w: u32, h: u32) -> std::path::PathBuf {
	let path = dir.join(name);
	let mut writer = fx_io::tiff_write::TiffWriter::create(&path, w, h, 16, h).unwrap();
	let strip: Vec<u8> = (0..w * h)
		.flat_map(|i| [(i % 65535) as u16, 1000, 2000].into_iter().flat_map(u16::to_le_bytes))
		.collect();
	writer.write_strip(&strip).unwrap();
	writer.finish().unwrap();
	path
}

/// Wait for the next `document_opened` and return its id.
pub fn opened(harness: &Harness) -> DocId {
	harness.wait("a document", |s| match s {
		Seen::Ui(EngineToUi::DocumentOpened { info }) => Some(info.doc),
		_ => None,
	})
}
