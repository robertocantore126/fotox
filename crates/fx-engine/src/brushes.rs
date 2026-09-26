//! Brush presets and sampled tips (M8-T01, D-063).
//!
//! The library is `%APPDATA%\Fotox\brushes.json`: a list of presets, each the
//! option-bar values a brush needs (size, hardness, spacing, roundness,
//! angle), its dynamics and, for a sampled tip, the tip's grey pixels
//! (base64). At start every tip is registered with the brush engine
//! (`fx_ops::brush::tip::register`), so a preset's `tip` id is what the
//! option bar sends back. `.abr` files add their sampled tips as presets.

use std::path::PathBuf;

use fx_core::stroke::{BrushParams, Dynamics, StrokeSample};
use fx_ops::brush::{DabPath, Tip};
use serde::{Deserialize, Serialize};

/// A sampled tip's pixels.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TipData {
	pub width: u32,
	pub height: u32,
	/// 8-bit grey, base64.
	pub gray: String,
}

/// One preset of the Brushes panel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
	pub name: String,
	pub size: f32,
	#[serde(default = "full")]
	pub hardness: f32,
	#[serde(default = "quarter")]
	pub spacing: f32,
	#[serde(default = "one")]
	pub roundness: f32,
	#[serde(default)]
	pub angle: f32,
	#[serde(default)]
	pub dynamics: Dynamics,
	#[serde(default)]
	pub tip: Option<TipData>,
}

fn full() -> f32 {
	1.0
}
fn quarter() -> f32 {
	0.25
}
fn one() -> f32 {
	1.0
}

/// Where the library lives.
pub fn path() -> Option<PathBuf> {
	std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Fotox").join("brushes.json"))
}

/// The brush library.
#[derive(Clone, Debug, Default)]
pub struct Library {
	pub presets: Vec<Preset>,
}

impl Library {
	/// Read the file, or Photoshop-like defaults.
	pub fn load() -> Self {
		let presets = path()
			.and_then(|p| std::fs::read_to_string(p).ok())
			.and_then(|text| serde_json::from_str::<Vec<Preset>>(&text).ok())
			.unwrap_or_else(defaults);
		let library = Self { presets };
		for preset in &library.presets {
			tip_id(preset);
		}
		library
	}

	/// Write the file. FAST: errors are logged, not reported.
	pub fn save(&self) {
		let Some(path) = path() else { return };
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		match serde_json::to_string(&self.presets) {
			Ok(text) => {
				if let Err(error) = std::fs::write(&path, text) {
					tracing::warn!("cannot write {}: {error}", path.display());
				}
			}
			Err(error) => tracing::warn!("cannot serialise the brushes: {error}"),
		}
	}

	/// The presets as the UI shows them.
	pub fn infos(&self) -> Vec<serde_json::Value> {
		self.presets
			.iter()
			.map(|p| {
				serde_json::json!({
					"name": p.name,
					"size": p.size,
					"hardness": p.hardness,
					"spacing": p.spacing,
					"roundness": p.roundness,
					"angle": p.angle,
					"dynamics": p.dynamics,
					"tip": tip_id(p),
					"thumb": crate::b64::encode(&thumbnail(p)),
				})
			})
			.collect()
	}

	/// Add the sampled tips of an `.abr` file.
	pub fn import_abr(&mut self, data: &[u8]) -> Result<usize, String> {
		let tips = fx_io::abr::read_abr(data).map_err(|e| e.to_string())?;
		let n = tips.len();
		for tip in tips {
			self.presets.push(Preset {
				name: tip.name,
				size: tip.width.max(tip.height) as f32,
				hardness: 1.0,
				spacing: tip.spacing,
				roundness: 1.0,
				angle: 0.0,
				dynamics: Dynamics::default(),
				tip: Some(TipData {
					width: tip.width,
					height: tip.height,
					gray: crate::b64::encode(&tip.gray),
				}),
			});
		}
		Ok(n)
	}
}

/// Register a preset's tip (idempotent); 0 for a round tip.
pub fn tip_id(preset: &Preset) -> u64 {
	match &preset.tip {
		Some(t) => {
			let gray = crate::b64::decode(&t.gray);
			if gray.len() < (t.width * t.height) as usize {
				return 0;
			}
			fx_ops::brush::tip::register(t.width, t.height, &gray)
		}
		None => 0,
	}
}

fn defaults() -> Vec<Preset> {
	let round = |name: &str, size: f32, hardness: f32| Preset {
		name: name.into(),
		size,
		hardness,
		spacing: 0.25,
		roundness: 1.0,
		angle: 0.0,
		dynamics: Dynamics::default(),
		tip: None,
	};
	vec![
		round("Soft Round", 30.0, 0.0),
		round("Hard Round", 30.0, 1.0),
		round("Soft Round Pressure Size", 45.0, 0.0),
		round("Hard Round Pressure Opacity", 45.0, 1.0),
		Preset {
			name: "Spatter".into(),
			dynamics: Dynamics {
				size_jitter: 0.6,
				scatter: 1.2,
				count: 3,
				..Dynamics::default()
			},
			..round("Spatter", 26.0, 0.8)
		},
		Preset {
			name: "Chalk".into(),
			roundness: 0.4,
			angle: 35.0,
			dynamics: Dynamics {
				angle_jitter: 0.1,
				..Dynamics::default()
			},
			..round("Chalk", 36.0, 0.6)
		},
	]
}

/// Width and height of a preset thumbnail.
pub const THUMB: (u32, u32) = (96, 32);

/// The preset's thumbnail stroke, drawn by the brush engine's dab placer and
/// tips: an S curve, 8-bit coverage, `THUMB` pixels.
pub fn thumbnail(p: &Preset) -> Vec<u8> {
	let (w, h) = THUMB;
	let diameter = (h as f32 * 0.6).min(p.size.max(1.0));
	let params = BrushParams {
		diameter,
		hardness: p.hardness,
		roundness: p.roundness,
		angle: p.angle,
		spacing: p.spacing.max(0.02),
		tip: tip_id(p),
		dynamics: Dynamics {
			scatter: p.dynamics.scatter.min(0.5),
			..p.dynamics
		},
		seed: 7,
		..BrushParams::default()
	};
	let samples: Vec<StrokeSample> = (0..=24)
		.map(|i| {
			let t = f64::from(i) / 24.0;
			StrokeSample {
				x: 8.0 + t * f64::from(w - 16),
				y: f64::from(h) / 2.0 + (t * std::f64::consts::TAU).sin() * f64::from(h) * 0.22,
				pressure: (0.3 + 0.7 * (t * std::f64::consts::PI).sin()) as f32,
				tilt_x: 0.0,
				tilt_y: 0.0,
				time_us: 0,
			}
		})
		.collect();
	let mut path = DabPath::new(params);
	let dabs = path.push(&samples);
	let sampled = fx_ops::brush::tip::sampled(params.tip);
	let mut coverage = vec![0.0f32; (w * h) as usize];
	for dab in dabs {
		let tip = match &sampled {
			Some(t) => Tip::sampled(t.clone(), dab.diameter, dab.roundness, dab.angle, false),
			None => Tip::new(dab.diameter, params.hardness, dab.roundness, dab.angle, false),
		};
		let reach = tip.reach();
		let (x0, x1) = (
			(dab.x as f32 - reach).floor().max(0.0) as u32,
			((dab.x as f32 + reach).ceil() as u32).min(w - 1),
		);
		let (y0, y1) = (
			(dab.y as f32 - reach).floor().max(0.0) as u32,
			((dab.y as f32 + reach).ceil() as u32).min(h - 1),
		);
		for y in y0..=y1 {
			for x in x0..=x1 {
				let d = tip.coverage(x as f32 + 0.5 - dab.x as f32, y as f32 + 0.5 - dab.y as f32) * dab.strength;
				let c = &mut coverage[(y * w + x) as usize];
				*c = 1.0 - (1.0 - *c) * (1.0 - d * 0.5);
			}
		}
	}
	coverage.iter().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8).collect()
}
