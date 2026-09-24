//! The final composite pass: everything the window shows is drawn by this one
//! full-screen triangle.
//!
//! Ported from `reference/graphite-desktop/src/render/state.rs`, with Graphite's
//! `WgpuExecutor` and its Vello overlay stage removed (`docs/GRAPHITE.md` §2
//! says drop both; overlays become part of the engine's viewport texture).
//!
//! The viewport texture is drawn inside the rectangle the UI reports with
//! `viewport_bounds` (the transparent hole in the page); outside it, and
//! wherever the viewport texture is transparent, the background colour shows.
//! The viewport texture itself comes from the engine's render thread
//! (`EngineOutput::ViewportFrame`); until the first one arrives the slot is
//! transparent.

use anyhow::Result;

use crate::gpu::Gpu;
use crate::window::Window;

/// Uniform block handed to the composite shader.
///
/// Field order and padding must match `Immediates` in `composite.wgsl`: in the
/// `immediate` address space `background_color`'s `vec4` alignment pushes it to
/// offset 32, so `_pad` covers the gap after `ui_scale`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Immediates {
	viewport_scale: [f32; 2],
	viewport_offset: [f32; 2],
	ui_scale: [f32; 2],
	_pad: [f32; 2],
	background_color: [f32; 4],
}

/// The colour of everything the UI does not paint. `#282828`.
const BACKGROUND: [f32; 4] = [0x28 as f32 / 0xff as f32, 0x28 as f32 / 0xff as f32, 0x28 as f32 / 0xff as f32, 1.0];

/// The viewport rectangle in physical window pixels, as reported by the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewportBounds {
	pub(crate) x: u32,
	pub(crate) y: u32,
	pub(crate) width: u32,
	pub(crate) height: u32,
}

impl ViewportBounds {
	/// Round the UI's floating-point rectangle to whole physical pixels.
	/// Negative or non-finite values are clamped to zero.
	pub(crate) fn from_physical(x: f64, y: f64, width: f64, height: f64) -> Self {
		let px = |v: f64| if v.is_finite() && v > 0.0 { v.round() as u32 } else { 0 };
		Self {
			x: px(x),
			y: px(y),
			width: px(width),
			height: px(height),
		}
	}
}

/// Surface, composite pipeline and the textures it samples.
pub(crate) struct RenderState {
	surface: wgpu_sync::Surface,
	device: wgpu::Device,
	queue: wgpu_sync::Queue,
	config: wgpu::SurfaceConfiguration,
	pipeline: wgpu::RenderPipeline,
	/// Bound in place of a texture that does not exist yet (overlays; the
	/// viewport before the UI reports its rectangle).
	transparent: wgpu::Texture,
	sampler: wgpu::Sampler,
	desired_width: u32,
	desired_height: u32,
	/// Where the viewport hole is; `None` until the UI reports it.
	viewport_bounds: Option<ViewportBounds>,
	viewport_texture: Option<wgpu::Texture>,
	ui_texture: Option<wgpu::Texture>,
	bind_group: Option<wgpu::BindGroup>,
	outdated: bool,
}

impl RenderState {
	/// Create the surface and the composite pipeline for `window`.
	pub(crate) fn new(window: &Window, gpu: &Gpu) -> Result<Self> {
		let size = window.surface_size();
		let surface = window.create_surface(&gpu.instance)?;

		let surface_caps = surface.get_capabilities(&gpu.adapter);
		let format = surface_caps
			.formats
			.iter()
			.find(|f| f.is_srgb())
			.copied()
			.unwrap_or_else(|| surface_caps.formats[0]);

		let config = wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format,
			width: size.width,
			height: size.height,
			present_mode: surface_caps.present_modes[0],
			alpha_mode: surface_caps.alpha_modes[0],
			view_formats: vec![],
			desired_maximum_frame_latency: 1,
		};
		surface.configure(&gpu.device, &config);

		let transparent = gpu.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("transparent_fallback"),
			size: wgpu::Extent3d {
				width: 1,
				height: 1,
				depth_or_array_layers: 1,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Bgra8UnormSrgb,
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
			view_formats: &[],
		});

		let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
			address_mode_u: wgpu::AddressMode::ClampToEdge,
			address_mode_v: wgpu::AddressMode::ClampToEdge,
			address_mode_w: wgpu::AddressMode::ClampToEdge,
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Nearest,
			mipmap_filter: wgpu::MipmapFilterMode::Nearest,
			..Default::default()
		});

		let shader = gpu.device.create_shader_module(wgpu::include_wgsl!("composite.wgsl"));

		let texture_layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("composite_textures"),
			entries: &[
				texture_entry(0),
				texture_entry(1),
				texture_entry(2),
				wgpu::BindGroupLayoutEntry {
					binding: 3,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
			],
		});

		let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("composite_pipeline_layout"),
			bind_group_layouts: &[Some(&texture_layout)],
			immediate_size: size_of::<Immediates>() as u32,
		});

		let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("composite_pipeline"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &shader,
				entry_point: Some("vs_main"),
				buffers: &[],
				compilation_options: Default::default(),
			},
			fragment: Some(wgpu::FragmentState {
				module: &shader,
				entry_point: Some("fs_main"),
				targets: &[Some(wgpu::ColorTargetState {
					format: config.format,
					blend: Some(wgpu::BlendState::REPLACE),
					write_mask: wgpu::ColorWrites::ALL,
				})],
				compilation_options: Default::default(),
			}),
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleList,
				strip_index_format: None,
				front_face: wgpu::FrontFace::Ccw,
				cull_mode: Some(wgpu::Face::Back),
				polygon_mode: wgpu::PolygonMode::Fill,
				unclipped_depth: false,
				conservative: false,
			},
			depth_stencil: None,
			multisample: wgpu::MultisampleState {
				count: 1,
				mask: !0,
				alpha_to_coverage_enabled: false,
			},
			multiview_mask: None,
			cache: None,
		});

		let mut state = Self {
			surface,
			device: gpu.device.clone(),
			queue: gpu.queue.clone(),
			config,
			pipeline,
			transparent,
			sampler,
			desired_width: size.width,
			desired_height: size.height,
			viewport_bounds: None,
			viewport_texture: None,
			ui_texture: None,
			bind_group: None,
			outdated: true,
		};
		state.update_bind_group();
		Ok(state)
	}

	/// Record the new window size. Applied on the next presented frame, so a
	/// resize that arrives mid-frame cannot reconfigure the surface under it.
	pub(crate) fn resize(&mut self, width: u32, height: u32) {
		if width == self.desired_width && height == self.desired_height {
			return;
		}
		self.desired_width = width;
		self.desired_height = height;
		self.outdated = true;
	}

	/// Move the viewport to `bounds` (physical window pixels). Until the
	/// engine renders at the new size, the previous texture is stretched over
	/// the new rectangle.
	pub(crate) fn set_viewport_bounds(&mut self, bounds: ViewportBounds) {
		if self.viewport_bounds == Some(bounds) {
			return;
		}
		self.viewport_bounds = Some(bounds);
		self.outdated = true;
	}

	/// Composite this engine frame in the viewport from now on.
	pub(crate) fn bind_viewport_texture(&mut self, texture: wgpu::Texture) {
		if self.viewport_texture.as_ref() == Some(&texture) {
			self.outdated = true;
			return;
		}
		self.viewport_texture = Some(texture);
		self.update_bind_group();
	}

	/// Composite the newest UI frame.
	pub(crate) fn bind_ui_texture(&mut self, texture: wgpu::Texture) {
		if self.ui_texture.as_ref() == Some(&texture) {
			self.outdated = true;
			return;
		}
		self.ui_texture = Some(texture);
		self.update_bind_group();
	}

	/// Draw and present one frame.
	pub(crate) fn render(&mut self, window: &Window) -> Result<(), RenderError> {
		if !self.outdated {
			return Ok(());
		}

		if self.desired_width > 0 && self.desired_height > 0 && (self.config.width != self.desired_width || self.config.height != self.desired_height) {
			self.config.width = self.desired_width;
			self.config.height = self.desired_height;
			self.surface.configure(&self.device, &self.config);
		}

		// CEF renders at the size it was last told about, so between a resize
		// and the next UI frame the texture is the wrong size. Stretch it for
		// this frame and ask the UI to re-render (RenderError::OutdatedUiTexture).
		let ui_scale = match &self.ui_texture {
			Some(texture) if self.desired_width != texture.width() || self.desired_height != texture.height() => Some([
				self.desired_width as f32 / texture.width() as f32,
				self.desired_height as f32 / texture.height() as f32,
			]),
			_ => None,
		};

		let (surface_texture, suboptimal) = match self.surface.get_current_texture(&self.queue) {
			wgpu_sync::CurrentSurfaceTexture::Success(texture) => (texture, false),
			wgpu_sync::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
			wgpu_sync::CurrentSurfaceTexture::Occluded => return Ok(()),
			wgpu_sync::CurrentSurfaceTexture::Lost => return Err(RenderError::SurfaceLost),
			wgpu_sync::CurrentSurfaceTexture::Outdated => return Err(RenderError::SurfaceOutdated),
			wgpu_sync::CurrentSurfaceTexture::Timeout => return Err(RenderError::SurfaceTimeout),
			wgpu_sync::CurrentSurfaceTexture::Validation => return Err(RenderError::SurfaceValidation),
		};

		let view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
		let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("composite_encoder"),
		});

		{
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("composite_pass"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view: &view,
					resolve_target: None,
					ops: wgpu::Operations {
						load: wgpu::LoadOp::Clear(wgpu::Color {
							r: BACKGROUND[0] as f64,
							g: BACKGROUND[1] as f64,
							b: BACKGROUND[2] as f64,
							a: 1.0,
						}),
						store: wgpu::StoreOp::Store,
					},
					depth_slice: None,
				})],
				depth_stencil_attachment: None,
				occlusion_query_set: None,
				timestamp_writes: None,
				multiview_mask: None,
			});

			let (viewport_offset, viewport_scale) = self.viewport_transform();
			pass.set_pipeline(&self.pipeline);
			pass.set_immediates(
				0,
				bytemuck::bytes_of(&Immediates {
					viewport_scale,
					viewport_offset,
					ui_scale: ui_scale.unwrap_or([1.0, 1.0]),
					_pad: [0.0, 0.0],
					background_color: BACKGROUND,
				}),
			);
			if let Some(bind_group) = &self.bind_group {
				pass.set_bind_group(0, bind_group, &[]);
				pass.draw(0..3, 0..1);
			}
		}

		// `queue` is the guard held by the surface texture: submitting through it
		// is what keeps this submit ordered against surface reconfiguration.
		surface_texture.queue.submit(std::iter::once(encoder.finish()));
		window.pre_present_notify();
		surface_texture.present();

		if suboptimal {
			self.surface.configure(&self.device, &self.config);
		}

		if ui_scale.is_some() {
			return Err(RenderError::OutdatedUiTexture);
		}
		self.outdated = false;
		Ok(())
	}

	/// The shader's viewport mapping, `(offset, scale)`: window texture
	/// coordinates `t` map to viewport coordinates `(t - offset) * scale`.
	/// Computed from the *current* window size, so a window resize that
	/// arrives before the UI's next `viewport_bounds` keeps the hole in place.
	/// Without bounds the viewport spans the window (and is transparent).
	fn viewport_transform(&self) -> ([f32; 2], [f32; 2]) {
		match self.viewport_bounds {
			Some(bounds) if bounds.width > 0 && bounds.height > 0 && self.desired_width > 0 && self.desired_height > 0 => {
				let (window_w, window_h) = (self.desired_width as f32, self.desired_height as f32);
				(
					[bounds.x as f32 / window_w, bounds.y as f32 / window_h],
					[window_w / bounds.width as f32, window_h / bounds.height as f32],
				)
			}
			_ => ([0.0, 0.0], [1.0, 1.0]),
		}
	}

	/// Rebuild the bind group from the current viewport, overlay and UI textures.
	fn update_bind_group(&mut self) {
		self.outdated = true;
		let viewport = match &self.viewport_texture {
			Some(texture) => texture.create_view(&wgpu::TextureViewDescriptor::default()),
			None => self.transparent.create_view(&wgpu::TextureViewDescriptor::default()),
		};
		// Overlays are drawn into the engine's viewport texture (GRAPHITE.md §2),
		// so this slot stays transparent.
		let overlays = self.transparent.create_view(&wgpu::TextureViewDescriptor::default());
		let ui = match &self.ui_texture {
			Some(texture) => texture.create_view(&wgpu::TextureViewDescriptor::default()),
			None => self.transparent.create_view(&wgpu::TextureViewDescriptor::default()),
		};

		self.bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("composite_bind_group"),
			layout: &self.pipeline.get_bind_group_layout(0),
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&viewport),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(&overlays),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::TextureView(&ui),
				},
				wgpu::BindGroupEntry {
					binding: 3,
					resource: wgpu::BindingResource::Sampler(&self.sampler),
				},
			],
		}));
	}
}

/// A filterable `texture_2d<f32>` bound to a fragment shader slot.
fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
	wgpu::BindGroupLayoutEntry {
		binding,
		visibility: wgpu::ShaderStages::FRAGMENT,
		ty: wgpu::BindingType::Texture {
			multisampled: false,
			view_dimension: wgpu::TextureViewDimension::D2,
			sample_type: wgpu::TextureSampleType::Float { filterable: true },
		},
		count: None,
	}
}

/// Why a frame could not be presented. The caller decides whether the error is
/// fatal; a lost or outdated surface is not.
#[derive(Debug)]
pub(crate) enum RenderError {
	/// The UI texture is a different size from the window: presented stretched
	/// once, and the UI has been asked for a frame that matches.
	OutdatedUiTexture,
	SurfaceLost,
	SurfaceOutdated,
	SurfaceTimeout,
	SurfaceValidation,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn bounds_round_to_whole_physical_pixels() {
		// 125 % scaling: 44.2 CSS px × 1.25 = 55.25 physical px.
		assert_eq!(
			ViewportBounds::from_physical(55.25, 100.5, 1199.6, 0.4),
			ViewportBounds {
				x: 55,
				y: 101,
				width: 1200,
				height: 0
			}
		);
		let clamped = ViewportBounds::from_physical(-3.0, f64::NAN, f64::INFINITY, 10.0);
		assert_eq!((clamped.x, clamped.y, clamped.width, clamped.height), (0, 0, 0, 10));
	}
}
