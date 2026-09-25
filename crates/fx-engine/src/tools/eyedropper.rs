//! The Eyedropper tool (`I`, or Alt with any painting tool): samples the
//! document at the clicked point and tells the UI (M5-T01).
//!
//! Sampling composites **one tile** with the CPU reference compositor, so the
//! cost does not depend on the document size; it does not read whole
//! documents.

use std::collections::HashMap;

use fx_core::{CommandError, Document, LayerId};
use fx_render::adjust::LutCache;
use fx_render::blend::{Premul, unpremultiply};
use fx_render::build_program;
use fx_render::reference::render_tile;
use fx_tiles::{TILE_SIZE, TileBuffer, TileHandle, TileStore};

use crate::export::{keep_layers, to_u16};
use crate::tools::{ColorTarget, DocPointer, Tool, ToolContext, ToolResult};
use crate::{CursorShape, Modifiers, PointerKind};

/// The options the eyedropper reads from its option bar (M5-T01/T09).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Options {
	/// 1, 3 or 5: average over an `area × area` square centred on the point.
	area: u32,
	/// Sample the active layer alone instead of the composite of all layers.
	current_layer: bool,
}

impl Options {
	fn from(ctx: &ToolContext<'_>) -> Self {
		let area = match ctx.settings.string("eyedropper", "Sample Size").as_deref() {
			Some("3 by 3 Average") => 3,
			Some("5 by 5 Average") => 5,
			_ => 1,
		};
		let current_layer = ctx.settings.string("eyedropper", "Sample").as_deref() == Some("Current Layer");
		Self { area, current_layer }
	}
}

/// The eyedropper (`I`): click samples the composite (Alt: into the
/// background swatch).
#[derive(Default)]
pub struct Eyedropper;

impl Tool for Eyedropper {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		if event.kind != PointerKind::Down {
			return ToolResult::default();
		}
		let options = Options::from(ctx);
		// Photoshop: Alt picks into the background swatch.
		let target = if event.modifiers.alt {
			ColorTarget::Background
		} else {
			ColorTarget::Foreground
		};
		let layer = if options.current_layer {
			match ctx.doc.active_layer() {
				Some(id) => Some(id),
				None => {
					return ToolResult {
						info: Some("No layer to sample".into()),
						..Default::default()
					};
				}
			}
		} else {
			None
		};
		match sample_pixel(ctx.doc, event.x, event.y, options.area, layer, ctx.store) {
			Ok(rgba) => ToolResult {
				picked: Some((rgba, target)),
				..Default::default()
			},
			Err(error) => ToolResult {
				info: Some(error.to_string()),
				..Default::default()
			},
		}
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Crosshair
	}
}

/// The composite colour at document pixel `(x, y)` (M5-T01).
///
/// Only the tile(s) under the sample square are composited: one
/// [`build_program`] and one [`render_tile`] per tile, so a click on a
/// 30 000² document costs the same as on a small one.
///
/// * `area` — 1, 3 or 5 (odd): the average over an `area × area` square
///   centred on the point, clamped to the canvas (Photoshop's Point Sample /
///   3 by 3 / 5 by 5).
/// * `layer` — `Some(id)` samples that layer alone, `None` the composite of
///   every visible layer.
pub fn sample_pixel(doc: &Document, x: f64, y: f64, area: u32, layer: Option<LayerId>, store: &TileStore) -> Result<[u16; 4], CommandError> {
	if doc.width == 0 || doc.height == 0 {
		return Err(CommandError::NotAllowed("the document is empty".into()));
	}
	if !x.is_finite() || !y.is_finite() {
		return Err(CommandError::NotAllowed("the sample point is not finite".into()));
	}
	let (cx, cy) = (x.floor() as i64, y.floor() as i64);
	if cx < 0 || cy < 0 || cx >= i64::from(doc.width) || cy >= i64::from(doc.height) {
		return Err(CommandError::NotAllowed("the sample point is outside the canvas".into()));
	}
	let half = i64::from(area.max(1) / 2);
	let x0 = (cx - half).max(0);
	let y0 = (cy - half).max(0);
	let x1 = (cx + half).min(i64::from(doc.width) - 1);
	let y1 = (cy + half).min(i64::from(doc.height) - 1);

	// Current Layer: only that layer contributes. All Layers: the document.
	let subject = match layer {
		Some(id) => {
			let mut sub = doc.clone();
			sub.layers = keep_layers(&doc.layers, &std::iter::once(id).collect());
			sub
		}
		None => doc.clone(),
	};

	let mut luts = LutCache::default();
	let fetch = |h: &TileHandle| -> std::sync::Arc<TileBuffer> { store.get(h).expect("tile of a live document") };
	let (tx0, ty0) = (x0 as u32 / TILE_SIZE, y0 as u32 / TILE_SIZE);
	let (tx1, ty1) = (x1 as u32 / TILE_SIZE, y1 as u32 / TILE_SIZE);
	let mut tiles: HashMap<(u32, u32), Vec<Premul>> = HashMap::new();
	for ty in ty0..=ty1 {
		for tx in tx0..=tx1 {
			let program =
				build_program(&subject, 0, tx, ty, &mut |a| luts.get(a)).map_err(|_| CommandError::NotAllowed("full-resolution tiles are missing".into()))?;
			tiles.insert((tx, ty), render_tile(&program, &fetch));
		}
	}

	let mut sum = [0.0f64; 4];
	let mut count = 0u32;
	for py in y0..=y1 {
		for px in x0..=x1 {
			let tile = &tiles[&(px as u32 / TILE_SIZE, py as u32 / TILE_SIZE)];
			let at = ((py as u32 % TILE_SIZE) * TILE_SIZE + (px as u32 % TILE_SIZE)) as usize;
			for (channel, value) in sum.iter_mut().enumerate() {
				*value += tile[at][channel];
			}
			count += 1;
		}
	}
	let inv = 1.0 / f64::from(count.max(1));
	let average = [sum[0] * inv, sum[1] * inv, sum[2] * inv, sum[3] * inv];
	let rgb = unpremultiply(average);
	Ok([to_u16(rgb[0]), to_u16(rgb[1]), to_u16(rgb[2]), to_u16(average[3])])
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use fx_core::{BitDepth, ColorProfile, DocumentColor, Layer, LayerKind};
	use fx_tiles::{PixelFormat, PixelValue, TileClass, TileSlot, TileStoreConfig, TiledImage};

	use super::*;

	const RED: [u16; 4] = [65535, 0, 0, 65535];
	const HALF_BLUE: [u16; 4] = [0, 0, 65535, 32768];

	fn dir() -> std::path::PathBuf {
		let dir = std::env::temp_dir().join("fx-engine-eyedropper-tests");
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	/// A tall solid tile column: `fill(tx)` for every tile row.
	fn solid_column(w: u32, h: u32, fill: impl Fn(u32) -> [u16; 4]) -> TiledImage {
		let mut image = TiledImage::new(w, h, PixelFormat::Rgba16);
		for ty in 0..h.div_ceil(TILE_SIZE) {
			for tx in 0..w.div_ceil(TILE_SIZE) {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue(fill(tx))));
			}
		}
		image
	}

	/// Red bottom layer plus a half-transparent blue layer over the left
	/// 300 px (two tiles: 256 + 44).
	fn document(store: &TileStore) -> Document {
		let (w, h) = (400u32, 300u32);
		let mut doc = Document::new(
			w,
			h,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let bottom = solid_column(w, h, |_| RED);
		let mut top = TiledImage::new(w, h, PixelFormat::Rgba16);
		for ty in 0..h.div_ceil(TILE_SIZE) {
			top.set_slot(0, ty, TileSlot::Solid(PixelValue(HALF_BLUE)));
			let mut tile = TileBuffer::zeroed(PixelFormat::Rgba16);
			let px = tile.as_u16_mut();
			for y in 0..TILE_SIZE {
				for x in 0..44 {
					px[((y * TILE_SIZE + x) * 4) as usize..][..4].copy_from_slice(&HALF_BLUE);
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

	fn close(a: [u16; 4], b: [u16; 4]) -> bool {
		a.iter().zip(&b).all(|(x, y)| i32::from(*x).abs_diff(i32::from(*y)) <= 2)
	}

	#[test]
	fn a_click_returns_the_composite_of_the_layers() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir())).unwrap();
		let doc = document(&store);
		// Half blue over red where the top layer covers, plain red right of it.
		assert!(close(sample_pixel(&doc, 10.0, 10.0, 1, None, &store).unwrap(), [32767, 0, 32768, 65535]));
		assert_eq!(sample_pixel(&doc, 350.0, 20.0, 1, None, &store).unwrap(), RED);
		// The layer edge at x = 300: last blue pixel, first red pixel.
		assert!(close(sample_pixel(&doc, 299.0, 10.0, 1, None, &store).unwrap(), [32767, 0, 32768, 65535]));
		assert_eq!(sample_pixel(&doc, 300.0, 10.0, 1, None, &store).unwrap(), RED);
	}

	#[test]
	fn current_layer_keeps_the_layers_own_alpha() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir())).unwrap();
		let doc = document(&store);
		let blue = doc.layers[1].id;
		assert!(close(sample_pixel(&doc, 10.0, 10.0, 1, Some(blue), &store).unwrap(), HALF_BLUE));
		// Outside the blue layer's own pixels: transparent.
		assert_eq!(sample_pixel(&doc, 350.0, 20.0, 1, Some(blue), &store).unwrap(), [0, 0, 0, 0]);
	}

	#[test]
	fn a_square_sample_averages_neighbouring_pixels() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir())).unwrap();
		let doc = document(&store);
		// 3 by 3 centred on (299, 128): columns 298 and 299 are blue, 300 is red.
		let average = sample_pixel(&doc, 299.0, 128.0, 3, None, &store).unwrap();
		// r = (6 * 32767 + 3 * 65535) / 9, b = (6 * 32768) / 9.
		assert!(close(average, [43690, 0, 21845, 65535]), "{average:?}");
		// Centred on the tile boundary at x = 256 the square reads both tiles.
		let seam = sample_pixel(&doc, 255.0, 128.0, 3, None, &store).unwrap();
		assert!(close(seam, [32767, 0, 32768, 65535]), "{seam:?}");
	}

	#[test]
	fn a_point_outside_the_canvas_is_refused() {
		let store = TileStore::new(TileStoreConfig::for_tests(dir())).unwrap();
		let doc = document(&store);
		assert!(sample_pixel(&doc, -1.0, 10.0, 1, None, &store).is_err());
		assert!(sample_pixel(&doc, 10.0, 300.0, 1, None, &store).is_err());
		assert!(sample_pixel(&doc, 399.0, 299.0, 1, None, &store).is_ok());
	}
}
