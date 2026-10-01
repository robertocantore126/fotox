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

/// One layer-list frame as the engine sent it.
#[derive(Clone, Copy, Debug)]
pub struct LayersFrame {
	/// `layers_patch` (else a full `layers`).
	pub patch: bool,
	/// Rows it carried.
	pub rows: usize,
	/// Encoded size.
	pub bytes: usize,
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
	/// The render thread delivered a viewport frame.
	Frame,
}

pub struct Harness {
	pub engine: EngineHandle,
	pub seen: Arc<Mutex<Vec<Seen>>>,
	/// The latest viewport frame (see [`Harness::frame_pixels`]).
	pub last_frame: Arc<Mutex<Option<wgpu::Texture>>>,
	/// When the latest viewport frame arrived (stress tests time how long the
	/// view takes to settle).
	pub last_frame_at: Arc<Mutex<Option<Instant>>>,
	/// Every `layers` / `layers_patch` frame the engine sent, oldest first.
	pub layer_frames: Arc<Mutex<Vec<LayersFrame>>>,
	device: wgpu::Device,
	queue: wgpu_sync::Queue,
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
		let last_frame = Arc::new(Mutex::new(None));
		let frame_sink = last_frame.clone();
		let last_frame_at = Arc::new(Mutex::new(None));
		let frame_time_sink = last_frame_at.clone();
		let layer_frames = Arc::new(Mutex::new(Vec::new()));
		let frames_sink = layer_frames.clone();
		// The UI's copy of each document's layer list: `layers_patch` frames are
		// applied to it and handed to the tests as the full `layers` message, as
		// the Layers panel sees it. A patch against another list is an error.
		let lists: Mutex<std::collections::HashMap<DocId, (u64, Vec<fx_protocol::LayerInfo>)>> = Mutex::default();
		let engine = EngineHandle::spawn(device.clone(), queue.clone(), dir.join("scratch"), move |output| {
			let item = match output {
				EngineOutput::ToUi(frame) => match fx_protocol::decode::<EngineToUi>(&frame) {
					Ok((EngineToUi::Layers { doc, revision, layers, seq }, _)) => {
						frames_sink.lock().unwrap().push(LayersFrame {
							patch: false,
							rows: layers.len(),
							bytes: frame.len(),
						});
						lists.lock().unwrap().insert(doc, (seq, layers.clone()));
						Seen::Ui(EngineToUi::Layers { doc, revision, layers, seq })
					}
					Ok((
						EngineToUi::LayersPatch {
							doc,
							revision,
							seq,
							base,
							changed,
						},
						_,
					)) => {
						frames_sink.lock().unwrap().push(LayersFrame {
							patch: true,
							rows: changed.len(),
							bytes: frame.len(),
						});
						let mut lists = lists.lock().unwrap();
						match lists.get_mut(&doc) {
							Some((held, layers)) if *held == base => {
								fx_protocol::apply_layers_patch(layers, &changed);
								*held = seq;
								Seen::Ui(EngineToUi::Layers {
									doc,
									revision,
									layers: layers.clone(),
									seq,
								})
							}
							held => Seen::Ui(EngineToUi::Error {
								text: format!("layers_patch against list {base}, the UI holds {:?}", held.map(|h| h.0)),
							}),
						}
					}
					// VERIFY-FIX(4.2): follow structural patches like layers-panel.js does.
					Ok((
						EngineToUi::LayersStructurePatch {
							doc,
							revision,
							seq,
							base,
							ops,
							changed,
						},
						_,
					)) => {
						frames_sink.lock().unwrap().push(LayersFrame {
							patch: true,
							rows: changed.len() + ops.len(),
							bytes: frame.len(),
						});
						let mut lists = lists.lock().unwrap();
						let applied = match lists.get_mut(&doc) {
							Some((held, layers)) if *held == base => fx_protocol::apply_layers_structure_patch(layers, &ops, &changed).is_ok(),
							_ => false,
						};
						match lists.get_mut(&doc) {
							Some((held, layers)) if applied => {
								*held = seq;
								Seen::Ui(EngineToUi::Layers {
									doc,
									revision,
									layers: layers.clone(),
									seq,
								})
							}
							held => Seen::Ui(EngineToUi::Error {
								text: format!("layers_structure_patch against list {base}, the UI holds {:?}", held.map(|h| h.0)),
							}),
						}
					}
					Ok((message, _)) => Seen::Ui(message),
					Err(_) => return,
				},
				EngineOutput::NeedSavePath { doc, .. } => Seen::NeedSavePath(doc),
				EngineOutput::MayClose(may) => Seen::MayClose(may),
				EngineOutput::Cursor(shape) => Seen::Cursor(shape),
				EngineOutput::ViewportFrame(texture) => {
					*frame_sink.lock().unwrap() = Some(texture);
					*frame_time_sink.lock().unwrap() = Some(Instant::now());
					Seen::Frame
				}
				_ => return,
			};
			sink.lock().unwrap().push(item);
		})
		.unwrap();
		engine.send(EngineInput::ViewportResized { width: 800, height: 600 });
		Harness {
			engine,
			seen,
			last_frame,
			last_frame_at,
			layer_frames,
			device,
			queue,
			_one_at_a_time: one_at_a_time,
		}
	}

	/// The latest frame's RGBA8 pixels (row-major) and size: what the user
	/// would see in the viewport. `None` before the first frame.
	pub fn frame_pixels(&self) -> Option<(Vec<[u8; 4]>, (u32, u32))> {
		let texture = self.last_frame.lock().unwrap().clone()?;
		let (w, h) = (texture.width(), texture.height());
		let row = (w * 4).div_ceil(256) * 256;
		let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("fx-test-frame-readback"),
			size: u64::from(row * h),
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		let mut encoder = self
			.device
			.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx-test-frame") });
		encoder.copy_texture_to_buffer(
			texture.as_image_copy(),
			wgpu::TexelCopyBufferInfo {
				buffer: &readback,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(row),
					rows_per_image: Some(h),
				},
			},
			wgpu::Extent3d {
				width: w,
				height: h,
				depth_or_array_layers: 1,
			},
		);
		self.queue.submit([encoder.finish()]);
		let slice = readback.slice(..);
		slice.map_async(wgpu::MapMode::Read, |r| r.expect("readback map failed"));
		self.device.poll(wgpu::PollType::wait_indefinitely()).expect("device lost during readback");
		let data = slice.get_mapped_range();
		let mut out = Vec::with_capacity((w * h) as usize);
		for y in 0..h {
			let line = &data[(y * row) as usize..][..(w * 4) as usize];
			out.extend(line.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]));
		}
		Some((out, (w, h)))
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
