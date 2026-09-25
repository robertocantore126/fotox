//! Dabs along the pointer path (M5-T06).
//!
//! A dab is placed every `spacing × current diameter` along the polyline of
//! the samples. The distance left over at the end of a segment is carried to
//! the next one, *also across event batches*: the path keeps its state, so a
//! stroke gives the same dabs however the samples were batched. Pressure, tilt
//! and time are interpolated linearly between samples.

use fx_core::stroke::{BrushParams, StrokeSample};

/// One stamp of the tip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
	/// Centre, document pixels.
	pub x: f64,
	pub y: f64,
	/// Diameter after pressure.
	pub diameter: f32,
	/// Pressure factor on the dab's strength (1 without pen pressure).
	pub strength: f32,
	/// The input time it came from (latency statistics, M5-T11).
	pub time_us: u64,
}

/// The dab placer of one stroke.
#[derive(Clone, Debug)]
pub struct DabPath {
	params: BrushParams,
	last: Option<StrokeSample>,
	/// Distance travelled since the last dab.
	carry: f64,
}

/// Smallest distance between dabs, in pixels (a 1 px brush at 1 % spacing
/// would otherwise stamp a hundred dabs per pixel).
const MIN_STEP: f64 = 0.25;

impl DabPath {
	/// A new path for `params`.
	pub fn new(params: BrushParams) -> Self {
		Self {
			params,
			last: None,
			carry: 0.0,
		}
	}

	/// The dab for a sample (pressure dynamics applied).
	fn dab(&self, s: &StrokeSample) -> Dab {
		let pressure = s.pressure.clamp(0.0, 1.0);
		let diameter = if self.params.pressure_size {
			(self.params.diameter * pressure).max(1.0)
		} else {
			self.params.diameter
		};
		Dab {
			x: s.x,
			y: s.y,
			diameter,
			strength: if self.params.pressure_opacity { pressure } else { 1.0 },
			time_us: s.time_us,
		}
	}

	/// The distance to the next dab at a sample.
	fn step(&self, s: &StrokeSample) -> f64 {
		(f64::from(self.params.spacing) * f64::from(self.dab(s).diameter)).max(MIN_STEP)
	}

	/// Feed samples; returns the dabs they place.
	pub fn push(&mut self, samples: &[StrokeSample]) -> Vec<Dab> {
		let mut dabs = Vec::new();
		for s in samples {
			let Some(last) = self.last else {
				// The first sample always stamps.
				dabs.push(self.dab(s));
				self.last = Some(*s);
				self.carry = 0.0;
				continue;
			};
			let (dx, dy) = (s.x - last.x, s.y - last.y);
			let length = (dx * dx + dy * dy).sqrt();
			if length <= 0.0 {
				self.last = Some(*s);
				continue;
			}
			// Walk the segment: `t` is how far along it we are, 0..=length.
			let mut t = 0.0;
			loop {
				let here = lerp_sample(&last, s, t / length);
				let needed = self.step(&here) - self.carry;
				if t + needed > length {
					self.carry += length - t;
					break;
				}
				t += needed;
				self.carry = 0.0;
				dabs.push(self.dab(&lerp_sample(&last, s, t / length)));
			}
			self.last = Some(*s);
		}
		dabs
	}
}

/// The sample a fraction `f` of the way from `a` to `b`.
fn lerp_sample(a: &StrokeSample, b: &StrokeSample, f: f64) -> StrokeSample {
	let ff = f as f32;
	StrokeSample {
		x: a.x + (b.x - a.x) * f,
		y: a.y + (b.y - a.y) * f,
		pressure: a.pressure + (b.pressure - a.pressure) * ff,
		tilt_x: a.tilt_x + (b.tilt_x - a.tilt_x) * ff,
		tilt_y: a.tilt_y + (b.tilt_y - a.tilt_y) * ff,
		time_us: a.time_us + ((b.time_us.saturating_sub(a.time_us)) as f64 * f) as u64,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn sample(x: f64, y: f64, pressure: f32) -> StrokeSample {
		StrokeSample {
			x,
			y,
			pressure,
			tilt_x: 0.0,
			tilt_y: 0.0,
			time_us: 0,
		}
	}

	#[test]
	fn dabs_are_spaced_by_a_fraction_of_the_diameter() {
		let mut path = DabPath::new(BrushParams {
			diameter: 40.0,
			spacing: 0.25,
			..Default::default()
		});
		let dabs = path.push(&[sample(0.0, 0.0, 1.0), sample(100.0, 0.0, 1.0)]);
		let xs: Vec<f64> = dabs.iter().map(|d| d.x).collect();
		assert_eq!(xs, [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0]);
	}

	#[test]
	fn spacing_does_not_depend_on_how_the_events_were_batched() {
		let params = BrushParams {
			diameter: 17.0,
			spacing: 0.3,
			pressure_size: true,
			..Default::default()
		};
		let samples: Vec<StrokeSample> = (0..50)
			.map(|i| {
				let t = f64::from(i);
				sample(t * 7.3, (t * 0.4).sin() * 30.0, 0.3 + 0.7 * (i as f32 / 50.0))
			})
			.collect();
		let mut once = DabPath::new(params);
		let all = once.push(&samples);
		let mut batched = DabPath::new(params);
		let mut pieces = Vec::new();
		for chunk in samples.chunks(7) {
			pieces.extend(batched.push(chunk));
		}
		assert_eq!(all, pieces);
		assert!(all.len() > 50);
	}

	#[test]
	fn pressure_scales_the_diameter_and_the_strength() {
		let mut path = DabPath::new(BrushParams {
			diameter: 40.0,
			pressure_size: true,
			pressure_opacity: true,
			..Default::default()
		});
		let dab = path.push(&[sample(0.0, 0.0, 0.5)])[0];
		assert_eq!((dab.diameter, dab.strength), (20.0, 0.5));
	}
}
