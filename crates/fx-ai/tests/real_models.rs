//! The real models on a synthetic picture (M13-T02/T04). Ignored by default:
//! they need ONNX Runtime (`FOTOX_ORT_DYLIB`) and download 265 MB into the
//! models folder on first run.
//!
//! `cargo test -p fx-ai --test real_models -- --ignored --nocapture`

use fx_ai::models::{BIREFNET, EFFICIENT_SAM, ModelSpec};
use fx_ai::session::{Input, Model, Tensor32};

const W: usize = 640;
const H: usize = 480;

/// A warm disc (the subject) on a cool, slightly textured background.
fn picture() -> (Vec<[f32; 4]>, Vec<bool>) {
	let (cx, cy, r) = (330.0f32, 250.0f32, 120.0f32);
	let mut pixels = Vec::with_capacity(W * H);
	let mut inside = Vec::with_capacity(W * H);
	for y in 0..H {
		for x in 0..W {
			let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
			let disc = d < r;
			let n = (((x * 7 + y * 13) % 17) as f32) / 170.0;
			pixels.push(if disc {
				[0.9 - d / r * 0.3, 0.45, 0.2, 1.0]
			} else {
				[0.25 + n, 0.35 + n, 0.45 + n, 1.0]
			});
			inside.push(disc);
		}
	}
	(pixels, inside)
}

fn ready(spec: &ModelSpec) -> bool {
	if fx_ai::runtime::find_library().is_none() {
		eprintln!("skipped: no ONNX Runtime (FOTOX_ORT_DYLIB)");
		return false;
	}
	if !spec.installed() {
		eprintln!(
			"downloading {} ({} MB) into {}",
			spec.name,
			spec.bytes() / 1_000_000,
			fx_ai::models::models_dir().display()
		);
		fx_ai::models::download(spec, &mut |_, _| true).expect("download");
	}
	true
}

/// IoU of a thresholded mask (sampled over the picture) with the disc.
fn iou(mask: &[f32], mw: usize, mh: usize, inside: &[bool]) -> f64 {
	let (mut both, mut either) = (0usize, 0usize);
	for y in 0..H {
		for x in 0..W {
			let u = (x as f32 + 0.5) * mw as f32 / W as f32;
			let v = (y as f32 + 0.5) * mh as f32 / H as f32;
			let m = fx_ai::image::sample(mask, mw, mh, u, v) > 0.5;
			let d = inside[y * W + x];
			both += usize::from(m && d);
			either += usize::from(m || d);
		}
	}
	both as f64 / either.max(1) as f64
}

#[test]
#[ignore = "real model: downloads 224 MB"]
fn birefnet_finds_the_disc() {
	if !ready(&BIREFNET) {
		return;
	}
	let (pixels, inside) = picture();
	let model = Model::load(&BIREFNET.path(BIREFNET.files[0].file)).expect("loads");
	eprintln!("BiRefNet inputs {:?} outputs {:?}", model.inputs, model.outputs);
	let input = fx_ai::image::planar_rgb(&pixels, W, H, 1024, 1024, fx_ai::image::IMAGENET_MEAN, fx_ai::image::IMAGENET_STD);
	let t = std::time::Instant::now();
	let out = model.run(vec![Tensor32::new(vec![1, 3, 1024, 1024], input).into()]).expect("runs");
	eprintln!("BiRefNet: {:?}", t.elapsed());
	let best = out.iter().rev().find(|t| t.data.len() == 1024 * 1024).expect("a 1024² output");
	let mask: Vec<f32> = best.data.iter().map(|v| fx_ai::image::sigmoid(*v)).collect();
	let score = iou(&mask, 1024, 1024, &inside);
	eprintln!("BiRefNet IoU {score:.3}");
	assert!(score > 0.8, "IoU {score}");
}

#[test]
#[ignore = "real model: downloads 41 MB"]
fn efficientsam_finds_the_disc_in_a_box() {
	if !ready(&EFFICIENT_SAM) {
		return;
	}
	let (pixels, inside) = picture();
	eprintln!("loading the encoder");
	let encoder = Model::load(&EFFICIENT_SAM.path(EFFICIENT_SAM.files[0].file)).expect("encoder loads");
	eprintln!("loading the decoder");
	let decoder = Model::load(&EFFICIENT_SAM.path(EFFICIENT_SAM.files[1].file)).expect("decoder loads");
	eprintln!("decoder inputs {:?} outputs {:?}", decoder.inputs, decoder.outputs);
	let input = fx_ai::image::planar_rgb(&pixels, W, H, W, H, [0.0; 3], [1.0; 3]);
	let t = std::time::Instant::now();
	let embedding = encoder.run(vec![Tensor32::new(vec![1, 3, H, W], input).into()]).expect("encodes").remove(0);
	eprintln!("encoder: {:?}, embedding {:?}", t.elapsed(), embedding.shape);
	let (coords, labels, size) = fx_ai::sam::prompt(Some([200.0, 120.0, 460.0, 380.0]), &[], (W, H));
	let t = std::time::Instant::now();
	let mut named = vec![
		("image_embeddings", Input::F32(embedding)),
		("batched_point_coords", coords),
		("batched_point_labels", labels),
		("orig_im_size", size),
	];
	let inputs: Vec<Input> = decoder
		.inputs
		.iter()
		.map(|n| named.swap_remove(named.iter().position(|(k, _)| k == n).expect("known input")).1)
		.collect();
	let out = decoder.run(inputs).expect("decodes");
	eprintln!("decoder: {:?}", t.elapsed());
	let masks = &out[decoder.outputs.iter().position(|o| o == "output_masks").unwrap_or(0)];
	let ious = &out[decoder.outputs.iter().position(|o| o == "iou_predictions").unwrap_or(1)];
	let best = fx_ai::sam::best_mask(masks, ious).expect("a mask");
	let mask: Vec<f32> = best.iter().map(|v| fx_ai::image::sigmoid(*v)).collect();
	let score = iou(&mask, W, H, &inside);
	eprintln!("EfficientSAM IoU {score:.3}");
	assert!(score > 0.8, "IoU {score}");
}
