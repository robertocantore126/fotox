//! Select ▸ Grow and Select ▸ Similar (M9-T02).
//!
//! The selected pixels' colours (coverage ≥ ½) are quantised to 5 bits per
//! channel into a 32³ table, dilated by the tolerance. **Similar** selects
//! every pixel whose colour is in the table; **Grow** keeps only the matches
//! connected to the selection (a flood across tiles, seeded by the selected
//! pixels). The result includes the old selection.
//!
//! FAST: the colour test is quantised (±1 level of 32); Photoshop compares
//! with the wand's exact distance (VERIFY).

use std::collections::{HashMap, VecDeque};

use fx_core::selection::{Selection, TileCoverage, canvas_grid, valid_extent};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};
use rayon::prelude::*;

use super::{assemble, pixels};
use crate::flood::WandSource;

const Q: usize = 32;

fn cell(p: [f32; 4]) -> usize {
	let q = |v: f32| ((v.clamp(0.0, 1.0) * (Q as f32 - 1.0)).round() as usize).min(Q - 1);
	(q(p[0]) * Q + q(p[1])) * Q + q(p[2])
}

/// The colour table of the selected pixels, dilated by `tolerance` (`0..=1`).
fn colour_table(source: &dyn WandSource, selection: &Selection, size: (u32, u32), tolerance: f64, store: &TileStore) -> Result<Vec<bool>, CommandError> {
	let (cols, rows) = canvas_grid(size);
	let tiles: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
	let tables: Result<Vec<Vec<bool>>, CommandError> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			let mut table = vec![false; Q * Q * Q];
			let coverage = selection.tile_coverage(store, tx, ty)?;
			if matches!(coverage, TileCoverage::Uniform(v) if v < 0.5) {
				return Ok(table);
			}
			let px = pixels(source.tile(tx, ty)?);
			let (vw, vh) = valid_extent(size, tx, ty);
			for y in 0..vh {
				for x in 0..vw {
					if coverage.at(x, y) >= 0.5 {
						table[cell(px[(y * TILE_SIZE + x) as usize])] = true;
					}
				}
			}
			Ok(table)
		})
		.collect();
	let mut table = vec![false; Q * Q * Q];
	for t in tables? {
		for (a, b) in table.iter_mut().zip(t) {
			*a |= b;
		}
	}
	// Dilate by the tolerance in cells, one axis at a time (L∞).
	let r = (tolerance * (Q as f64 - 1.0)).ceil() as isize;
	for axis in 0..3 {
		let stride = [Q * Q, Q, 1][axis];
		let src = table.clone();
		for i in 0..Q * Q * Q {
			if src[i] {
				let c = (i / stride % Q) as isize;
				for d in -r..=r {
					let n = c + d;
					if (0..Q as isize).contains(&n) {
						table[(i as isize + d * stride as isize) as usize] = true;
					}
				}
			}
		}
	}
	Ok(table)
}

fn bit(bits: &[u64], i: usize) -> bool {
	bits[i / 64] >> (i % 64) & 1 == 1
}
fn set(bits: &mut [u64], i: usize) {
	bits[i / 64] |= 1 << (i % 64);
}

/// Grow (`contiguous`) or Similar.
pub fn grow_or_similar(
	source: &dyn WandSource,
	selection: &Selection,
	size: (u32, u32),
	tolerance: f64,
	contiguous: bool,
	depth: BitDepth,
	store: &TileStore,
) -> Result<Option<Selection>, CommandError> {
	let table = colour_table(source, selection, size, tolerance, store)?;
	if !contiguous {
		return assemble(size, depth, store, &|tx, ty| {
			let coverage = selection.tile_coverage(store, tx, ty)?;
			let px = pixels(source.tile(tx, ty)?);
			let values: Vec<f32> = (0..TILE_PIXELS)
				.map(|i| {
					if table[cell(px[i])] {
						1.0
					} else {
						coverage.at(i as u32 % TILE_SIZE, i as u32 / TILE_SIZE)
					}
				})
				.collect();
			Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
		});
	}
	// Grow: a flood over the matching pixels from the selected ones.
	// FAST: one bitset per reached tile (like the wand), on one thread.
	let (cols, rows) = canvas_grid(size);
	let words = TILE_PIXELS / 64;
	let mut reached: HashMap<(u32, u32), Vec<u64>> = HashMap::new();
	let mut matches: HashMap<(u32, u32), Vec<u64>> = HashMap::new();
	let mut queue: VecDeque<(u32, u32, u32, u32)> = VecDeque::new();
	let matches_of = |tx: u32, ty: u32| -> Result<Vec<u64>, CommandError> {
		let px = pixels(source.tile(tx, ty)?);
		let (vw, vh) = valid_extent(size, tx, ty);
		let mut bits = vec![0u64; words];
		for y in 0..vh {
			for x in 0..vw {
				let i = (y * TILE_SIZE + x) as usize;
				if table[cell(px[i])] {
					set(&mut bits, i);
				}
			}
		}
		Ok(bits)
	};
	// Seeds: every selected pixel.
	for ty in 0..rows {
		for tx in 0..cols {
			let coverage = selection.tile_coverage(store, tx, ty)?;
			if matches!(coverage, TileCoverage::Uniform(v) if v < 0.5) {
				continue;
			}
			let (vw, vh) = valid_extent(size, tx, ty);
			let mut bits = vec![0u64; words];
			for y in 0..vh {
				for x in 0..vw {
					if coverage.at(x, y) >= 0.5 {
						let i = (y * TILE_SIZE + x) as usize;
						set(&mut bits, i);
						// Only the boundary of the selection can spread (tile
						// edges count as boundary).
						let edge = x == 0 || y == 0 || x + 1 >= vw || y + 1 >= vh;
						if edge || coverage.at(x - 1, y) < 0.5 || coverage.at(x + 1, y) < 0.5 || coverage.at(x, y - 1) < 0.5 || coverage.at(x, y + 1) < 0.5 {
							queue.push_back((tx, ty, x, y));
						}
					}
				}
			}
			reached.insert((tx, ty), bits);
		}
	}
	while let Some((tx, ty, x, y)) = queue.pop_front() {
		let (cx, cy) = (i64::from(tx * TILE_SIZE + x), i64::from(ty * TILE_SIZE + y));
		for (nx, ny) in [(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)] {
			if nx < 0 || ny < 0 || nx >= i64::from(size.0) || ny >= i64::from(size.1) {
				continue;
			}
			let key = ((nx / i64::from(TILE_SIZE)) as u32, (ny / i64::from(TILE_SIZE)) as u32);
			let i = ((ny % i64::from(TILE_SIZE)) * i64::from(TILE_SIZE) + nx % i64::from(TILE_SIZE)) as usize;
			if !matches.contains_key(&key) {
				matches.insert(key, matches_of(key.0, key.1)?);
			}
			let r = reached.entry(key).or_insert_with(|| vec![0u64; words]);
			if bit(r, i) || !bit(&matches[&key], i) {
				continue;
			}
			set(r, i);
			queue.push_back((key.0, key.1, (nx % i64::from(TILE_SIZE)) as u32, (ny % i64::from(TILE_SIZE)) as u32));
		}
	}
	assemble(size, depth, store, &|tx, ty| {
		let Some(bits) = reached.get(&(tx, ty)) else { return Ok(None) };
		let coverage = selection.tile_coverage(store, tx, ty)?;
		let values: Vec<f32> = (0..TILE_PIXELS)
			.map(|i| {
				if bit(bits, i) {
					1.0f32.max(coverage.at(i as u32 % TILE_SIZE, i as u32 / TILE_SIZE))
				} else {
					coverage.at(i as u32 % TILE_SIZE, i as u32 / TILE_SIZE)
				}
			})
			.collect();
		Ok(Some(TileCoverage::Data(values.into_boxed_slice())))
	})
}
