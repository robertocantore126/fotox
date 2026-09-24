//! Test helpers: small documents built from pixel functions.

use std::sync::Arc;

use fx_core::{BitDepth, BlendMode, ColorProfile, Document, DocumentColor, Layer, LayerKind, Mask};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileStore, TileStoreConfig, TiledImage};

pub fn store() -> TileStore {
	TileStore::new(TileStoreConfig::for_tests(std::env::temp_dir().join("fx-render-tests"))).unwrap()
}

pub fn doc(width: u32, height: u32) -> Document {
	Document::new(
		width,
		height,
		DocumentColor {
			depth: BitDepth::U16,
			profile: ColorProfile::Srgb,
		},
		72.0,
	)
}

/// An image whose pixel `(x, y)` is `f(x, y)` (16-bit values), level 0 only.
pub fn image(store: &TileStore, width: u32, height: u32, format: PixelFormat, f: &dyn Fn(u32, u32) -> [u16; 4]) -> TiledImage {
	let mut img = TiledImage::new(width, height, format);
	let grid = img.grid(0).clone();
	for ty in 0..grid.rows() {
		for tx in 0..grid.cols() {
			let mut buf = TileBuffer::zeroed(format);
			for y in 0..TILE_SIZE {
				for x in 0..TILE_SIZE {
					let (gx, gy) = (tx * TILE_SIZE + x, ty * TILE_SIZE + y);
					if gx >= width || gy >= height {
						continue; // edge pixels stay transparent / zero
					}
					let v = f(gx, gy);
					let i = (y * TILE_SIZE + x) as usize;
					match format {
						PixelFormat::Rgba16 => {
							for (c, value) in v.iter().enumerate() {
								buf.bytes_mut()[i * 8 + c * 2..i * 8 + c * 2 + 2].copy_from_slice(&value.to_ne_bytes());
							}
						}
						PixelFormat::Rgba8 => {
							for (c, value) in v.iter().enumerate() {
								buf.bytes_mut()[i * 4 + c] = (value / 257) as u8;
							}
						}
						PixelFormat::Gray16 => buf.bytes_mut()[i * 2..i * 2 + 2].copy_from_slice(&v[0].to_ne_bytes()),
						PixelFormat::Gray8 => buf.bytes_mut()[i] = (v[0] / 257) as u8,
					}
				}
			}
			img.put_buffer(store, tx, ty, buf);
		}
	}
	img
}

pub fn pixel_layer(doc: &mut Document, store: &TileStore, f: &dyn Fn(u32, u32) -> [u16; 4]) -> Layer {
	let id = doc.allocate_layer_id();
	Layer::new(
		id,
		format!("Layer {}", id.0),
		LayerKind::Pixel {
			image: image(store, doc.width, doc.height, PixelFormat::Rgba16, f),
			offset: (0, 0),
		},
	)
}

pub fn solid_layer(doc: &mut Document, rgba: [u16; 4]) -> Layer {
	let id = doc.allocate_layer_id();
	Layer::new(id, format!("Fill {}", id.0), LayerKind::SolidFill { rgba })
}

pub fn group(doc: &mut Document, blend: BlendMode, children: Vec<Layer>) -> Layer {
	let id = doc.allocate_layer_id();
	let mut layer = Layer::new(
		id,
		format!("Group {}", id.0),
		LayerKind::Group {
			children: children.into_iter().map(Arc::new).collect(),
			expanded: true,
		},
	);
	layer.blend = blend;
	layer
}

pub fn mask(doc: &Document, store: &TileStore, f: &dyn Fn(u32, u32) -> u16) -> Mask {
	Mask {
		image: image(store, doc.width, doc.height, PixelFormat::Gray16, &|x, y| [f(x, y), 0, 0, 0]),
		enabled: true,
		linked: true,
		outside_value: 0,
	}
}

/// Deterministic pseudo-random 16-bit value.
pub fn hash16(x: u32, y: u32, seed: u32) -> u16 {
	(crate::reference::dissolve_hash(x, y, seed) * 65535.0) as u16
}

/// A layer with a mix of empty, solid and noisy tiles, partly transparent.
pub fn busy_layer(doc: &mut Document, store: &TileStore, seed: u32) -> Layer {
	let f = move |x: u32, y: u32| {
		let tile = (x / TILE_SIZE + 3 * (y / TILE_SIZE) + seed) % 4;
		match tile {
			0 => [0, 0, 0, 0],
			1 => [40000, 20000, 1000, 65535],
			_ => [
				hash16(x, y, seed),
				hash16(x, y, seed + 1),
				hash16(x, y, seed + 2),
				hash16(x / 7, y / 7, seed + 3),
			],
		}
	};
	pixel_layer(doc, store, &f)
}
