//! Selection reshaping (M5-T03): feather, expand, contract, border, smooth.
//!
//! All of it runs one output tile at a time over a patch with an apron, so
//! the memory cost is one patch, never the document.
//!
//! * **Feather** is a Gaussian blur of the coverage (`σ = radius / 2`, VERIFY
//!   against Photoshop), separable, trucated at 3σ.
//! * **Expand / contract** threshold at 50 %, then an exact Euclidean distance
//!   transform (Felzenszwalb–Huttenlocher, separable —
//!   `docs/tasks/SNIPPETS.md` §15) within the apron, re-anti-aliased over
//!   1 px at the new edge.
//! * **Border(w)** = Expand(w/2) − Contract(w/2); **Smooth(r)** = blur r then
//!   threshold at 50 %.

use std::collections::HashMap;
use std::sync::Arc;

use fx_core::selection::{SelectModify, Selection, gray_at, set_gray};
use fx_core::{BitDepth, CommandError};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileSlot, TileStore};

/// Largest apron (in pixels) a reshape may read. Bounds the patch memory: at
/// 1024 the patch is ~2 300² × 4 B ≈ 21 MB.
const MAX_APRON: u32 = 1024;

/// Apply a [`SelectModify`] to `selection`.
pub fn modify(selection: &Selection, op: &SelectModify, size: (u32, u32), depth: BitDepth, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let format = depth.gray_format();
	let out = match *op {
		SelectModify::Feather(radius) => {
			let sigma = radius / 2.0;
			if sigma <= 0.0 {
				Some(selection.clone())
			} else {
				blur(selection, sigma, size, format, store)?
			}
		}
		SelectModify::Expand(px) => expand_contract(selection, px, size, format, store)?,
		SelectModify::Contract(px) => expand_contract(selection, -px, size, format, store)?,
		SelectModify::Border(width) => {
			let half = width / 2.0;
			match (
				expand_contract(selection, half, size, format, store)?,
				expand_contract(selection, -half, size, format, store)?,
			) {
				(Some(expanded), Some(contracted)) => subtract(&expanded, &contracted, size, format, store)?,
				(Some(expanded), None) => Some(expanded),
				_ => None,
			}
		}
		SelectModify::Smooth(radius) => {
			if radius <= 0.0 {
				Some(selection.clone())
			} else {
				match blur(selection, radius, size, format, store)? {
					Some(blurred) => threshold(&blurred, size, format, store)?,
					None => None,
				}
			}
		}
	};
	Ok(out.filter(|selection| !selection.is_empty()))
}

/// Every output tile the reshape may touch: the input's non-empty tiles grown
/// by `grow` tiles.
fn output_tiles(selection: &Selection, grow: u32, size: (u32, u32)) -> Vec<(u32, u32)> {
	let cols = size.0.div_ceil(TILE_SIZE);
	let rows = size.1.div_ceil(TILE_SIZE);
	let mut bbox: Option<(u32, u32, u32, u32)> = None;
	for (tx, ty) in selection.tiles() {
		bbox = Some(match bbox {
			None => (tx, ty, tx, ty),
			Some((x0, y0, x1, y1)) => (x0.min(tx), y0.min(ty), x1.max(tx), y1.max(ty)),
		});
	}
	let Some((x0, y0, x1, y1)) = bbox else {
		return Vec::new();
	};
	let x0 = x0.saturating_sub(grow);
	let y0 = y0.saturating_sub(grow);
	let x1 = (x1 + grow).min(cols.saturating_sub(1));
	let y1 = (y1 + grow).min(rows.saturating_sub(1));
	let mut tiles = Vec::new();
	for ty in y0..=y1 {
		for tx in x0..=x1 {
			tiles.push((tx, ty));
		}
	}
	tiles
}

/// A patch of coverage around one output tile, read through the tile store.
struct Reader<'a> {
	selection: &'a Selection,
	store: &'a TileStore,
	format: PixelFormat,
	cache: HashMap<(u32, u32), Option<Arc<TileBuffer>>>,
}

impl<'a> Reader<'a> {
	fn new(selection: &'a Selection, store: &'a TileStore, format: PixelFormat) -> Self {
		Self {
			selection,
			store,
			format,
			cache: HashMap::new(),
		}
	}

	fn at(&mut self, doc_x: i64, doc_y: i64) -> f32 {
		let (ix, iy) = (doc_x - i64::from(self.selection.offset.0), doc_y - i64::from(self.selection.offset.1));
		if ix < 0 || iy < 0 || ix >= i64::from(self.selection.image.width()) || iy >= i64::from(self.selection.image.height()) {
			return 0.0;
		}
		let (tx, ty) = (ix as u32 / TILE_SIZE, iy as u32 / TILE_SIZE);
		if !self.cache.contains_key(&(tx, ty)) {
			let tile = match self.selection.image.slot(0, tx, ty) {
				TileSlot::Empty => None,
				TileSlot::Solid(value) => Some(Arc::new(TileBuffer::filled(self.format, *value))),
				TileSlot::Data(handle) => self.store.get(handle).ok(),
			};
			self.cache.insert((tx, ty), tile);
		}
		match &self.cache[&(tx, ty)] {
			Some(buffer) => gray_at(buffer, self.format, ix as u32 % TILE_SIZE, iy as u32 % TILE_SIZE),
			None => 0.0,
		}
	}
}

/// Gaussian blur of the coverage (`sigma` in pixels), tile by tile.
fn blur(selection: &Selection, sigma: f64, size: (u32, u32), format: PixelFormat, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	if sigma <= 0.0 {
		return Ok(Some(selection.clone()));
	}
	let kernel_radius = (3.0 * sigma).ceil() as u32;
	if kernel_radius > MAX_APRON {
		return Err(CommandError::NotAllowed(format!("a feather of {sigma} px is too large")));
	}
	let kernel: Vec<f32> = (-(i64::from(kernel_radius))..=i64::from(kernel_radius))
		.map(|i| {
			let x = i as f64 / sigma;
			(-0.5 * x * x).exp() as f32
		})
		.collect();
	let sum: f32 = kernel.iter().sum();
	let kernel: Vec<f32> = kernel.into_iter().map(|k| k / sum).collect();
	let patch = (TILE_SIZE + 2 * kernel_radius) as usize;

	let side = TILE_SIZE as usize;
	let mut result = Selection::empty(size, if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 });
	let mut reader = Reader::new(selection, store, format);
	let mut column = vec![0.0f32; patch];
	let mut out = vec![0.0f32; side];
	for (tx, ty) in output_tiles(selection, kernel_radius.div_ceil(TILE_SIZE) + 1, size) {
		let (base_x, base_y) = (i64::from(tx) * i64::from(TILE_SIZE), i64::from(ty) * i64::from(TILE_SIZE));
		// Gather the patch (transparent outside the canvas).
		let mut source = vec![0.0f32; patch * patch];
		for j in 0..patch {
			let doc_y = base_y - i64::from(kernel_radius) + j as i64;
			for i in 0..patch {
				let doc_x = base_x - i64::from(kernel_radius) + i as i64;
				source[j * patch + i] = reader.at(doc_x, doc_y);
			}
		}
		// Horizontal, then vertical. A convolution drops the kernel's radius
		// at both ends, so each pass is one tile wide and the result lines up
		// with the tile's own pixels — no index shifting afterwards.
		let mut band = vec![0.0f32; patch * side];
		for j in 0..patch {
			let row = &source[j * patch..(j + 1) * patch];
			convolve(row, &kernel, &mut band[j * side..(j + 1) * side]);
		}
		let mut tile = TileBuffer::zeroed(format);
		for i in 0..side {
			for j in 0..patch {
				column[j] = band[j * side + i];
			}
			convolve(&column, &kernel, &mut out);
			for (j, value) in out.iter().enumerate() {
				if *value > 0.0 {
					set_gray(&mut tile, format, j as u32, i as u32, *value);
				}
			}
		}
		result.image.put_buffer(store, tx, ty, tile);
	}
	Ok(Some(result))
}

/// One separable pass: `src` with `radius` margins, `dst` gets `src.len() - 2r`.
fn convolve(src: &[f32], kernel: &[f32], dst: &mut [f32]) {
	let radius = kernel.len() / 2;
	debug_assert_eq!(dst.len(), src.len() - 2 * radius);
	for (i, out) in dst.iter_mut().enumerate() {
		let mut sum = 0.0f32;
		for (k, weight) in kernel.iter().enumerate() {
			sum += src[i + k] * weight;
		}
		*out = sum;
	}
}

/// Expand (`delta > 0`) or contract (`delta < 0`) by `|delta|` pixels, with a
/// 1 px anti-aliased edge.
fn expand_contract(selection: &Selection, delta: f64, size: (u32, u32), format: PixelFormat, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let magnitude = delta.abs();
	if magnitude <= 0.0 {
		return Ok(Some(selection.clone()));
	}
	let apron = magnitude.ceil() as u32 + 1;
	if apron > MAX_APRON {
		return Err(CommandError::NotAllowed(format!("a distance of {magnitude} px is too large")));
	}
	let patch = (TILE_SIZE + 2 * apron) as usize;
	let mut result = Selection::empty(size, if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 });
	let mut reader = Reader::new(selection, store, format);
	for (tx, ty) in output_tiles(selection, apron.div_ceil(TILE_SIZE) + 1, size) {
		let (base_x, base_y) = (i64::from(tx) * i64::from(TILE_SIZE), i64::from(ty) * i64::from(TILE_SIZE));
		// The feature is "inside" for an expand, "outside" for a contract.
		let mut mask = vec![false; patch * patch];
		for j in 0..patch {
			let doc_y = base_y - i64::from(apron) + j as i64;
			for i in 0..patch {
				let doc_x = base_x - i64::from(apron) + i as i64;
				let inside = reader.at(doc_x, doc_y) >= 0.5;
				mask[j * patch + i] = if delta > 0.0 { inside } else { !inside };
			}
		}
		let distance = edt_2d(&mask, patch, patch);
		let mut tile = TileBuffer::zeroed(format);
		for py in 0..TILE_SIZE {
			for px in 0..TILE_SIZE {
				let d = distance[(py + apron) as usize * patch + (px + apron) as usize];
				let coverage = if delta > 0.0 {
					(magnitude + 0.5 - d).clamp(0.0, 1.0)
				} else {
					(d - magnitude + 0.5).clamp(0.0, 1.0)
				};
				if coverage > 0.0 {
					set_gray(&mut tile, format, px, py, coverage as f32);
				}
			}
		}
		result.image.put_buffer(store, tx, ty, tile);
	}
	Ok(Some(result))
}

/// `a - b` per pixel, clamped at 0 (the border of a selection).
fn subtract(a: &Selection, b: &Selection, size: (u32, u32), format: PixelFormat, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let mut result = Selection::empty(size, if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 });
	let mut reader_b = Reader::new(b, store, format);
	for (tx, ty) in a.tiles() {
		let mut reader_a = Reader::new(a, store, format);
		let mut tile = TileBuffer::zeroed(format);
		let (base_x, base_y) = (i64::from(tx) * i64::from(TILE_SIZE), i64::from(ty) * i64::from(TILE_SIZE));
		for py in 0..TILE_SIZE {
			for px in 0..TILE_SIZE {
				let (doc_x, doc_y) = (base_x + i64::from(px), base_y + i64::from(py));
				let value = (reader_a.at(doc_x, doc_y) - reader_b.at(doc_x, doc_y)).clamp(0.0, 1.0);
				if value > 0.0 {
					set_gray(&mut tile, format, px, py, value);
				}
			}
		}
		result.image.put_buffer(store, tx, ty, tile);
	}
	Ok(Some(result))
}

/// Binarise at 50 % (Smooth's second half).
fn threshold(selection: &Selection, size: (u32, u32), format: PixelFormat, store: &TileStore) -> Result<Option<Selection>, CommandError> {
	let mut result = Selection::empty(size, if format == PixelFormat::Gray16 { BitDepth::U16 } else { BitDepth::U8 });
	let mut reader = Reader::new(selection, store, format);
	for (tx, ty) in selection.tiles() {
		let mut tile = TileBuffer::zeroed(format);
		let (base_x, base_y) = (i64::from(tx) * i64::from(TILE_SIZE), i64::from(ty) * i64::from(TILE_SIZE));
		for py in 0..TILE_SIZE {
			for px in 0..TILE_SIZE {
				if reader.at(base_x + i64::from(px), base_y + i64::from(py)) >= 0.5 {
					set_gray(&mut tile, format, px, py, 1.0);
				}
			}
		}
		result.image.put_buffer(store, tx, ty, tile);
	}
	Ok(Some(result))
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
	use fx_tiles::{TileSlot, TileStoreConfig};

	use super::*;
	use fx_core::selection::gray_at;

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
