//! Export of the flattened document (M3): the CPU reference compositor
//! renders one row of tiles at a time (tiles in parallel), `fx_io::export`
//! encodes it. Memory stays at one band, whatever the document height.
//!
//! The reference compositor is exact but slow (f64 per pixel); a GPU readback
//! path can replace it later without changing the file side.

use std::path::Path;

use fx_core::{BitDepth, Document, LayerKind};
use fx_io::export::{EXPORT_BAND_ROWS, ExportFormat, ExportOptions, JpegChroma, export_image};
use fx_io::{IoError, Progress};
use fx_render::adjust::LutCache;
use fx_render::blend::unpremultiply;
use fx_render::build_program;
use fx_render::reference::render_tile;
use fx_tiles::{PixelFormat, TILE_SIZE, TileSlot, TileStore};
use rayon::prelude::*;

const _: () = assert!(EXPORT_BAND_ROWS == TILE_SIZE, "one band = one row of tiles");

/// Options for exporting `doc` to `path`: the format from the extension, the
/// document's bit depth, its ppi; transparency kept unless `opaque` (see
/// [`opaque_background`]).
pub fn options_for(doc: &Document, path: &Path, opaque: bool) -> Result<ExportOptions, IoError> {
	let format = ExportFormat::from_path(path).ok_or_else(|| IoError::Unsupported("export to this file type (use .tif, .png or .jpg)".into()))?;
	// JPEG is 8-bit and has no alpha: the band is flattened onto white.
	let jpeg = format == ExportFormat::Jpeg;
	let bits = if jpeg {
		8
	} else {
		match doc.color.depth {
			BitDepth::U8 => 8,
			BitDepth::U16 => 16,
		}
	};
	Ok(ExportOptions {
		format,
		bits,
		alpha: !opaque && !jpeg,
		ppi: doc.ppi,
		quality: 90,
		chroma: JpegChroma::Full,
	})
}

/// Whether the composite is certainly opaque everywhere: the bottom visible
/// root layer covers the canvas with opaque pixels at full opacity, no mask.
/// Every layer above then keeps alpha at 1 (source-over and source-atop both
/// do, whatever the blend mode). Reads that layer's tiles once, in parallel.
pub fn opaque_background(doc: &Document, store: &TileStore) -> bool {
	let Some(bottom) = doc.layers.iter().find(|l| l.visible) else { return false };
	if bottom.opacity < 1.0 || bottom.fill < 1.0 || bottom.mask.is_some() {
		return false;
	}
	let image = match &bottom.kind {
		LayerKind::SolidFill { rgba } => return rgba[3] == u16::MAX,
		LayerKind::Pixel { image, offset: (0, 0) } if image.width() >= doc.width && image.height() >= doc.height => image,
		_ => return false,
	};
	let (tiles_x, tiles_y) = (doc.width.div_ceil(TILE_SIZE), doc.height.div_ceil(TILE_SIZE));
	(0..tiles_x * tiles_y).into_par_iter().all(|i| {
		let (tx, ty) = (i % tiles_x, i / tiles_x);
		match image.slot(0, tx, ty) {
			TileSlot::Empty => false,
			TileSlot::Solid(v) => v.0[3] == u16::MAX,
			TileSlot::Data(handle) => {
				let Ok(tile) = store.get(handle) else { return false };
				// Only the pixels inside the canvas count.
				let cols = TILE_SIZE.min(doc.width - tx * TILE_SIZE) as usize;
				let rows = TILE_SIZE.min(doc.height - ty * TILE_SIZE) as usize;
				let row_opaque = |row: usize| match tile.format() {
					PixelFormat::Rgba8 => tile.bytes()[row * TILE_SIZE as usize * 4..][..cols * 4]
						.chunks_exact(4)
						.all(|p| p[3] == u8::MAX),
					PixelFormat::Rgba16 => tile.as_u16()[row * TILE_SIZE as usize * 4..][..cols * 4]
						.chunks_exact(4)
						.all(|p| p[3] == u16::MAX),
					PixelFormat::Gray8 | PixelFormat::Gray16 => true,
				};
				(0..rows).all(row_opaque)
			}
		}
	})
}

/// Apply the Export As dialog's choices to the default options: 8-bit,
/// transparency on/off (never for JPEG, which has no alpha), JPEG quality and
/// chroma. `None` keeps the defaults.
pub fn apply_choice(mut options: ExportOptions, choice: Option<crate::ExportChoice>, opaque: bool) -> ExportOptions {
	let Some(choice) = choice else { return options };
	if choice.eight_bit {
		options.bits = 8;
	}
	let jpeg = options.format == ExportFormat::Jpeg;
	options.alpha = !jpeg && choice.transparency.unwrap_or(!opaque);
	options.quality = choice.quality.min(100);
	options.chroma = if choice.chroma_half { JpegChroma::Half } else { JpegChroma::Full };
	options
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

/// The composite of `layers` of `doc` as one pixel image of the document's
/// size (level 0), for Merge, Flatten and Stamp Visible (M4-T08). The layers
/// are composited as an isolated stack, each with its own blend mode,
/// opacity, mask and clipping, adjustment layers baked in; a selected group
/// brings its whole content, and a layer inside a group keeps its enclosing
/// groups (with only the selected children). With `background`, the result is
/// composited onto that opaque colour (Flatten fills with white).
///
/// Only tiles where a selected layer contributes are rendered (with a
/// background, every tile: it is opaque everywhere).
pub fn composite_layers(
	doc: &Document,
	layers: &[fx_core::LayerId],
	background: Option<[u16; 4]>,
	store: &TileStore,
	progress: Option<&crate::ops::ProgressFn>,
) -> Result<fx_tiles::TiledImage, fx_core::CommandError> {
	use std::sync::atomic::{AtomicUsize, Ordering};

	let wanted: std::collections::HashSet<fx_core::LayerId> = layers.iter().copied().collect();
	let mut sub = doc.clone();
	sub.layers = keep_layers(&doc.layers, &wanted);
	let format = doc.color.depth.rgba_format();
	let (cols, rows) = (doc.width.div_ceil(TILE_SIZE), doc.height.div_ceil(TILE_SIZE));
	let mut luts = LutCache::default();
	let mut programs = Vec::new();
	for ty in 0..rows {
		for tx in 0..cols {
			let program = build_program(&sub, 0, tx, ty, &mut |a| luts.get(a))
				.map_err(|_| fx_core::CommandError::NotAllowed("full-resolution tiles are missing".into()))?;
			if background.is_some() || !program.is_empty() {
				programs.push(((tx, ty), program));
			}
		}
	}
	let fetch = |h: &fx_tiles::TileHandle| store.get(h).expect("tile of a live document");
	let bg = background.map(|c| c.map(|v| f64::from(v) / 65535.0));
	let done = AtomicUsize::new(0);
	let total = programs.len().max(1);
	let tiles: Vec<((u32, u32), fx_tiles::TileBuffer)> = programs
		.par_iter()
		.map(|(pos, program)| {
			let pixels = render_tile(program, &fetch);
			let mut tile = fx_tiles::TileBuffer::zeroed(format);
			for (i, &p) in pixels.iter().enumerate() {
				let p = match bg {
					// Source-over onto the opaque background.
					Some(b) => [p[0] + b[0] * (1.0 - p[3]), p[1] + b[1] * (1.0 - p[3]), p[2] + b[2] * (1.0 - p[3]), 1.0],
					None => p,
				};
				let rgb = unpremultiply(p);
				let px = [to_u16(rgb[0]), to_u16(rgb[1]), to_u16(rgb[2]), to_u16(p[3])];
				match format {
					PixelFormat::Rgba16 => tile.as_u16_mut()[i * 4..][..4].copy_from_slice(&px),
					_ => {
						for (c, v) in px.iter().enumerate() {
							tile.bytes_mut()[i * 4 + c] = ((u32::from(*v) * 255 + 32767) / 65535) as u8;
						}
					}
				}
			}
			let n = done.fetch_add(1, Ordering::Relaxed) + 1;
			if let Some(progress) = progress
				&& n * 100 / total != (n - 1) * 100 / total
			{
				progress(n as f32 / total as f32);
			}
			(*pos, tile)
		})
		.collect();
	let mut image = fx_tiles::TiledImage::new(doc.width, doc.height, format);
	for ((tx, ty), tile) in tiles {
		image.put_buffer(store, tx, ty, tile);
	}
	Ok(image)
}

/// `layers` reduced to the wanted ones: a wanted layer with its whole content,
/// a group only if it holds wanted layers (then with only those).
fn keep_layers(layers: &[std::sync::Arc<fx_core::Layer>], wanted: &std::collections::HashSet<fx_core::LayerId>) -> Vec<std::sync::Arc<fx_core::Layer>> {
	let mut out = Vec::new();
	for layer in layers {
		if wanted.contains(&layer.id) {
			out.push(layer.clone());
		} else if let LayerKind::Group { children, expanded } = &layer.kind {
			let kept = keep_layers(children, wanted);
			if !kept.is_empty() {
				let mut group = (**layer).clone();
				group.kind = LayerKind::Group {
					children: kept,
					expanded: *expanded,
				};
				out.push(std::sync::Arc::new(group));
			}
		}
	}
	out
}

fn to_u16(v: f64) -> u16 {
	(v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use fx_core::{ColorProfile, Document, DocumentColor, Layer};
	use fx_tiles::{PixelValue, TileBuffer, TileClass, TileStoreConfig, TiledImage};

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
	fn composite_layers_matches_what_is_on_screen() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir().join("scratch-merge"))).unwrap();
		let doc = document(&store);
		let ids: Vec<_> = doc.layers.iter().map(|l| l.id).collect();
		let merged = composite_layers(&doc, &ids, None, &store, None).unwrap();
		let at = |image: &fx_tiles::TiledImage, x: u32, y: u32| -> [u16; 4] {
			let tile = match image.slot(0, x / 256, y / 256) {
				TileSlot::Solid(v) => TileBuffer::filled(PixelFormat::Rgba16, *v),
				TileSlot::Data(h) => (*store.get(h).unwrap()).clone(),
				TileSlot::Empty => TileBuffer::zeroed(PixelFormat::Rgba16),
			};
			let i = ((y % 256) * 256 + x % 256) as usize * 4;
			let s = tile.as_u16();
			[s[i], s[i + 1], s[i + 2], s[i + 3]]
		};
		// Half blue over red under the blue layer, plain red elsewhere.
		let mixed = at(&merged, 10, 10);
		assert!(
			(i32::from(mixed[0]) - 32767).abs() <= 2 && (i32::from(mixed[2]) - 32768).abs() <= 2 && mixed[3] == 65535,
			"{mixed:?}"
		);
		assert_eq!(at(&merged, 350, 20), [65535, 0, 0, 65535]);
		// Only the blue layer, onto white: half blue over white.
		let on_white = composite_layers(&doc, &ids[1..], Some([65535; 4]), &store, None).unwrap();
		let p = at(&on_white, 10, 10);
		assert!((i32::from(p[0]) - 32767).abs() <= 2 && p[2] == 65535 && p[3] == 65535, "{p:?}");
	}

	#[test]
	fn exports_the_composite() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir().join("scratch"))).unwrap();
		let doc = document(&store);
		let path = dir().join("composite.png");
		assert!(opaque_background(&doc, &store), "the red background is opaque");
		let options = options_for(&doc, &path, false).unwrap();
		assert_eq!(
			options,
			ExportOptions {
				format: ExportFormat::Png,
				bits: 16,
				alpha: true,
				ppi: 72.0,
				quality: 90,
				chroma: JpegChroma::Full
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
		assert!(options_for(&doc, Path::new("x.bmp"), true).is_err());
	}
}

#[cfg(test)]
mod opaque_tests {
	use std::sync::Arc;

	use fx_core::{ColorProfile, Document, DocumentColor, Layer};
	use fx_tiles::{PixelValue, TileBuffer, TileClass, TileStoreConfig, TiledImage};

	use super::*;

	fn doc_with(image: TiledImage) -> Document {
		let mut doc = Document::new(
			300,
			200,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let id = doc.allocate_layer_id();
		doc.layers
			.push(Arc::new(Layer::new(id, "Background", LayerKind::Pixel { image, offset: (0, 0) })));
		doc
	}

	/// A 300 × 200 Rgba8 image: opaque inside, transparent beyond the canvas
	/// in the edge tile (as an import leaves it), with one pixel of alpha `hole`.
	fn image(store: &TileStore, hole: u8) -> TiledImage {
		let mut image = TiledImage::new(300, 200, PixelFormat::Rgba8);
		image.set_slot(0, 0, TileSlot::Solid(PixelValue::rgba8(10, 20, 30, 255)));
		let mut tile = TileBuffer::zeroed(PixelFormat::Rgba8);
		let bytes = tile.bytes_mut();
		for y in 0..200 {
			for x in 0..44 {
				bytes[(y * 256 + x) * 4..][..4].copy_from_slice(&[1, 2, 3, 255]);
			}
		}
		bytes[(199 * 256 + 43) * 4 + 3] = hole;
		image.set_slot(1, 0, TileSlot::Data(store.insert(tile, TileClass::Authoritative)));
		image
	}

	#[test]
	fn opaque_only_when_every_canvas_pixel_is() {
		let dir = std::env::temp_dir().join("fx-engine-opaque-tests");
		let store = TileStore::new(TileStoreConfig::for_tests(dir)).unwrap();
		assert!(opaque_background(&doc_with(image(&store, 255)), &store));
		assert!(!opaque_background(&doc_with(image(&store, 254)), &store));

		let mut doc = doc_with(image(&store, 255));
		Arc::make_mut(&mut doc.layers[0]).opacity = 0.5;
		assert!(!opaque_background(&doc, &store), "half opacity");

		let mut doc = doc_with(image(&store, 255));
		Arc::make_mut(&mut doc.layers[0]).visible = false;
		assert!(!opaque_background(&doc, &store), "no visible layer");
	}
}
