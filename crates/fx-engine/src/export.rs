//! Export of the flattened document (M3): the CPU reference compositor
//! renders one row of tiles at a time (tiles in parallel), `fx_io::export`
//! encodes it. Memory stays at one band, whatever the document height.
//!
//! The reference compositor is exact but slow (f64 per pixel); a GPU readback
//! path can replace it later without changing the file side.

use std::path::Path;

use fx_core::{BitDepth, Document};
use fx_io::export::{EXPORT_BAND_ROWS, ExportFormat, ExportOptions, export_image};
use fx_io::{IoError, Progress};
use fx_render::adjust::LutCache;
use fx_render::blend::unpremultiply;
use fx_render::build_program;
use fx_render::reference::render_tile;
use fx_tiles::{TILE_SIZE, TileStore};
use rayon::prelude::*;

const _: () = assert!(EXPORT_BAND_ROWS == TILE_SIZE, "one band = one row of tiles");

/// Options for exporting `doc` to `path`: the format from the extension, the
/// document's bit depth (8-bit only for JPEG-like targets later), transparency kept.
pub fn options_for(doc: &Document, path: &Path) -> Result<ExportOptions, IoError> {
	let format = ExportFormat::from_path(path).ok_or_else(|| IoError::Unsupported("export to this file type (use .tif or .png)".into()))?;
	let bits = match doc.color.depth {
		BitDepth::U8 => 8,
		BitDepth::U16 => 16,
	};
	Ok(ExportOptions {
		format,
		bits,
		alpha: true,
		ppi: doc.ppi,
	})
}

/// Composite `doc` and write it to `path`.
pub fn export_document(doc: &Document, store: &TileStore, path: &Path, options: ExportOptions, progress: Progress<'_>) -> Result<(), IoError> {
	let tiles_x = doc.width.div_ceil(TILE_SIZE);
	let width = doc.width as usize;
	let mut luts = LutCache::default();
	let fetch = |h: &fx_tiles::TileHandle| store.get(h).expect("tile of a live document");
	let mut render = |y: u32, rows: u32, out: &mut [[u16; 4]]| -> Result<(), IoError> {
		let ty = y / TILE_SIZE;
		// Programs are cheap and need the LUT cache: build them first, render in parallel.
		let programs = (0..tiles_x)
			.map(|tx| build_program(doc, 0, tx, ty, &mut |a| luts.get(a)))
			.collect::<Result<Vec<_>, _>>()
			.map_err(|_| IoError::Decode("full-resolution tiles are missing".into()))?;
		let tiles: Vec<_> = programs.par_iter().map(|p| render_tile(p, &fetch)).collect();
		for (tx, tile) in tiles.iter().enumerate() {
			let x0 = tx * TILE_SIZE as usize;
			let cols = (TILE_SIZE as usize).min(width - x0);
			for row in 0..rows as usize {
				let src = &tile[row * TILE_SIZE as usize..][..cols];
				let dst = &mut out[row * width + x0..][..cols];
				for (d, &p) in dst.iter_mut().zip(src) {
					let rgb = unpremultiply(p);
					*d = [to_u16(rgb[0]), to_u16(rgb[1]), to_u16(rgb[2]), to_u16(p[3])];
				}
			}
		}
		Ok(())
	};
	export_image(path, doc.width, doc.height, options, &mut render, progress)
}

fn to_u16(v: f64) -> u16 {
	(v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use fx_core::{ColorProfile, Document, DocumentColor, Layer, LayerKind};
	use fx_tiles::{PixelFormat, PixelValue, TileBuffer, TileClass, TileSlot, TileStore, TileStoreConfig, TiledImage};

	use super::*;

	fn dir() -> std::path::PathBuf {
		let dir = std::env::temp_dir().join("fx-engine-export-tests");
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	/// Red background + a half-transparent blue layer over the left 300 px.
	fn document(store: &TileStore) -> Document {
		let (w, h) = (400, 300);
		let mut doc = Document::new(
			w,
			h,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let solid = |rgba: [u16; 4]| {
			let mut image = TiledImage::new(w, h, PixelFormat::Rgba16);
			for ty in 0..h.div_ceil(TILE_SIZE) {
				for tx in 0..w.div_ceil(TILE_SIZE) {
					image.set_slot(tx, ty, TileSlot::Solid(PixelValue(rgba)));
				}
			}
			image
		};
		let bottom = solid([65535, 0, 0, 65535]);
		let mut top = TiledImage::new(w, h, PixelFormat::Rgba16);
		for ty in 0..2 {
			// Tile column 0 fully, column 1 up to x = 300 (44 px of 256).
			top.set_slot(0, ty, TileSlot::Solid(PixelValue([0, 0, 65535, 32768])));
			let mut tile = TileBuffer::zeroed(PixelFormat::Rgba16);
			let px = tile.as_u16_mut();
			for y in 0..256 {
				for x in 0..44 {
					px[(y * 256 + x) * 4..][..4].copy_from_slice(&[0, 0, 65535, 32768]);
				}
			}
			top.set_slot(1, ty, TileSlot::Data(store.insert(tile, TileClass::Authoritative)));
		}
		for (name, image) in [("Background", bottom), ("Blue", top)] {
			let id = doc.allocate_layer_id();
			doc.layers.push(Arc::new(Layer::new(id, name, LayerKind::Pixel { image, offset: (0, 0) })));
		}
		doc
	}

	#[test]
	fn exports_the_composite() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir().join("scratch"))).unwrap();
		let doc = document(&store);
		let path = dir().join("composite.png");
		let options = options_for(&doc, &path).unwrap();
		assert_eq!(
			options,
			ExportOptions {
				format: ExportFormat::Png,
				bits: 16,
				alpha: true,
				ppi: 72.0
			}
		);
		export_document(&doc, &store, &path, options, &mut |_| true).unwrap();

		let back = fx_io::import_file(&path, &store, &mut |_| true).unwrap();
		assert_eq!((back.width, back.height), (400, 300));
		let at = |x: u32, y: u32| -> [u16; 4] {
			let tile = match back.image.slot(0, x / 256, y / 256) {
				TileSlot::Solid(v) => TileBuffer::filled(PixelFormat::Rgba16, *v),
				TileSlot::Data(h) => (*store.get(h).unwrap()).clone(),
				TileSlot::Empty => TileBuffer::zeroed(PixelFormat::Rgba16),
			};
			let i = ((y % 256) * 256 + x % 256) as usize * 4;
			let s = tile.as_u16();
			[s[i], s[i + 1], s[i + 2], s[i + 3]]
		};
		// Half blue over red; plain red right of x = 300.
		let mixed = at(10, 10);
		assert!(
			(i32::from(mixed[0]) - 32767).abs() <= 2 && (i32::from(mixed[2]) - 32768).abs() <= 2 && mixed[3] == 65535,
			"{mixed:?}"
		);
		assert_eq!(at(299, 299), mixed);
		assert_eq!(at(300, 0), [65535, 0, 0, 65535]);
		assert_eq!(at(399, 299), [65535, 0, 0, 65535]);
	}

	#[test]
	fn unknown_extension_is_refused() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir().join("scratch2"))).unwrap();
		let doc = document(&store);
		assert!(options_for(&doc, Path::new("x.jpg")).is_err());
	}
}
