//! Notes, counts and colour samplers (M9-T08, D-072): document data saved in
//! the `.fxd`, never printed or exported.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
	pub x: f64,
	pub y: f64,
	pub author: String,
	pub text: String,
	/// Straight 16-bit RGBA.
	pub color: [u16; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CountGroup {
	pub name: String,
	pub color: [u16; 4],
	/// Marker size and label size, pixels.
	pub marker: f32,
	pub label: f32,
	pub visible: bool,
	pub points: Vec<(f64, f64)>,
}

/// A Color Sampler point; the average size is the option bar's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sampler {
	pub x: f64,
	pub y: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Annotations {
	#[serde(default)]
	pub notes: Vec<Note>,
	#[serde(default)]
	pub counts: Vec<CountGroup>,
	#[serde(default)]
	pub samplers: Vec<Sampler>,
}
