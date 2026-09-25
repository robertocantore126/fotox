//! The clipboard (M5-T05, D-048).
//!
//! Copied pixels stay in the tile store as a [`ClipboardImage`]: a copy of a
//! 30 000² layer costs the tile handles, not gigabytes. A copy that is at most
//! [`OS_LIMIT`] pixels on a side is also handed to the shell as straight RGBA8
//! for the Windows clipboard (CF_DIBV5), and an image another program put on
//! the Windows clipboard pastes as a new layer.

use fx_core::pixels::{self, ClipboardImage, Placed};
use fx_core::{Document, LayerKind, Selection};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileError, TileSlot, TileStore, TiledImage};

/// Largest side of a copy that also goes to the Windows clipboard.
pub const OS_LIMIT: u32 = 8192;

/// The selected pixels of `layer` (or all of them without a selection) as
/// clipboard content. `None` when nothing is there to copy.
pub fn copy_layer(
	image: &TiledImage,
	offset: (i32, i32),
	selection: Option<&Selection>,
	canvas: (u32, u32),
	store: &TileStore,
) -> Result<Option<ClipboardImage>, TileError> {
	let taken = pixels::extract(Placed { image, offset }, selection, canvas, store)?;
	let Some(bounds) = alpha_bounds(&taken, offset, store)? else {
		return Ok(None);
	};
	Ok(Some(ClipboardImage { image: taken, offset, bounds }))
}

/// The active pixel layer of `doc`: its image and offset.
pub fn active_pixels(doc: &Document) -> Option<(&TiledImage, (i32, i32))> {
	let id = doc.active_layer()?;
	match &doc.layer(id)?.kind {
		LayerKind::Pixel { image, offset } => Some((image, *offset)),
		_ => None,
	}
}

/// The canvas rectangle `(x0, y0, x1, y1)` (exclusive) of the pixels with
/// alpha > 0: the non-empty tiles' box, refined on its border tiles.
pub fn alpha_bounds(image: &TiledImage, offset: (i32, i32), store: &TileStore) -> Result<Option<(i32, i32, i32, i32)>, TileError> {
	let mut tiles = image.grid(0).non_empty().map(|(x, y, _)| (x, y)).peekable();
	if tiles.peek().is_none() {
		return Ok(None);
	}
	let (mut tx0, mut ty0, mut tx1, mut ty1) = (u32::MAX, u32::MAX, 0, 0);
	for (x, y) in tiles {
		tx0 = tx0.min(x);
		ty0 = ty0.min(y);
		tx1 = tx1.max(x);
		ty1 = ty1.max(y);
	}
	let format = image.format();
	let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
	for (tx, ty, slot) in image.grid(0).non_empty() {
		// Only tiles on the box's border can move its edges.
		if tx != tx0 && tx != tx1 && ty != ty0 && ty != ty1 {
			continue;
		}
		let (bx, by) = (i64::from(tx * TILE_SIZE), i64::from(ty * TILE_SIZE));
		let (vw, vh) = (
			(image.width() - tx * TILE_SIZE).min(TILE_SIZE),
			(image.height() - ty * TILE_SIZE).min(TILE_SIZE),
		);
		let alpha: Box<dyn Fn(u32, u32) -> bool> = match slot {
			TileSlot::Empty => continue,
			TileSlot::Solid(_) => Box::new(|_, _| true),
			TileSlot::Data(handle) => {
				let buffer = store.get(handle)?;
				Box::new(move |x, y| alpha_at(&buffer, format, x, y) > 0)
			}
		};
		for y in 0..vh {
			for x in 0..vw {
				if alpha(x, y) {
					x0 = x0.min(bx + i64::from(x));
					y0 = y0.min(by + i64::from(y));
					x1 = x1.max(bx + i64::from(x) + 1);
					y1 = y1.max(by + i64::from(y) + 1);
				}
			}
		}
	}
	if x0 >= x1 || y0 >= y1 {
		return Ok(None);
	}
	let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
	Ok(Some(((x0 + ox) as i32, (y0 + oy) as i32, (x1 + ox) as i32, (y1 + oy) as i32)))
}

fn alpha_at(buffer: &TileBuffer, format: PixelFormat, x: u32, y: u32) -> u16 {
	let i = ((y * TILE_SIZE + x) * 4 + 3) as usize;
	match format {
		PixelFormat::Rgba16 => buffer.as_u16()[i],
		_ => u16::from(buffer.bytes()[i]),
	}
}

/// The clipboard content as straight RGBA8 rows (its bounds), for the Windows
/// clipboard. `None` above [`OS_LIMIT`] (D-048: the pixels stay internal).
pub fn os_pixels(clip: &ClipboardImage, store: &TileStore) -> Result<Option<(u32, u32, Vec<u8>)>, TileError> {
	let (x0, y0, x1, y1) = clip.bounds;
	let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
	if w == 0 || h == 0 || w > OS_LIMIT || h > OS_LIMIT {
		return Ok(None);
	}
	let format = clip.image.format();
	let mut out = vec![0u8; (w * h * 4) as usize];
	let mut cache: std::collections::HashMap<(u32, u32), Option<std::sync::Arc<TileBuffer>>> = std::collections::HashMap::new();
	for y in 0..h {
		let iy = i64::from(y0) + i64::from(y) - i64::from(clip.offset.1);
		if iy < 0 || iy >= i64::from(clip.image.height()) {
			continue;
		}
		for x in 0..w {
			let ix = i64::from(x0) + i64::from(x) - i64::from(clip.offset.0);
			if ix < 0 || ix >= i64::from(clip.image.width()) {
				continue;
			}
			let (ix, iy) = (ix as u32, iy as u32);
			let key = (ix / TILE_SIZE, iy / TILE_SIZE);
			if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
				entry.insert(match clip.image.slot(0, key.0, key.1) {
					TileSlot::Empty => None,
					TileSlot::Solid(v) => Some(std::sync::Arc::new(TileBuffer::filled(format, *v))),
					TileSlot::Data(handle) => Some(store.get(handle)?),
				});
			}
			let Some(buffer) = &cache[&key] else { continue };
			let i = (((iy % TILE_SIZE) * TILE_SIZE + ix % TILE_SIZE) * 4) as usize;
			let o = ((y * w + x) * 4) as usize;
			match format {
				PixelFormat::Rgba16 => {
					for c in 0..4 {
						out[o + c] = ((u32::from(buffer.as_u16()[i + c]) * 255 + 32767) / 65535) as u8;
					}
				}
				_ => out[o..o + 4].copy_from_slice(&buffer.bytes()[i..i + 4]),
			}
		}
	}
	Ok(Some((w, h, out)))
}

/// An image from the Windows clipboard (straight RGBA8, `w × h`) as clipboard
/// content at the canvas origin, in `format`.
pub fn from_os(width: u32, height: u32, rgba8: &[u8], format: PixelFormat, store: &TileStore) -> ClipboardImage {
	let mut image = TiledImage::new(width.max(1), height.max(1), format);
	for ty in 0..height.div_ceil(TILE_SIZE) {
		for tx in 0..width.div_ceil(TILE_SIZE) {
			let mut buffer = TileBuffer::zeroed(format);
			for y in 0..TILE_SIZE.min(height - ty * TILE_SIZE) {
				for x in 0..TILE_SIZE.min(width - tx * TILE_SIZE) {
					let src = (((ty * TILE_SIZE + y) * width + tx * TILE_SIZE + x) * 4) as usize;
					let dst = ((y * TILE_SIZE + x) * 4) as usize;
					match format {
						PixelFormat::Rgba16 => {
							for c in 0..4 {
								buffer.as_u16_mut()[dst + c] = u16::from(rgba8[src + c]) * 257;
							}
						}
						_ => buffer.bytes_mut()[dst..dst + 4].copy_from_slice(&rgba8[src..src + 4]),
					}
				}
			}
			image.put_buffer(store, tx, ty, buffer);
		}
	}
	ClipboardImage {
		image,
		offset: (0, 0),
		bounds: (0, 0, width as i32, height as i32),
	}
}

#[cfg(test)]
mod tests {
	use fx_tiles::{PixelValue, TileStoreConfig};

	use super::*;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-engine-clipboard-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	#[test]
	fn alpha_bounds_are_exact_across_tiles() {
		let store = store();
		let mut image = TiledImage::new(1000, 600, PixelFormat::Rgba8);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
		buffer.bytes_mut()[((10 * TILE_SIZE + 20) * 4 + 3) as usize] = 255;
		image.put_buffer(&store, 0, 0, buffer);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba8);
		buffer.bytes_mut()[((5 * TILE_SIZE + 7) * 4 + 3) as usize] = 1;
		image.put_buffer(&store, 3, 2, buffer);
		image.set_slot(1, 1, TileSlot::Solid(PixelValue::rgba8(1, 2, 3, 4)));
		assert_eq!(
			alpha_bounds(&image, (100, -50), &store).unwrap(),
			Some((120, -40, 100 + 3 * 256 + 8, -50 + 2 * 256 + 6))
		);
	}

	#[test]
	fn a_copy_round_trips_through_rgba8() {
		let store = store();
		let rgba: Vec<u8> = (0..300 * 2).flat_map(|i| [(i % 256) as u8, 7, 9, 255]).collect();
		let clip = from_os(300, 2, &rgba, PixelFormat::Rgba16, &store);
		let (w, h, back) = os_pixels(&clip, &store).unwrap().unwrap();
		assert_eq!((w, h), (300, 2));
		assert_eq!(back, rgba);
	}
}
