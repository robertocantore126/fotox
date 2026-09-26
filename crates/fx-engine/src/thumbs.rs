//! Layer thumbnails for the Layers panel (M2-T07).
//!
//! A thumbnail is the layer alone, in the document frame (so a moved layer
//! shows where it sits), fitted into a `size × size` box, RGBA8 straight.
//! It is rendered from the deepest mip level that still has at least `size`
//! pixels along the document's longer side, so only a handful of tiles are
//! read whatever the document size. Runs on the rayon pool: it may read tiles
//! from disk and compute missing mips (on its own copy of the image).

use fx_core::LayerKind;
use fx_core::vector::{Paint, StrokeStyle, VectorShape};
use fx_tiles::{PixelFormat, TILE_SIZE, TileClass, TileError, TileSlot, TileStore, TiledImage};

use crate::mips;

/// A rendered thumbnail.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnail {
	pub width: u32,
	pub height: u32,
	/// `width × height × 4` bytes, RGBA8, straight alpha.
	pub pixels: Vec<u8>,
}

/// What a thumbnail job needs from the layer, taken on the engine thread.
pub enum ThumbSource {
	Pixels {
		image: TiledImage,
		offset: (i32, i32),
	},
	Solid([u16; 4]),
	/// A shape layer: no stored pixels, so the job draws the tiles it needs
	/// from the geometry (M6-T06).
	Shape {
		shape: VectorShape,
		fill: Option<Paint>,
		stroke: Option<StrokeStyle>,
		transform: [f64; 6],
	},
}

impl ThumbSource {
	/// The thumbnail source of a layer; `None` for groups and adjustment
	/// layers (the panel shows icons for those, like Photoshop).
	pub fn of(kind: &LayerKind) -> Option<Self> {
		match kind {
			LayerKind::Pixel { image, offset } => Some(Self::Pixels {
				image: image.clone(),
				offset: *offset,
			}),
			LayerKind::SolidFill { rgba } => Some(Self::Solid(*rgba)),
			LayerKind::Shape {
				shape,
				fill,
				stroke,
				transform,
				cache: _,
			} => Some(Self::Shape {
				shape: shape.clone(),
				fill: *fill,
				stroke: stroke.clone(),
				transform: *transform,
			}),
			// FAST: a Smart Object's thumbnail is its source composite at its
			// box's origin (the transform's scale / rotation are ignored).
			LayerKind::Smart { smart, .. } => Some(Self::Pixels {
				image: smart.source.composite.clone(),
				offset: smart.bounds().map_or((0, 0), |(at, _)| at),
			}),
			_ => None,
		}
	}
}

/// The thumbnail box for a `doc_w × doc_h` document: the longer side is `size`.
pub fn fitted(doc_w: u32, doc_h: u32, size: u32) -> (u32, u32) {
	let size = size.max(1);
	if doc_w >= doc_h {
		(size, ((u64::from(doc_h) * u64::from(size)).div_ceil(u64::from(doc_w)) as u32).max(1))
	} else {
		(((u64::from(doc_w) * u64::from(size)).div_ceil(u64::from(doc_h)) as u32).max(1), size)
	}
}

/// Render the thumbnail of `source` in a `doc_w × doc_h` document.
pub fn render(source: ThumbSource, doc_w: u32, doc_h: u32, size: u32, store: &TileStore) -> Result<Thumbnail, TileError> {
	let (tw, th) = fitted(doc_w, doc_h, size);
	// A shape has no pixels to read: its tiles are drawn at whatever level the
	// thumbnail picks (M6-T06), so it is kept aside rather than read.
	let shape = match &source {
		ThumbSource::Shape {
			shape,
			fill,
			stroke,
			transform,
		} => Some((shape.clone(), *fill, stroke.clone(), *transform)),
		_ => None,
	};
	let (mut image, offset) = match &source {
		ThumbSource::Solid(rgba) => {
			let px = [rgba[0], rgba[1], rgba[2], rgba[3]].map(|v| (v / 257) as u8);
			return Ok(Thumbnail {
				width: tw,
				height: th,
				pixels: px.repeat((tw * th) as usize),
			});
		}
		// Cloning a `TiledImage` clones tile handles, not pixels.
		ThumbSource::Pixels { image, offset } => (image.clone(), *offset),
		// A shape fills the whole canvas, so its placement is the canvas's.
		ThumbSource::Shape { .. } => (TiledImage::derived(doc_w, doc_h, PixelFormat::Rgba8), (0, 0)),
	};

	// Deepest level whose longer side still has ≥ the thumbnail's pixels.
	let longer = doc_w.max(doc_h);
	let mut level = 0;
	while level + 1 < image.level_count() && longer.div_ceil(1 << (level + 1)) >= tw.max(th) {
		level += 1;
	}
	let scale = f64::from(1u32 << level);

	// Every tile of that level, stitched (a few tiles at most).
	let (lw, lh) = image.level_size(level);
	let grid = image.grid(level);
	let (cols, rows) = (grid.cols(), grid.rows());
	let mut level_px = vec![[0f32; 4]; (lw * lh) as usize];
	for ty in 0..rows {
		for tx in 0..cols {
			let slot = match &shape {
				// Drawn at the level the thumbnail reads, so a shape is as
				// sharp in the panel as it is on the canvas (M6-T06).
				Some((shape, fill, stroke, transform)) => {
					let buffer = fx_render::render_shape_tile(shape, fill.as_ref(), stroke.as_ref(), *transform, level, (tx, ty), PixelFormat::Rgba8);
					match buffer.uniform_value() {
						Some(value) if value.is_transparent(PixelFormat::Rgba8) => TileSlot::Empty,
						_ => TileSlot::Data(store.insert(buffer, TileClass::Derived)),
					}
				}
				None if level == 0 => image.slot(0, tx, ty).clone(),
				None => mips::ensure_mip(&mut image, store, level, tx, ty)?,
			};
			copy_tile(&slot, image.format(), store, tx, ty, lw, lh, &mut level_px)?;
		}
	}

	// Box-filter the level into the thumbnail, in the document frame.
	let mut pixels = vec![0u8; (tw * th * 4) as usize];
	let (sx, sy) = (f64::from(doc_w) / f64::from(tw), f64::from(doc_h) / f64::from(th));
	for y in 0..th {
		for x in 0..tw {
			// Document rectangle of this thumbnail pixel → layer level pixels.
			let x0 = ((f64::from(x) * sx - f64::from(offset.0)) / scale).floor() as i64;
			let x1 = ((f64::from(x + 1) * sx - f64::from(offset.0)) / scale).ceil() as i64;
			let y0 = ((f64::from(y) * sy - f64::from(offset.1)) / scale).floor() as i64;
			let y1 = ((f64::from(y + 1) * sy - f64::from(offset.1)) / scale).ceil() as i64;
			let mut acc = [0f64; 4];
			for ly in y0.max(0)..y1.min(i64::from(lh)) {
				for lx in x0.max(0)..x1.min(i64::from(lw)) {
					let p = level_px[(ly as u32 * lw + lx as u32) as usize];
					let a = f64::from(p[3]);
					for c in 0..3 {
						acc[c] += f64::from(p[c]) * a;
					}
					acc[3] += a;
				}
			}
			// Pixels of the box outside the layer count as transparent.
			let area = ((x1 - x0).max(1) * (y1 - y0).max(1)) as f64;
			let out = &mut pixels[((y * tw + x) * 4) as usize..][..4];
			if acc[3] > 0.0 {
				for c in 0..3 {
					out[c] = (acc[c] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
				}
				out[3] = (acc[3] / area * 255.0).round().clamp(0.0, 255.0) as u8;
			}
		}
	}
	Ok(Thumbnail { width: tw, height: th, pixels })
}

/// Copy tile `(tx, ty)` of a level into `out` (`lw × lh`, 0..1 straight RGBA).
#[allow(clippy::too_many_arguments)]
fn copy_tile(slot: &TileSlot, format: PixelFormat, store: &TileStore, tx: u32, ty: u32, lw: u32, lh: u32, out: &mut [[f32; 4]]) -> Result<(), TileError> {
	let (x0, y0) = (tx * TILE_SIZE, ty * TILE_SIZE);
	let (w, h) = (TILE_SIZE.min(lw - x0), TILE_SIZE.min(lh - y0));
	let value = |v: u16| f32::from(v) / 65535.0;
	match slot {
		TileSlot::Empty => {}
		TileSlot::Solid(v) => {
			let px = [value(v.0[0]), value(v.0[1]), value(v.0[2]), value(v.0[3])];
			for y in 0..h {
				out[((y0 + y) * lw + x0) as usize..][..w as usize].fill(px);
			}
		}
		TileSlot::Data(handle) => {
			let tile = store.get(handle)?;
			for y in 0..h {
				for x in 0..w {
					let i = (y * TILE_SIZE + x) as usize * 4;
					let px = match format {
						PixelFormat::Rgba16 => {
							let s = &tile.as_u16()[i..i + 4];
							[value(s[0]), value(s[1]), value(s[2]), value(s[3])]
						}
						_ => {
							let b = &tile.bytes()[i..i + 4];
							[b[0], b[1], b[2], b[3]].map(|v| f32::from(v) / 255.0)
						}
					};
					out[((y0 + y) * lw + x0 + x) as usize] = px;
				}
			}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_tiles::{PixelValue, TileBuffer, TileStoreConfig};

	fn store() -> TileStore {
		let mut config = TileStoreConfig::for_tests(std::env::temp_dir().join("fx-engine-thumb-tests"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).unwrap()
	}

	#[test]
	fn thumbnails_keep_the_document_aspect() {
		assert_eq!(fitted(30_000, 20_000, 48), (48, 32));
		assert_eq!(fitted(1000, 4000, 40), (10, 40));
		assert_eq!(fitted(5, 5, 0), (1, 1));
	}

	#[test]
	fn a_half_red_layer_gives_a_half_red_thumbnail() {
		let store = store();
		let mut image = TiledImage::new(2048, 1024, PixelFormat::Rgba8);
		// Left half opaque red (tiles 0..4), right half empty.
		for ty in 0..4 {
			for tx in 0..4 {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue::rgba8(255, 0, 0, 255)));
			}
		}
		let thumb = render(ThumbSource::Pixels { image, offset: (0, 0) }, 2048, 1024, 64, &store).unwrap();
		assert_eq!((thumb.width, thumb.height), (64, 32));
		let px = |x: u32, y: u32| &thumb.pixels[((y * 64 + x) * 4) as usize..][..4];
		assert_eq!(px(5, 10), &[255, 0, 0, 255]);
		assert_eq!(px(60, 10), &[0, 0, 0, 0]);
	}

	#[test]
	fn an_offset_layer_moves_in_the_thumbnail() {
		let store = store();
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba16);
		let mut tile = TileBuffer::zeroed(PixelFormat::Rgba16);
		tile.as_u16_mut().chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[0, 65535, 0, 65535]));
		image.put_buffer(&store, 0, 0, tile);
		// Top-left 256² tile green, moved right by 256: shows top-right.
		let thumb = render(ThumbSource::Pixels { image, offset: (256, 0) }, 512, 512, 32, &store).unwrap();
		let px = |x: u32, y: u32| &thumb.pixels[((y * 32 + x) * 4) as usize..][..4];
		assert_eq!(px(24, 4), &[0, 255, 0, 255]);
		assert_eq!(px(4, 4), &[0, 0, 0, 0]);
	}

	#[test]
	fn solid_fill_thumbnail() {
		let thumb = render(ThumbSource::Solid([65535, 32896, 0, 65535]), 100, 100, 8, &store()).unwrap();
		assert_eq!(&thumb.pixels[..4], &[255, 128, 0, 255]);
		assert_eq!(thumb.pixels.len(), 8 * 8 * 4);
	}
}
