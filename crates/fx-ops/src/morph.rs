//! Selection reshaping (M5-T03, parallel and offset-aware since the M5
//! review): feather, expand, contract, border, smooth.
//!
//! All of it runs one output tile at a time over a patch with an apron, on
//! rayon, so the memory cost is one patch per worker, never the document. A
//! tile whose whole patch is uniform (the inside of a big selection, or far
//! outside it) is answered without touching pixels.
//!
//! * **Feather** is a Gaussian blur of the coverage (`σ = radius / 2`, VERIFY
//!   against Photoshop), separable, truncated at 3σ.
//! * **Expand / contract** threshold at 50 %, then an exact Euclidean distance
//!   transform (Felzenszwalb–Huttenlocher, separable —
//!   `docs/tasks/SNIPPETS.md` §15) within the apron, re-anti-aliased over
//!   1 px at the new edge.
//! * **Border(w)** = Expand(w/2) − Contract(w/2); **Smooth(r)** = blur r then
//!   threshold at 50 %.

use fx_core::selection::{OutTile, PatchReader, SelectModify, Selection, TileCoverage, canvas_grid, out_tile, valid_extent};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileError, TileStore};
use rayon::prelude::*;

/// Largest apron (in pixels) a reshape may read. Bounds the patch memory: at
/// 1024 the patch is ~2 300² × 4 B ≈ 21 MB per worker.
const MAX_APRON: u32 = 1024;

/// Apply a [`SelectModify`] to `selection`. `None` = nothing left selected.
pub fn modify(selection: &Selection, op: &SelectModify, size: (u32, u32), depth: BitDepth, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let out = match *op {
		SelectModify::Feather(radius) => {
			if radius <= 0.0 {
				Some(selection.clone())
			} else {
				blur(selection, radius / 2.0, size, depth, store)?
			}
		}
		SelectModify::Expand(px) => expand_contract(selection, px, size, depth, store)?,
		SelectModify::Contract(px) => expand_contract(selection, -px, size, depth, store)?,
		SelectModify::Border(width) => {
			let half = width / 2.0;
			match (
				expand_contract(selection, half, size, depth, store)?,
				expand_contract(selection, -half, size, depth, store)?,
			) {
				(Some(expanded), Some(contracted)) => per_tile(&[&expanded, &contracted], size, depth, store, |v| (v[0] - v[1]).clamp(0.0, 1.0))?,
				(Some(expanded), None) => Some(expanded),
				_ => None,
			}
		}
		SelectModify::Smooth(radius) => {
			if radius <= 0.0 {
				Some(selection.clone())
			} else {
				match blur(selection, radius, size, depth, store)? {
					Some(blurred) => per_tile(&[&blurred], size, depth, store, |v| if v[0] >= 0.5 { 1.0 } else { 0.0 })?,
					None => None,
				}
			}
		}
	};
	Ok(out.filter(|selection| !selection.is_empty()))
}

/// Every output tile a reshape reaching `reach` pixels may touch: the
/// selection's canvas tiles grown by `reach`, clipped to the canvas.
fn output_tiles(selection: &Selection, reach: u32, size: (u32, u32)) -> Vec<(u32, u32)> {
	let Some((x0, y0, x1, y1)) = selection.canvas_bounds(size) else {
		return Vec::new();
	};
	let (cols, rows) = canvas_grid(size);
	let tx0 = x0.saturating_sub(reach) / TILE_SIZE;
	let ty0 = y0.saturating_sub(reach) / TILE_SIZE;
	let tx1 = ((x1 + reach).saturating_sub(1) / TILE_SIZE).min(cols - 1);
	let ty1 = ((y1 + reach).saturating_sub(1) / TILE_SIZE).min(rows - 1);
	(ty0..=ty1).flat_map(|ty| (tx0..=tx1).map(move |tx| (tx, ty))).collect()
}

/// Run `tile` for every output tile on rayon, each worker with its own patch
/// reader, and build the result.
fn run_tiles(
	selection: &Selection,
	tiles: Vec<(u32, u32)>,
	size: (u32, u32),
	depth: BitDepth,
	store: &TileStore,
	tile: impl Fn(&mut PatchReader<'_>, u32, u32) -> Result<OutTile, TileError> + Sync,
) -> Result<Option<Selection>, CommandError> {
	let results: Result<Vec<_>, TileError> = tiles
		.par_iter()
		.map_init(
			|| PatchReader::new(selection, store, size),
			|reader, &(tx, ty)| Ok(((tx, ty), tile(reader, tx, ty)?)),
		)
		.collect();
	Ok(Selection::from_tiles(size, depth, results?, store))
}

/// `f` of the selections' coverages, pixel by pixel, over the tiles of the
/// first (uniform tiles combine as one value).
fn per_tile(
	inputs: &[&Selection],
	size: (u32, u32),
	depth: BitDepth,
	store: &TileStore,
	f: impl Fn(&[f32]) -> f32 + Sync,
) -> Result<Option<Selection>, CommandError> {
	let format = depth.gray_format();
	let tiles = inputs[0].canvas_tiles(size);
	let results: Result<Vec<_>, TileError> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			let covs: Vec<TileCoverage> = inputs.iter().map(|s| s.tile_coverage(store, tx, ty)).collect::<Result<_, _>>()?;
			let mut values = vec![0.0f32; inputs.len()];
			if covs.iter().all(|c| matches!(c, TileCoverage::Uniform(_))) {
				for (v, c) in values.iter_mut().zip(&covs) {
					*v = c.at(0, 0);
				}
				return Ok(((tx, ty), OutTile::Uniform(f(&values))));
			}
			let mut out = vec![0.0f32; TILE_PIXELS];
			for (i, o) in out.iter_mut().enumerate() {
				let (x, y) = (i as u32 % TILE_SIZE, i as u32 / TILE_SIZE);
				for (v, c) in values.iter_mut().zip(&covs) {
					*v = c.at(x, y);
				}
				*o = f(&values);
			}
			Ok(((tx, ty), out_tile(format, &out, valid_extent(size, tx, ty))))
		})
		.collect();
	Ok(Selection::from_tiles(size, depth, results?, store))
}

/// A normalised Gaussian kernel of `sigma`, truncated at 3σ.
fn gaussian_kernel(sigma: f64) -> Vec<f32> {
	let radius = (3.0 * sigma).ceil() as i64;
	let kernel: Vec<f64> = (-radius..=radius)
		.map(|i| {
			let x = i as f64 / sigma;
			(-0.5 * x * x).exp()
		})
		.collect();
	let sum: f64 = kernel.iter().sum();
	kernel.into_iter().map(|k| (k / sum) as f32).collect()
}

/// Gaussian blur of the coverage (`sigma` in pixels), tile by tile.
fn blur(selection: &Selection, sigma: f64, size: (u32, u32), depth: BitDepth, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	if sigma <= 0.0 {
		return Ok(Some(selection.clone()));
	}
	let kernel = gaussian_kernel(sigma);
	let radius = (kernel.len() / 2) as u32;
	if radius > MAX_APRON {
		return Err(CommandError::NotAllowed(format!("a feather of {} px is too large", sigma * 2.0)));
	}
	let format = depth.gray_format();
	let side = TILE_SIZE as usize;
	let patch = side + 2 * radius as usize;
	let r = i64::from(radius);
	run_tiles(selection, output_tiles(selection, radius, size), size, depth, store, |reader, tx, ty| {
		let (bx, by) = (i64::from(tx * TILE_SIZE), i64::from(ty * TILE_SIZE));
		for value in [0.0, 1.0] {
			if reader.is_uniform(bx - r, by - r, bx + side as i64 + r, by + side as i64 + r, value)? {
				return Ok(OutTile::Uniform(value));
			}
		}
		let source = reader.patch(bx - r, by - r, patch, patch)?;
		// Horizontal, then vertical. A convolution drops the kernel's radius
		// at both ends, so each pass is one tile wide and the result lines up
		// with the tile's own pixels.
		let mut band = vec![0.0f32; patch * side];
		for j in 0..patch {
			convolve(&source[j * patch..(j + 1) * patch], &kernel, &mut band[j * side..(j + 1) * side]);
		}
		let mut column = vec![0.0f32; patch];
		let mut out = vec![0.0f32; side];
		let mut values = vec![0.0f32; TILE_PIXELS];
		for i in 0..side {
			for (j, c) in column.iter_mut().enumerate() {
				*c = band[j * side + i];
			}
			convolve(&column, &kernel, &mut out);
			for (j, value) in out.iter().enumerate() {
				values[j * side + i] = *value;
			}
		}
		Ok(out_tile(format, &values, valid_extent(size, tx, ty)))
	})
}

/// One separable pass: `src` with `radius` margins, `dst` gets `src.len() - 2r`.
fn convolve(src: &[f32], kernel: &[f32], dst: &mut [f32]) {
	let radius = kernel.len() / 2;
	debug_assert_eq!(dst.len(), src.len() - 2 * radius);
	for (i, out) in dst.iter_mut().enumerate() {
		*out = src[i..i + kernel.len()].iter().zip(kernel).map(|(s, k)| s * k).sum();
	}
}

/// Expand (`delta > 0`) or contract (`delta < 0`) by `|delta|` pixels, with a
/// 1 px anti-aliased edge.
fn expand_contract(selection: &Selection, delta: f64, size: (u32, u32), depth: BitDepth, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let magnitude = delta.abs();
	if magnitude <= 0.0 {
		return Ok(Some(selection.clone()));
	}
	let apron = magnitude.ceil() as u32 + 1;
	if apron > MAX_APRON {
		return Err(CommandError::NotAllowed(format!("a distance of {magnitude} px is too large")));
	}
	let format = depth.gray_format();
	let patch = (TILE_SIZE + 2 * apron) as usize;
	let a = i64::from(apron);
	let side = i64::from(TILE_SIZE);
	run_tiles(selection, output_tiles(selection, apron, size), size, depth, store, |reader, tx, ty| {
		let (bx, by) = (i64::from(tx * TILE_SIZE), i64::from(ty * TILE_SIZE));
		for value in [0.0, 1.0] {
			if reader.is_uniform(bx - a, by - a, bx + side + a, by + side + a, value)? {
				return Ok(OutTile::Uniform(value));
			}
		}
		let source = reader.patch(bx - a, by - a, patch, patch)?;
		// The feature is "inside" for an expand, "outside" for a contract.
		let mask: Vec<bool> = source.iter().map(|v| (*v >= 0.5) == (delta > 0.0)).collect();
		let distance = edt_2d(&mask, patch, patch);
		let mut values = vec![0.0f32; TILE_PIXELS];
		for py in 0..TILE_SIZE {
			for px in 0..TILE_SIZE {
				let d = distance[(py + apron) as usize * patch + (px + apron) as usize];
				let coverage = if delta > 0.0 {
					(magnitude + 0.5 - d).clamp(0.0, 1.0)
				} else {
					(d - magnitude + 0.5).clamp(0.0, 1.0)
				};
				values[(py * TILE_SIZE + px) as usize] = coverage as f32;
			}
		}
		Ok(out_tile(format, &values, valid_extent(size, tx, ty)))
	})
}

/// Exact Euclidean distance transform of the `true` pixels (Felzenszwalb–
/// Huttenlocher, separable). `mask` is row-major `w × h`.
fn edt_2d(mask: &[bool], w: usize, h: usize) -> Vec<f64> {
	// Wrong: `f64::INFINITY` → ∞ − ∞ = NaN in `s`.
	const FAR: f64 = 1e20;
	let mut buf: Vec<f64> = mask.iter().map(|&feature| if feature { 0.0 } else { FAR }).collect();
	let (mut d, mut v, mut z) = (vec![0.0; w.max(h)], vec![0usize; w.max(h)], vec![0.0; w.max(h) + 1]);
	for y in 0..h {
		edt_1d(&buf[y * w..(y + 1) * w], &mut d[..w], &mut v[..w], &mut z[..w + 1]);
		buf[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
	}
	let mut column = vec![0.0; h];
	let mut out = vec![0.0; h];
	for x in 0..w {
		for y in 0..h {
			column[y] = buf[y * w + x];
		}
		edt_1d(&column, &mut out, &mut v[..h], &mut z[..h + 1]);
		for y in 0..h {
			buf[y * w + x] = out[y];
		}
	}
	buf.into_iter().map(f64::sqrt).collect()
}

/// One 1-D pass of the Felzenszwalb–Huttenlocher transform (SNIPPETS §15).
/// `v` needs `n` entries and `z` needs `n + 1`.
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
	let n = f.len();
	if n == 0 {
		return;
	}
	let mut k = 0usize;
	v[0] = 0;
	z[0] = f64::NEG_INFINITY;
	z[1] = f64::INFINITY;
	for q in 1..n {
		let mut s;
		loop {
			let p = v[k];
			s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2 * q - 2 * p) as f64;
			if s > z[k] {
				break;
			}
			// Never below 0: z[0] = −∞.
			k -= 1;
		}
		k += 1;
		v[k] = q;
		z[k] = s;
		z[k + 1] = f64::INFINITY;
	}
	k = 0;
	for (q, out) in d.iter_mut().enumerate().take(n) {
		while z[k + 1] < q as f64 {
			k += 1;
		}
		let p = v[k];
		*out = (q as f64 - p as f64).powi(2) + f[p];
	}
}

#[cfg(test)]
mod tests {
	use fx_tiles::{PixelFormat, TileBuffer, TileSlot, TileStoreConfig};

	use super::*;
	use fx_core::selection::{gray_at, set_gray};

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-ops-morph-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// A selection covering the document rectangle `(x0, y0)..(x1, y1)`.
	fn rect(store: &TileStore, size: (u32, u32), rect: (u32, u32, u32, u32)) -> Selection {
		let mut selection = Selection::empty(size, BitDepth::U8);
		let (x0, y0, x1, y1) = rect;
		for ty in y0 / TILE_SIZE..=(y1 - 1) / TILE_SIZE {
			for tx in x0 / TILE_SIZE..=(x1 - 1) / TILE_SIZE {
				let mut buffer = TileBuffer::zeroed(PixelFormat::Gray8);
				for py in 0..TILE_SIZE {
					for px in 0..TILE_SIZE {
						let (x, y) = (tx * TILE_SIZE + px, ty * TILE_SIZE + py);
						if x >= x0 && x < x1 && y >= y0 && y < y1 {
							set_gray(&mut buffer, PixelFormat::Gray8, px, py, 1.0);
						}
					}
				}
				selection.image.put_buffer(store, tx, ty, buffer);
			}
		}
		selection
	}

	fn at(selection: &Selection, store: &TileStore, x: u32, y: u32) -> f32 {
		let format = selection.image.format();
		match selection.image.slot(0, x / TILE_SIZE, y / TILE_SIZE) {
			TileSlot::Empty => 0.0,
			TileSlot::Solid(value) => f32::from(value.0[0]) / 65535.0,
			TileSlot::Data(handle) => gray_at(&store.get(handle).unwrap(), format, x % TILE_SIZE, y % TILE_SIZE),
		}
	}

	/// The bounding box of everything selected, `(x0, y0, x1, y1)` inclusive.
	fn bbox(selection: &Selection, store: &TileStore, size: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
		let mut b: Option<(u32, u32, u32, u32)> = None;
		for y in 0..size.1 {
			for x in 0..size.0 {
				if at(selection, store, x, y) >= 0.5 {
					b = Some(match b {
						None => (x, y, x, y),
						Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
					});
				}
			}
		}
		b
	}

	#[test]
	fn feathering_a_rectangle_is_symmetric() {
		let store = store();
		let size = (200, 200);
		let selection = rect(&store, size, (50, 50, 150, 150));
		let blurred = modify(&selection, &SelectModify::Feather(20.0), size, BitDepth::U8, &store).unwrap().unwrap();
		// The blurred edge is symmetric around x = 49.5 / x = 150.5.
		for offset in 1..8 {
			let left = at(&blurred, &store, 50 - offset, 100);
			let right = at(&blurred, &store, 150 + offset - 1, 100);
			assert!((left - right).abs() < 0.02, "offset {offset}: {left} vs {right}");
		}
		// The middle stays fully selected, far outside stays empty.
		assert_eq!(at(&blurred, &store, 100, 100), 1.0);
		assert_eq!(at(&blurred, &store, 5, 100), 0.0);
	}

	#[test]
	fn expand_then_contract_returns_a_convex_shape() {
		let store = store();
		let size = (300, 300);
		let selection = rect(&store, size, (100, 100, 200, 200));
		let expanded = modify(&selection, &SelectModify::Expand(5.0), size, BitDepth::U8, &store).unwrap().unwrap();
		assert_eq!(bbox(&expanded, &store, size), Some((95, 95, 204, 204)));
		let back = modify(&expanded, &SelectModify::Contract(5.0), size, BitDepth::U8, &store).unwrap().unwrap();
		let b = bbox(&back, &store, size).unwrap();
		for (got, expected) in [(b.0, 100), (b.1, 100), (b.2, 199), (b.3, 199)] {
			assert!(got.abs_diff(expected) <= 1, "bbox {b:?} vs the original");
		}
	}

	#[test]
	fn a_border_keeps_only_the_edge() {
		let store = store();
		let size = (300, 300);
		let selection = rect(&store, size, (100, 100, 200, 200));
		let border = modify(&selection, &SelectModify::Border(6.0), size, BitDepth::U8, &store).unwrap().unwrap();
		// Expand(3) − Contract(3): the ring is roughly x ∈ [97, 102] ∪ [197, 202].
		assert!(at(&border, &store, 99, 150) >= 0.5, "on the edge");
		assert!(at(&border, &store, 200, 150) >= 0.5, "on the opposite edge");
		assert_eq!(at(&border, &store, 105, 150), 0.0, "further in than the ring");
		assert_eq!(at(&border, &store, 150, 150), 0.0, "the middle is cut out");
		assert_eq!(at(&border, &store, 150, 90), 0.0, "outside stays empty");
	}

	#[test]
	fn smooth_binarises_a_blurred_edge() {
		let store = store();
		let size = (200, 200);
		let selection = rect(&store, size, (50, 50, 150, 150));
		let smoothed = modify(&selection, &SelectModify::Smooth(10.0), size, BitDepth::U8, &store).unwrap().unwrap();
		// Everything is 0 or 1 (plus the 1/255 rounding).
		for (x, y) in [(100, 100), (50, 100), (40, 100), (10, 10)] {
			let v = at(&smoothed, &store, x, y);
			assert!(!(0.01..=0.99).contains(&v), "({x}, {y}) = {v}");
		}
		// A rectangle stays a rectangle within a pixel or two.
		let b = bbox(&smoothed, &store, size).unwrap();
		assert!(b.0.abs_diff(50) <= 2 && b.2.abs_diff(149) <= 2, "{b:?}");
	}
}
