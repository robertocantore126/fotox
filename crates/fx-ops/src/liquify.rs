//! Liquify's brushes (M11-T06, D-078) on a backward displacement field
//! (`fx_core::warp_map::DispField`, nodes every 4 px): the destination point
//! `p` shows the source at `p + d(p)`.
//!
//! Every brush is a local re-mapping `q(p)` of the destination, weighted by
//! the brush falloff: the new field is `d'(p) = q(p) + d(q(p)) − p`, read
//! from the field as it was before the dab.
//!
//! FAST: no Freeze / Thaw mask, no stylus pressure, one dab per event for
//! the "held" brushes (Twirl, Pucker, Bloat, Reconstruct, Smooth).

use fx_core::warp_map::DispField;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
	ForwardWarp,
	Reconstruct,
	Smooth,
	TwirlClockwise,
	TwirlCounter,
	Pucker,
	Bloat,
	PushLeft,
}

impl Brush {
	pub fn from_name(name: &str) -> Brush {
		match name {
			"Reconstruct" => Brush::Reconstruct,
			"Smooth" => Brush::Smooth,
			"Twirl Clockwise" => Brush::TwirlClockwise,
			"Twirl Counterclockwise" => Brush::TwirlCounter,
			"Pucker" => Brush::Pucker,
			"Bloat" => Brush::Bloat,
			"Push Left" => Brush::PushLeft,
			_ => Brush::ForwardWarp,
		}
	}
}

/// One dab at `centre` of `radius` document pixels. `motion` is the pointer's
/// movement since the last dab (Forward Warp, Push Left); `strength` `0..=1`
/// is the bar's Pressure (× Rate for the held brushes).
pub fn dab(field: &mut DispField, brush: Brush, centre: (f64, f64), radius: f64, motion: (f64, f64), strength: f64) {
	let before = field.clone();
	let cell = field.cell;
	let (i0, j0) = (((centre.0 - radius) / cell).floor() as i32, ((centre.1 - radius) / cell).floor() as i32);
	let (i1, j1) = (((centre.0 + radius) / cell).ceil() as i32, ((centre.1 + radius) / cell).ceil() as i32);
	for j in j0..=j1 {
		for i in i0..=i1 {
			let p = (f64::from(i) * cell, f64::from(j) * cell);
			let (dx, dy) = (p.0 - centre.0, p.1 - centre.1);
			let r = (dx * dx + dy * dy).sqrt() / radius;
			if r >= 1.0 {
				continue;
			}
			// Smooth falloff (VERIFY: Photoshop's brush profile).
			let w = strength * (1.0 - r * r) * (1.0 - r * r);
			let new = match brush {
				Brush::Reconstruct => {
					let d = before.node(i, j);
					let k = (1.0 - w) as f32;
					[d[0] * k, d[1] * k]
				}
				Brush::Smooth => {
					let mut acc = [0.0f32; 2];
					for (a, b) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
						let n = before.node(i + a, j + b);
						acc[0] += n[0] / 4.0;
						acc[1] += n[1] / 4.0;
					}
					let d = before.node(i, j);
					let k = w as f32;
					[d[0] + (acc[0] - d[0]) * k, d[1] + (acc[1] - d[1]) * k]
				}
				_ => {
					let q = match brush {
						Brush::ForwardWarp => (p.0 - motion.0 * w, p.1 - motion.1 * w),
						Brush::PushLeft => {
							// Left of the stroke's direction moves… left of it.
							(p.0 + motion.1 * w, p.1 - motion.0 * w)
						}
						Brush::TwirlClockwise | Brush::TwirlCounter => {
							let sign = if brush == Brush::TwirlClockwise { -1.0 } else { 1.0 };
							let a = sign * 0.15 * w;
							let (s, c) = a.sin_cos();
							(centre.0 + dx * c - dy * s, centre.1 + dx * s + dy * c)
						}
						Brush::Pucker => (centre.0 + dx * (1.0 + 0.1 * w), centre.1 + dy * (1.0 + 0.1 * w)),
						Brush::Bloat => (centre.0 + dx * (1.0 - 0.1 * w), centre.1 + dy * (1.0 - 0.1 * w)),
						_ => p,
					};
					let d = before.at(q.0, q.1);
					[(q.0 + d[0] - p.0) as f32, (q.1 + d[1] - p.1) as f32]
				}
			};
			*field.node_mut(i, j) = new;
		}
	}
}
