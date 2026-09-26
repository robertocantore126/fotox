//! GPU compositor vs CPU reference. Runs on any Vulkan/DX12/Metal adapter,
//! including software ones (lavapipe in CI); skipped if there is none.

use std::sync::Arc;

use fx_core::layer::{Adjustment, LevelsChannel};
use fx_core::{BlendMode, Document, Layer, LayerKind};
use fx_tiles::{TileHandle, TileStore};

use super::{CompositorConfig, GpuCompositor, TileOutcome};
use crate::adjust::LutCache;
use crate::program::{TileProgram, build_program};
use crate::reference::render_tile;
use crate::testing::*;

fn gpu() -> Option<(wgpu::Device, wgpu_sync::Queue)> {
	let instance = wgpu_sync::Instance::new(wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()));
	let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
	let limits = adapter.limits();
	let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
		label: Some("fx-render-tests"),
		required_limits: limits,
		..Default::default()
	}))
	.ok()?;
	Some((device, queue))
}

/// The GPU lock (see `testing::one_gpu_test`), a device and its queue, or
/// skip the test when there is no adapter.
macro_rules! gpu_or_skip {
	() => {{
		let one_at_a_time = crate::testing::one_gpu_test();
		match gpu() {
			Some((device, queue)) => (one_at_a_time, device, queue),
			None => {
				eprintln!("no GPU adapter: test skipped");
				return;
			}
		}
	}};
}

fn small_config() -> CompositorConfig {
	CompositorConfig {
		atlas_budget: 256 * 512 * 1024,
		composite_slots: 64,
		upload_budget: 512,
	}
}

fn programs(doc: &Document, luts: &mut LutCache) -> Vec<TileProgram> {
	let (cols, rows) = (doc.width.div_ceil(256), doc.height.div_ceil(256));
	let mut out = Vec::new();
	for ty in 0..rows {
		for tx in 0..cols {
			out.push(build_program(doc, 0, tx, ty, &mut |a| luts.get(a)).unwrap());
		}
	}
	out
}

/// Max abs error and the share of channels above `tol`, GPU vs CPU.
fn compare(gpu: &GpuCompositor, programs: &[TileProgram], outcomes: &[TileOutcome], store: &TileStore) -> (f64, f64) {
	let fetch = |h: &TileHandle| store.get(h).unwrap();
	let mut max_err: f64 = 0.0;
	let mut over = 0usize;
	let mut total = 0usize;
	for (program, outcome) in programs.iter().zip(outcomes) {
		let cpu = render_tile(program, &fetch);
		let gpu_px = match outcome {
			TileOutcome::Ready { slot } => gpu.read_tile(*slot),
			TileOutcome::Empty => vec![[0.0; 4]; cpu.len()],
			TileOutcome::Deferred { .. } => panic!("unexpected deferral"),
		};
		for (c, g) in cpu.iter().zip(&gpu_px) {
			for i in 0..4 {
				let e = (c[i] - g[i] as f64).abs();
				max_err = max_err.max(e);
				if e > 2.0 / 1024.0 {
					over += 1;
				}
				total += 1;
			}
		}
	}
	(max_err, over as f64 / total as f64)
}

const MODES: [BlendMode; 27] = {
	use BlendMode::*;
	[
		Normal,
		Dissolve,
		Darken,
		Multiply,
		ColorBurn,
		LinearBurn,
		DarkerColor,
		Lighten,
		Screen,
		ColorDodge,
		LinearDodge,
		LighterColor,
		Overlay,
		SoftLight,
		HardLight,
		VividLight,
		LinearLight,
		PinLight,
		HardMix,
		Difference,
		Exclusion,
		Subtract,
		Divide,
		Hue,
		Saturation,
		Color,
		Luminosity,
	]
};

#[test]
fn every_blend_mode_matches_the_reference() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	for mode in MODES {
		let mut doc = doc(512, 256);
		let bottom = busy_layer(&mut doc, &store, 11);
		let mut top = busy_layer(&mut doc, &store, 22);
		top.blend = mode;
		top.opacity = 0.8;
		doc.layers.push(Arc::new(bottom));
		doc.layers.push(Arc::new(top));
		let programs = programs(&doc, &mut luts);
		gpu.begin_frame();
		let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
		let (max_err, over) = compare(&gpu, &programs, &outcomes, &store);
		eprintln!("{mode:?}: max error {max_err:.5}, {:.4} % of channels > 2/1024", over * 100.0);
		// f16 storage of inputs: modes with singularities (dodge/burn/divide,
		// hard mix, dissolve thresholds) can flip a few pixels.
		assert!(over < 0.002, "{mode:?}: {:.3} % of channels differ", over * 100.0);
	}
}

#[test]
fn groups_clipping_masks_offsets_adjustments_match() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	let doc = complex_doc(&store);
	let programs = programs(&doc, &mut luts);
	gpu.begin_frame();
	let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
	let (max_err, over) = compare(&gpu, &programs, &outcomes, &store);
	eprintln!("complex stack: max error {max_err:.5}, {:.4} % > 2/1024", over * 100.0);
	assert!(over < 0.002);
}

#[test]
fn hue_saturation_and_brightness_contrast_match_the_reference() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	let mut doc = doc(512, 256);
	let bg = busy_layer(&mut doc, &store, 31);

	let hs_id = doc.allocate_layer_id();
	let mut hs = Layer::new(
		hs_id,
		"Hue/Saturation",
		LayerKind::Adjustment(Adjustment::HueSaturation {
			hue: 47.0,
			saturation: 35.0,
			lightness: -12.0,
			colorize: false,
		}),
	);
	hs.mask = Some(mask(&doc, &store, &|x, y| hash16(x / 20, y / 20, 9)));

	let tint_id = doc.allocate_layer_id();
	let mut tint = Layer::new(
		tint_id,
		"Colorize",
		LayerKind::Adjustment(Adjustment::HueSaturation {
			hue: 210.0,
			saturation: 40.0,
			lightness: 15.0,
			colorize: true,
		}),
	);
	tint.opacity = 0.5;

	let bc_id = doc.allocate_layer_id();
	let bc = Layer::new(
		bc_id,
		"Brightness/Contrast",
		LayerKind::Adjustment(Adjustment::BrightnessContrast {
			brightness: 40.0,
			contrast: 30.0,
			legacy: false,
		}),
	);
	for layer in [bg, hs, tint, bc] {
		doc.layers.push(Arc::new(layer));
	}

	let programs = programs(&doc, &mut luts);
	gpu.begin_frame();
	let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
	let (max_err, over) = compare(&gpu, &programs, &outcomes, &store);
	eprintln!("hue/saturation + brightness/contrast: max error {max_err:.5}, {:.4} % > 2/1024", over * 100.0);
	assert!(over < 0.002, "{:.3} % of channels differ", over * 100.0);
}

#[test]
fn m4_adjustments_match_the_reference() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	let adjustments = [
		Adjustment::Posterize { levels: 5 },
		Adjustment::Threshold { level: 110 },
		Adjustment::GradientMap {
			stops: vec![
				fx_core::GradientStop {
					position: 0.0,
					color: [0.1, 0.0, 0.3],
				},
				fx_core::GradientStop {
					position: 0.6,
					color: [0.9, 0.4, 0.1],
				},
				fx_core::GradientStop {
					position: 1.0,
					color: [1.0, 1.0, 0.8],
				},
			],
			reverse: false,
		},
		Adjustment::ChannelMixer {
			red: [80.0, 30.0, -10.0, 5.0],
			green: [10.0, 90.0, 0.0, 0.0],
			blue: [0.0, 20.0, 70.0, -5.0],
			monochrome: false,
		},
		Adjustment::ChannelMixer {
			red: [40.0, 40.0, 20.0, 0.0],
			green: [0.0, 100.0, 0.0, 0.0],
			blue: [0.0, 0.0, 100.0, 0.0],
			monochrome: true,
		},
		Adjustment::PhotoFilter {
			color: [0.92, 0.54, 0.0],
			density: 0.25,
			preserve_luminosity: true,
		},
		Adjustment::ColorBalance {
			shadows: [20.0, -10.0, 5.0],
			midtones: [-15.0, 25.0, 0.0],
			highlights: [0.0, 10.0, -30.0],
			preserve_luminosity: true,
		},
		Adjustment::ColorBalance {
			shadows: [0.0, 0.0, 0.0],
			midtones: [40.0, 0.0, -20.0],
			highlights: [0.0, 0.0, 0.0],
			preserve_luminosity: false,
		},
		Adjustment::Vibrance {
			vibrance: 45.0,
			saturation: 10.0,
		},
		Adjustment::Vibrance {
			vibrance: -30.0,
			saturation: 0.0,
		},
		Adjustment::BlackWhite {
			reds: 40.0,
			yellows: 60.0,
			greens: 40.0,
			cyans: 60.0,
			blues: 20.0,
			magentas: 80.0,
			tint: false,
			tint_hue: 0.0,
			tint_saturation: 0.0,
		},
		Adjustment::BlackWhite {
			reds: 120.0,
			yellows: -20.0,
			greens: 70.0,
			cyans: 10.0,
			blues: 200.0,
			magentas: 30.0,
			tint: true,
			tint_hue: 35.0,
			tint_saturation: 25.0,
		},
	];
	for adjustment in adjustments {
		let mut doc = doc(512, 256);
		let bg = busy_layer(&mut doc, &store, 41);
		let id = doc.allocate_layer_id();
		let mut layer = Layer::new(id, "adjustment", LayerKind::Adjustment(adjustment.clone()));
		layer.opacity = 0.9;
		doc.layers.push(Arc::new(bg));
		doc.layers.push(Arc::new(layer));
		let programs = programs(&doc, &mut luts);
		gpu.begin_frame();
		let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
		let (max_err, over) = compare(&gpu, &programs, &outcomes, &store);
		eprintln!("{adjustment:?}: max error {max_err:.5}, {:.4} % > 2/1024", over * 100.0);
		// Posterize and Threshold are step functions: f16 inputs flip the odd
		// pixel sitting exactly on a step.
		assert!(over < 0.002, "{adjustment:?}: {:.3} % of channels differ", over * 100.0);
	}
}

/// Background, isolated + pass-through groups, clipping, masks, offsets,
/// solid fills, LUT adjustments, nested groups.
fn complex_doc(store: &TileStore) -> Document {
	let mut doc = doc(768, 512);
	let bg = busy_layer(&mut doc, store, 1);
	let mut shifted = busy_layer(&mut doc, store, 2);
	shifted.blend = BlendMode::Overlay;
	if let LayerKind::Pixel { offset, .. } = &mut shifted.kind {
		*offset = (37, -101);
	}
	shifted.mask = Some(mask(&doc, store, &|x, y| hash16(x / 3, y / 5, 7)));

	let base = busy_layer(&mut doc, store, 3);
	let mut clipped = solid_layer(&mut doc, [50000, 10000, 30000, 65535]);
	clipped.clipped = true;
	clipped.blend = BlendMode::Multiply;
	let mut clipped2 = busy_layer(&mut doc, store, 4);
	clipped2.clipped = true;
	clipped2.blend = BlendMode::Screen;

	let mut in_group = busy_layer(&mut doc, store, 5);
	in_group.blend = BlendMode::SoftLight;
	let curves_id = doc.allocate_layer_id();
	let mut curves = Layer::new(
		curves_id,
		"Curves",
		LayerKind::Adjustment(Adjustment::Curves {
			channels: [vec![(0.0, 0.1), (0.4, 0.6), (1.0, 0.9)], vec![(0.0, 0.0), (1.0, 0.8)], vec![], vec![]],
		}),
	);
	curves.opacity = 0.7;
	let inner_child = busy_layer(&mut doc, store, 6);
	let inner = group(&mut doc, BlendMode::Normal, vec![inner_child]);
	let mut pass = group(&mut doc, BlendMode::PassThrough, vec![in_group, curves, inner]);
	pass.opacity = 0.6;
	pass.mask = Some(mask(&doc, store, &|x, _| if x < 400 { 65535 } else { 20000 }));

	let iso_a = busy_layer(&mut doc, store, 8);
	let iso_b = solid_layer(&mut doc, [0, 30000, 60000, 30000]);
	let mut iso = group(&mut doc, BlendMode::Difference, vec![iso_a, iso_b]);
	iso.opacity = 0.9;

	let levels_id = doc.allocate_layer_id();
	let mut levels = [LevelsChannel::default(); 4];
	levels[0].in_black = 0.1;
	levels[0].gamma = 1.4;
	levels[2].out_white = 0.8;
	let mut levels_layer = Layer::new(levels_id, "Levels", LayerKind::Adjustment(Adjustment::Levels { channels: levels }));
	levels_layer.mask = Some(mask(&doc, store, &|x, y| hash16(x / 50, y / 50, 3)));

	let invert_id = doc.allocate_layer_id();
	let mut invert = Layer::new(invert_id, "Invert", LayerKind::Adjustment(Adjustment::Invert));
	invert.blend = BlendMode::Color;
	invert.opacity = 0.3;

	for layer in [bg, shifted, base, clipped, clipped2, pass, iso, levels_layer, invert] {
		doc.layers.push(Arc::new(layer));
	}
	doc
}

#[test]
fn cache_hits_and_deferrals() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	let doc = complex_doc(&store);
	let programs = programs(&doc, &mut luts);

	// Nothing hot: every non-empty tile is deferred with its missing tiles listed.
	gpu.begin_frame();
	let outcomes = gpu.composite(&programs, &|_| None).unwrap();
	assert!(outcomes.iter().all(|o| matches!(o, TileOutcome::Deferred { missing } if !missing.is_empty())));

	gpu.begin_frame();
	gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
	let before = gpu.stats();
	gpu.begin_frame();
	let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
	let after = gpu.stats();
	assert_eq!(after.composited, before.composited, "unchanged programs are cache hits");
	assert_eq!(after.cache_hits - before.cache_hits, programs.len() as u64);
	assert!(outcomes.iter().all(|o| matches!(o, TileOutcome::Ready { .. })));
}

#[test]
fn upload_budget_defers_without_missing_tiles() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut config = small_config();
	config.upload_budget = 4;
	let mut gpu = GpuCompositor::new(&device, &queue, config);
	let mut luts = LutCache::default();
	let doc = complex_doc(&store);
	let programs = programs(&doc, &mut luts);
	let mut frames = 0;
	loop {
		gpu.begin_frame();
		let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
		frames += 1;
		assert!(outcomes.iter().all(|o| !matches!(o, TileOutcome::Deferred { missing } if !missing.is_empty())));
		if outcomes.iter().all(|o| matches!(o, TileOutcome::Ready { .. })) {
			break;
		}
		assert!(frames < 50, "never converged");
	}
	assert!(frames > 1, "a budget of 4 uploads needs several frames");
}

#[test]
fn prefix_cache_is_used_and_exact() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	let mut doc = complex_doc(&store);
	// Edit the top layer: everything below becomes a cached prefix.
	let hot = doc.layers.last().unwrap().id;
	gpu.set_hot_layer(Some(hot));

	for step in 0..3 {
		doc.layer_mut(hot).unwrap().opacity = 0.2 + 0.3 * step as f32;
		let programs = programs(&doc, &mut luts);
		gpu.begin_frame();
		let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
		let (_, over) = compare(&gpu, &programs, &outcomes, &store);
		assert!(over < 0.002, "step {step}");
	}
	assert!(gpu.stats().prefix_hits > 0, "later frames reuse the prefix");
}

#[test]
fn viewport_pass_draws_background_checkerboard_and_tiles() {
	let (_gpu, device, queue) = gpu_or_skip!();
	let store = store();
	let mut gpu = GpuCompositor::new(&device, &queue, small_config());
	let mut luts = LutCache::default();
	// 512×256 document: left tile opaque red, right tile transparent.
	let mut doc = doc(512, 256);
	let layer = pixel_layer(&mut doc, &store, &|x, _| if x < 256 { [65535, 0, 0, 65535] } else { [0; 4] });
	doc.layers.push(Arc::new(layer));
	let programs = programs(&doc, &mut luts);
	gpu.begin_frame();
	let outcomes = gpu.composite(&programs, &|h| store.try_get_hot(h)).unwrap();
	let slots: std::collections::HashMap<(u32, u32), u32> = programs
		.iter()
		.zip(&outcomes)
		.filter_map(|(p, o)| match o {
			TileOutcome::Ready { slot } => Some(((p.tx, p.ty), *slot)),
			_ => None,
		})
		.collect();

	// 128×64 viewport showing the whole document at 25 %, centred.
	let vp = crate::ViewportSize { width: 128, height: 96 };
	let view = crate::ViewTransform {
		zoom: 0.25,
		center_x: 256.0,
		center_y: 128.0,
		rotation: 0.0,
	};
	// Level-0 tiles only exist, so ask the planner at a level-0 zoom for the lookup.
	let plan = crate::plan_frame(&view, vp, 512, 256, 1, &|k| slots.get(&(k.tx, k.ty)).copied());
	assert_eq!(plan.draws.len(), 1, "only the red tile is non-empty");

	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: None,
		size: wgpu::Extent3d {
			width: vp.width,
			height: vp.height,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: wgpu::TextureFormat::Rgba8Unorm,
		usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
		view_formats: &[],
	});
	let view_tex = texture.create_view(&Default::default());
	let mut renderer = super::ViewportRenderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
	let mut encoder = device.create_command_encoder(&Default::default());
	renderer.render(
		&mut encoder,
		&view_tex,
		(vp.width, vp.height),
		&plan,
		view.zoom,
		view.rotation,
		gpu.composite_view(),
		&[],
		0.0,
	);
	let readback = device.create_buffer(&wgpu::BufferDescriptor {
		label: None,
		size: (512 * vp.height) as u64,
		usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
		mapped_at_creation: false,
	});
	encoder.copy_texture_to_buffer(
		wgpu::TexelCopyTextureInfo {
			texture: &texture,
			mip_level: 0,
			origin: wgpu::Origin3d::ZERO,
			aspect: wgpu::TextureAspect::All,
		},
		wgpu::TexelCopyBufferInfo {
			buffer: &readback,
			layout: wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(512),
				rows_per_image: Some(vp.height),
			},
		},
		wgpu::Extent3d {
			width: vp.width,
			height: vp.height,
			depth_or_array_layers: 1,
		},
	);
	queue.submit([encoder.finish()]);
	let slice = readback.slice(..);
	slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
	device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
	let data = slice.get_mapped_range();
	let px = |x: u32, y: u32| {
		let i = (y * 512 + x * 4) as usize;
		[data[i], data[i + 1], data[i + 2], data[i + 3]]
	};
	// Document spans x 0..128, y 16..80 on screen. Left half red, right half checkerboard.
	assert_eq!(px(30, 48), [255, 0, 0, 255], "red tile");
	let c = px(100, 48);
	assert!(c == [255, 255, 255, 255] || c == [204, 204, 204, 255], "checkerboard, got {c:?}");
	assert_eq!(px(60, 5), [40, 40, 40, 255], "background above the document");
}
