//! The ComfyUI bridge (M13-T06, D-091): Generative Fill / Expand run on the
//! local ComfyUI server Rob already has, through its HTTP API.
//!
//! One run: the image (RGBA8, alpha 0 where new content goes: ComfyUI's
//! `LoadImage` turns `1 − alpha` into the mask) is uploaded with
//! `/upload/image`, a bundled workflow template (`assets/comfy/*.json`, or a
//! file named in Preferences) is filled in and queued with `/prompt`,
//! `/history/<id>` is polled until the images are listed, and each is read
//! back with `/view`.
//!
//! Every call has a timeout and the wait checks a cancel callback, so an
//! unreachable or stuck server gives a clear error, never a hang (D-092).
//! Nothing leaves the machine unless the address says so.

use std::io::Cursor;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::AiError;

/// The bundled workflows: `{{name}}` strings are replaced by [`fill`].
pub const INPAINT: &str = include_str!("../assets/comfy/inpaint.json");
pub const OUTPAINT: &str = include_str!("../assets/comfy/outpaint.json");

/// Where ComfyUI usually listens: the desktop app (8000), then the portable /
/// git install (8188).
pub const DEFAULT_ADDRESSES: [&str; 2] = ["http://127.0.0.1:8000", "http://127.0.0.1:8188"];

/// What a run needs besides the image.
#[derive(Clone, Debug)]
pub struct Job {
	pub address: String,
	pub checkpoint: String,
	pub prompt: String,
	pub negative: String,
	pub seed: u64,
	pub count: u32,
	pub steps: u32,
	pub cfg: f32,
	/// The workflow JSON (API format) with `{{…}}` placeholders.
	pub workflow: String,
}

/// One image back from ComfyUI: straight RGBA8.
#[derive(Clone, Debug)]
pub struct Image {
	pub width: u32,
	pub height: u32,
	pub rgba: Vec<u8>,
}

fn agent(timeout: Duration) -> ureq::Agent {
	ureq::Agent::config_builder()
		.timeout_connect(Some(Duration::from_secs(3)))
		.timeout_global(Some(timeout))
		.http_status_as_error(false)
		.build()
		.into()
}

fn base(address: &str) -> String {
	let a = address.trim().trim_end_matches('/');
	if a.starts_with("http://") || a.starts_with("https://") {
		a.to_owned()
	} else {
		format!("http://{a}")
	}
}

fn unreachable(address: &str, e: impl std::fmt::Display) -> AiError {
	AiError::Comfy(format!(
		"not reachable at {address} ({e}) — start ComfyUI or change the address in Preferences ▸ AI"
	))
}

fn get_json(address: &str, path: &str, timeout: Duration) -> Result<Value, AiError> {
	let url = format!("{}{path}", base(address));
	let mut response = agent(timeout).get(&url).call().map_err(|e| unreachable(address, e))?;
	let status = response.status();
	let text = response.body_mut().read_to_string().map_err(|e| AiError::Comfy(e.to_string()))?;
	if !status.is_success() {
		return Err(AiError::Comfy(format!("{path}: HTTP {status}: {}", text.chars().take(300).collect::<String>())));
	}
	serde_json::from_str(&text).map_err(|e| AiError::Comfy(format!("{path}: {e}")))
}

/// The server's version (`/system_stats`): the Preferences page's "Test".
pub fn ping(address: &str) -> Result<String, AiError> {
	let stats = get_json(address, "/system_stats", Duration::from_secs(5))?;
	let version = stats.pointer("/system/comfyui_version").and_then(Value::as_str).unwrap_or("?");
	let device = stats.pointer("/devices/0/name").and_then(Value::as_str).unwrap_or("?");
	Ok(format!("ComfyUI {version} on {device}"))
}

/// The first address that answers: `preferred` if set, else the defaults.
pub fn find(preferred: Option<&str>) -> Result<String, AiError> {
	if let Some(p) = preferred.filter(|p| !p.trim().is_empty()) {
		ping(p)?;
		return Ok(p.to_owned());
	}
	let mut last = None;
	for a in DEFAULT_ADDRESSES {
		match ping(a) {
			Ok(_) => return Ok(a.to_owned()),
			Err(e) => last = Some(e),
		}
	}
	Err(last.unwrap_or_else(|| AiError::Comfy("no address".into())))
}

/// The checkpoints ComfyUI offers (`CheckpointLoaderSimple`'s list).
pub fn checkpoints(address: &str) -> Result<Vec<String>, AiError> {
	let info = get_json(address, "/object_info/CheckpointLoaderSimple", Duration::from_secs(10))?;
	Ok(info
		.pointer("/CheckpointLoaderSimple/input/required/ckpt_name/0")
		.and_then(Value::as_array)
		.map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
		.unwrap_or_default())
}

/// A checkpoint to use when none is set: an inpainting one if there is one,
/// else an SDXL-class one, else the first.
pub fn pick_checkpoint(list: &[String]) -> Option<String> {
	let lower = |s: &String| s.to_lowercase();
	list.iter()
		.find(|s| lower(s).contains("inpaint"))
		.or_else(|| list.iter().find(|s| lower(s).contains("xl")))
		.or_else(|| list.first())
		.cloned()
}

/// Encode straight RGBA8 as PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, AiError> {
	let mut out = Vec::new();
	{
		let mut encoder = png::Encoder::new(&mut out, width, height);
		encoder.set_color(png::ColorType::Rgba);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder.write_header().map_err(|e| AiError::Comfy(e.to_string()))?;
		writer.write_image_data(rgba).map_err(|e| AiError::Comfy(e.to_string()))?;
	}
	Ok(out)
}

/// Decode a PNG to straight RGBA8.
pub fn decode_png(data: &[u8]) -> Result<Image, AiError> {
	let bad = |e: String| AiError::Comfy(format!("an image could not be read: {e}"));
	let mut decoder = png::Decoder::new(Cursor::new(data));
	decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
	let mut reader = decoder.read_info().map_err(|e| bad(e.to_string()))?;
	let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(|| bad("too large".into()))?];
	let info = reader.next_frame(&mut buffer).map_err(|e| bad(e.to_string()))?;
	let (w, h) = (info.width, info.height);
	let n = (w * h) as usize;
	let rgba = match info.color_type {
		png::ColorType::Rgba => buffer[..n * 4].to_vec(),
		png::ColorType::GrayscaleAlpha => buffer[..n * 2].chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
		other => return Err(bad(format!("colour type {other:?}"))),
	};
	Ok(Image { width: w, height: h, rgba })
}

/// Upload a PNG (`/upload/image`, multipart); the name ComfyUI stored it as.
fn upload(address: &str, name: &str, png: &[u8]) -> Result<String, AiError> {
	let nanos = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| d.as_nanos() as u64);
	let boundary = format!("----fotox{nanos:016x}");
	let mut body = Vec::with_capacity(png.len() + 512);
	body.extend_from_slice(
		format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n").as_bytes(),
	);
	body.extend_from_slice(png);
	body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"type\"\r\n\r\ninput\r\n").as_bytes());
	body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"overwrite\"\r\n\r\ntrue\r\n--{boundary}--\r\n").as_bytes());
	let url = format!("{}/upload/image", base(address));
	let mut response = agent(Duration::from_secs(60))
		.post(&url)
		.header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
		.send(&body[..])
		.map_err(|e| unreachable(address, e))?;
	let status = response.status();
	let text = response.body_mut().read_to_string().map_err(|e| AiError::Comfy(e.to_string()))?;
	if !status.is_success() {
		return Err(AiError::Comfy(format!("upload: HTTP {status}: {text}")));
	}
	let v: Value = serde_json::from_str(&text).map_err(|e| AiError::Comfy(format!("upload: {e}")))?;
	let stored = v.get("name").and_then(Value::as_str).unwrap_or(name);
	Ok(match v.get("subfolder").and_then(Value::as_str) {
		Some(sub) if !sub.is_empty() => format!("{sub}/{stored}"),
		_ => stored.to_owned(),
	})
}

/// Replace every string `"{{key}}"` in `workflow` by `values[key]`.
pub fn fill(workflow: &str, values: &Value) -> Result<Value, AiError> {
	fn walk(v: &mut Value, values: &Value) {
		match v {
			Value::String(s) if s.starts_with("{{") && s.ends_with("}}") => {
				if let Some(new) = values.get(&s[2..s.len() - 2]) {
					*v = new.clone();
				}
			}
			Value::Array(a) => a.iter_mut().for_each(|x| walk(x, values)),
			Value::Object(o) => o.values_mut().for_each(|x| walk(x, values)),
			_ => {}
		}
	}
	let mut w: Value = serde_json::from_str(workflow).map_err(|e| AiError::Comfy(format!("the workflow is not valid JSON: {e}")))?;
	walk(&mut w, values);
	Ok(w)
}

/// The workflow a run queues (the uploaded image's name filled in).
pub fn workflow_for(job: &Job, image: &str) -> Result<Value, AiError> {
	fill(
		&job.workflow,
		&json!({
			"checkpoint": job.checkpoint, "prompt": job.prompt, "negative": job.negative,
			"image": image, "seed": job.seed, "count": job.count.max(1), "steps": job.steps, "cfg": job.cfg,
		}),
	)
}

/// Run `job` on `image`; `progress(fraction)` returns `false` to cancel.
pub fn run(job: &Job, image: &Image, progress: &mut dyn FnMut(f32) -> bool) -> Result<Vec<Image>, AiError> {
	let address = job.address.as_str();
	let png = encode_png(image.width, image.height, &image.rgba)?;
	let stored = upload(address, "fotox-input.png", &png)?;
	if !progress(0.05) {
		return Err(AiError::Cancelled);
	}
	let workflow = workflow_for(job, &stored)?;
	let body = serde_json::to_vec(&json!({ "prompt": workflow, "client_id": "fotox" })).map_err(|e| AiError::Comfy(e.to_string()))?;
	let url = format!("{}/prompt", base(address));
	let mut response = agent(Duration::from_secs(30))
		.post(&url)
		.header("Content-Type", "application/json")
		.send(&body[..])
		.map_err(|e| unreachable(address, e))?;
	let status = response.status();
	let text = response.body_mut().read_to_string().map_err(|e| AiError::Comfy(e.to_string()))?;
	if !status.is_success() {
		// ComfyUI explains a bad workflow (a missing checkpoint, a node) here.
		return Err(AiError::Comfy(format!(
			"the workflow was refused: {}",
			text.chars().take(600).collect::<String>()
		)));
	}
	let queued: Value = serde_json::from_str(&text).map_err(|e| AiError::Comfy(format!("/prompt: {e}")))?;
	let id = queued
		.get("prompt_id")
		.and_then(Value::as_str)
		.ok_or_else(|| AiError::Comfy(format!("/prompt gave no id: {text}")))?
		.to_owned();
	// Poll the history. FAST: no websocket, so the progress is a guess by time.
	let start = Instant::now();
	let limit = Duration::from_secs(15 * 60);
	let entry = loop {
		let elapsed = start.elapsed();
		if elapsed > limit {
			interrupt(address);
			return Err(AiError::Comfy("no result after 15 minutes".into()));
		}
		let guess = 0.05 + 0.85 * (1.0 - (-elapsed.as_secs_f32() / 40.0).exp());
		if !progress(guess) {
			interrupt(address);
			return Err(AiError::Cancelled);
		}
		let history = get_json(address, &format!("/history/{id}"), Duration::from_secs(10))?;
		if let Some(entry) = history.get(&id) {
			if entry.pointer("/status/status_str").and_then(Value::as_str) == Some("error") {
				let messages = entry.pointer("/status/messages").map(Value::to_string).unwrap_or_default();
				return Err(AiError::Comfy(format!("the run failed: {}", messages.chars().take(600).collect::<String>())));
			}
			if entry.get("outputs").and_then(Value::as_object).is_some_and(|o| !o.is_empty()) {
				break entry.clone();
			}
		}
		std::thread::sleep(Duration::from_millis(500));
	};
	let mut images = Vec::new();
	for output in entry["outputs"].as_object().into_iter().flat_map(|o| o.values()) {
		for img in output.get("images").and_then(Value::as_array).into_iter().flatten() {
			if img.get("type").and_then(Value::as_str) == Some("temp") {
				continue;
			}
			let q = |k: &str| img.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
			let url = format!(
				"{}/view?filename={}&subfolder={}&type={}",
				base(address),
				enc(&q("filename")),
				enc(&q("subfolder")),
				enc(&q("type"))
			);
			let mut response = agent(Duration::from_secs(60)).get(&url).call().map_err(|e| unreachable(address, e))?;
			let data = response
				.body_mut()
				.with_config()
				.limit(256 << 20)
				.read_to_vec()
				.map_err(|e| AiError::Comfy(e.to_string()))?;
			images.push(decode_png(&data)?);
		}
	}
	if images.is_empty() {
		return Err(AiError::Comfy("the workflow saved no image (it needs a SaveImage node)".into()));
	}
	progress(1.0);
	Ok(images)
}

/// Ask ComfyUI to stop the running prompt (Cancel).
fn interrupt(address: &str) {
	let _ = agent(Duration::from_secs(3))
		.post(format!("{}/interrupt", base(address)))
		.header("Content-Type", "application/json")
		.send(&b"{}"[..]);
}

/// Percent-encode a query value.
fn enc(s: &str) -> String {
	s.bytes()
		.map(|b| match b {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
			_ => format!("%{b:02X}"),
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_templates_fill_in() {
		let job = Job {
			address: "x".into(),
			checkpoint: "sdxl.safetensors".into(),
			prompt: "a cat".into(),
			negative: String::new(),
			seed: 7,
			count: 3,
			steps: 20,
			cfg: 6.0,
			workflow: INPAINT.into(),
		};
		let w = workflow_for(&job, "in.png").unwrap();
		assert_eq!(w["1"]["inputs"]["ckpt_name"], "sdxl.safetensors");
		assert_eq!(w["4"]["inputs"]["image"], "in.png");
		assert_eq!(w["6"]["inputs"]["amount"], 3);
		assert_eq!(w["7"]["inputs"]["seed"], 7);
		assert!(fill(OUTPAINT, &json!({})).is_ok());
	}

	#[test]
	fn png_round_trips() {
		let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| (i * 9) as u8).collect();
		let back = decode_png(&encode_png(3, 2, &rgba).unwrap()).unwrap();
		assert_eq!((back.width, back.height), (3, 2));
		assert_eq!(back.rgba, rgba);
	}
}
