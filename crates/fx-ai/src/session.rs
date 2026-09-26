//! One ONNX model loaded for the session, run with f32 tensors.

use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;
use ort::value::Tensor;

use crate::AiError;

/// A loaded model; `run` takes the inputs in the model's input order.
pub struct Model {
	session: Mutex<Session>,
	pub inputs: Vec<String>,
	pub outputs: Vec<String>,
}

/// A tensor: shape and row-major values.
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor32 {
	pub shape: Vec<usize>,
	pub data: Vec<f32>,
}

impl Tensor32 {
	pub fn new(shape: Vec<usize>, data: Vec<f32>) -> Self {
		debug_assert_eq!(shape.iter().product::<usize>(), data.len());
		Self { shape, data }
	}
}

/// One model input.
#[derive(Clone, Debug)]
pub enum Input {
	F32(Tensor32),
	I64 { shape: Vec<usize>, data: Vec<i64> },
}

impl From<Tensor32> for Input {
	fn from(t: Tensor32) -> Self {
		Input::F32(t)
	}
}

impl Model {
	/// Load `path`, DirectML first (Windows), the CPU otherwise.
	pub fn load(path: &Path) -> Result<Self, AiError> {
		crate::runtime::ensure()?;
		let mut builder = Session::builder()?;
		// DirectML only when the runtime lists it: registering it on a CPU-only
		// build (ComfyUI's onnxruntime 1.29) crashes EfficientSAM's decoder
		// with an access violation instead of returning an error.
		// `FOTOX_AI_CPU=1` forces the CPU.
		#[cfg(windows)]
		let providers = {
			use ort::ep::ExecutionProvider;
			let dml = ort::ep::DirectML::default();
			if std::env::var_os("FOTOX_AI_CPU").is_none() && dml.is_available().unwrap_or(false) {
				vec![dml.build(), ort::ep::CPU::default().build()]
			} else {
				vec![ort::ep::CPU::default().build()]
			}
		};
		#[cfg(not(windows))]
		let providers = [ort::ep::CPU::default().build()];
		builder = builder.with_execution_providers(providers).map_err(|e| AiError::Inference(e.to_string()))?;
		tracing::debug!("loading {}", path.display());
		let session = builder.commit_from_file(path)?;
		let inputs = session.inputs().iter().map(|o| o.name().to_owned()).collect();
		let outputs = session.outputs().iter().map(|o| o.name().to_owned()).collect();
		Ok(Self {
			session: Mutex::new(session),
			inputs,
			outputs,
		})
	}

	/// Run with `inputs` (the model's inputs, in order); every output as f32.
	pub fn run(&self, inputs: Vec<Input>) -> Result<Vec<Tensor32>, AiError> {
		let mut values = Vec::with_capacity(inputs.len());
		for (name, input) in self.inputs.iter().zip(inputs) {
			let value: ort::session::SessionInputValue<'_> = match input {
				Input::F32(t) => Tensor::from_array((t.shape, t.data.into_boxed_slice()))?.into(),
				Input::I64 { shape, data } => Tensor::from_array((shape, data.into_boxed_slice()))?.into(),
			};
			values.push((std::borrow::Cow::<str>::from(name.clone()), value));
		}
		let mut session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		let outputs = session.run(values)?;
		let mut out = Vec::with_capacity(self.outputs.len());
		for name in &self.outputs {
			let value = &outputs[name.as_str()];
			let (shape, data) = value.try_extract_tensor::<f32>()?;
			out.push(Tensor32::new(shape.iter().map(|d| (*d).max(0) as usize).collect(), data.to_vec()));
		}
		Ok(out)
	}
}
