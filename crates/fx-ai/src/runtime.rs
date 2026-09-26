//! ONNX Runtime, loaded once per process (D-088: `load-dynamic`).
//!
//! The library is looked for, in order: `FOTOX_ORT_DYLIB`; next to the
//! executable (`onnxruntime.dll` / `libonnxruntime.so` / `.dylib`); the
//! models folder. Without it every AI feature says so (D-092).

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::AiError;

static STATE: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn lib_name() -> &'static str {
	if cfg!(windows) {
		"onnxruntime.dll"
	} else if cfg!(target_os = "macos") {
		"libonnxruntime.dylib"
	} else {
		"libonnxruntime.so"
	}
}

/// Where the runtime library would be, if anywhere.
pub fn find_library() -> Option<PathBuf> {
	if let Ok(p) = std::env::var("FOTOX_ORT_DYLIB") {
		let p = PathBuf::from(p);
		if p.exists() {
			return Some(p);
		}
	}
	let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(lib_name())));
	let in_models = Some(crate::models::models_dir().join(lib_name()));
	[beside, in_models].into_iter().flatten().find(|p| p.exists())
}

/// Load and initialise ONNX Runtime (once); the library's path, or why not.
pub fn ensure() -> Result<PathBuf, AiError> {
	STATE
		.get_or_init(|| {
			let path = find_library().ok_or_else(|| format!("{} was not found (set FOTOX_ORT_DYLIB or install it next to Fotox)", lib_name()))?;
			let builder = ort::init_from(&path).map_err(|e| e.to_string())?;
			builder.with_name("fotox").commit();
			tracing::info!("ONNX Runtime loaded from {}", path.display());
			Ok(path)
		})
		.clone()
		.map_err(AiError::Runtime)
}

/// Whether the runtime is (or can be) loaded, without loading it.
pub fn available() -> bool {
	STATE.get().map_or_else(|| find_library().is_some(), Result::is_ok)
}
