//! PatchMatch hole filling (M11-T01, D-077): Barnes et al.'s randomized
//! nearest-neighbour field with Wexler et al.'s multi-scale EM voting — what
//! Content-Aware Fill, the Patch tool, Content-Aware Move and the Spot
//! Healing Brush's Content-Aware type are built on.
//!
//! The caller hands a **region of interest** (the hole and the area to sample
//! from) as a plain buffer: the engine reads it tile by tile and keeps it under
//! a pixel budget (downsampling a big region first), so nothing the size of
//! the document is allocated here.
//!
//! FAST: one patch size (7), no rotation / scale / mirror of patches (the
//! CAF workspace options), the NNF search runs on one thread (voting is on
//! rayon), votes are unweighted.

use rayon::prelude::*;

/// Straight RGBA `0..=1`.
pub type Px = [f32; 4];

#[derive(Clone, Copy, Debug)]
pub struct Params {
	/// Patch side (odd).
	pub patch: usize,
	/// PatchMatch passes per EM iteration.
	pub iterations: usize,
	/// EM iterations per scale.
	pub em: usize,
	pub seed: u64,
}

impl Default for Params {
	fn default() -> Self {
		Self {
			patch: 7,
			iterations: 4,
			em: 3,
			seed: 1,
		}
	}
}

struct Rng(u64);

impl Rng {
	fn next(&mut self) -> u64 {
		self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
		let mut z = self.0;
		z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
		z ^ (z >> 31)
	}
	fn below(&mut self, n: usize) -> usize {
		(self.next() % n.max(1) as u64) as usize
	}
}

/// One scale of the problem.
struct Level {
	w: usize,
	h: usize,
	px: Vec<Px>,
	hole: Vec<bool>,
	/// Pixels a source patch may use.
	sampling: Vec<bool>,
}

impl Level {
	fn down(&self) -> Level {
		let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
		let mut px = vec![[0.0; 4]; w * h];
		let mut hole = vec![false; w * h];
		let mut sampling = vec![false; w * h];
		for y in 0..h {
			for x in 0..w {
				let mut acc = [0.0f32; 4];
				let mut n = 0.0;
				let mut any_hole = false;
				let mut all_sampling = true;
				for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
					let (sx, sy) = ((2 * x + dx).min(self.w - 1), (2 * y + dy).min(self.h - 1));
					let i = sy * self.w + sx;
					any_hole |= self.hole[i];
					all_sampling &= self.sampling[i] && !self.hole[i];
					if !self.hole[i] {
						for c in 0..4 {
							acc[c] += self.px[i][c];
						}
						n += 1.0;
					}
				}
				let i = y * w + x;
				if n > 0.0 {
					px[i] = acc.map(|v| v / n);
				}
				hole[i] = any_hole;
				sampling[i] = all_sampling;
			}
		}
		Level { w, h, px, hole, sampling }
	}
}

/// Fill `hole` pixels of the `w × h` buffer from patches of `sampling`
/// pixels. Returns the buffer with the hole filled (other pixels unchanged).
pub fn fill(pixels: &[Px], w: usize, h: usize, hole: &[bool], sampling: &[bool], params: &Params) -> Vec<Px> {
	let r = params.patch / 2;
	let mut levels = vec![Level {
		w,
		h,
		px: pixels.to_vec(),
		hole: hole.to_vec(),
		sampling: sampling.iter().zip(hole).map(|(s, h)| *s && !*h).collect(),
	}];
	// Down to where the hole is a few patches wide.
	loop {
		let top = levels.last().expect("one level");
		let holes = top.hole.iter().filter(|h| **h).count();
		if top.w < 4 * params.patch || top.h < 4 * params.patch || holes < 64 || levels.len() > 8 {
			break;
		}
		let next = top.down();
		// Keep enough valid source centres at the coarser level.
		if next.sampling.iter().filter(|s| **s).count() < 16 {
			break;
		}
		levels.push(next);
	}
	let mut rng = Rng(params.seed ^ 0x5eed);
	let mut nnf: Vec<(i32, i32)> = Vec::new();
	for li in (0..levels.len()).rev() {
		let (lw, lh) = (levels[li].w, levels[li].h);
		// Valid source centres: the whole patch inside the sampling area.
		let valid: Vec<(i32, i32)> = (r..lh.saturating_sub(r))
			.flat_map(|y| (r..lw.saturating_sub(r)).map(move |x| (x, y)))
			.filter(|&(x, y)| (0..params.patch).all(|j| (0..params.patch).all(|i| levels[li].sampling[(y + j - r) * lw + x + i - r])))
			.map(|(x, y)| (x as i32, y as i32))
			.collect();
		if valid.is_empty() {
			continue;
		}
		let is_valid = |x: i32, y: i32, level: &Level| -> bool {
			if x < r as i32 || y < r as i32 || x >= (level.w - r) as i32 || y >= (level.h - r) as i32 {
				return false;
			}
			let (x, y) = (x as usize, y as usize);
			(0..params.patch).all(|j| (0..params.patch).all(|i| level.sampling[(y + j - r) * level.w + x + i - r]))
		};
		let targets: Vec<usize> = (0..lw * lh).filter(|&i| levels[li].hole[i]).collect();
		// The NNF of this level: upsampled from the coarser one, else random.
		let mut field: Vec<(i32, i32)> = vec![(0, 0); lw * lh];
		let coarse_w = if li + 1 < levels.len() { levels[li + 1].w } else { 0 };
		for &t in &targets {
			let (x, y) = ((t % lw) as i32, (t / lw) as i32);
			let from_coarse = if !nnf.is_empty() && coarse_w > 0 {
				let c = nnf[(y as usize / 2).min(levels[li + 1].h - 1) * coarse_w + (x as usize / 2).min(coarse_w - 1)];
				let s = (c.0 * 2 + x % 2, c.1 * 2 + y % 2);
				is_valid(s.0, s.1, &levels[li]).then_some(s)
			} else {
				None
			};
			field[t] = from_coarse.unwrap_or_else(|| valid[rng.below(valid.len())]);
		}
		// The coarsest level starts from the mean colour of the samples.
		if nnf.is_empty() {
			let level = &mut levels[li];
			let (mut acc, mut n) = ([0.0f32; 4], 0.0f32);
			for i in 0..lw * lh {
				if level.sampling[i] {
					for c in 0..4 {
						acc[c] += level.px[i][c];
					}
					n += 1.0;
				}
			}
			let mean = acc.map(|v| v / n.max(1.0));
			for &t in &targets {
				level.px[t] = mean;
			}
		} else {
			vote(&mut levels[li], &field, &targets, r);
		}
		for _ in 0..params.em {
			for pass in 0..params.iterations {
				search(&levels[li], &mut field, &targets, r, pass % 2 == 1, &valid, &mut rng, &is_valid);
			}
			vote(&mut levels[li], &field, &targets, r);
		}
		nnf = field;
	}
	levels.swap_remove(0).px
}

/// The patch distance between target `(tx, ty)` and source `(sx, sy)`.
fn distance(level: &Level, t: (i32, i32), s: (i32, i32), r: usize, best: f32) -> f32 {
	let mut d = 0.0f32;
	let r = r as i32;
	for j in -r..=r {
		let (ty, sy) = (t.1 + j, s.1 + j);
		if ty < 0 || ty >= level.h as i32 {
			continue;
		}
		for i in -r..=r {
			let tx = t.0 + i;
			if tx < 0 || tx >= level.w as i32 {
				continue;
			}
			let a = level.px[ty as usize * level.w + tx as usize];
			let b = level.px[sy as usize * level.w + (s.0 + i) as usize];
			for c in 0..4 {
				let e = a[c] - b[c];
				d += e * e;
			}
		}
		if d >= best {
			return d;
		}
	}
	d
}

#[allow(clippy::too_many_arguments)]
fn search(
	level: &Level,
	field: &mut [(i32, i32)],
	targets: &[usize],
	r: usize,
	reverse: bool,
	valid: &[(i32, i32)],
	rng: &mut Rng,
	is_valid: &dyn Fn(i32, i32, &Level) -> bool,
) {
	let w = level.w;
	let order: Box<dyn Iterator<Item = &usize>> = if reverse { Box::new(targets.iter().rev()) } else { Box::new(targets.iter()) };
	let step: i32 = if reverse { 1 } else { -1 };
	for &t in order {
		let p = ((t % w) as i32, (t / w) as i32);
		let mut best = field[t];
		let mut best_d = distance(level, p, best, r, f32::INFINITY);
		// Propagation from the previous neighbours in scan order.
		for (nx, ny) in [(p.0 + step, p.1), (p.0, p.1 + step)] {
			if nx < 0 || ny < 0 || nx >= w as i32 || ny >= level.h as i32 {
				continue;
			}
			let n = ny as usize * w + nx as usize;
			if !level.hole[n] {
				continue;
			}
			let cand = (field[n].0 - (nx - p.0), field[n].1 - (ny - p.1));
			if is_valid(cand.0, cand.1, level) {
				let d = distance(level, p, cand, r, best_d);
				if d < best_d {
					best = cand;
					best_d = d;
				}
			}
		}
		// Random search in shrinking windows, plus one global sample.
		let mut radius = w.max(level.h) as i32;
		while radius >= 1 {
			let cand = (
				best.0 + (rng.below(2 * radius as usize + 1) as i32 - radius),
				best.1 + (rng.below(2 * radius as usize + 1) as i32 - radius),
			);
			if is_valid(cand.0, cand.1, level) {
				let d = distance(level, p, cand, r, best_d);
				if d < best_d {
					best = cand;
					best_d = d;
				}
			}
			radius /= 2;
		}
		let cand = valid[rng.below(valid.len())];
		let d = distance(level, p, cand, r, best_d);
		if d < best_d {
			best = cand;
		}
		field[t] = best;
	}
}

/// EM voting: each hole pixel = the mean of what the patches covering it say.
fn vote(level: &mut Level, field: &[(i32, i32)], _targets: &[usize], r: usize) {
	let (w, h) = (level.w, level.h);
	let r = r as i32;
	let px = &level.px;
	let hole = &level.hole;
	let new: Vec<Px> = (0..w * h)
		.into_par_iter()
		.map(|i| {
			if !hole[i] {
				return px[i];
			}
			let (x, y) = ((i % w) as i32, (i / w) as i32);
			let mut acc = [0.0f32; 4];
			let mut n = 0.0f32;
			for j in -r..=r {
				for k in -r..=r {
					let (qx, qy) = (x + k, y + j);
					if qx < 0 || qy < 0 || qx >= w as i32 || qy >= h as i32 {
						continue;
					}
					let q = qy as usize * w + qx as usize;
					if !hole[q] {
						continue;
					}
					let s = field[q];
					let (sx, sy) = (s.0 - k, s.1 - j);
					if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
						continue;
					}
					let c = px[sy as usize * w + sx as usize];
					for ch in 0..4 {
						acc[ch] += c[ch];
					}
					n += 1.0;
				}
			}
			if n > 0.0 { acc.map(|v| v / n) } else { px[i] }
		})
		.collect();
	level.px = new;
}
