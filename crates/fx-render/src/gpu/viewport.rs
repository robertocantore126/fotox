//! Viewport pass: draws a [`FramePlan`] into the viewport texture.
//!
//! Clear to the background colour, draw the transparency checkerboard over
//! the document rectangle, then every tile quad (premultiplied, source-over).
//!
//! The last step of the pipeline is the display transform (M4-T02): the tile
//! colour goes through a 33³ [`fx_color::Lut3d`] in straight alpha, so what
//! the user sees is the document in the monitor's colour space. The LUT is
//! built on the engine thread; this renderer only uploads it (a 280 KB
//! texture write) when its key changes, and skips it entirely when the
//! document and the monitor profiles are the same.

use bytemuck::{Pod, Zeroable};
use fx_color::{LUT_GRID, Lut3d};

use crate::frame::FramePlan;
use crate::overlay::OverlayVertex;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Globals {
	size: [f32; 2],
	nearest: u32,
	/// 1 = run the composite through the display LUT.
	apply_lut: u32,
	/// 1.0 = paint out-of-gamut colours grey (the LUT's alpha flag, M4-T04).
	gamut_warning: f32,
	_unused: [f32; 3],
	doc_rect: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GpuDraw {
	src: [f32; 4],
	dst: [f32; 4],
	slot: u32,
	_pad: [u32; 3],
}

/// Uniforms of the overlay pass (M5-T02): viewport size and the animation
/// time for the marching ants.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct OverlayGlobals {
	size: [f32; 2],
	time: f32,
	_pad: f32,
}

/// One overlay vertex as the shader's `Vertex` (matching padding: WGSL aligns
/// `vec2` to 8 and `vec4` to 16, giving a 48-byte stride).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GpuOverlayVertex {
	pos: [f32; 2],
	_pad0: [f32; 2],
	color: [f32; 4],
	arc: f32,
	ants: f32,
	_pad1: [f32; 2],
}

/// Background outside the document (Photoshop's default dark grey).
pub const BACKGROUND: wgpu::Color = wgpu::Color {
	r: 0.157,
	g: 0.157,
	b: 0.157,
	a: 1.0,
};

pub struct ViewportRenderer {
	device: wgpu::Device,
	queue: wgpu_sync::Queue,
	pipeline: wgpu::RenderPipeline,
	layout: wgpu::BindGroupLayout,
	globals: wgpu::Buffer,
	draws: wgpu::Buffer,
	linear: wgpu::Sampler,
	nearest: wgpu::Sampler,
	/// The display LUT, `Rgba16Float` 33³. Always bound; only sampled when
	/// `apply_lut` is 1, so a frame without a LUT binds the 1×1×1 placeholder.
	lut: wgpu::Texture,
	lut_view: wgpu::TextureView,
	placeholder: wgpu::TextureView,
	/// Key of the LUT currently in `lut` (so an unchanged profile is not
	/// re-uploaded every frame).
	lut_key: Option<u64>,
	/// Gamut warning on (needs a proof LUT, M4-T04).
	gamut_warning: bool,
	/// Overlay pass (M5-T02): ants, lasso, brush outline, handles.
	overlay_pipeline: wgpu::RenderPipeline,
	overlay_layout: wgpu::BindGroupLayout,
	overlay_globals: wgpu::Buffer,
	overlay_verts: wgpu::Buffer,
}

impl ViewportRenderer {
	/// `format` must be a non-sRGB format (see viewport.wgsl).
	pub fn new(device: &wgpu::Device, queue: &wgpu_sync::Queue, format: wgpu::TextureFormat) -> Self {
		assert!(!format.is_srgb(), "viewport target must store encoded values as-is");
		let module = device.create_shader_module(wgpu::include_wgsl!("viewport.wgsl"));
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("fx-viewport"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
						view_dimension: wgpu::TextureViewDimension::D2Array,
						multisampled: false,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 2,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 3,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 4,
					visibility: wgpu::ShaderStages::VERTEX,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Storage { read_only: true },
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 5,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
						view_dimension: wgpu::TextureViewDimension::D3,
						multisampled: false,
					},
					count: None,
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("fx-viewport"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("fx-viewport"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &module,
				entry_point: Some("vs_main"),
				compilation_options: Default::default(),
				buffers: &[],
			},
			fragment: Some(wgpu::FragmentState {
				module: &module,
				entry_point: Some("fs_main"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format,
					blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			primitive: wgpu::PrimitiveState::default(),
			depth_stencil: None,
			multisample: wgpu::MultisampleState::default(),
			multiview_mask: None,
			cache: None,
		});
		// Overlay pass (M5-T02): screen-space vertices from a storage buffer.
		let overlay_module = device.create_shader_module(wgpu::include_wgsl!("overlay.wgsl"));
		let overlay_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("fx-overlay"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::VERTEX,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Storage { read_only: true },
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
			],
		});
		let overlay_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("fx-overlay"),
			bind_group_layouts: &[Some(&overlay_layout)],
			immediate_size: 0,
		});
		let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("fx-overlay"),
			layout: Some(&overlay_pipeline_layout),
			vertex: wgpu::VertexState {
				module: &overlay_module,
				entry_point: Some("vs_main"),
				compilation_options: Default::default(),
				buffers: &[],
			},
			fragment: Some(wgpu::FragmentState {
				module: &overlay_module,
				entry_point: Some("fs_main"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format,
					blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			primitive: wgpu::PrimitiveState::default(),
			depth_stencil: None,
			multisample: wgpu::MultisampleState::default(),
			multiview_mask: None,
			cache: None,
		});
		let sampler = |filter| {
			device.create_sampler(&wgpu::SamplerDescriptor {
				label: Some("fx-viewport"),
				mag_filter: filter,
				min_filter: filter,
				..Default::default()
			})
		};
		let lut = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("fx-display-lut"),
			size: wgpu::Extent3d {
				width: LUT_GRID as u32,
				height: LUT_GRID as u32,
				depth_or_array_layers: LUT_GRID as u32,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D3,
			format: wgpu::TextureFormat::Rgba16Float,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		let lut_view = lut.create_view(&wgpu::TextureViewDescriptor {
			dimension: Some(wgpu::TextureViewDimension::D3),
			..Default::default()
		});
		// Bound in place of the LUT while `apply_lut` is 0 (a 3D binding must
		// always be present).
		let placeholder = device
			.create_texture(&wgpu::TextureDescriptor {
				label: Some("fx-display-lut-placeholder"),
				size: wgpu::Extent3d {
					width: 1,
					height: 1,
					depth_or_array_layers: 1,
				},
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D3,
				format: wgpu::TextureFormat::Rgba16Float,
				usage: wgpu::TextureUsages::TEXTURE_BINDING,
				view_formats: &[],
			})
			.create_view(&wgpu::TextureViewDescriptor {
				dimension: Some(wgpu::TextureViewDimension::D3),
				..Default::default()
			});
		Self {
			device: device.clone(),
			queue: queue.clone(),
			pipeline,
			layout,
			globals: device.create_buffer(&wgpu::BufferDescriptor {
				label: Some("fx-viewport-globals"),
				size: std::mem::size_of::<Globals>() as u64,
				usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
				mapped_at_creation: false,
			}),
			draws: draws_buffer(device, 256),
			linear: sampler(wgpu::FilterMode::Linear),
			nearest: sampler(wgpu::FilterMode::Nearest),
			lut,
			lut_view,
			placeholder,
			lut_key: None,
			gamut_warning: false,
			overlay_pipeline,
			overlay_layout,
			overlay_globals: device.create_buffer(&wgpu::BufferDescriptor {
				label: Some("fx-overlay-globals"),
				size: std::mem::size_of::<OverlayGlobals>() as u64,
				usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
				mapped_at_creation: false,
			}),
			overlay_verts: overlay_buffer(device, 256),
		}
	}

	/// Paint the colours the proof LUT flags as out of gamut grey (M4-T04).
	pub fn set_gamut_warning(&mut self, on: bool) {
		self.gamut_warning = on;
	}

	/// Upload the display LUT if it is a new one. `None` = document and monitor
	/// profiles are the same: the composite reaches the screen untouched (C1).
	pub fn set_display_lut(&mut self, lut: Option<&Lut3d>) {
		let Some(lut) = lut else {
			self.lut_key = None;
			return;
		};
		if self.lut_key == Some(lut.key()) {
			return;
		}
		self.queue.write_texture(
			wgpu::TexelCopyTextureInfo {
				texture: &self.lut,
				mip_level: 0,
				origin: wgpu::Origin3d::ZERO,
				aspect: wgpu::TextureAspect::All,
			},
			lut.bytes(),
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(LUT_GRID as u32 * 8),
				rows_per_image: Some(LUT_GRID as u32),
			},
			wgpu::Extent3d {
				width: LUT_GRID as u32,
				height: LUT_GRID as u32,
				depth_or_array_layers: LUT_GRID as u32,
			},
		);
		self.lut_key = Some(lut.key());
	}

	/// Record the viewport pass. `tiles` = `GpuCompositor::composite_view()`.
	/// `zoom >= 1` draws hard pixels like Photoshop. The display transform is
	/// whatever the last [`set_display_lut`] call chose.
	///
	/// `overlay` is the tool overlay, already tessellated to screen pixels
	/// (M5-T02, [`crate::overlay::tessellate`]); it is drawn in the same pass,
	/// after the tiles, and `time` (seconds) animates the marching ants.
	///
	/// [`set_display_lut`]: Self::set_display_lut
	#[allow(clippy::too_many_arguments)]
	pub fn render(
		&mut self,
		encoder: &mut wgpu::CommandEncoder,
		target: &wgpu::TextureView,
		size: (u32, u32),
		plan: &FramePlan,
		zoom: f64,
		tiles: &wgpu::TextureView,
		overlay: &[OverlayVertex],
		time: f32,
	) {
		let draws: Vec<GpuDraw> = plan
			.draws
			.iter()
			.map(|d| GpuDraw {
				src: d.src,
				dst: d.dst,
				slot: d.slot,
				_pad: [0; 3],
			})
			.collect();
		let bytes: &[u8] = bytemuck::cast_slice(&draws);
		if self.draws.size() < bytes.len() as u64 {
			self.draws = draws_buffer(&self.device, (bytes.len() as u64).next_power_of_two());
		}
		if !bytes.is_empty() {
			self.queue.write_buffer(&self.draws, 0, bytes);
		}
		self.queue.write_buffer(
			&self.globals,
			0,
			bytemuck::bytes_of(&Globals {
				size: [size.0 as f32, size.1 as f32],
				nearest: (zoom >= 1.0) as u32,
				apply_lut: self.lut_key.is_some() as u32,
				gamut_warning: if self.gamut_warning && self.lut_key.is_some() { 1.0 } else { 0.0 },
				doc_rect: plan.doc_rect,
				..Default::default()
			}),
		);
		let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("fx-viewport"),
			layout: &self.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: self.globals.as_entire_binding(),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(tiles),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::Sampler(&self.linear),
				},
				wgpu::BindGroupEntry {
					binding: 3,
					resource: wgpu::BindingResource::Sampler(&self.nearest),
				},
				wgpu::BindGroupEntry {
					binding: 4,
					resource: self.draws.as_entire_binding(),
				},
				wgpu::BindGroupEntry {
					binding: 5,
					resource: wgpu::BindingResource::TextureView(if self.lut_key.is_some() { &self.lut_view } else { &self.placeholder }),
				},
			],
		});
		let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
			label: Some("fx-viewport"),
			color_attachments: &[Some(wgpu::RenderPassColorAttachment {
				view: target,
				depth_slice: None,
				resolve_target: None,
				ops: wgpu::Operations {
					load: wgpu::LoadOp::Clear(BACKGROUND),
					store: wgpu::StoreOp::Store,
				},
			})],
			depth_stencil_attachment: None,
			timestamp_writes: None,
			occlusion_query_set: None,
			multiview_mask: None,
		});
		pass.set_pipeline(&self.pipeline);
		pass.set_bind_group(0, &bind_group, &[]);
		pass.draw(0..6, 0..(1 + draws.len() as u32));

		// Overlay (M5-T02), after the tiles, in the same pass.
		if !overlay.is_empty() {
			let verts: Vec<GpuOverlayVertex> = overlay
				.iter()
				.map(|v| GpuOverlayVertex {
					pos: v.pos,
					color: v.color,
					arc: v.arc,
					ants: v.ants,
					..Default::default()
				})
				.collect();
			let bytes: &[u8] = bytemuck::cast_slice(&verts);
			if self.overlay_verts.size() < bytes.len() as u64 {
				self.overlay_verts = overlay_buffer(&self.device, (bytes.len() as u64).next_power_of_two());
			}
			self.queue.write_buffer(&self.overlay_verts, 0, bytes);
			self.queue.write_buffer(
				&self.overlay_globals,
				0,
				bytemuck::bytes_of(&OverlayGlobals {
					size: [size.0 as f32, size.1 as f32],
					time,
					_pad: 0.0,
				}),
			);
			let overlay_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("fx-overlay"),
				layout: &self.overlay_layout,
				entries: &[
					wgpu::BindGroupEntry {
						binding: 0,
						resource: self.overlay_globals.as_entire_binding(),
					},
					wgpu::BindGroupEntry {
						binding: 1,
						resource: self.overlay_verts.as_entire_binding(),
					},
				],
			});
			pass.set_pipeline(&self.overlay_pipeline);
			pass.set_bind_group(0, &overlay_bind, &[]);
			pass.draw(0..verts.len() as u32, 0..1);
		}
	}
}

fn draws_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some("fx-viewport-draws"),
		size: size.max(256),
		usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
		mapped_at_creation: false,
	})
}

fn overlay_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some("fx-overlay-verts"),
		size: size.max(256),
		usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
		mapped_at_creation: false,
	})
}
