//! The common filter families (M12-T03b, D-084) on the M4 tile driver: one
//! output tile at a time, at any level, distances scaled by 2⁻ᴸ.
//!
//! Every filter here reads a window (the tile grown by its apron) through
//! [`gather`] and returns the tile's **straight** pixels.
//!
//! FAST: square windows (Median, Minimum, Maximum, Dust & Scratches); Radial
//! Blur's window is capped at [`MAX_APRON`] (far from the centre the streaks
//! are clipped); noise and clouds are hashed per level-0 pixel, so a preview
//! level shows a subsample of them; Photoshop's exact formulas are VERIFY.

use fx_core::FilterParams;
use fx_tiles::TILE_SIZE;

use crate::filter::Geometry;
use crate::gaussian;
use crate::neighbourhood::{LevelSource, Px, Rect, gather, unpremul};
use fx_tiles::TileError;

/// The largest apron (pixels of the level) a filter window gets.
pub const MAX_APRON: i64 = 1024;

fn scale(level: usize) -> f32 {
	(1u32 << level) as f32
}

fn rad(r: f32, level: usize) -> i64 {
	((r / scale(level)).round() as i64).clamp(0, MAX_APRON)
}

/// How far (pixels of `level`) the filter reads around a pixel.
pub fn apron(params: &FilterParams, level: usize, geometry: &Geometry) -> i64 {
	match params {
		FilterParams::BoxBlur { radius } | FilterParams::Median { radius } | FilterParams::Minimum { radius } | FilterParams::Maximum { radius } => {
			rad(*radius, level)
		}
		FilterParams::SurfaceBlur { radius, .. } | FilterParams::DustScratches { radius, .. } => rad(*radius, level),
		// Exactly the Gaussian's radius: `gaussian::blur` returns the inner tile.
		FilterParams::HighPass { radius } => gaussian::radius((radius / scale(level)).clamp(0.1, 32.0)) as i64,
		FilterParams::MotionBlur { distance, .. } => rad(distance / 2.0, level) + 2,
		FilterParams::RadialBlur { .. } => {
			let c = geometry.canvas_at(level);
			((c.width().max(c.height()) as i64) / 2).min(MAX_APRON)
		}
		FilterParams::Emboss { height, .. } => rad(*height, level) + 1,
		FilterParams::Despeckle | FilterParams::Sharpen { .. } | FilterParams::FindEdges => 2,
		FilterParams::Offset { dx, dy, .. } => rad(dx.abs().max(dy.abs()), level) + 1,
		_ => 0,
	}
}

/// Tiles (of `level`) the filter spreads content by, for the output set.
pub fn spread(params: &FilterParams, level: usize, geometry: &Geometry) -> u32 {
	match params {
		FilterParams::AddNoise { .. } | FilterParams::Emboss { .. } | FilterParams::Sharpen { .. } | FilterParams::FindEdges | FilterParams::Despeckle => 0,
		// Clouds cover the whole layer: the caller treats `u32::MAX` as "all".
		FilterParams::Clouds { .. } => u32::MAX,
		_ => (apron(params, level, geometry) as u32).div_ceil(TILE_SIZE),
	}
}

/// A 32-bit hash of a pixel position and a seed.
fn hash(x: i64, y: i64, seed: u32) -> u32 {
	let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
	h ^= h >> 13;
	h = h.wrapping_mul(0x5bd1_e995);
	h ^ (h >> 15)
}

fn unit(h: u32) -> f32 {
	(h as f32) / (u32::MAX as f32)
}

fn luma(p: Px) -> f32 {
	0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// The filtered tile's straight pixels (256² row-major).
pub fn tile(src: &dyn LevelSource, geometry: &Geometry, params: &FilterParams, level: usize, tile: Rect) -> Result<Vec<Px>, TileError> {
	let t = TILE_SIZE as usize;
	let canvas = geometry.edge_canvas_at(level);
	if let FilterParams::Clouds { fg, bg, seed } = params {
		return Ok(clouds(tile, level, *fg, *bg, *seed));
	}
	let a = apron(params, level, geometry);
	let area = Rect {
		x0: tile.x0 - a,
		y0: tile.y0 - a,
		x1: tile.x1 + a,
		y1: tile.y1 + a,
	};
	let (aw, ah) = (area.width(), area.height());
	let win = gather(src, level, area, canvas)?;
	let at = |x: i64, y: i64| -> Px {
		let (xx, yy) = ((x - area.x0).clamp(0, aw as i64 - 1) as usize, (y - area.y0).clamp(0, ah as i64 - 1) as usize);
		win[yy * aw + xx]
	};
	let a_us = a as usize;
	let orig = |i: usize| -> Px { win[(i / t + a_us) * aw + i % t + a_us] };
	let mut out = vec![[0.0f32; 4]; t * t];
	match params {
		FilterParams::BoxBlur { .. } => {
			let blurred = box_blur(&win, aw, ah, a_us);
			for (i, o) in out.iter_mut().enumerate() {
				*o = unpremul(blurred[(i / t + a_us) * aw + i % t + a_us]);
			}
		}
		FilterParams::MotionBlur { angle, distance } => {
			let d = distance / scale(level);
			let (s, c) = angle.to_radians().sin_cos();
			let n = (d.ceil() as usize).max(1) + 1;
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let mut acc = [0.0f32; 4];
				for k in 0..n {
					let f = (k as f32 / (n - 1).max(1) as f32 - 0.5) * d;
					let p = at(x + (f * c).round() as i64, y - (f * s).round() as i64);
					for ch in 0..4 {
						acc[ch] += p[ch] / n as f32;
					}
				}
				*o = unpremul(acc);
			}
		}
		FilterParams::RadialBlur { amount, zoom } => {
			let cv = geometry.canvas_at(level);
			let (cx, cy) = ((cv.x0 + cv.x1) as f32 / 2.0, (cv.y0 + cv.y1) as f32 / 2.0);
			let n = 16usize;
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = ((tile.x0 + (i % t) as i64) as f32, (tile.y0 + (i / t) as i64) as f32);
				let (dx, dy) = (x - cx, y - cy);
				let mut acc = [0.0f32; 4];
				for k in 0..n {
					let f = k as f32 / (n - 1) as f32 - 0.5;
					let (sx, sy) = if *zoom {
						// VERIFY: Photoshop's zoom length per amount.
						let z = 1.0 + f * amount / 100.0 * 0.4;
						(cx + dx * z, cy + dy * z)
					} else {
						// VERIFY: Spin's angle per amount (here amount° in total).
						let ang = f * amount.to_radians();
						let (s, c) = ang.sin_cos();
						(cx + dx * c - dy * s, cy + dx * s + dy * c)
					};
					let p = at(sx.round() as i64, sy.round() as i64);
					for ch in 0..4 {
						acc[ch] += p[ch] / n as f32;
					}
				}
				*o = unpremul(acc);
			}
		}
		FilterParams::SurfaceBlur { threshold, .. } => {
			let th = (threshold / 255.0).max(1e-3) * 2.5;
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let c = unpremul(at(x, y));
				let mut acc = [0.0f32; 4];
				let mut wsum = 0.0f32;
				for j in -a..=a {
					for k in -a..=a {
						let p = unpremul(at(x + k, y + j));
						let w = (1.0 - (luma(p) - luma(c)).abs() / th).max(0.0) * p[3];
						for ch in 0..3 {
							acc[ch] += p[ch] * w;
						}
						wsum += w;
					}
				}
				*o = if wsum > 0.0 { [acc[0] / wsum, acc[1] / wsum, acc[2] / wsum, c[3]] } else { c };
			}
		}
		FilterParams::AddNoise {
			amount,
			gaussian: gauss,
			monochromatic,
			seed,
		} => {
			let k = amount / 100.0;
			let s = 1i64 << level;
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = ((tile.x0 + (i % t) as i64) * s, (tile.y0 + (i / t) as i64) * s);
				let mut p = unpremul(orig(i));
				let noise = |ch: u32| -> f32 {
					let h = hash(x, y, seed.wrapping_add(ch * 7919));
					if *gauss {
						// Box–Muller from two hashes.
						let (u1, u2) = (unit(h).max(1e-7), unit(hash(y, x, seed.wrapping_add(ch * 104_729))));
						(-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos() * 0.5
					} else {
						unit(h) * 2.0 - 1.0
					}
				};
				let mono = noise(0);
				for ch in 0..3 {
					let n = if *monochromatic { mono } else { noise(ch as u32) };
					p[ch] = (p[ch] + n * k).clamp(0.0, 1.0);
				}
				*o = p;
			}
		}
		FilterParams::Median { .. } | FilterParams::DustScratches { .. } => {
			let threshold = match params {
				FilterParams::DustScratches { threshold, .. } => threshold / 255.0,
				_ => -1.0,
			};
			let mut buf = Vec::with_capacity((2 * a_us + 1).pow(2));
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let c = orig(i);
				let mut m = [0.0f32; 4];
				for ch in 0..4 {
					buf.clear();
					for j in -a..=a {
						for k in -a..=a {
							buf.push(at(x + k, y + j)[ch]);
						}
					}
					let mid = buf.len() / 2;
					buf.select_nth_unstable_by(mid, f32::total_cmp);
					m[ch] = buf[mid];
				}
				let differs = (0..4).any(|ch| (m[ch] - c[ch]).abs() > threshold);
				*o = unpremul(if differs { m } else { c });
			}
		}
		FilterParams::Despeckle => {
			// A 3 × 3 median where the neighbourhood is flat (no edge).
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let c = orig(i);
				let edge = (luma(at(x + 1, y)) - luma(at(x - 1, y))).abs() + (luma(at(x, y + 1)) - luma(at(x, y - 1))).abs() > 0.25;
				if edge {
					*o = unpremul(c);
					continue;
				}
				let mut m = [0.0f32; 4];
				for ch in 0..4 {
					let mut v = [0.0f32; 9];
					for (n, (k, j)) in (-1..=1).flat_map(|j| (-1..=1).map(move |k| (k, j))).enumerate() {
						v[n] = at(x + k, y + j)[ch];
					}
					v.sort_unstable_by(f32::total_cmp);
					m[ch] = v[4];
				}
				*o = unpremul(m);
			}
		}
		FilterParams::Sharpen { edges } => {
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let c = orig(i);
				let n = [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)];
				let edge = (luma(n[1]) - luma(n[0])).abs() + (luma(n[3]) - luma(n[2])).abs();
				if *edges && edge < 0.1 {
					*o = unpremul(c);
					continue;
				}
				// VERIFY: Photoshop's Sharpen strength.
				let mut p = c;
				for ch in 0..3 {
					let mean = (n[0][ch] + n[1][ch] + n[2][ch] + n[3][ch]) / 4.0;
					p[ch] = (c[ch] + 0.6 * (c[ch] - mean)).clamp(0.0, c[3]);
				}
				*o = unpremul(p);
			}
		}
		FilterParams::Emboss { angle, height, amount } => {
			let h = (height / scale(level)).max(1.0);
			let (s, c) = angle.to_radians().sin_cos();
			let (ox, oy) = ((c * h).round() as i64, (-s * h).round() as i64);
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let d = luma(unpremul(at(x + ox, y + oy))) - luma(unpremul(at(x - ox, y - oy)));
				let v = (0.5 + d * amount / 100.0).clamp(0.0, 1.0);
				*o = [v, v, v, unpremul(orig(i))[3]];
			}
		}
		FilterParams::FindEdges => {
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64, tile.y0 + (i / t) as i64);
				let g = |dx: i64, dy: i64| unpremul(at(x + dx, y + dy));
				let mut p = [0.0f32; 4];
				for ch in 0..3 {
					let gx = g(1, -1)[ch] + 2.0 * g(1, 0)[ch] + g(1, 1)[ch] - g(-1, -1)[ch] - 2.0 * g(-1, 0)[ch] - g(-1, 1)[ch];
					let gy = g(-1, 1)[ch] + 2.0 * g(0, 1)[ch] + g(1, 1)[ch] - g(-1, -1)[ch] - 2.0 * g(0, -1)[ch] - g(1, -1)[ch];
					p[ch] = (1.0 - (gx * gx + gy * gy).sqrt() / 4.0).clamp(0.0, 1.0);
				}
				p[3] = unpremul(orig(i))[3];
				*o = p;
			}
		}
		FilterParams::HighPass { radius } => {
			let sigma = (radius / scale(level)).clamp(0.1, 32.0);
			let blurred = gaussian::blur(&win, aw, ah, sigma);
			for (i, o) in out.iter_mut().enumerate() {
				let k = (i / t + a_us) * aw + i % t + a_us;
				let (c, b) = (unpremul(win[k]), unpremul(blurred[i]));
				*o = [
					(c[0] - b[0] + 0.5).clamp(0.0, 1.0),
					(c[1] - b[1] + 0.5).clamp(0.0, 1.0),
					(c[2] - b[2] + 0.5).clamp(0.0, 1.0),
					c[3],
				];
			}
		}
		FilterParams::Minimum { .. } | FilterParams::Maximum { .. } => {
			let max = matches!(params, FilterParams::Maximum { .. });
			let pick = |p: f32, q: f32| if max { p.max(q) } else { p.min(q) };
			// Separable: rows, then columns.
			let mut rows = vec![[0.0f32; 4]; aw * ah];
			for y in 0..ah {
				for x in 0..aw {
					let mut v = win[y * aw + x];
					for k in x.saturating_sub(a_us)..(x + a_us + 1).min(aw) {
						let q = win[y * aw + k];
						for ch in 0..4 {
							v[ch] = pick(v[ch], q[ch]);
						}
					}
					rows[y * aw + x] = v;
				}
			}
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (i % t + a_us, i / t + a_us);
				let mut v = rows[y * aw + x];
				for j in y - a_us..=y + a_us {
					let q = rows[j * aw + x];
					for ch in 0..4 {
						v[ch] = pick(v[ch], q[ch]);
					}
				}
				*o = unpremul(v);
			}
		}
		FilterParams::Offset { dx, dy, mode } => {
			let (ox, oy) = ((dx / scale(level)).round() as i64, (dy / scale(level)).round() as i64);
			let cv = geometry.canvas_at(level);
			let (cw, ch) = (cv.width() as i64, cv.height() as i64);
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (tile.x0 + (i % t) as i64 - ox, tile.y0 + (i / t) as i64 - oy);
				let inside = x >= cv.x0 && y >= cv.y0 && x < cv.x1 && y < cv.y1;
				*o = match mode {
					0 if !inside => [0.0; 4],
					// FAST: wrap reads the pixel one canvas away only when it is in
					// the window (offsets under the apron), else repeats the edge.
					2 if !inside => {
						let (wx, wy) = ((x - cv.x0).rem_euclid(cw.max(1)) + cv.x0, (y - cv.y0).rem_euclid(ch.max(1)) + cv.y0);
						unpremul(at(wx, wy))
					}
					_ => unpremul(at(x, y)),
				};
			}
		}
		_ => {
			for (i, o) in out.iter_mut().enumerate() {
				*o = unpremul(orig(i));
			}
		}
	}
	Ok(out)
}

/// Separable box mean of radius `r` over a premultiplied window.
fn box_blur(win: &[Px], w: usize, h: usize, r: usize) -> Vec<Px> {
	let n = (2 * r + 1) as f32;
	let mut tmp = vec![[0.0f32; 4]; w * h];
	for y in 0..h {
		for x in 0..w {
			let mut acc = [0.0f32; 4];
			for k in x as i64 - r as i64..=x as i64 + r as i64 {
				let p = win[y * w + k.clamp(0, w as i64 - 1) as usize];
				for c in 0..4 {
					acc[c] += p[c];
				}
			}
			tmp[y * w + x] = acc.map(|v| v / n);
		}
	}
	let mut out = vec![[0.0f32; 4]; w * h];
	for y in 0..h {
		for x in 0..w {
			let mut acc = [0.0f32; 4];
			for k in y as i64 - r as i64..=y as i64 + r as i64 {
				let p = tmp[k.clamp(0, h as i64 - 1) as usize * w + x];
				for c in 0..4 {
					acc[c] += p[c];
				}
			}
			out[y * w + x] = acc.map(|v| v / n);
		}
	}
	out
}

/// Value-noise fBm clouds between `bg` and `fg`, in level-0 coordinates.
fn clouds(tile: Rect, level: usize, fg: [u16; 4], bg: [u16; 4], seed: u32) -> Vec<Px> {
	let s = scale(level);
	let t = TILE_SIZE as usize;
	let lattice = |x: i64, y: i64, o: u32| unit(hash(x, y, seed.wrapping_add(o)));
	let noise = |x: f32, y: f32, o: u32| -> f32 {
		let (x0, y0) = (x.floor(), y.floor());
		let (fx, fy) = (x - x0, y - y0);
		let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
		let (ix, iy) = (x0 as i64, y0 as i64);
		let a = lattice(ix, iy, o) + (lattice(ix + 1, iy, o) - lattice(ix, iy, o)) * sx;
		let b = lattice(ix, iy + 1, o) + (lattice(ix + 1, iy + 1, o) - lattice(ix, iy + 1, o)) * sx;
		a + (b - a) * sy
	};
	let (f, b) = (fg.map(|v| f32::from(v) / 65535.0), bg.map(|v| f32::from(v) / 65535.0));
	(0..t * t)
		.map(|i| {
			let (x, y) = ((tile.x0 + (i % t) as i64) as f32 * s, (tile.y0 + (i / t) as i64) as f32 * s);
			let mut v = 0.0;
			let mut amp = 0.5;
			let mut freq = 1.0 / 256.0;
			for o in 0..6 {
				v += amp * noise(x * freq, y * freq, o);
				amp *= 0.5;
				freq *= 2.0;
			}
			let v = v.clamp(0.0, 1.0);
			[b[0] + (f[0] - b[0]) * v, b[1] + (f[1] - b[1]) * v, b[2] + (f[2] - b[2]) * v, 1.0]
		})
		.collect()
}
