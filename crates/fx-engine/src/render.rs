//! The render thread: draws the viewport texture whenever the engine thread
//! asks, and hands it to the shell (docs/ARCHITECTURE.md §2.2).
//!
//! M0-T06 draws the procedural test pattern; M1-T07 replaces it with the tile
//! compositor. Rules that already hold:
//! * Only render when something changed (the engine sends a request only
//!   then); a burst of requests collapses into one frame for the latest one.
//! * Every queue submission goes through the [`wgpu_sync::Queue`], so it cannot
//!   race the shell's surface reconfiguration.
//! * Double-buffered: the texture the shell is showing is never the one being
//!   drawn into.

use crossbeam_channel::Receiver;
use fx_render::{TestPatternRenderer, VIEWPORT_FORMAT, ViewTransform, ViewportSize};

use crate::{EngineOutput, OutputSink};

/// Work for the render thread.
pub(crate) enum RenderRequest {
	/// Draw this view.
	Frame {
		view: ViewTransform,
		viewport: ViewportSize,
		doc: (u32, u32),
	},
	/// Finish.
	Stop,
}

/// Body of the render thread. Returns when it receives [`RenderRequest::Stop`]
/// or the engine thread goes away.
pub(crate) fn run(device: wgpu::Device, queue: wgpu_sync::Queue, requests: Receiver<RenderRequest>, output: OutputSink) {
	let renderer = TestPatternRenderer::new(&device, VIEWPORT_FORMAT);
	let mut textures: [Option<wgpu::Texture>; 2] = [None, None];
	let mut next = 0;

	while let Ok(first) = requests.recv() {
		// Only the newest request matters.
		let mut request = first;
		for newer in requests.try_iter() {
			request = newer;
		}
		let RenderRequest::Frame { view, viewport, doc } = request else {
			break;
		};
		if viewport.width == 0 || viewport.height == 0 {
			continue;
		}

		let reuse = textures[next]
			.as_ref()
			.is_some_and(|t| t.width() == viewport.width && t.height() == viewport.height);
		if !reuse {
			textures[next] = Some(create_viewport_texture(&device, viewport));
		}
		let texture = textures[next].as_ref().expect("created above when missing or resized");

		let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("fx-viewport-frame"),
		});
		let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
		renderer.render(&queue, &mut encoder, &target, viewport, &view, doc);
		queue.submit(std::iter::once(encoder.finish()));

		output(EngineOutput::ViewportFrame(texture.clone()));
		next ^= 1;
	}
	tracing::debug!("render thread finished");
}

fn create_viewport_texture(device: &wgpu::Device, viewport: ViewportSize) -> wgpu::Texture {
	device.create_texture(&wgpu::TextureDescriptor {
		label: Some("fx-viewport"),
		size: wgpu::Extent3d {
			width: viewport.width,
			height: viewport.height,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: VIEWPORT_FORMAT,
		usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
		view_formats: &[],
	})
}
