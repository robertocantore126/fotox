//! Local AI for Fotox (M13, D-088..D-092).
//!
//! * [`runtime`]: ONNX Runtime loaded at run time, DirectML first on
//!   Windows, the CPU otherwise; one [`session::Model`] per model file, kept.
//! * [`models`]: the model registry, the one models folder, downloads with
//!   the size shown first and a checksum.
//! * [`image`]: the working-resolution buffers a model reads and writes.
//! * [`sam`]: EfficientSAM's prompt geometry (M13-T04).
//! * [`comfy`]: the ComfyUI bridge for Generative Fill / Expand (M13-T06).
//!
//! Nothing here touches the document: the engine reads the pixels at a
//! bounded working resolution and turns the outputs into selections,
//! masks and layers.

pub mod comfy;
pub mod image;
pub mod models;
pub mod runtime;
pub mod sam;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum AiError {
	#[error("ONNX Runtime is not available: {0}")]
	Runtime(String),
	#[error("the model {0} is not installed (Preferences ▸ AI Models)")]
	Missing(String),
	#[error("inference failed: {0}")]
	Inference(String),
	#[error("download failed: {0}")]
	Download(String),
	#[error("ComfyUI: {0}")]
	Comfy(String),
	#[error("cancelled")]
	Cancelled,
	#[error(transparent)]
	Io(#[from] std::io::Error),
}

impl From<ort::Error> for AiError {
	fn from(e: ort::Error) -> Self {
		AiError::Inference(e.to_string())
	}
}
