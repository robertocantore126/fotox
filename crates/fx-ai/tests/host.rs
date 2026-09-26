//! The inference host end to end with a tiny model (M13-T01): mean of the
//! channels × 20 − 10, so bright pixels give positive logits.
//! Needs ONNX Runtime: set `FOTOX_ORT_DYLIB`; skipped (passes) without it.

use fx_ai::session::{Input, Model, Tensor32};

#[test]
fn a_tiny_model_runs_through_the_host() {
	if fx_ai::runtime::find_library().is_none() {
		eprintln!("skipped: no ONNX Runtime (FOTOX_ORT_DYLIB)");
		return;
	}
	let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/threshold.onnx");
	let model = Model::load(&path).expect("the model loads");
	assert_eq!(model.inputs, ["input"]);
	let (w, h) = (4usize, 2usize);
	let mut data = vec![0.0f32; 3 * w * h];
	// The right half is white.
	for c in 0..3 {
		for y in 0..h {
			for x in 2..w {
				data[c * w * h + y * w + x] = 1.0;
			}
		}
	}
	let out = model.run(vec![Input::F32(Tensor32::new(vec![1, 3, h, w], data))]).expect("it runs");
	assert_eq!(out[0].shape, vec![1, 1, h, w]);
	assert_eq!(out[0].data[0], -10.0);
	assert_eq!(out[0].data[3], 10.0);
}
