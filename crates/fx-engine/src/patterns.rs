//! The pattern library (M8-T06): `%APPDATA%\Fotox\patterns.json`, a list of
//! patterns (8-bit RGBA, base64). A few generated defaults when there is no
//! file. Patterns a document uses are copied into its `patterns` (saved with
//! it); the library is the user's collection.

use std::path::PathBuf;

use fx_core::pattern::{MAX_SIDE, Pattern};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
	name: String,
	width: u32,
	height: u32,
	/// 8-bit straight RGBA, base64.
	rgba: String,
}

pub fn path() -> Option<PathBuf> {
	std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Fotox").join("patterns.json"))
}

#[derive(Clone, Debug, Default)]
pub struct Library {
	pub patterns: Vec<Pattern>,
}

fn to8(p: &Pattern) -> Vec<u8> {
	p.pixels.iter().flat_map(|c| c.map(|v| (v / 257) as u8)).collect()
}

fn from8(name: &str, width: u32, height: u32, rgba: &[u8]) -> Option<Pattern> {
	if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || rgba.len() < (width * height * 4) as usize {
		return None;
	}
	let pixels = rgba
		.chunks_exact(4)
		.take((width * height) as usize)
		.map(|c| [c[0], c[1], c[2], c[3]].map(|v| u16::from(v) * 257))
		.collect();
	Some(Pattern::new(name, width, height, pixels))
}

impl Library {
	pub fn load() -> Self {
		let stored: Option<Vec<Stored>> = path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok());
		let patterns = match stored {
			Some(list) => list
				.iter()
				.filter_map(|s| from8(&s.name, s.width, s.height, &crate::b64::decode(&s.rgba)))
				.collect(),
			None => defaults(),
		};
		Self { patterns }
	}

	/// FAST: errors are logged.
	pub fn save(&self) {
		let Some(path) = path() else { return };
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		let list: Vec<Stored> = self
			.patterns
			.iter()
			.map(|p| Stored {
				name: p.name.clone(),
				width: p.width,
				height: p.height,
				rgba: crate::b64::encode(&to8(p)),
			})
			.collect();
		if let Ok(text) = serde_json::to_string(&list)
			&& let Err(error) = std::fs::write(&path, text)
		{
			tracing::warn!("cannot write {}: {error}", path.display());
		}
	}

	pub fn get(&self, id: u64) -> Option<&Pattern> {
		self.patterns.iter().find(|p| p.id == id)
	}

	/// Add (or keep, when the same pixels are there) a pattern; its id.
	pub fn add(&mut self, pattern: Pattern) -> u64 {
		let id = pattern.id;
		if self.get(id).is_none() {
			self.patterns.push(pattern);
		}
		id
	}

	/// A PNG file as a pattern.
	pub fn import_png(&mut self, name: &str, data: &[u8]) -> Result<u64, String> {
		let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
		decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
		let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
		let mut buffer = vec![0; reader.output_buffer_size().ok_or("the image is too large")?];
		let info = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
		let (w, h) = (info.width, info.height);
		let rgba: Vec<u8> = match info.color_type {
			png::ColorType::Rgba => buffer[..(w * h * 4) as usize].to_vec(),
			png::ColorType::GrayscaleAlpha => buffer[..(w * h * 2) as usize].chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
			other => return Err(format!("unsupported PNG colour type {other:?}")),
		};
		let pattern = from8(name, w, h, &rgba).ok_or_else(|| format!("a pattern is at most {MAX_SIDE} px per side"))?;
		Ok(self.add(pattern))
	}

	/// The list the UI shows: id, name, size and a 48 × 48 RGBA8 thumbnail.
	pub fn infos(&self) -> Vec<serde_json::Value> {
		self.patterns
			.iter()
			.map(|p| {
				let mut thumb = Vec::with_capacity(48 * 48 * 4);
				for y in 0..48 {
					for x in 0..48 {
						let c = p.at(i64::from(x) * i64::from(p.width.max(48)) / 48, i64::from(y) * i64::from(p.height.max(48)) / 48);
						thumb.extend(c.map(|v| (v * 255.0).round() as u8));
					}
				}
				serde_json::json!({ "id": p.id, "name": p.name, "width": p.width, "height": p.height, "thumb": crate::b64::encode(&thumb) })
			})
			.collect()
	}
}

/// Generated defaults: a checkerboard, dots, diagonal lines, a brick wall.
fn defaults() -> Vec<Pattern> {
	let make = |name: &str, size: u32, f: &dyn Fn(u32, u32) -> [u16; 4]| {
		let pixels = (0..size * size).map(|i| f(i % size, i / size)).collect();
		Pattern::new(name, size, size, pixels)
	};
	let white = [65535, 65535, 65535, 65535];
	let grey = [40000, 40000, 40000, 65535];
	let dark = [12000, 12000, 12000, 65535];
	vec![
		make("Checkerboard", 32, &|x, y| if (x / 16 + y / 16) % 2 == 0 { white } else { grey }),
		make("Dots", 24, &|x, y| {
			let (dx, dy) = (f64::from(x) - 11.5, f64::from(y) - 11.5);
			if dx * dx + dy * dy < 25.0 { dark } else { white }
		}),
		make("Diagonal Lines", 16, &|x, y| if (x + y) % 16 < 3 { dark } else { white }),
		make("Bricks", 64, &|x, y| {
			let row = y / 16;
			let shift = if row % 2 == 0 { 0 } else { 16 };
			if y % 16 < 2 || (x + shift) % 32 < 2 {
				[52000, 52000, 50000, 65535]
			} else {
				[42000, 16000, 11000, 65535]
			}
		}),
	]
}
