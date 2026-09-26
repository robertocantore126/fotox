//! The model registry and the one models folder (D-089, D-090).
//!
//! `models_dir` is **the** path: the runtime, the Preferences page and any
//! installer call it, so a model is never downloaded twice into two caches.

use std::io::{Read, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::AiError;

/// One file of a model.
#[derive(Clone, Copy, Debug)]
pub struct ModelFile {
	pub file: &'static str,
	pub url: &'static str,
	pub bytes: u64,
	/// Lower-case hex SHA-256 of the file.
	pub sha256: &'static str,
}

/// A model Fotox knows how to use.
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
	pub id: &'static str,
	pub name: &'static str,
	pub licence: &'static str,
	pub files: &'static [ModelFile],
	/// What it is used for (Preferences).
	pub purpose: &'static str,
}

impl ModelSpec {
	pub fn bytes(&self) -> u64 {
		self.files.iter().map(|f| f.bytes).sum()
	}

	pub fn path(&self, file: &str) -> PathBuf {
		models_dir().join(self.id).join(file)
	}

	pub fn installed(&self) -> bool {
		self.files.iter().all(|f| self.path(f.file).exists())
	}
}

/// BiRefNet (MIT), the general "lite" (Swin-T) export: Select Subject,
/// Remove Background. Input `input_image` [1, 3, 1024, 1024] ImageNet-
/// normalised RGB; output `output_image` [1, 1, 1024, 1024] logits.
/// VERIFY: the ONNX export is rembg's mirror of BiRefNet's weights.
pub const BIREFNET: ModelSpec = ModelSpec {
	id: "birefnet-lite",
	name: "BiRefNet (general, lite)",
	licence: "MIT",
	purpose: "Select Subject, Remove Background",
	files: &[ModelFile {
		file: "BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
		url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
		bytes: 224_005_088,
		sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
	}],
};

/// EfficientSAM ViT-T (Apache-2.0): the Object Selection tool. Encoder
/// `batched_images` [1, 3, H, W] RGB 0..1 → `image_embeddings` [1, 256, 64,
/// 64]; decoder: embeddings, `batched_point_coords` [1, 1, N, 2] (image
/// pixels), `batched_point_labels` [1, 1, N] (1 in, 0 out, 2 / 3 box
/// corners), `orig_im_size` [h, w] i64 → `output_masks` [1, 1, 3, h, w]
/// logits and `iou_predictions` [1, 1, 3].
pub const EFFICIENT_SAM: ModelSpec = ModelSpec {
	id: "efficientsam-vitt",
	name: "EfficientSAM (ViT-T)",
	licence: "Apache-2.0",
	purpose: "Object Selection tool",
	files: &[
		ModelFile {
			file: "efficient_sam_vitt_encoder.onnx",
			url: "https://raw.githubusercontent.com/yformer/EfficientSAM/main/weights/efficient_sam_vitt_encoder.onnx",
			bytes: 24_799_761,
			sha256: "84ed466ffcc5c1f8d08409bc34a23bb364ab2c15e402cb12d4335a42be0e0951",
		},
		ModelFile {
			file: "efficient_sam_vitt_decoder.onnx",
			url: "https://raw.githubusercontent.com/yformer/EfficientSAM/main/weights/efficient_sam_vitt_decoder.onnx",
			bytes: 16_565_728,
			sha256: "a62f8fa5ea080447c0689418d69e58f1e83e0b7adf9c142e2bd9bcc8045c0b11",
		},
	],
};

pub const ALL: [ModelSpec; 2] = [BIREFNET, EFFICIENT_SAM];

pub fn by_id(id: &str) -> Option<ModelSpec> {
	ALL.iter().copied().find(|m| m.id == id)
}

/// `%LOCALAPPDATA%\Fotox\models` on Windows, `$XDG_DATA_HOME/fotox/models`
/// (or `~/.local/share/fotox/models`) elsewhere; `FOTOX_MODELS` overrides.
pub fn models_dir() -> PathBuf {
	if let Ok(p) = std::env::var("FOTOX_MODELS") {
		return PathBuf::from(p);
	}
	if cfg!(windows)
		&& let Ok(p) = std::env::var("LOCALAPPDATA")
	{
		return PathBuf::from(p).join("Fotox").join("models");
	}
	if let Ok(p) = std::env::var("XDG_DATA_HOME") {
		return PathBuf::from(p).join("fotox").join("models");
	}
	let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
	PathBuf::from(home).join(".local").join("share").join("fotox").join("models")
}

/// Download every missing file of `spec` (checksummed; a mismatch deletes
/// the file). `progress(done, total)` returns `false` to cancel.
pub fn download(spec: &ModelSpec, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), AiError> {
	let total = spec.bytes();
	let mut done = 0u64;
	for f in spec.files {
		let target = spec.path(f.file);
		if target.exists() {
			done += f.bytes;
			continue;
		}
		std::fs::create_dir_all(target.parent().expect("under the models folder"))?;
		let part = target.with_extension("part");
		let mut response = ureq::get(f.url).call().map_err(|e| AiError::Download(e.to_string()))?;
		let mut reader = response.body_mut().as_reader();
		let mut out = std::fs::File::create(&part)?;
		let mut hash = Sha256::new();
		let mut buf = vec![0u8; 1 << 20];
		loop {
			let n = reader.read(&mut buf)?;
			if n == 0 {
				break;
			}
			out.write_all(&buf[..n])?;
			hash.update(&buf[..n]);
			done += n as u64;
			if !progress(done, total) {
				drop(out);
				let _ = std::fs::remove_file(&part);
				return Err(AiError::Cancelled);
			}
		}
		drop(out);
		let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
		if !f.sha256.is_empty() && digest != f.sha256 {
			let _ = std::fs::remove_file(&part);
			return Err(AiError::Download(format!("{}: checksum mismatch", f.file)));
		}
		std::fs::rename(&part, &target)?;
	}
	Ok(())
}

/// Delete an installed model.
pub fn delete(spec: &ModelSpec) -> Result<(), AiError> {
	let dir = models_dir().join(spec.id);
	if dir.exists() {
		std::fs::remove_dir_all(dir)?;
	}
	Ok(())
}
