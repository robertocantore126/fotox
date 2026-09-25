//! The B3 benchmark document (docs/PERFORMANCE.md §3, M2-T08), built in
//! memory on top of an open document (normally B1): 199 sparse pixel layers
//! (each 2–10 % of the area, random blend modes and opacities) and 20
//! adjustment layers.
//!
//! Each pixel layer is a rectangle of one colour with a soft 64 px edge.
//! Tiles fully inside are `Solid` and tiles outside `Empty` (no memory); only
//! the feathered edge tiles hold pixels. That keeps B3 at a few GB instead of
//! the ~86 GB 199 photographic layers would take, while the compositor still
//! blends every layer on every tile it covers — which is what B3 measures.
//! Deterministic: the same seed gives the same document.

use std::sync::Arc;

use fx_core::layer::{Adjustment, LevelsChannel};
use fx_core::{BlendMode, Layer, LayerId, LayerKind};
use fx_tiles::{PixelFormat, PixelValue, TILE_SIZE, TileBuffer, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

pub const PIXEL_LAYERS: usize = 199;
pub const ADJUSTMENT_LAYERS: usize = 20;
/// Width of the soft edge, in pixels.
const FEATHER: f64 = 64.0;

/// Blend modes the random layers use (no Pass Through: not a group).
const MODES: [BlendMode; 26] = {
	use BlendMode::*;
	[
		Normal,
		Dissolve,
		Darken,
		Multiply,
		ColorBurn,
		LinearBurn,
		DarkerColor,
		Lighten,
		Screen,
		ColorDodge,
		LinearDodge,
		LighterColor,
		Overlay,
		SoftLight,
		HardLight,
		VividLight,
		LinearLight,
		PinLight,
		HardMix,
		Difference,
		Exclusion,
		Subtract,
		Divide,
		Hue,
		Saturation,
		Color,
	]
};

/// splitmix64: small, fast, deterministic.
struct Rng(u64);

impl Rng {
	fn next(&mut self) -> u64 {
		self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
		let mut z = self.0;
		z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
		z ^ (z >> 31)
	}
	fn unit(&mut self) -> f64 {
		(self.next() >> 11) as f64 / (1u64 << 53) as f64
	}
	fn range(&mut self, lo: f64, hi: f64) -> f64 {
		lo + (hi - lo) * self.unit()
	}
}

/// The 219 new layers, bottom → top, for a `width × height` document of
/// `format`. `ids` must hold [`PIXEL_LAYERS`] + [`ADJUSTMENT_LAYERS`] fresh ids.
pub fn build(width: u32, height: u32, format: PixelFormat, ids: &[LayerId], seed: u64, store: &TileStore) -> Vec<Arc<Layer>> {
	assert_eq!(ids.len(), PIXEL_LAYERS + ADJUSTMENT_LAYERS, "one id per B3 layer");
	let mut rng = Rng(seed);
	// Every 10th slot is an adjustment layer (20 of 219), the rest pixel layers.
	let plan: Vec<bool> = (0..ids.len()).map(|i| i % 11 == 10).collect();
	debug_assert_eq!(plan.iter().filter(|a| **a).count(), ADJUSTMENT_LAYERS - 1);

	// Parameters first (sequential, deterministic), pixels in parallel.
	struct Spec {
		id: LayerId,
		rect: (f64, f64, f64, f64),
		rgb: [u16; 3],
		blend: BlendMode,
		opacity: f32,
	}
	let mut pixel_specs = Vec::new();
	let mut adjustments = Vec::new();
	let mut adjustment_count = 0;
	for (i, &id) in ids.iter().enumerate() {
		let is_adjustment = plan[i] || (adjustment_count < ADJUSTMENT_LAYERS && i == ids.len() - 1);
		if is_adjustment {
			adjustments.push((i, id, adjustment(adjustment_count, &mut rng)));
			adjustment_count += 1;
			continue;
		}
		let area = rng.range(0.02, 0.10) * f64::from(width) * f64::from(height);
		let aspect = rng.range(0.4, 2.5);
		let w = (area * aspect).sqrt().min(f64::from(width));
		let h = (area / w).min(f64::from(height));
		let x0 = rng.range(0.0, f64::from(width) - w);
		let y0 = rng.range(0.0, f64::from(height) - h);
		pixel_specs.push((
			i,
			Spec {
				id,
				rect: (x0, y0, x0 + w, y0 + h),
				rgb: [0, 1, 2].map(|_| (rng.range(0.1, 0.95) * 65535.0) as u16),
				blend: MODES[(rng.next() % MODES.len() as u64) as usize],
				opacity: rng.range(0.35, 1.0) as f32,
			},
		));
	}

	let mut layers: Vec<(usize, Layer)> = pixel_specs
		.into_par_iter()
		.map(|(i, spec)| {
			let image = feathered_rect(width, height, format, spec.rect, spec.rgb, store);
			let mut layer = Layer::new(spec.id, format!("B3 layer {i}"), LayerKind::Pixel { image, offset: (0, 0) });
			layer.blend = spec.blend;
			layer.opacity = spec.opacity;
			(i, layer)
		})
		.collect();
	for (i, id, adjustment) in adjustments {
		let name = format!("B3 adjustment {i}");
		layers.push((i, Layer::new(id, name, LayerKind::Adjustment(adjustment))));
	}
	layers.sort_by_key(|(i, _)| *i);
	layers.into_iter().map(|(_, l)| Arc::new(l)).collect()
}

/// A mild adjustment of one of the M2 kinds, cycling through them.
fn adjustment(n: usize, rng: &mut Rng) -> Adjustment {
	match n % 5 {
		0 => Adjustment::Curves {
			channels: [
				vec![(0.0, 0.0), (0.3, rng.range(0.25, 0.4) as f32), (0.7, rng.range(0.65, 0.8) as f32), (1.0, 1.0)],
				vec![],
				vec![],
				vec![],
			],
		},
		1 => {
			let mut channels = [LevelsChannel::default(); 4];
			channels[0].gamma = rng.range(0.8, 1.25) as f32;
			Adjustment::Levels { channels }
		}
		2 => Adjustment::HueSaturation {
			hue: rng.range(-20.0, 20.0) as f32,
			saturation: rng.range(-15.0, 15.0) as f32,
			lightness: 0.0,
			colorize: false,
		},
		3 => Adjustment::BrightnessContrast {
			brightness: rng.range(-15.0, 15.0) as f32,
			contrast: rng.range(-10.0, 10.0) as f32,
			legacy: false,
		},
		_ => Adjustment::Exposure {
			exposure: rng.range(-0.3, 0.3) as f32,
			offset: 0.0,
			gamma: 1.0,
		},
	}
}

/// A document-sized image: `rgb` inside `rect`, fading to transparent over
/// [`FEATHER`] px at the edges.
fn feathered_rect(width: u32, height: u32, format: PixelFormat, rect: (f64, f64, f64, f64), rgb: [u16; 3], store: &TileStore) -> TiledImage {
	let mut image = TiledImage::new(width, height, format);
	let (x0, y0, x1, y1) = rect;
	let tile = f64::from(TILE_SIZE);
	let solid = TileSlot::Solid(PixelValue::rgba16(rgb[0], rgb[1], rgb[2], 65535));
	let (c0, c1) = ((x0 / tile).floor() as u32, ((x1 / tile).ceil() as u32).min(image.grid(0).cols()));
	let (r0, r1) = ((y0 / tile).floor() as u32, ((y1 / tile).ceil() as u32).min(image.grid(0).rows()));
	for ty in r0..r1 {
		for tx in c0..c1 {
			let (tx0, ty0) = (f64::from(tx) * tile, f64::from(ty) * tile);
			let inside = tx0 >= x0 + FEATHER && ty0 >= y0 + FEATHER && tx0 + tile <= x1 - FEATHER && ty0 + tile <= y1 - FEATHER;
			if inside {
				image.set_slot(tx, ty, solid.clone());
				continue;
			}
			let mut buffer = TileBuffer::zeroed(format);
			for y in 0..TILE_SIZE {
				for x in 0..TILE_SIZE {
					let (px, py) = (tx0 + f64::from(x) + 0.5, ty0 + f64::from(y) + 0.5);
					let edge = (px - x0).min(x1 - px).min(py - y0).min(y1 - py);
					if edge <= 0.0 {
						continue;
					}
					let a = (edge / FEATHER).min(1.0);
					let alpha = (a * a * (3.0 - 2.0 * a) * 65535.0).round() as u16;
					write_pixel(&mut buffer, x, y, [rgb[0], rgb[1], rgb[2], alpha]);
				}
			}
			image.put_buffer(store, tx, ty, buffer);
		}
	}
	image
}

fn write_pixel(buffer: &mut TileBuffer, x: u32, y: u32, v: [u16; 4]) {
	let i = ((y * TILE_SIZE + x) * 4) as usize;
	match buffer.format() {
		PixelFormat::Rgba16 => buffer.as_u16_mut()[i..i + 4].copy_from_slice(&v),
		_ => {
			let b = buffer.bytes_mut();
			for c in 0..4 {
				b[i + c] = (v[c] / 257) as u8;
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_tiles::TileStoreConfig;

	fn store() -> TileStore {
		let mut config = TileStoreConfig::for_tests(std::env::temp_dir().join("fx-engine-b3-tests"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).unwrap()
	}

	#[test]
	fn b3_has_199_pixel_and_20_adjustment_layers_deterministically() {
		let store = store();
		let ids: Vec<LayerId> = (1..=219).map(LayerId).collect();
		let a = build(4096, 4096, PixelFormat::Rgba8, &ids, 3, &store);
		assert_eq!(a.len(), 219);
		let pixels = a.iter().filter(|l| matches!(l.kind, LayerKind::Pixel { .. })).count();
		let adjustments = a.iter().filter(|l| matches!(l.kind, LayerKind::Adjustment(_))).count();
		assert_eq!((pixels, adjustments), (PIXEL_LAYERS, ADJUSTMENT_LAYERS));
		let b = build(4096, 4096, PixelFormat::Rgba8, &ids, 3, &store);
		let summary = |layers: &[Arc<Layer>]| layers.iter().map(|l| (l.id, l.blend, l.opacity.to_bits(), l.name.clone())).collect::<Vec<_>>();
		assert_eq!(summary(&a), summary(&b), "same seed, same document");
	}

	#[test]
	fn a_feathered_rect_is_mostly_solid_and_empty_tiles() {
		let store = store();
		let image = feathered_rect(4096, 4096, PixelFormat::Rgba16, (300.0, 300.0, 3000.0, 2500.0), [100, 200, 300], &store);
		let grid = image.grid(0);
		let (mut empty, mut solid, mut data) = (0, 0, 0);
		for ty in 0..grid.rows() {
			for tx in 0..grid.cols() {
				match grid.slot(tx, ty) {
					TileSlot::Empty => empty += 1,
					TileSlot::Solid(_) => solid += 1,
					TileSlot::Data(_) => data += 1,
				}
			}
		}
		assert!(solid > data, "interior tiles cost no memory ({solid} solid, {data} data)");
		assert!(empty > 0 && data > 0);
	}
}
