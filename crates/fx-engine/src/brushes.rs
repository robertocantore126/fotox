//! Brush presets and sampled tips (M8-T01, D-063).
//!
//! The library is `%APPDATA%\Fotox\brushes.json`: a list of presets, each the
//! option-bar values a brush needs (size, hardness, spacing, roundness,
//! angle), its dynamics and, for a sampled tip, the tip's grey pixels
//! (base64). At start every tip is registered with the brush engine
//! (`fx_ops::brush::tip::register`), so a preset's `tip` id is what the
//! option bar sends back. `.abr` files add their sampled tips as presets.
//!
//! Presets belong to a `group` (the picker's folders: General Brushes, Dry
//! Media, Wet Media, Special Effects, Fotox Classic, one per imported file…).
//! A library saved before groups existed gets the built-in groups added and
//! keeps its own presets under "My Brushes".

use std::path::PathBuf;

use fx_core::stroke::{BrushParams, Control, Dynamics, StrokeSample, TipProfile};
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
	/// The picker folder; empty = "My Brushes".
	#[serde(default)]
	pub group: String,
	/// The round tip's fall-off.
	#[serde(default)]
	pub profile: TipProfile,
}

/// The folder presets without a group are shown in.
pub const MY_BRUSHES: &str = "My Brushes";

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
		let presets = match path()
			.and_then(|p| std::fs::read_to_string(p).ok())
			.and_then(|text| serde_json::from_str::<Vec<Preset>>(&text).ok())
		{
			// Saved before the groups: the built-in ones first, the file's
			// own presets after them.
			Some(own) if own.iter().all(|p| p.group.is_empty()) => {
				let mut all = defaults();
				all.extend(own.into_iter().map(|p| Preset { group: MY_BRUSHES.into(), ..p }));
				all
			}
			Some(presets) => presets,
			None => defaults(),
		};
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
					"group": if p.group.is_empty() { MY_BRUSHES } else { p.group.as_str() },
					"profile": p.profile,
					"thumb": crate::b64::encode(&thumbnail(p)),
					"tip_thumb": crate::b64::encode(&tip_thumbnail(p)),
				})
			})
			.collect()
	}

	/// Add the sampled tips of an `.abr` file, in a group named after it.
	pub fn import_abr(&mut self, data: &[u8], file_name: &str) -> Result<usize, String> {
		let tips = fx_io::abr::read_abr(data).map_err(|e| e.to_string())?;
		let n = tips.len();
		let group = file_name.trim_end_matches(".abr").trim_end_matches(".ABR");
		let group = if group.is_empty() { "Imported" } else { group }.to_owned();
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
				group: group.clone(),
				profile: TipProfile::Classic,
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
	let round = |group: &str, name: &str, size: f32, hardness: f32, profile: TipProfile| Preset {
		name: name.into(),
		size,
		hardness,
		spacing: 0.25,
		roundness: 1.0,
		angle: 0.0,
		dynamics: Dynamics::default(),
		tip: None,
		group: group.into(),
		profile,
	};
	let sampled = |group: &str, name: &str, size: f32, spacing: f32, (width, height, gray): crate::brush_tips::Generated, dynamics: Dynamics| Preset {
		name: name.into(),
		size,
		hardness: 1.0,
		spacing,
		roundness: 1.0,
		angle: 0.0,
		dynamics,
		tip: Some(TipData {
			width,
			height,
			gray: crate::b64::encode(&gray),
		}),
		group: group.into(),
		profile: TipProfile::Classic,
	};
	let pressure_size = Dynamics {
		size_control: Control::PenPressure,
		..Dynamics::default()
	};
	let pressure_opacity = Dynamics {
		opacity_control: Control::PenPressure,
		..Dynamics::default()
	};
	use crate::brush_tips as t;
	const GENERAL: &str = "General Brushes";
	const DRY: &str = "Dry Media";
	const WET: &str = "Wet Media";
	const FX: &str = "Special Effects";
	const CLASSIC: &str = "Fotox Classic";
	vec![
		// Photoshop's soft round, measured from Photopea (TipProfile::Gaussian).
		round(GENERAL, "Soft Round", 45.0, 0.0, TipProfile::Gaussian),
		round(GENERAL, "Hard Round", 30.0, 1.0, TipProfile::Gaussian),
		Preset {
			dynamics: pressure_size,
			..round(GENERAL, "Soft Round Pressure Size", 45.0, 0.0, TipProfile::Gaussian)
		},
		Preset {
			dynamics: pressure_opacity,
			..round(GENERAL, "Soft Round Pressure Opacity", 45.0, 0.0, TipProfile::Gaussian)
		},
		Preset {
			dynamics: pressure_size,
			..round(GENERAL, "Hard Round Pressure Size", 30.0, 1.0, TipProfile::Gaussian)
		},
		Preset {
			dynamics: pressure_opacity,
			..round(GENERAL, "Hard Round Pressure Opacity", 30.0, 1.0, TipProfile::Gaussian)
		},
		Preset {
			spacing: 0.1,
			..round(GENERAL, "Soft Airbrush", 120.0, 0.0, TipProfile::Gaussian)
		},
		Preset {
			dynamics: Dynamics {
				size_control: Control::PenPressure,
				min_diameter: 0.15,
				..Dynamics::default()
			},
			spacing: 0.08,
			..round(GENERAL, "Inking Pen", 12.0, 0.9, TipProfile::Gaussian)
		},
		sampled(
			DRY,
			"Chalk",
			60.0,
			0.15,
			t::chalk(),
			Dynamics {
				angle_jitter: 0.1,
				..Dynamics::default()
			},
		),
		sampled(
			DRY,
			"Charcoal",
			50.0,
			0.1,
			t::charcoal(),
			Dynamics {
				angle_jitter: 0.04,
				size_jitter: 0.1,
				..Dynamics::default()
			},
		),
		sampled(DRY, "Pencil Grain", 14.0, 0.12, t::pencil(), pressure_opacity),
		sampled(DRY, "Dry Brush", 70.0, 0.05, t::dry_brush(), Dynamics::default()),
		sampled(
			WET,
			"Watercolor Blot",
			90.0,
			0.18,
			t::watercolor(),
			Dynamics {
				size_jitter: 0.25,
				angle_jitter: 1.0,
				..Dynamics::default()
			},
		),
		sampled(WET, "Round Bristle", 60.0, 0.04, t::bristle(), Dynamics::default()),
		Preset {
			dynamics: Dynamics {
				size_control: Control::PenPressure,
				min_diameter: 0.3,
				flow_control: Control::PenPressure,
				..Dynamics::default()
			},
			spacing: 0.05,
			..round(WET, "Wet Ink", 24.0, 0.7, TipProfile::Gaussian)
		},
		sampled(
			FX,
			"Spatter",
			80.0,
			0.6,
			t::spatter(),
			Dynamics {
				size_jitter: 0.5,
				angle_jitter: 1.0,
				scatter: 0.8,
				count: 2,
				..Dynamics::default()
			},
		),
		sampled(
			FX,
			"Sponge",
			70.0,
			0.3,
			t::sponge(),
			Dynamics {
				angle_jitter: 1.0,
				scatter: 0.3,
				..Dynamics::default()
			},
		),
		sampled(
			FX,
			"Stipple",
			60.0,
			0.5,
			t::stipple(),
			Dynamics {
				angle_jitter: 1.0,
				scatter: 1.0,
				count: 3,
				..Dynamics::default()
			},
		),
		// The tips Fotox had before the measured one.
		round(CLASSIC, "Soft Round (classic)", 30.0, 0.0, TipProfile::Classic),
		round(CLASSIC, "Hard Round (classic)", 30.0, 1.0, TipProfile::Classic),
		Preset {
			roundness: 0.4,
			angle: 35.0,
			dynamics: Dynamics {
				angle_jitter: 0.1,
				..Dynamics::default()
			},
			..round(CLASSIC, "Flat Chisel", 36.0, 0.6, TipProfile::Classic)
		},
	]
}

/// Side of a preset's tip thumbnail (the picker's grid).
pub const TIP_THUMB: u32 = 40;

/// One dab of the preset's tip, fitted in `TIP_THUMB²`, 8-bit coverage.
pub fn tip_thumbnail(p: &Preset) -> Vec<u8> {
	let n = TIP_THUMB;
	let sampled = fx_ops::brush::tip::sampled(tip_id(p));
	// Soft Gaussian tips reach ~1.6 R: leave room for the tail.
	let diameter = if sampled.is_none() && p.profile == TipProfile::Gaussian && p.hardness < 0.5 {
		n as f32 * 0.6
	} else {
		n as f32 * 0.9
	};
	let tip = match &sampled {
		Some(t) => Tip::sampled(t.clone(), diameter, p.roundness, p.angle, false),
		None => Tip::new(diameter, p.hardness, p.roundness, p.angle, false).with_profile(p.profile),
	};
	let c = n as f32 / 2.0;
	(0..n * n)
		.map(|i| {
			let (x, y) = ((i % n) as f32 + 0.5 - c, (i / n) as f32 + 0.5 - c);
			(tip.coverage(x, y).clamp(0.0, 1.0) * 255.0).round() as u8
		})
		.collect()
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
		profile: p.profile,
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
			None => Tip::new(dab.diameter, params.hardness, dab.roundness, dab.angle, false).with_profile(params.profile),
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
