//! Color Dynamics (M8-T01): the stroke's colour jittered toward the
//! background and in hue / saturation / brightness, from the stroke's seed.
//!
//! FAST: once per stroke (the stroke model paints one colour); Photoshop's
//! "Apply Per Tip" varies it per dab.

use fx_core::stroke::Dynamics;

use super::path::Jitter;

/// The colour a stroke paints with, straight 16-bit RGBA.
pub fn jitter_color(fg: [u16; 4], bg: [u16; 4], d: &Dynamics, seed: u64) -> [u16; 4] {
	if d.fg_bg_jitter == 0.0 && d.hue_jitter == 0.0 && d.saturation_jitter == 0.0 && d.brightness_jitter == 0.0 {
		return fg;
	}
	let mut rng = Jitter::new(seed.rotate_left(17) ^ 0x00c0_10c0);
	let t = f64::from(d.fg_bg_jitter.clamp(0.0, 1.0) * rng.next());
	let mut rgb = [0usize, 1, 2].map(|i| (f64::from(fg[i]) + (f64::from(bg[i]) - f64::from(fg[i])) * t) / 65535.0);
	let (mut h, mut s, mut v) = rgb_to_hsv(rgb);
	h = (h + f64::from(d.hue_jitter.clamp(0.0, 1.0) * rng.signed()) * 0.5).rem_euclid(1.0);
	s = (s + f64::from(d.saturation_jitter.clamp(0.0, 1.0) * rng.signed())).clamp(0.0, 1.0);
	v = (v + f64::from(d.brightness_jitter.clamp(0.0, 1.0) * rng.signed())).clamp(0.0, 1.0);
	rgb = hsv_to_rgb(h, s, v);
	[
		(rgb[0] * 65535.0).round() as u16,
		(rgb[1] * 65535.0).round() as u16,
		(rgb[2] * 65535.0).round() as u16,
		fg[3],
	]
}

/// RGB `0..=1` → hue, saturation, value, all `0..=1`.
pub fn rgb_to_hsv([r, g, b]: [f64; 3]) -> (f64, f64, f64) {
	let max = r.max(g).max(b);
	let min = r.min(g).min(b);
	let delta = max - min;
	let h = if delta <= 0.0 {
		0.0
	} else if max == r {
		((g - b) / delta).rem_euclid(6.0) / 6.0
	} else if max == g {
		((b - r) / delta + 2.0) / 6.0
	} else {
		((r - g) / delta + 4.0) / 6.0
	};
	let s = if max <= 0.0 { 0.0 } else { delta / max };
	(h, s, max)
}

/// Hue, saturation, value `0..=1` → RGB `0..=1`.
pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
	let h6 = h.rem_euclid(1.0) * 6.0;
	let i = h6.floor();
	let f = h6 - i;
	let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
	match i as i32 {
		0 => [v, t, p],
		1 => [q, v, p],
		2 => [p, v, t],
		3 => [p, q, v],
		4 => [t, p, v],
		_ => [v, p, q],
	}
}
