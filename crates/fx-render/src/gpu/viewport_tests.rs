//! Viewport pass vs the display transform (M4-T02).
//!
//! The pass is driven directly with a hand-built [`FramePlan`] and a tile
//! texture we fill ourselves, so the test measures exactly what the shader
//! does with the document colour: the identity shortcut (criterion C1) and the
//! 3D LUT sampling (coordinates from docs/tasks/SNIPPETS.md §14) against the
//! CPU sampler of the same table.
//!
//! Skipped when there is no GPU adapter, like the compositor tests.

use fx_color::{Intent, Lut3d, display_lut};
use fx_core::ColorProfile;
use half::f16;

use crate::frame::{FramePlan, TileDraw};
use crate::gpu::viewport::ViewportRenderer;
use crate::overlay::{Overlay, OverlayItem, OverlayStyle, OverlayVertex, tessellate};
use crate::test_pattern::VIEWPORT_FORMAT;
use crate::viewport::{ViewTransform, ViewportSize};

const SIZE: u32 = 256;

fn gpu() -> Option<(wgpu::Device, wgpu_sync::Queue)> {
	let instance = wgpu_sync::Instance::new(wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()));
	let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
	let limits = adapter.limits();
	pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
		label: Some("fx-render-viewport-tests"),
		required_limits: limits,
		..Default::default()
	}))
	.ok()
}

macro_rules! gpu_or_skip {
	() => {
		match gpu() {
			Some(g) => g,
			None => {
				eprintln!("no GPU adapter: test skipped");
				return;
			}
		}
	};
}

/// A `SIZE × SIZE` `Rgba16Float` array texture with one layer holding one
/// premultiplied colour — the shape `composite_view()` gives the viewport.
fn tiles_texture(device: &wgpu::Device, queue: &wgpu_sync::Queue, premultiplied: [f32; 4]) -> wgpu::TextureView {
	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("fx-test-tiles"),
		size: wgpu::Extent3d {
			width: SIZE,
			height: SIZE,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: wgpu::TextureFormat::Rgba16Float,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	});
	let texels: Vec<[u16; 4]> = (0..SIZE * SIZE).map(|_| premultiplied.map(|c| f16::from_f32(c).to_bits())).collect();
	queue.write_texture(
		wgpu::TexelCopyTextureInfo {
			texture: &texture,
			mip_level: 0,
			origin: wgpu::Origin3d::ZERO,
			aspect: wgpu::TextureAspect::All,
		},
		bytemuck::cast_slice(&texels),
		wgpu::TexelCopyBufferLayout {
			offset: 0,
			bytes_per_row: Some(SIZE * 8),
			rows_per_image: Some(SIZE),
		},
		wgpu::Extent3d {
			width: SIZE,
			height: SIZE,
			depth_or_array_layers: 1,
		},
	);
	// `composite_view()` is a 2D array view; binding 1 expects one.
	texture.create_view(&wgpu::TextureViewDescriptor {
		dimension: Some(wgpu::TextureViewDimension::D2Array),
		..Default::default()
	})
}

/// `FramePlan` with the one tile drawn over the whole viewport at 1:1.
fn plan() -> FramePlan {
	let rect = [0.0, 0.0, SIZE as f32, SIZE as f32];
	FramePlan {
		draws: vec![TileDraw {
			slot: 0,
			src: [0.0, 0.0, 1.0, 1.0],
			dst: rect,
			level: 0,
		}],
		requests: Vec::new(),
		complete: true,
		doc_rect: rect,
	}
}

/// Render the pass and read the target back as RGBA8 rows.
fn render(device: &wgpu::Device, queue: &wgpu_sync::Queue, tiles: &wgpu::TextureView, lut: Option<&Lut3d>, overlay: &[OverlayVertex]) -> Vec<[u8; 4]> {
	let mut renderer = ViewportRenderer::new(device, queue, VIEWPORT_FORMAT);
	let target = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("fx-test-viewport"),
		size: wgpu::Extent3d {
			width: SIZE,
			height: SIZE,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: VIEWPORT_FORMAT,
		usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
		view_formats: &[],
	});
	let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
		label: Some("fx-test-viewport"),
	});
	renderer.set_display_lut(lut);
	renderer.render(
		&mut encoder,
		&target.create_view(&Default::default()),
		(SIZE, SIZE),
		&plan(),
		1.0,
		tiles,
		overlay,
		0.0,
	);
	let readback = device.create_buffer(&wgpu::BufferDescriptor {
		label: Some("fx-test-viewport-readback"),
		size: (SIZE * SIZE * 4) as u64,
		usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
		mapped_at_creation: false,
	});
	encoder.copy_texture_to_buffer(
		target.as_image_copy(),
		wgpu::TexelCopyBufferInfo {
			buffer: &readback,
			layout: wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(SIZE * 4),
				rows_per_image: Some(SIZE),
			},
		},
		wgpu::Extent3d {
			width: SIZE,
			height: SIZE,
			depth_or_array_layers: 1,
		},
	);
	queue.submit([encoder.finish()]);
	let slice = readback.slice(..);
	slice.map_async(wgpu::MapMode::Read, |r| r.expect("readback map failed"));
	device.poll(wgpu::PollType::wait_indefinitely()).expect("device lost during readback");
	slice.get_mapped_range().chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()
}

fn to_u8(v: f64) -> u8 {
	(v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[test]
fn without_a_lut_the_document_reaches_the_screen_untouched() {
	// Criterion C1 on the GPU side: an sRGB document on an sRGB monitor
	// (no LUT) is displayed bit-exactly.
	let (device, queue) = gpu_or_skip!();
	let tiles = tiles_texture(&device, &queue, [0.5, 0.5, 0.5, 1.0]);
	let pixels = render(&device, &queue, &tiles, None, &[]);
	assert_eq!(pixels.len(), (SIZE * SIZE) as usize);
	// The document's 0.5 reaches the screen as 128 ± 1: the remaining step is
	// the float → 8-bit unorm conversion of the swapchain format, not the
	// display transform (which is what C1 is about).
	let expected = [to_u8(0.5), to_u8(0.5), to_u8(0.5), 255];
	for (i, p) in pixels.iter().enumerate() {
		assert_eq!(*p, pixels[0], "pixel {i}: the tile is uniform, the display must be too");
		for c in 0..4 {
			assert!(
				(i32::from(p[c]) - i32::from(expected[c])).abs() <= 1,
				"pixel {i} ({p:?}) is not the document's value ({expected:?})"
			);
		}
	}
}

#[test]
fn the_display_lut_maps_the_tile_colour() {
	let (device, queue) = gpu_or_skip!();
	// A saturated Adobe RGB colour: sRGB cannot show it unchanged, so the
	// result must differ from the un-transformed bytes.
	let colour = [0.9, 0.25, 0.4];
	let lut = display_lut(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).expect("transform");
	let tiles = tiles_texture(&device, &queue, [colour[0], colour[1], colour[2], 1.0]);
	let mapped = render(&device, &queue, &tiles, Some(&lut), &[]);
	let plain = render(&device, &queue, &tiles, None, &[]);
	let expected = lut.sample(colour.map(f64::from)).map(to_u8);
	let got = mapped[0];
	for c in 0..3 {
		assert!(
			(i32::from(got[c]) - i32::from(expected[c])).abs() <= 2,
			"channel {c}: shader {} vs CPU LUT {} for {colour:?}",
			got[c],
			expected[c]
		);
	}
	assert!(
		(0..3).any(|c| (i32::from(got[c]) - i32::from(plain[0][c])).abs() > 2),
		"the LUT changed nothing: {got:?} vs {:?}",
		plain[0]
	);
}

#[test]
fn the_lut_runs_on_straight_colour_before_the_checkerboard() {
	let (device, queue) = gpu_or_skip!();
	// Half-transparent saturated colour over the light checkerboard: the
	// shader must un-premultiply, transform, premultiply, then blend.
	let colour = [0.9, 0.25, 0.4];
	let alpha = 0.5f32;
	let tiles = tiles_texture(&device, &queue, [colour[0] * alpha, colour[1] * alpha, colour[2] * alpha, alpha]);
	let lut = display_lut(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).expect("transform");
	let pixels = render(&device, &queue, &tiles, Some(&lut), &[]);
	// Pixel (10, 10) sits in checkerboard cell (1, 1): odd + odd = even → white.
	let mapped = lut.sample(colour.map(f64::from));
	for c in 0..3 {
		// mapped * alpha over the white checkerboard (the background outside
		// the document is the dark UI colour; inside it is the checkerboard).
		let expected = to_u8(mapped[c] * f64::from(alpha) + 1.0 * (1.0 - f64::from(alpha)));
		let at = pixels[(10 * SIZE + 10) as usize][c];
		assert!(
			(i32::from(at) - i32::from(expected)).abs() <= 3,
			"channel {c}: {at} vs {expected} (mapped {mapped:?})"
		);
	}
}

#[test]
fn an_overlay_line_lands_on_the_expected_pixels() {
	// M5-T02: the overlay pass draws after the tiles, in screen space. At zoom
	// 1 with the view centred on the viewport the document and screen
	// coordinates coincide, so a line at y = 128.5 must light up row 128.
	let (device, queue) = gpu_or_skip!();
	let tiles = tiles_texture(&device, &queue, [0.25, 0.25, 0.25, 1.0]);
	let overlay = Overlay {
		items: vec![OverlayItem::Polyline {
			points: vec![(10.0, 128.5), (200.0, 128.5)],
			closed: false,
			style: OverlayStyle::Solid([1.0, 1.0, 1.0, 1.0]),
		}],
	};
	let view = ViewTransform {
		zoom: 1.0,
		center_x: 128.0,
		center_y: 128.0,
	};
	let viewport = ViewportSize { width: SIZE, height: SIZE };
	let vertices = tessellate(&overlay, &view, viewport);
	let pixels = render(&device, &queue, &tiles, None, &vertices);
	let at = |x: u32, y: u32| pixels[(y * SIZE + x) as usize];
	assert_eq!(at(100, 128), [255, 255, 255, 255], "the line's row");
	let background = at(100, 120);
	for c in 0..3 {
		assert!(
			(i32::from(background[c]) - i32::from(to_u8(0.25))).abs() <= 1,
			"above the line the tile shows through: {background:?}"
		);
	}
	assert!(at(5, 128)[0] < 128, "the line starts at x = 10");
	assert!(at(210, 128)[0] < 128, "the line ends at x = 200");
}
