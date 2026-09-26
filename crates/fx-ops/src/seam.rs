//! Seam carving (M11-T05, D-080): Avidan–Shamir, on the working grid the
//! command reads from the layer (a mip-like reduction under a pixel budget).
//!
//! The result is a **map**: for every pixel of the carved image, the working
//! pixel it comes from. The caller maps it back up to full resolution.
//!
//! FAST: the energy is recomputed over the whole image after every seam; an
//! enlargement inserts at most half the width (height) in one pass.

/// Straight or premultiplied RGBA `0..=1`.
pub type Px = [f32; 4];

/// Carve the `w × h` image towards `tw × th`. `protect` (`0..=1` per pixel)
/// makes pixels costly to remove. Returns the achieved size and, row-major
/// over it, each carved pixel's source `(x, y)`.
pub fn carve(pixels: &[Px], w: usize, h: usize, protect: &[f32], tw: usize, th: usize) -> (usize, usize, Vec<(u32, u32)>) {
	// Columns first.
	let rows: Vec<Vec<u32>> = (0..h).map(|_| (0..w as u32).collect()).collect();
	let cols = carve_rows(&|x, y| pixels[y * w + x], &|x, y| protect[y * w + x], rows, tw.max(1));
	let aw = cols[0].len();
	// Then rows, on the transposed carved image: "row" x' holds source y's.
	let columns: Vec<Vec<u32>> = (0..aw).map(|_| (0..h as u32).collect()).collect();
	let get = |x: usize, y: usize| -> Px {
		// In the transposed view `x` runs along the original y.
		let (cx, cy) = (y, x);
		pixels[cy * w + cols[cy][cx] as usize]
	};
	let prot = |x: usize, y: usize| -> f32 {
		let (cx, cy) = (y, x);
		protect[cy * w + cols[cy][cx] as usize]
	};
	let rows_of = carve_rows(&get, &prot, columns, th.max(1));
	let ah = rows_of[0].len();
	let mut map = Vec::with_capacity(aw * ah);
	for y in 0..ah {
		for x in 0..aw {
			let sy = rows_of[x][y] as usize;
			map.push((cols[sy][x], sy as u32));
		}
	}
	(aw, ah, map)
}

/// Remove or insert vertical seams until every row has `target` entries.
/// `rows[y]` lists, for each current column, the original column index;
/// `px(x, y)` / `protect(x, y)` read the ORIGINAL image.
fn carve_rows(px: &dyn Fn(usize, usize) -> Px, protect: &dyn Fn(usize, usize) -> f32, rows: Vec<Vec<u32>>, target: usize) -> Vec<Vec<u32>> {
	let h = rows.len();
	let w = rows[0].len();
	if target == w || h == 0 {
		return rows;
	}
	if target < w {
		let mut rows = rows;
		while rows[0].len() > target {
			let seam = find_seam(px, protect, &rows);
			for (y, x) in seam.into_iter().enumerate() {
				rows[y].remove(x);
			}
		}
		return rows;
	}
	// Enlarge: the seams a removal would take first, each duplicated.
	let k = (target - w).min(w / 2).max(1);
	let mut work = rows.clone();
	let mut chosen: Vec<Vec<u32>> = vec![Vec::with_capacity(k); h];
	for _ in 0..k {
		let seam = find_seam(px, protect, &work);
		for (y, x) in seam.into_iter().enumerate() {
			chosen[y].push(work[y].remove(x));
		}
	}
	rows.into_iter()
		.zip(chosen)
		.map(|(row, mut dup)| {
			dup.sort_unstable();
			let mut out = Vec::with_capacity(row.len() + dup.len());
			let mut d = 0;
			for c in row {
				out.push(c);
				while d < dup.len() && dup[d] == c {
					out.push(c);
					d += 1;
				}
			}
			out
		})
		.collect()
}

fn luma(p: Px) -> f32 {
	0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2] + p[3]
}

/// The lowest-energy 8-connected vertical seam: one current column per row.
fn find_seam(px: &dyn Fn(usize, usize) -> Px, protect: &dyn Fn(usize, usize) -> f32, rows: &[Vec<u32>]) -> Vec<usize> {
	let h = rows.len();
	let w = rows[0].len();
	let lum: Vec<f32> = (0..h)
		.flat_map(|y| rows[y].iter().map(move |&c| (c as usize, y)))
		.map(|(c, y)| luma(px(c, y)))
		.collect();
	let energy = |x: usize, y: usize| -> f32 {
		let l = |xx: usize, yy: usize| lum[yy * w + xx];
		let dx = l((x + 1).min(w - 1), y) - l(x.saturating_sub(1), y);
		let dy = l(x, (y + 1).min(h - 1)) - l(x, y.saturating_sub(1));
		dx.abs() + dy.abs() + 1000.0 * protect(rows[y][x] as usize, y)
	};
	let mut cost = vec![0.0f32; w * h];
	for x in 0..w {
		cost[x] = energy(x, 0);
	}
	for y in 1..h {
		for x in 0..w {
			let mut best = cost[(y - 1) * w + x];
			if x > 0 {
				best = best.min(cost[(y - 1) * w + x - 1]);
			}
			if x + 1 < w {
				best = best.min(cost[(y - 1) * w + x + 1]);
			}
			cost[y * w + x] = best + energy(x, y);
		}
	}
	let mut seam = vec![0usize; h];
	let last = &cost[(h - 1) * w..];
	seam[h - 1] = (0..w).min_by(|a, b| last[*a].total_cmp(&last[*b])).unwrap_or(0);
	for y in (0..h - 1).rev() {
		let x = seam[y + 1];
		let mut best = x;
		for c in [x.saturating_sub(1), x + 1] {
			if c < w && cost[y * w + c] < cost[y * w + best] {
				best = c;
			}
		}
		seam[y] = best;
	}
	seam
}
