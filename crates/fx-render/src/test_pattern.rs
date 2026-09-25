//! Procedural test pattern of a virtual document (M0-T06).
//!
//! Draws what a document *would* look like under a [`ViewTransform`] without
//! any tiles: a 256 px checker in document space (so it scales and moves with
//! zoom and pan), a thicker line every 4096 px, the document bounds in cyan
//! and `#282828` outside the document. Used to prove pan/zoom, input routing
//! and the rulers before real documents exist (M1).
//!
//! The target must be a non-sRGB format: like the tile viewport, the shader
//! writes sRGB-encoded values as they are and the shell's composite pass
//! decodes them.

use crate::viewport::{ViewTransform, ViewportSize};

/// The format the render thread's viewport textures use.
pub const VIEWPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Uniform block of `test_pattern.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
	viewport: [f32; 2],
	center: [f32; 2],
	doc: [f32; 2],
	zoom: f32,
	_pad: f32,
}

/// GPU state for drawing the test pattern.
pub struct TestPatternRenderer {
	pipeline: wgpu::RenderPipeline,
	params: wgpu::Buffer,
	bind_group: wgpu::BindGroup,
}

impl TestPatternRenderer {
	/// Build the pipeline for a `format` target (must not be sRGB).
	pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
		assert!(!format.is_srgb(), "the test pattern writes encoded values as-is");
		let module = device.create_shader_module(wgpu::include_wgsl!("test_pattern.wgsl"));
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("fx-test-pattern"),
			entries: &[wgpu::BindGroupLayoutEntry {
				binding: 0,
				visibility: wgpu::ShaderStages::FRAGMENT,
				ty: wgpu::BindingType::Buffer {
					ty: wgpu::BufferBindingType::Uniform,
					has_dynamic_offset: false,
					min_binding_size: None,
				},
				count: None,
			}],
		});
		let params = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("fx-test-pattern-params"),
			size: size_of::<Params>() as u64,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("fx-test-pattern"),
			layout: &layout,
			entries: &[wgpu::BindGroupEntry {
				binding: 0,
				resource: params.as_entire_binding(),
			}],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("fx-test-pattern"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("fx-test-pattern"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &module,
				entry_point: Some("vs_main"),
				buffers: &[],
				compilation_options: Default::default(),
			},
			fragment: Some(wgpu::FragmentState {
				module: &module,
				entry_point: Some("fs_main"),
				targets: &[Some(wgpu::ColorTargetState {
					format,
					blend: None,
					write_mask: wgpu::ColorWrites::ALL,
				})],
				compilation_options: Default::default(),
			}),
			primitive: wgpu::PrimitiveState::default(),
			depth_stencil: None,
			multisample: wgpu::MultisampleState::default(),
			multiview_mask: None,
			cache: None,
		});
		Self { pipeline, params, bind_group }
	}

	/// Record one frame of the pattern into `target` (`viewport` pixels).
	///
	/// The parameters are written through `queue` before the pass; submit
	/// `encoder` through the same queue afterwards.
	pub fn render(
		&self,
		queue: &wgpu_sync::Queue,
		encoder: &mut wgpu::CommandEncoder,
		target: &wgpu::TextureView,
		viewport: ViewportSize,
		view: &ViewTransform,
		doc_size: (u32, u32),
	) {
		let params = Params {
			viewport: [viewport.width as f32, viewport.height as f32],
			center: [view.center_x as f32, view.center_y as f32],
			doc: [doc_size.0 as f32, doc_size.1 as f32],
			zoom: view.zoom as f32,
			_pad: 0.0,
		};
		queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
		let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
			label: Some("fx-test-pattern"),
			color_attachments: &[Some(wgpu::RenderPassColorAttachment {
				view: target,
				resolve_target: None,
				ops: wgpu::Operations {
					load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
					store: wgpu::StoreOp::Store,
				},
				depth_slice: None,
			})],
			depth_stencil_attachment: None,
			occlusion_query_set: None,
			timestamp_writes: None,
			multiview_mask: None,
		});
		pass.set_pipeline(&self.pipeline);
		pass.set_bind_group(0, &self.bind_group, &[]);
		pass.draw(0..3, 0..1);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn gpu() -> Option<(wgpu::Device, wgpu_sync::Queue)> {
		let instance = wgpu_sync::Instance::new(wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()));
		let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
		pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
	}

	/// Render a 64 × 64 viewport and read it back as RGBA8 rows.
	fn render(view: ViewTransform, doc: (u32, u32)) -> Option<Vec<u8>> {
		let _one_at_a_time = crate::testing::one_gpu_test();
		let (device, queue) = gpu()?;
		let size = wgpu::Extent3d {
			width: 64,
			height: 64,
			depth_or_array_layers: 1,
		};
		let texture = device.create_texture(&wgpu::TextureDescriptor {
			label: None,
			size,
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: VIEWPORT_FORMAT,
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
			view_formats: &[],
		});
		let renderer = TestPatternRenderer::new(&device, VIEWPORT_FORMAT);
		let mut encoder = device.create_command_encoder(&Default::default());
		let viewport = ViewportSize { width: 64, height: 64 };
		renderer.render(&queue, &mut encoder, &texture.create_view(&Default::default()), viewport, &view, doc);
		let readback = device.create_buffer(&wgpu::BufferDescriptor {
			label: None,
			size: 256 * 64,
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		encoder.copy_texture_to_buffer(
			texture.as_image_copy(),
			wgpu::TexelCopyBufferInfo {
				buffer: &readback,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(256),
					rows_per_image: Some(64),
				},
			},
			size,
		);
		queue.submit([encoder.finish()]);
		let slice = readback.slice(..);
		slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
		device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
		let data = slice.get_mapped_range().to_vec();
		Some(data)
	}

	fn px(data: &[u8], x: usize, y: usize) -> [u8; 4] {
		let i = y * 256 + x * 4;
		[data[i], data[i + 1], data[i + 2], data[i + 3]]
	}

	#[test]
	fn checker_bounds_and_outside() {
		// 1:1 zoom, document 512 × 512, viewport centred on document (16, 16):
		// screen (0..16) is outside the document, (16..) inside.
		let view = ViewTransform {
			zoom: 1.0,
			center_x: 16.0,
			center_y: 16.0,
		};
		let Some(data) = render(view, (512, 512)) else {
			eprintln!("no GPU adapter: test skipped");
			return;
		};
		assert_eq!(px(&data, 2, 2), [40, 40, 40, 255], "outside the document");
		let bounds = px(&data, 15, 40);
		assert!(
			bounds[0] == 0 && bounds[1] > 200 && bounds[2] > 200,
			"cyan bounds just outside x = 0, got {bounds:?}"
		);
		// Inside, away from the major line at x = 0 / y = 0: first checker cell.
		let inside = px(&data, 40, 40);
		assert_ne!(inside, [40, 40, 40, 255], "inside the document is not the outside colour");
		assert!(inside[0] > 140, "checker grey, got {inside:?}");
	}

	#[test]
	fn neighbouring_checker_cells_differ() {
		// 1/8 zoom: 64 screen px = 512 document px, so the viewport spans two
		// 256 px cells in each direction around the centre (384, 384).
		let view = ViewTransform {
			zoom: 0.125,
			center_x: 384.0,
			center_y: 384.0,
		};
		let Some(data) = render(view, (1024, 1024)) else {
			eprintln!("no GPU adapter: test skipped");
			return;
		};
		// Screen x 8 → doc 192 (cell 0); x 40 → doc 448 (cell 1); same row.
		assert_ne!(px(&data, 8, 8), px(&data, 40, 8));
		assert_eq!(px(&data, 8, 8), px(&data, 40, 40), "diagonal cells match");
	}
}
