//! The Magic Wand's region growing (M5-T04).
//!
//! The wand compares every pixel with the colour at the clicked point: a
//! pixel matches when no channel of its premultiplied RGBA differs from the
//! seed's by more than the tolerance (VERIFY: Photoshop's exact distance).
//! Premultiplied, so all fully transparent pixels are one colour.
//!
//! * **Contiguous**: a scanline flood fill that works tile by tile. Spans that
//!   leave a tile become seeds of the neighbour; the set of tiles the flood
//!   reaches is processed in rounds, and the tiles a round needs are read
//!   (composited, by the caller's [`WandSource`]) in parallel. Nothing the
//!   size of the document is allocated: per visited tile the flood keeps two
//!   bitsets (8 KB each), and a tile that matches entirely keeps none.
//! * **Not contiguous**: every canvas tile is thresholded on its own, in
//!   parallel.
//! * **Anti-alias** softens the one-pixel edge: `clamp(1.5·box3 − 0.25)` of
//!   the binary result, which keeps the area of a straight edge and leaves
//!   the inside and the outside untouched.

use std::collections::HashMap;
use std::sync::Arc;

use fx_core::selection::{OutTile, Selection, canvas_grid, out_tile, valid_extent};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{TILE_PIXELS, TILE_SIZE, TileStore};
use rayon::prelude::*;

/// Premultiplied RGBA of one canvas tile (`0..=1`), as the wand reads it.
pub enum WandTile {
	/// Every pixel has this colour.
	Uniform([f32; 4]),
	/// `TILE_SIZE²` pixels, row-major.
	Data(Vec<[f32; 4]>),
}

/// Where the wand reads its colours: the active layer or the composite of
/// all layers, at level 0, in canvas coordinates.
pub trait WandSource: Sync {
	/// Canvas tile `(tx, ty)`. Pixels outside the canvas are ignored.
	fn tile(&self, tx: u32, ty: u32) -> Result<WandTile, CommandError>;
}

/// What the wand does (the option bar), in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wand {
	/// The clicked pixel.
	pub seed: (u32, u32),
	/// Largest per-channel difference that still matches, `0..=1`.
	pub tolerance: f32,
	pub contiguous: bool,
	pub anti_alias: bool,
}

const WORDS: usize = TILE_PIXELS / 64;

/// One bit per pixel of a tile.
#[derive(Clone)]
struct Bits(Box<[u64]>);

impl Bits {
	fn new() -> Self {
		Self(vec![0u64; WORDS].into_boxed_slice())
	}

	fn get(&self, i: usize) -> bool {
		self.0[i / 64] >> (i % 64) & 1 == 1
	}

	fn set(&mut self, i: usize) {
		self.0[i / 64] |= 1 << (i % 64);
	}
}

/// Which pixels of a tile match the seed colour.
enum Matches {
	All,
	None,
	Some(Bits),
}

impl Matches {
	fn at(&self, i: usize) -> bool {
		match self {
			Matches::All => true,
			Matches::None => false,
			Matches::Some(bits) => bits.get(i),
		}
	}
}

/// The pixels of a tile the wand selected.
#[derive(Clone)]
enum Selected {
	All,
	Some(Bits),
}

impl Selected {
	fn at(&self, i: usize) -> bool {
		match self {
			Selected::All => true,
			Selected::Some(bits) => bits.get(i),
		}
	}
}

/// Run the wand over a canvas of `size`. `None` = nothing matched (which only
/// happens when the seed is outside the canvas).
pub fn magic_wand(source: &dyn WandSource, size: (u32, u32), wand: &Wand, depth: BitDepth, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let (sx, sy) = wand.seed;
	if sx >= size.0 || sy >= size.1 {
		return Ok(None);
	}
	let seed_tile = source.tile(sx / TILE_SIZE, sy / TILE_SIZE)?;
	let colour = match &seed_tile {
		WandTile::Uniform(c) => *c,
		WandTile::Data(pixels) => pixels[((sy % TILE_SIZE) * TILE_SIZE + sx % TILE_SIZE) as usize],
	};
	// A hair over the tolerance so tolerance 0 still matches the seed's own
	// colour after f32 rounding.
	let tolerance = wand.tolerance.max(0.0) + 1e-6;
	let matcher = |tile: WandTile, tx: u32, ty: u32| -> Matches { match_tile(tile, colour, tolerance, valid_extent(size, tx, ty)) };

	let selected: HashMap<(u32, u32), Selected> = if wand.contiguous {
		let mut matches: HashMap<(u32, u32), Arc<Matches>> = HashMap::new();
		matches.insert((sx / TILE_SIZE, sy / TILE_SIZE), Arc::new(matcher(seed_tile, sx / TILE_SIZE, sy / TILE_SIZE)));
		flood(source, size, (sx, sy), &mut matches, &matcher)?
	} else {
		let (cols, rows) = canvas_grid(size);
		let tiles: Vec<(u32, u32)> = (0..rows).flat_map(|ty| (0..cols).map(move |tx| (tx, ty))).collect();
		let found: Result<Vec<_>, CommandError> = tiles
			.par_iter()
			.map(|&(tx, ty)| {
				Ok(match matcher(source.tile(tx, ty)?, tx, ty) {
					Matches::All => Some(((tx, ty), Selected::All)),
					Matches::None => None,
					Matches::Some(bits) => Some(((tx, ty), Selected::Some(bits))),
				})
			})
			.collect();
		found?.into_iter().flatten().collect()
	};
	Ok(to_selection(&selected, size, wand.anti_alias, depth, store))
}

/// Compare a tile with the seed colour; only the canvas part can match.
fn match_tile(tile: WandTile, colour: [f32; 4], tolerance: f32, valid: (u32, u32)) -> Matches {
	let close = |c: &[f32; 4]| c.iter().zip(colour.iter()).all(|(a, b)| (a - b).abs() <= tolerance);
	let full = valid == (TILE_SIZE, TILE_SIZE);
	match tile {
		WandTile::Uniform(c) if close(&c) && full => Matches::All,
		WandTile::Uniform(c) if !close(&c) => Matches::None,
		WandTile::Uniform(_) => {
			let mut bits = Bits::new();
			for y in 0..valid.1 {
				for x in 0..valid.0 {
					bits.set((y * TILE_SIZE + x) as usize);
				}
			}
			Matches::Some(bits)
		}
		WandTile::Data(pixels) => {
			let mut bits = Bits::new();
			let mut count = 0usize;
			for y in 0..valid.1 {
				for x in 0..valid.0 {
					let i = (y * TILE_SIZE + x) as usize;
					if close(&pixels[i]) {
						bits.set(i);
						count += 1;
					}
				}
			}
			if count == 0 {
				Matches::None
			} else if full && count == TILE_PIXELS {
				Matches::All
			} else {
				Matches::Some(bits)
			}
		}
	}
}

/// A run of pixels to examine: row `y` of a tile, columns `x0..=x1`.
#[derive(Clone, Copy)]
struct Seed {
	y: u32,
	x0: u32,
	x1: u32,
}

/// The contiguous flood, in rounds: every round reads the tiles its seeds
/// reached (in parallel), then fills them one by one.
fn flood(
	source: &dyn WandSource,
	size: (u32, u32),
	(sx, sy): (u32, u32),
	matches: &mut HashMap<(u32, u32), Arc<Matches>>,
	matcher: &(dyn Fn(WandTile, u32, u32) -> Matches + Sync),
) -> Result<HashMap<(u32, u32), Selected>, CommandError> {
	let (cols, rows) = canvas_grid(size);
	let mut visited: HashMap<(u32, u32), Selected> = HashMap::new();
	let mut pending: HashMap<(u32, u32), Vec<Seed>> = HashMap::new();
	pending.insert(
		(sx / TILE_SIZE, sy / TILE_SIZE),
		vec![Seed {
			y: sy % TILE_SIZE,
			x0: sx % TILE_SIZE,
			x1: sx % TILE_SIZE,
		}],
	);
	while !pending.is_empty() {
		// Read the tiles this round needs, in parallel.
		let missing: Vec<(u32, u32)> = pending.keys().filter(|k| !matches.contains_key(*k)).copied().collect();
		let read: Result<Vec<_>, CommandError> = missing
			.par_iter()
			.map(|&(tx, ty)| Ok(((tx, ty), Arc::new(matcher(source.tile(tx, ty)?, tx, ty)))))
			.collect();
		matches.extend(read?);
		let round = std::mem::take(&mut pending);
		for ((tx, ty), seeds) in round {
			let valid = valid_extent(size, tx, ty);
			let tile_matches = matches[&(tx, ty)].clone();
			let state = visited.entry((tx, ty)).or_insert_with(|| Selected::Some(Bits::new()));
			let mut out = |dx: i32, dy: i32, seed: Seed| {
				let (nx, ny) = (tx as i64 + i64::from(dx), ty as i64 + i64::from(dy));
				if nx >= 0 && ny >= 0 && nx < i64::from(cols) && ny < i64::from(rows) {
					pending.entry((nx as u32, ny as u32)).or_default().push(seed);
				}
			};
			fill_tile(&tile_matches, state, valid, seeds, &mut out);
		}
	}
	Ok(visited)
}

/// Scanline fill inside one tile from `seeds`; runs that touch the tile's
/// border hand seeds to the neighbour through `out(dx, dy, seed)`.
fn fill_tile(matches: &Matches, state: &mut Selected, valid: (u32, u32), seeds: Vec<Seed>, out: &mut dyn FnMut(i32, i32, Seed)) {
	let last = TILE_SIZE - 1;
	// A tile that matches entirely and has not been entered yet is taken
	// whole: no per-pixel walk over the inside of a big flat region.
	if let (Matches::All, Selected::Some(bits)) = (matches, &*state)
		&& bits.0.iter().all(|w| *w == 0)
		&& valid == (TILE_SIZE, TILE_SIZE)
	{
		*state = Selected::All;
		for y in 0..TILE_SIZE {
			out(-1, 0, Seed { y, x0: last, x1: last });
			out(1, 0, Seed { y, x0: 0, x1: 0 });
		}
		out(0, -1, Seed { y: last, x0: 0, x1: last });
		out(0, 1, Seed { y: 0, x0: 0, x1: last });
		return;
	}
	let Selected::Some(bits) = state else {
		// Already taken whole.
		return;
	};
	let mut stack = seeds;
	let at = |x: u32, y: u32| (y * TILE_SIZE + x) as usize;
	while let Some(seed) = stack.pop() {
		if seed.y >= valid.1 {
			continue;
		}
		let mut x = seed.x0;
		while x <= seed.x1.min(valid.0.saturating_sub(1)) {
			let i = at(x, seed.y);
			if !matches.at(i) || bits.get(i) {
				x += 1;
				continue;
			}
			// Grow the run both ways.
			let mut l = x;
			while l > 0 && matches.at(at(l - 1, seed.y)) && !bits.get(at(l - 1, seed.y)) {
				l -= 1;
			}
			let mut r = x;
			while r + 1 < valid.0 && matches.at(at(r + 1, seed.y)) && !bits.get(at(r + 1, seed.y)) {
				r += 1;
			}
			for px in l..=r {
				bits.set(at(px, seed.y));
			}
			if l == 0 {
				out(-1, 0, Seed { y: seed.y, x0: last, x1: last });
			}
			if r == last {
				out(1, 0, Seed { y: seed.y, x0: 0, x1: 0 });
			}
			if seed.y > 0 {
				stack.push(Seed { y: seed.y - 1, x0: l, x1: r });
			} else {
				out(0, -1, Seed { y: last, x0: l, x1: r });
			}
			if seed.y + 1 < valid.1 {
				stack.push(Seed { y: seed.y + 1, x0: l, x1: r });
			} else if seed.y == last {
				out(0, 1, Seed { y: 0, x0: l, x1: r });
			}
			x = r + 1;
		}
	}
}

/// The selection of the selected pixels, softened when `anti_alias`.
fn to_selection(selected: &HashMap<(u32, u32), Selected>, size: (u32, u32), anti_alias: bool, depth: BitDepth, store: &TileStore) -> Option<Selection> {
	let format = depth.gray_format();
	// With anti-aliasing, the neighbours of a selected tile get a soft edge too.
	let mut tiles: Vec<(u32, u32)> = selected.keys().copied().collect();
	if anti_alias {
		let (cols, rows) = canvas_grid(size);
		let mut grown: Vec<(u32, u32)> = Vec::new();
		for &(tx, ty) in &tiles {
			for dy in -1i64..=1 {
				for dx in -1i64..=1 {
					let (nx, ny) = (i64::from(tx) + dx, i64::from(ty) + dy);
					if nx >= 0 && ny >= 0 && nx < i64::from(cols) && ny < i64::from(rows) {
						grown.push((nx as u32, ny as u32));
					}
				}
			}
		}
		grown.sort_unstable();
		grown.dedup();
		tiles = grown;
	}
	let bit = |x: i64, y: i64| -> f32 {
		if x < 0 || y < 0 || x >= i64::from(size.0) || y >= i64::from(size.1) {
			return 0.0;
		}
		let key = ((x as u32) / TILE_SIZE, (y as u32) / TILE_SIZE);
		match selected.get(&key) {
			Some(s) if s.at(((y as u32 % TILE_SIZE) * TILE_SIZE + x as u32 % TILE_SIZE) as usize) => 1.0,
			_ => 0.0,
		}
	};
	let results: Vec<((u32, u32), OutTile)> = tiles
		.par_iter()
		.map(|&(tx, ty)| {
			let valid = valid_extent(size, tx, ty);
			if let Some(Selected::All) = selected.get(&(tx, ty))
				&& !anti_alias
			{
				return ((tx, ty), OutTile::Uniform(1.0));
			}
			let (bx, by) = (i64::from(tx * TILE_SIZE), i64::from(ty * TILE_SIZE));
			let mut values = vec![0.0f32; TILE_PIXELS];
			for y in 0..valid.1 {
				for x in 0..valid.0 {
					let (px, py) = (bx + i64::from(x), by + i64::from(y));
					values[(y * TILE_SIZE + x) as usize] = if anti_alias {
						let mut sum = 0.0;
						for dy in -1..=1 {
							for dx in -1..=1 {
								sum += bit(px + dx, py + dy);
							}
						}
						(1.5 * sum / 9.0 - 0.25).clamp(0.0, 1.0)
					} else {
						bit(px, py)
					};
				}
			}
			((tx, ty), out_tile(format, &values, valid))
		})
		.collect();
	Selection::from_tiles(size, depth, results, store)
}

#[cfg(test)]
mod tests {
	use fx_core::selection::TileCoverage;
	use fx_tiles::TileStoreConfig;

	use super::*;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-ops-flood-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// An image given by a function of the canvas pixel.
	struct Fn2<F: Fn(u32, u32) -> [f32; 4] + Sync>(F);

	impl<F: Fn(u32, u32) -> [f32; 4] + Sync> WandSource for Fn2<F> {
		fn tile(&self, tx: u32, ty: u32) -> Result<WandTile, CommandError> {
			let mut pixels = vec![[0.0; 4]; TILE_PIXELS];
			for (i, p) in pixels.iter_mut().enumerate() {
				*p = (self.0)(tx * TILE_SIZE + i as u32 % TILE_SIZE, ty * TILE_SIZE + i as u32 / TILE_SIZE);
			}
			Ok(WandTile::Data(pixels))
		}
	}

	fn at(selection: &Selection, store: &TileStore, x: u32, y: u32) -> f32 {
		match selection.tile_coverage(store, x / TILE_SIZE, y / TILE_SIZE).unwrap() {
			TileCoverage::Uniform(v) => v,
			c => c.at(x % TILE_SIZE, y % TILE_SIZE),
		}
	}

	fn wand(seed: (u32, u32), tolerance: f32, contiguous: bool) -> Wand {
		Wand {
			seed,
			tolerance,
			contiguous,
			anti_alias: false,
		}
	}

	#[test]
	fn tolerance_zero_selects_one_exact_colour() {
		let store = store();
		// A horizontal gradient: each column has its own grey.
		let source = Fn2(|x, _| {
			let v = x as f32 / 255.0;
			[v, v, v, 1.0]
		});
		let selection = magic_wand(&source, (300, 50), &wand((100, 10), 0.0, true), BitDepth::U8, &store)
			.unwrap()
			.unwrap();
		assert_eq!(at(&selection, &store, 100, 40), 1.0, "the whole column");
		assert_eq!(at(&selection, &store, 99, 10), 0.0);
		assert_eq!(at(&selection, &store, 101, 10), 0.0);
		// Tolerance 2 levels: columns 98..=102.
		let selection = magic_wand(&source, (300, 50), &wand((100, 10), 2.0 / 255.0, true), BitDepth::U8, &store)
			.unwrap()
			.unwrap();
		assert_eq!(at(&selection, &store, 98, 10), 1.0);
		assert_eq!(at(&selection, &store, 102, 10), 1.0);
		assert_eq!(at(&selection, &store, 97, 10), 0.0);
	}

	#[test]
	fn contiguous_stops_at_a_separating_line() {
		let store = store();
		// White with a black vertical line at x = 300.
		let source = Fn2(|x, _| if x == 300 { [0.0, 0.0, 0.0, 1.0] } else { [1.0; 4] });
		let size = (600, 400);
		let near = magic_wand(&source, size, &wand((10, 10), 0.1, true), BitDepth::U8, &store).unwrap().unwrap();
		assert_eq!(at(&near, &store, 299, 399), 1.0);
		assert_eq!(at(&near, &store, 300, 200), 0.0);
		assert_eq!(at(&near, &store, 301, 200), 0.0, "beyond the line");
		let all = magic_wand(&source, size, &wand((10, 10), 0.1, false), BitDepth::U8, &store).unwrap().unwrap();
		assert_eq!(at(&all, &store, 301, 200), 1.0, "not contiguous: both sides");
		assert_eq!(at(&all, &store, 300, 200), 0.0);
	}

	#[test]
	fn the_flood_follows_a_spiral_across_many_tiles() {
		let store = store();
		// A snake corridor of white on black, 1 000 px across: 20 px white
		// rows joined at alternating ends, so the one path crosses the tile
		// borders many times, going left, right and down.
		let size = (1000u32, 1000u32);
		let white = |x: u32, y: u32| -> bool {
			let band = y / 20;
			if band.is_multiple_of(2) {
				return true;
			}
			// A black band: open at the right end, then the left, alternately.
			if (band / 2).is_multiple_of(2) { x >= 980 } else { x < 20 }
		};
		let source = Fn2(move |x, y| if white(x, y) { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
		let selection = magic_wand(&source, size, &wand((0, 0), 0.1, true), BitDepth::U8, &store).unwrap().unwrap();
		for y in (0..1000).step_by(7) {
			for x in (0..1000).step_by(13) {
				assert_eq!(at(&selection, &store, x, y), f32::from(white(x, y)), "({x}, {y})");
			}
		}
	}

	#[test]
	fn a_flat_region_is_taken_as_solid_tiles() {
		let store = store();
		struct Flat;
		impl WandSource for Flat {
			fn tile(&self, _: u32, _: u32) -> Result<WandTile, CommandError> {
				Ok(WandTile::Uniform([0.5, 0.5, 0.5, 1.0]))
			}
		}
		let size = (2048, 2048);
		let selection = magic_wand(&Flat, size, &wand((1000, 1000), 0.0, true), BitDepth::U16, &store).unwrap().unwrap();
		assert!(selection.image.grid(0).non_empty().all(|(_, _, s)| matches!(s, fx_tiles::TileSlot::Solid(_))));
		assert_eq!(selection.canvas_tiles(size).len(), 64);
	}

	#[test]
	fn anti_aliasing_softens_a_straight_edge_symmetrically() {
		let store = store();
		let source = Fn2(|x, _| if x < 100 { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
		let mut w = wand((10, 10), 0.1, true);
		w.anti_alias = true;
		let selection = magic_wand(&source, (200, 50), &w, BitDepth::U16, &store).unwrap().unwrap();
		let (inside, outside) = (at(&selection, &store, 99, 25), at(&selection, &store, 100, 25));
		assert!((inside - 0.75).abs() < 0.01 && (outside - 0.25).abs() < 0.01, "{inside} {outside}");
		assert_eq!(at(&selection, &store, 50, 25), 1.0);
		assert_eq!(at(&selection, &store, 150, 25), 0.0);
	}
}
