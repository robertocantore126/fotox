//! The filter driver (M4-T05): one output tile at a time, at any mip level.
//!
//! * Coordinates are **layer-local** pixels of the level being computed; the
//!   canvas is expressed in the same coordinates for the edge rule (D-036).
//! * Distances scale with the level: a preview at level L uses σ·2⁻ᴸ
//!   (docs/ARCHITECTURE.md §4.5), so the preview and the final result agree.
//! * A blur with σ > [`EXACT_MAX_SIGMA`] (at the level computed) is done on a
//!   coarser mip level — where σ is back within the exact range — and
//!   bilinearly upsampled. Mip levels are exact 2×2 box averages, which add a
//!   variance of (2ᵐ)²/12 against σ² > 1024: invisible, and it keeps one
//!   tile's neighbourhood under ~1.3 MB whatever the radius (a direct 1000 px
//!   blur would need a 6 000 px apron).
//! * Only the layer's own pixels are written: output pixels outside the layer
//!   image stay transparent.

use fx_core::FilterParams;
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileError, TiledImage};

use crate::gaussian;
use crate::neighbourhood::{LevelSource, Px, Rect, gather, to_tile, unpremul};

/// Largest σ (in pixels of the level being blurred) blurred directly.
pub const EXACT_MAX_SIGMA: f32 = 32.0;

/// Where a layer sits in its document (level-0 values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
	/// The layer's offset in document pixels.
	pub offset: (i32, i32),
	/// The document size.
	pub canvas: (u32, u32),
	/// The layer image's size (its own tile grid).
	pub image: (u32, u32),
}

impl Geometry {
	/// The canvas in layer-local pixels of `level`. Offsets are rounded to
	/// whole pixels of the level exactly like the compositor (D-018).
	pub fn canvas_at(&self, level: usize) -> Rect {
		let scale = 1i64 << level;
		let off = |o: i32| (i64::from(o) * 2 + scale).div_euclid(scale * 2);
		let size = |s: u32| (i64::from(s) + scale - 1) / scale;
		let (ox, oy) = (off(self.offset.0), off(self.offset.1));
		Rect {
			x0: -ox,
			y0: -oy,
			x1: -ox + size(self.canvas.0),
			y1: -oy + size(self.canvas.1),
		}
	}

	/// The canvas used for the edge rule at `level`: like [`canvas_at`], but
	/// only the pixels the canvas covers *entirely* — a mip pixel on the
	/// canvas edge also averages what lies beyond it, and replicating that
	/// half-transparent pixel outwards would darken the edge.
	///
	/// [`canvas_at`]: Self::canvas_at
	pub fn edge_canvas_at(&self, level: usize) -> Rect {
		let mut rect = self.canvas_at(level);
		let scale = 1i64 << level;
		rect.x1 = rect.x0 + (i64::from(self.canvas.0) / scale).max(1);
		rect.y1 = rect.y0 + (i64::from(self.canvas.1) / scale).max(1);
		rect
	}

	/// The layer image in its own pixels of `level`.
	pub fn image_at(&self, level: usize) -> Rect {
		let scale = 1i64 << level;
		Rect {
			x0: 0,
			y0: 0,
			x1: (i64::from(self.image.0) + scale - 1) / scale,
			y1: (i64::from(self.image.1) + scale - 1) / scale,
		}
	}
}

/// The blur σ of `params` in pixels of `level`, if the filter blurs.
fn sigma_at(params: &FilterParams, level: usize) -> f32 {
	let radius = match params {
		FilterParams::GaussianBlur { radius } | FilterParams::UnsharpMask { radius, .. } => *radius,
		// The M12 families blur within their own window (`filter_more`).
		_ => 0.0,
	};
	radius / (1u32 << level) as f32
}

/// How many levels above `level` the blur of `params` is computed
/// (0 = directly), so that σ there is ≤ [`EXACT_MAX_SIGMA`]. The engine makes
/// the mips of `level + extra_levels(…)` valid before calling [`filter_tile`].
pub fn extra_levels(params: &FilterParams, level: usize) -> usize {
	let mut sigma = sigma_at(params, level);
	let mut m = 0;
	while sigma > EXACT_MAX_SIGMA {
		sigma /= 2.0;
		m += 1;
	}
	m
}

/// How far (in tiles of `level`) the filter spreads content: the output tile
/// set is the non-empty tiles grown by this much.
pub fn spread_tiles(params: &FilterParams, level: usize) -> u32 {
	match params {
		// A blur spreads colour into transparent neighbours.
		FilterParams::GaussianBlur { .. } => (3.0 * sigma_at(params, level) / TILE_SIZE as f32).ceil() as u32,
		// Unsharp Mask keeps the original alpha: nothing appears where the
		// layer is empty.
		FilterParams::UnsharpMask { .. } => 0,
		// FAST: the canvas-dependent aprons use a unit geometry here.
		other => crate::filter_more::spread(
			other,
			level,
			&Geometry {
				offset: (0, 0),
				canvas: (1, 1),
				image: (1, 1),
			},
		),
	}
}

/// The level-`level` tiles a filter writes on `image`: the non-empty ones
/// grown by [`spread_tiles`], clipped to the image grid and the canvas.
pub fn output_tiles(image: &TiledImage, geometry: &Geometry, params: &FilterParams, level: usize) -> Vec<(u32, u32)> {
	let grid = image.grid(level);
	let (cols, rows) = (grid.cols() as i64, grid.rows() as i64);
	let spread = match params {
		// Radial Blur reaches across the canvas; Clouds cover everything.
		FilterParams::RadialBlur { .. } | FilterParams::Clouds { .. } => u32::MAX,
		_ => spread_tiles(params, level),
	};
	if spread == u32::MAX {
		let canvas = geometry.canvas_at(level);
		let t = i64::from(TILE_SIZE);
		let (cx0, cy0) = (canvas.x0.div_euclid(t).max(0), canvas.y0.div_euclid(t).max(0));
		let (cx1, cy1) = ((canvas.x1 - 1).div_euclid(t).min(cols - 1), (canvas.y1 - 1).div_euclid(t).min(rows - 1));
		return (cy0..=cy1).flat_map(|y| (cx0..=cx1).map(move |x| (x as u32, y as u32))).collect();
	}
	let grow = i64::from(spread);
	let canvas = geometry.canvas_at(level);
	let t = i64::from(TILE_SIZE);
	let (cx0, cy0) = (canvas.x0.div_euclid(t), canvas.y0.div_euclid(t));
	let (cx1, cy1) = ((canvas.x1 - 1).div_euclid(t), (canvas.y1 - 1).div_euclid(t));
	let mut wanted = std::collections::BTreeSet::new();
	for (tx, ty, _) in grid.non_empty() {
		let (tx, ty) = (i64::from(tx), i64::from(ty));
		for y in (ty - grow).max(0).max(cy0)..=(ty + grow).min(rows - 1).min(cy1) {
			for x in (tx - grow).max(0).max(cx0)..=(tx + grow).min(cols - 1).min(cx1) {
				wanted.insert((y as u32, x as u32));
			}
		}
	}
	wanted.into_iter().map(|(y, x)| (x, y)).collect()
}

/// The filtered tile `(tx, ty)` of `level`, straight RGBA in the source's
/// format. The mips of `level + extra_levels(params, level)` must be valid.
pub fn filter_tile(src: &dyn LevelSource, geometry: &Geometry, params: &FilterParams, level: usize, tx: u32, ty: u32) -> Result<TileBuffer, TileError> {
	let format = src.format();
	if !matches!(format, PixelFormat::Rgba8 | PixelFormat::Rgba16) {
		return Err(TileError::Corrupt(format!("filters run on RGBA layers, not {format:?}")));
	}
	let t = i64::from(TILE_SIZE);
	let tile = Rect {
		x0: i64::from(tx) * t,
		y0: i64::from(ty) * t,
		x1: i64::from(tx) * t + t,
		y1: i64::from(ty) * t + t,
	};
	if !matches!(params, FilterParams::GaussianBlur { .. } | FilterParams::UnsharpMask { .. }) {
		let mut out = crate::filter_more::tile(src, geometry, params, level, tile)?;
		let image = geometry.image_at(level);
		for (i, p) in out.iter_mut().enumerate() {
			let (x, y) = (tile.x0 + (i as i64 % t), tile.y0 + (i as i64 / t));
			if x >= image.x1 || y >= image.y1 {
				*p = [0.0; 4];
			}
		}
		return Ok(to_tile(&out, format));
	}
	let blurred = blurred_tile(src, geometry, params, level, tile)?;
	let mut out: Vec<Px> = match params {
		FilterParams::GaussianBlur { .. } => blurred.into_iter().map(unpremul).collect(),
		FilterParams::UnsharpMask { amount, threshold, .. } => {
			let original = gather(src, level, tile, geometry.edge_canvas_at(level))?;
			let (amount, threshold) = (amount / 100.0, f32::from(*threshold) / 255.0);
			original
				.into_iter()
				.zip(blurred)
				.map(|(o, b)| {
					let (o, b) = (unpremul(o), unpremul(b));
					let mut p = o;
					for c in 0..3 {
						let d = o[c] - b[c];
						// VERIFY (M7): Photoshop's threshold per channel.
						if d.abs() >= threshold {
							p[c] = (o[c] + amount * d).clamp(0.0, 1.0);
						}
					}
					p
				})
				.collect()
		}
		_ => unreachable!("handled by filter_more above"),
	};
	// Only the layer's own pixels: outside its image the tile stays empty.
	let image = geometry.image_at(level);
	for (i, p) in out.iter_mut().enumerate() {
		let (x, y) = (tile.x0 + (i as i64 % t), tile.y0 + (i as i64 / t));
		if x >= image.x1 || y >= image.y1 {
			*p = [0.0; 4];
		}
	}
	Ok(to_tile(&out, format))
}

/// The Gaussian of σ(level) over `tile`, premultiplied, 256² row-major.
fn blurred_tile(src: &dyn LevelSource, geometry: &Geometry, params: &FilterParams, level: usize, tile: Rect) -> Result<Vec<Px>, TileError> {
	let m = extra_levels(params, level);
	let blur_level = level + m;
	let sigma = sigma_at(params, blur_level);
	let r = gaussian::radius(sigma) as i64;
	let canvas = geometry.edge_canvas_at(blur_level);
	if m == 0 {
		let area = Rect {
			x0: tile.x0 - r,
			y0: tile.y0 - r,
			x1: tile.x1 + r,
			y1: tile.y1 + r,
		};
		let pixels = gather(src, level, area, canvas)?;
		return Ok(gaussian::blur(&pixels, area.width(), area.height(), sigma));
	}
	// Blur at the coarser level, then sample it bilinearly at this level's
	// pixel centres: (x + ½)/2ᵐ − ½ in coarse pixels.
	let s = (1u32 << m) as f64;
	let coarse = |v: i64| (v as f64 + 0.5) / s - 0.5;
	let (bx0, by0) = (coarse(tile.x0).floor() as i64, coarse(tile.y0).floor() as i64);
	let (bx1, by1) = (coarse(tile.x1 - 1).floor() as i64 + 2, coarse(tile.y1 - 1).floor() as i64 + 2);
	let area = Rect {
		x0: bx0 - r,
		y0: by0 - r,
		x1: bx1 + r,
		y1: by1 + r,
	};
	let pixels = gather(src, blur_level, area, canvas)?;
	let blurred = gaussian::blur(&pixels, area.width(), area.height(), sigma);
	let bw = (bx1 - bx0) as usize;
	let at = |x: i64, y: i64| blurred[(y - by0) as usize * bw + (x - bx0) as usize];
	let t = i64::from(TILE_SIZE);
	let mut out = Vec::with_capacity((t * t) as usize);
	for y in tile.y0..tile.y1 {
		let v = coarse(y);
		let (iy, fy) = (v.floor() as i64, (v - v.floor()) as f32);
		for x in tile.x0..tile.x1 {
			let u = coarse(x);
			let (ix, fx) = (u.floor() as i64, (u - u.floor()) as f32);
			let (a, b, c, d) = (at(ix, iy), at(ix + 1, iy), at(ix, iy + 1), at(ix + 1, iy + 1));
			out.push(std::array::from_fn(|k| {
				let top = a[k] + (b[k] - a[k]) * fx;
				let bottom = c[k] + (d[k] - c[k]) * fx;
				top + (bottom - top) * fy
			}));
		}
	}
	Ok(out)
}
