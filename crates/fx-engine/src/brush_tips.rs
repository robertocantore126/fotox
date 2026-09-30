//! Procedural sampled tips for the built-in brush categories (Dry Media, Wet
//! Media, Special Effects): grey images, 255 = paint, drawn once at start
//! from a fixed seed so every run makes the same pixels (and the same tip
//! ids).

/// Side of a generated tip.
pub const SIDE: u32 = 128;

/// A generated tip: `(width, height, grey)`.
pub type Generated = (u32, u32, Vec<u8>);

/// A small deterministic hash → `0..1`.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
	let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
	h ^= h >> 13;
	h = h.wrapping_mul(0x5bd1_e995);
	h ^= h >> 15;
	(h & 0x00ff_ffff) as f32 / 16_777_215.0
}

/// Smooth value noise at `(x, y)` with cells of `cell` pixels.
fn noise(x: f32, y: f32, cell: f32, seed: u32) -> f32 {
	let (fx, fy) = (x / cell, y / cell);
	let (x0, y0) = (fx.floor(), fy.floor());
	let (tx, ty) = (fx - x0, fy - y0);
	let s = |t: f32| t * t * (3.0 - 2.0 * t);
	let (sx, sy) = (s(tx), s(ty));
	let (ix, iy) = (x0 as i32, y0 as i32);
	let a = hash(ix, iy, seed) + (hash(ix + 1, iy, seed) - hash(ix, iy, seed)) * sx;
	let b = hash(ix, iy + 1, seed) + (hash(ix + 1, iy + 1, seed) - hash(ix, iy + 1, seed)) * sx;
	a + (b - a) * sy
}

/// Fill a `SIDE²` image from `f(x, y)` in `-1..1` tip coordinates (value `0..1`).
fn render(f: impl Fn(f32, f32, i32, i32) -> f32) -> Generated {
	let n = SIDE as i32;
	let mut gray = Vec::with_capacity((SIDE * SIDE) as usize);
	for y in 0..n {
		for x in 0..n {
			let u = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
			let v = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
			gray.push((f(u, v, x, y).clamp(0.0, 1.0) * 255.0).round() as u8);
		}
	}
	(SIDE, SIDE, gray)
}

/// `1` inside radius `r`, fading to 0 over `soft` (tip units).
fn disc(d: f32, r: f32, soft: f32) -> f32 {
	((r - d) / soft.max(1e-3) + 0.5).clamp(0.0, 1.0)
}

/// Chalk: a round tip broken up by paper grain.
pub fn chalk() -> Generated {
	render(|u, v, x, y| {
		let d = (u * u + v * v).sqrt();
		let edge = 0.85 + 0.12 * (noise(x as f32, y as f32, 9.0, 11) - 0.5);
		let grain = noise(x as f32, y as f32, 2.5, 12) * 0.6 + noise(x as f32, y as f32, 6.0, 13) * 0.4;
		disc(d, edge, 0.08) * ((grain - 0.32) * 2.6).clamp(0.0, 1.0)
	})
}

/// Charcoal: a flat, streaky stick.
pub fn charcoal() -> Generated {
	render(|u, v, x, y| {
		let d = ((u / 0.95).powi(2) + (v / 0.38).powi(2)).sqrt();
		let streak = noise(x as f32 * 0.25, y as f32 * 3.0, 3.0, 21);
		let grain = noise(x as f32, y as f32, 2.0, 22);
		disc(d, 0.9, 0.12) * ((streak * 0.7 + grain * 0.5 - 0.35) * 2.2).clamp(0.0, 1.0)
	})
}

/// Pencil: a small hard tip with speckle.
pub fn pencil() -> Generated {
	render(|u, v, x, y| {
		let d = (u * u + v * v).sqrt();
		let speck = hash(x, y, 31);
		disc(d, 0.8, 0.1) * if speck > 0.28 { 0.55 + 0.45 * speck } else { 0.1 }
	})
}

/// Dry brush: parallel streaks with gaps.
pub fn dry_brush() -> Generated {
	render(|u, v, _x, y| {
		let d = ((u / 0.95).powi(2) + (v / 0.6).powi(2)).sqrt();
		let row = noise(0.0, y as f32, 2.2, 41);
		let along = noise(u * 40.0, y as f32 * 0.5, 7.0, 42);
		disc(d, 0.92, 0.2) * ((row - 0.38) * 3.0).clamp(0.0, 1.0) * (0.6 + 0.4 * along)
	})
}

/// Bristle: a round cluster of soft hairs.
pub fn bristle() -> Generated {
	let hairs: Vec<(f32, f32, f32, f32)> = (0..34)
		.map(|i| {
			let a = hash(i, 0, 51) * std::f32::consts::TAU;
			let r = hash(i, 1, 51).sqrt() * 0.78;
			(a.cos() * r, a.sin() * r, 0.05 + 0.06 * hash(i, 2, 51), 0.45 + 0.55 * hash(i, 3, 51))
		})
		.collect();
	render(|u, v, _x, _y| {
		let mut c: f32 = 0.0;
		for &(hx, hy, hr, hi) in &hairs {
			let d = ((u - hx).powi(2) + (v - hy).powi(2)).sqrt();
			let k = (1.0 - d / hr).clamp(0.0, 1.0);
			c = 1.0 - (1.0 - c) * (1.0 - k * k * hi);
		}
		c
	})
}

/// Watercolour: an irregular blot, lighter inside with a darker rim.
pub fn watercolor() -> Generated {
	render(|u, v, x, y| {
		let d = (u * u + v * v).sqrt();
		let a = v.atan2(u);
		let edge = 0.72 + 0.16 * (noise(a * 6.0 + 10.0, 0.0, 1.0, 61) - 0.5) + 0.08 * (noise(x as f32, y as f32, 14.0, 62) - 0.5);
		let inside = disc(d, edge, 0.06);
		let rim = (1.0 - ((edge - d) / 0.12).clamp(0.0, 1.0)) * inside;
		let pool = 0.42 + 0.18 * noise(x as f32, y as f32, 18.0, 63);
		inside * (pool + 0.45 * rim)
	})
}

/// Spatter: a cluster of round drops.
pub fn spatter() -> Generated {
	let drops: Vec<(f32, f32, f32)> = (0..26)
		.map(|i| {
			let a = hash(i, 0, 71) * std::f32::consts::TAU;
			let r = hash(i, 1, 71).powf(0.7) * 0.82;
			(a.cos() * r, a.sin() * r, 0.03 + 0.11 * hash(i, 2, 71).powi(2))
		})
		.collect();
	render(|u, v, _x, _y| {
		drops
			.iter()
			.map(|&(dx, dy, dr)| disc(((u - dx).powi(2) + (v - dy).powi(2)).sqrt(), dr, 0.03))
			.fold(0.0, f32::max)
	})
}

/// Sponge: a round tip full of holes.
pub fn sponge() -> Generated {
	render(|u, v, x, y| {
		let d = (u * u + v * v).sqrt();
		let pores = noise(x as f32, y as f32, 5.0, 81) * 0.65 + noise(x as f32, y as f32, 11.0, 82) * 0.35;
		disc(d, 0.85, 0.15) * ((pores - 0.45) * 5.0).clamp(0.0, 1.0)
	})
}

/// Stipple: a few tiny dots.
pub fn stipple() -> Generated {
	render(|u, v, x, y| {
		let d = (u * u + v * v).sqrt();
		let cell = 10;
		let (cx, cy) = (x / cell, y / cell);
		let (ox, oy) = (hash(cx, cy, 91), hash(cx, cy, 92));
		let (px, py) = (cx as f32 * cell as f32 + 2.0 + ox * 6.0, cy as f32 * cell as f32 + 2.0 + oy * 6.0);
		let dot = disc(((x as f32 - px).powi(2) + (y as f32 - py).powi(2)).sqrt(), 1.8, 1.0);
		if hash(cx, cy, 93) > 0.5 { dot * disc(d, 0.9, 0.05) } else { 0.0 }
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_generated_tip_paints_something_and_is_deterministic() {
		for f in [chalk, charcoal, pencil, dry_brush, bristle, watercolor, spatter, sponge, stipple] {
			let (w, h, a) = f();
			assert_eq!(a.len(), (w * h) as usize);
			let painted = a.iter().filter(|&&v| v > 32).count();
			assert!(painted > 50, "{painted} painted pixels");
			assert_eq!(a, f().2);
		}
	}
}
