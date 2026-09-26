//! The round brush tip (M5-T06): coverage of a pixel by one dab.
//!
//! Profile: 1 inside `hardness × R`, then a smooth fall-off to 0 at `R`:
//! `1 − smoothstep((r − hR) / (R − hR))` (VERIFY against Photoshop's curve).
//! Hardness 1 is a hard disc anti-aliased over one pixel. Tips under 4 px are
//! supersampled 4 × 4 so tiny brushes do not flicker as they move. The
//! pencil's tip is the hard disc thresholded at 50 % (no anti-aliasing).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A sampled tip (M8-T01): a grey image, 1 = paint, kept as one master and a
/// box-filtered pyramid of it (the "one master tip" rule: no per-size stamp
/// cache, every dab samples the level just above its size).
#[derive(Debug)]
pub struct SampledTip {
	/// `(width, height, coverage 0..=1)`, level 0 = the master.
	levels: Vec<(u32, u32, Vec<f32>)>,
}

impl SampledTip {
	/// A tip from an 8-bit grey image (255 = paint).
	pub fn new(width: u32, height: u32, gray: &[u8]) -> Self {
		let mut levels = vec![(width.max(1), height.max(1), gray.iter().map(|&v| f32::from(v) / 255.0).collect::<Vec<f32>>())];
		loop {
			let (w, h, px) = levels.last().expect("level 0 exists");
			if *w <= 2 && *h <= 2 {
				break;
			}
			let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
			let mut next = vec![0.0f32; (nw * nh) as usize];
			for y in 0..nh {
				for x in 0..nw {
					let mut sum = 0.0;
					let mut n = 0.0;
					for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
						let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
						sum += px[(sy * w + sx) as usize];
						n += 1.0;
					}
					next[(y * nw + x) as usize] = sum / n;
				}
			}
			levels.push((nw, nh, next));
		}
		Self { levels }
	}

	/// The master's size.
	pub fn size(&self) -> (u32, u32) {
		(self.levels[0].0, self.levels[0].1)
	}

	/// The level whose longest side is the smallest one still ≥ `diameter`.
	fn level_for(&self, diameter: f32) -> usize {
		let mut best = 0;
		for (i, (w, h, _)) in self.levels.iter().enumerate() {
			if (*w.max(h) as f32) >= diameter {
				best = i;
			}
		}
		best
	}

	/// Bilinear coverage at normalised tip coordinates (`-0.5..=0.5` on the
	/// longest side, centred), from level `level`.
	fn sample(&self, level: usize, u: f32, v: f32) -> f32 {
		let (w, h, px) = &self.levels[level];
		let long = *w.max(h) as f32;
		let x = u * long + *w as f32 / 2.0 - 0.5;
		let y = v * long + *h as f32 / 2.0 - 0.5;
		let (x0, y0) = (x.floor(), y.floor());
		let (fx, fy) = (x - x0, y - y0);
		let at = |xi: f32, yi: f32| -> f32 {
			if xi < 0.0 || yi < 0.0 || xi >= *w as f32 || yi >= *h as f32 {
				0.0
			} else {
				px[(yi as u32 * w + xi as u32) as usize]
			}
		};
		let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
		let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
		top * (1.0 - fy) + bottom * fy
	}
}

/// The process-wide sampled tips, by id.
fn registry() -> &'static Mutex<HashMap<u64, Arc<SampledTip>>> {
	static REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<SampledTip>>>> = OnceLock::new();
	REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a sampled tip; its id is a hash of its pixels (never 0), so the
/// same image registers once.
// FAST: tips live in a process-wide registry, not in the command: a stroke
// replayed in another session (a macro) without its tip paints round.
pub fn register(width: u32, height: u32, gray: &[u8]) -> u64 {
	let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
	for b in width.to_le_bytes().iter().chain(height.to_le_bytes().iter()).chain(gray.iter()) {
		hash ^= u64::from(*b);
		hash = hash.wrapping_mul(0x0100_0000_01b3);
	}
	// 52 bits: the id travels through JavaScript numbers.
	let id = (hash & ((1u64 << 52) - 1)) | 1;
	registry()
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner)
		.entry(id)
		.or_insert_with(|| Arc::new(SampledTip::new(width, height, gray)));
	id
}

/// A registered sampled tip.
pub fn sampled(id: u64) -> Option<Arc<SampledTip>> {
	if id == 0 {
		return None;
	}
	registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&id).cloned()
}

/// One dab's shape.
#[derive(Clone, Debug)]
pub struct Tip {
	/// Radius in document pixels.
	pub radius: f32,
	pub hardness: f32,
	/// `0..=1`; the tip's y axis is squashed by this.
	pub roundness: f32,
	/// `(cos, sin)` of the tip's angle.
	rotation: (f32, f32),
	/// The pencil: aliased edges.
	pub aliased: bool,
	/// A sampled tip and the pyramid level this dab reads (M8-T01).
	sample: Option<(Arc<SampledTip>, usize)>,
}

/// Below this diameter the coverage is supersampled.
const SUPERSAMPLE_BELOW: f32 = 4.0;

impl Tip {
	/// A tip of `diameter` pixels.
	pub fn new(diameter: f32, hardness: f32, roundness: f32, angle_deg: f32, aliased: bool) -> Self {
		let a = angle_deg.to_radians();
		Self {
			radius: (diameter / 2.0).max(0.5),
			hardness: if aliased { 1.0 } else { hardness.clamp(0.0, 1.0) },
			roundness: roundness.clamp(0.01, 1.0),
			rotation: (a.cos(), a.sin()),
			aliased,
			sample: None,
		}
	}

	/// A dab of a sampled tip (M8-T01): the image fills a square of
	/// `diameter` on its longest side, turned by the angle and squashed by the
	/// roundness like the round tip.
	pub fn sampled(tip: Arc<SampledTip>, diameter: f32, roundness: f32, angle_deg: f32, aliased: bool) -> Self {
		let mut t = Self::new(diameter, 1.0, roundness, angle_deg, aliased);
		let level = tip.level_for(diameter);
		t.sample = Some((tip, level));
		t
	}

	/// The distance from the dab's centre, past which coverage is 0 (plus
	/// the anti-aliasing pixel).
	pub fn reach(&self) -> f32 {
		if self.sample.is_some() {
			// The square's corners.
			return self.radius * std::f32::consts::SQRT_2 / self.roundness.max(0.01).sqrt() + 1.0;
		}
		self.radius + 1.0
	}

	/// Coverage (`0..=1`) of the pixel whose centre is `(dx, dy)` from the
	/// dab's centre.
	pub fn coverage(&self, dx: f32, dy: f32) -> f32 {
		if let Some((tip, level)) = &self.sample {
			let (c, s) = self.rotation;
			let u = (dx * c + dy * s) / (2.0 * self.radius);
			let v = (-dx * s + dy * c) / self.roundness / (2.0 * self.radius);
			if u.abs() > 0.5 || v.abs() > 0.5 {
				return 0.0;
			}
			let value = tip.sample(*level, u, v).clamp(0.0, 1.0);
			return if self.aliased { if value >= 0.5 { 1.0 } else { 0.0 } } else { value };
		}
		if self.aliased {
			return if self.raw(dx, dy) >= 0.5 { 1.0 } else { 0.0 };
		}
		if self.radius * 2.0 < SUPERSAMPLE_BELOW {
			let mut sum = 0.0;
			for j in 0..4 {
				for i in 0..4 {
					let ox = (i as f32 + 0.5) / 4.0 - 0.5;
					let oy = (j as f32 + 0.5) / 4.0 - 0.5;
					sum += self.point(dx + ox, dy + oy);
				}
			}
			return sum / 16.0;
		}
		self.raw(dx, dy)
	}

	/// The analytic profile at a pixel centre, with the hard edge anti-aliased
	/// over one pixel.
	fn raw(&self, dx: f32, dy: f32) -> f32 {
		let r = self.distance(dx, dy);
		let radius = self.radius;
		let core = self.hardness * radius;
		if self.hardness >= 1.0 || radius - core < 1.0 {
			// A hard edge: the pixel's overlap with the disc, to first order.
			return (radius - r + 0.5).clamp(0.0, 1.0);
		}
		if r <= core {
			return 1.0;
		}
		if r >= radius {
			return 0.0;
		}
		let t = (r - core) / (radius - core);
		1.0 - t * t * (3.0 - 2.0 * t)
	}

	/// The profile at one point (supersampling uses a hard 0/1 edge).
	fn point(&self, dx: f32, dy: f32) -> f32 {
		let r = self.distance(dx, dy);
		let core = self.hardness * self.radius;
		if r <= core {
			return 1.0;
		}
		if r >= self.radius {
			return 0.0;
		}
		let t = (r - core) / (self.radius - core).max(f32::EPSILON);
		1.0 - t * t * (3.0 - 2.0 * t)
	}

	/// Distance from the centre in tip space (rotated, the y axis stretched
	/// by 1 / roundness), in pixels of the unsquashed radius.
	fn distance(&self, dx: f32, dy: f32) -> f32 {
		if self.roundness >= 1.0 {
			return (dx * dx + dy * dy).sqrt();
		}
		let (c, s) = self.rotation;
		let u = dx * c + dy * s;
		let v = (-dx * s + dy * c) / self.roundness;
		(u * u + v * v).sqrt()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_hard_dab_covers_its_disc_area() {
		let tip = Tip::new(40.0, 1.0, 1.0, 0.0, false);
		let mut area = 0.0f64;
		for y in -25..25 {
			for x in -25..25 {
				area += f64::from(tip.coverage(x as f32 + 0.5, y as f32 + 0.5));
			}
		}
		let exact = std::f64::consts::PI * 20.0 * 20.0;
		assert!((area - exact).abs() / exact < 0.005, "{area} vs {exact}");
	}

	#[test]
	fn a_soft_dab_falls_off_to_zero_at_the_radius() {
		let tip = Tip::new(100.0, 0.0, 1.0, 0.0, false);
		assert!((tip.coverage(0.0, 0.0) - 1.0).abs() < 1e-6);
		assert!((tip.coverage(25.0, 0.0) - 0.5).abs() < 1e-6, "halfway: smoothstep(0.5) = 0.5");
		assert_eq!(tip.coverage(50.0, 0.0), 0.0);
	}

	#[test]
	fn the_pencil_is_aliased() {
		let tip = Tip::new(10.0, 0.3, 1.0, 0.0, true);
		for x in 0..8 {
			let c = tip.coverage(x as f32, 0.0);
			assert!(c == 0.0 || c == 1.0, "{c}");
		}
	}

	#[test]
	fn roundness_squashes_along_the_angle() {
		let tip = Tip::new(40.0, 1.0, 0.5, 0.0, false);
		assert_eq!(tip.coverage(15.0, 0.0), 1.0, "the long axis");
		assert_eq!(tip.coverage(0.0, 15.0), 0.0, "the short axis is 10 px");
		let turned = Tip::new(40.0, 1.0, 0.5, 90.0, false);
		assert_eq!(turned.coverage(0.0, 15.0), 1.0);
	}
}
