//! Brush strokes in the engine (M5-T07): what a stroke paints on, the clone
//! source, and the live stroke session.
//!
//! A live stroke and its replay (`Command::Stroke` through
//! [`EngineOps`](crate::ops::EngineOps)) set up the brush engine with the same
//! [`prepare`], so the recorded command reproduces the live pixels.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fx_core::stroke::{BrushParams, StrokeSample, StrokeTarget, StrokeTool};
use fx_core::{CommandError, Document, LayerId, LayerKind, Selection};
use fx_ops::brush::{LayerSource, SourceTiles, Stroke, StrokeSetup};
use fx_render::adjust::LutCache;
use fx_tiles::{TILE_PIXELS, TileBuffer, TileError, TileStore, TiledImage};

/// What a stroke on a layer needs from the document at its start.
pub struct Prepared {
	pub image: TiledImage,
	pub offset: (i32, i32),
	pub selection: Option<Selection>,
	pub lock_alpha: bool,
	pub source: Option<Arc<dyn SourceTiles>>,
}

/// Set up a stroke on `layer` of `doc` (its pixels or its mask), with the
/// clone source the tool needs: the layer as it is, or the composite of the
/// document as it is (D-044: the source never sees the stroke itself).
pub fn prepare(doc: &Document, layer: LayerId, target: StrokeTarget, tool: &StrokeTool, store: &TileStore) -> Result<Prepared, CommandError> {
	let found = doc.layer(layer).ok_or(CommandError::NotAllowed("the layer is gone".into()))?;
	let (image, offset) = match (target, &found.kind) {
		(StrokeTarget::Pixels, LayerKind::Pixel { image, offset }) => (image.clone(), *offset),
		(StrokeTarget::Pixels, _) => return Err(CommandError::NotAllowed("the layer has no pixels; rasterise it first".into())),
		(StrokeTarget::Mask, kind) => {
			let mask = found.mask.as_ref().ok_or(CommandError::NotAllowed("the layer has no mask".into()))?;
			// A linked mask sits at the layer's offset, an unlinked one at the origin.
			let offset = match kind {
				LayerKind::Pixel { offset, .. } if mask.linked => *offset,
				_ => (0, 0),
			};
			(mask.image.clone(), offset)
		}
	};
	let source: Option<Arc<dyn SourceTiles>> = match tool {
		StrokeTool::Clone { sample_all: true, .. } | StrokeTool::Heal { sample_all: true, .. } => {
			Some(Arc::new(CompositeTiles::new(doc.clone(), store.clone())))
		}
		StrokeTool::Clone { .. } | StrokeTool::Heal { .. } | StrokeTool::SpotHeal => Some(Arc::new(LayerSource {
			image: image.clone(),
			offset,
			store: store.clone(),
		})),
		_ => None,
	};
	Ok(Prepared {
		image,
		offset,
		selection: doc.selection.clone(),
		lock_alpha: target == StrokeTarget::Pixels && found.locked_transparency,
		source,
	})
}

/// The brush engine's setup for prepared parts.
pub fn setup<'a>(prepared: &'a Prepared, doc: &Document, tool: StrokeTool, brush: BrushParams, color: [u16; 4]) -> StrokeSetup<'a> {
	StrokeSetup {
		image: &prepared.image,
		offset: prepared.offset,
		canvas: (doc.width, doc.height),
		selection: prepared.selection.as_ref(),
		tool,
		brush,
		color,
		lock_alpha: prepared.lock_alpha,
		source: prepared.source.clone(),
	}
}

/// The composite of a document as a clone source: canvas tiles through the
/// CPU reference compositor, on demand, cached.
pub struct CompositeTiles {
	doc: Document,
	store: TileStore,
	luts: Mutex<LutCache>,
	cache: Mutex<HashMap<(u32, u32), fx_ops::brush::stroke::SourceTile>>,
}

impl CompositeTiles {
	/// A source reading `doc` (a snapshot taken when the stroke starts).
	pub fn new(doc: Document, store: TileStore) -> Self {
		Self {
			doc,
			store,
			luts: Mutex::new(LutCache::default()),
			cache: Mutex::new(HashMap::new()),
		}
	}
}

impl SourceTiles for CompositeTiles {
	fn tile(&self, tx: u32, ty: u32) -> Result<fx_ops::brush::stroke::SourceTile, TileError> {
		if let Some(t) = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(tx, ty)) {
			return Ok(t.clone());
		}
		let program = {
			let mut luts = self.luts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
			fx_render::build_program(&self.doc, 0, tx, ty, &mut |a| luts.get(a)).map_err(|_| TileError::Evicted)?
		};
		let pixels: Vec<[f32; 4]> = if program.is_empty() {
			vec![[0.0; 4]; TILE_PIXELS]
		} else {
			let fetch = |h: &fx_tiles::TileHandle| -> Arc<TileBuffer> { self.store.get(h).expect("tile of a live document") };
			fx_render::reference::render_tile(&program, &fetch)
				.iter()
				.map(|p| p.map(|v| v as f32))
				.collect()
		};
		let t = Arc::new(pixels);
		self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert((tx, ty), t.clone());
		Ok(t)
	}
}

/// A stroke being painted live (M5-T07).
pub struct Session {
	pub doc: fx_protocol::DocId,
	pub layer: LayerId,
	pub target: StrokeTarget,
	pub tool: StrokeTool,
	pub brush: BrushParams,
	pub color: [u16; 4],
	/// The document before the stroke: the History step's `before`.
	pub before: Document,
	pub stroke: Stroke,
	/// When the earliest input not yet on screen arrived (latency, M5-T11).
	pub pending_input: Option<std::time::Instant>,
}

impl Session {
	/// The samples painted so far (the recorded command's).
	pub fn samples(&self) -> Vec<StrokeSample> {
		self.stroke.samples().to_vec()
	}
}

#[cfg(test)]
mod tests {
	use fx_core::stroke::StrokeSample;
	use fx_core::{BitDepth, ColorProfile, Command, CommandContext, DocumentColor, LayerRef, PixelOps};
	use fx_tiles::{PixelFormat, PixelValue, TileSlot, TileStoreConfig};

	use super::*;
	use crate::ops::EngineOps;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-engine-stroke-tests");
		std::fs::create_dir_all(&dir).unwrap();
		TileStore::new(TileStoreConfig::for_tests(dir)).unwrap()
	}

	/// A 600 × 400 16-bit document with one white pixel layer moved by
	/// (37, -20): the stroke has to grow it to the canvas.
	fn document(store: &TileStore) -> (Document, LayerId) {
		let mut doc = Document::new(
			600,
			400,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let id = doc.allocate_layer_id();
		let mut image = TiledImage::new(400, 300, PixelFormat::Rgba16);
		for ty in 0..2 {
			for tx in 0..2 {
				image.set_slot(tx, ty, TileSlot::Solid(PixelValue::rgba16(65535, 65535, 65535, 65535)));
			}
		}
		let _ = store;
		doc.layers
			.push(Arc::new(fx_core::Layer::new(id, "Paint", LayerKind::Pixel { image, offset: (37, -20) })));
		doc.selected = vec![id];
		(doc, id)
	}

	fn samples() -> Vec<StrokeSample> {
		(0..60)
			.map(|i| StrokeSample {
				x: 30.0 + f64::from(i) * 9.0,
				y: 200.0 + (f64::from(i) * 0.3).sin() * 150.0,
				pressure: 0.4 + 0.6 * (f64::from(i) / 60.0) as f32,
				tilt_x: 0.0,
				tilt_y: 0.0,
				time_us: i as u64 * 8000,
			})
			.collect()
	}

	/// The spec test of M5-T07: the recorded command replayed on the document
	/// before the stroke gives the live result, tile by tile.
	#[test]
	fn the_recorded_stroke_replays_the_live_pixels() {
		let store = store();
		let (doc, layer) = document(&store);
		let brush = BrushParams {
			diameter: 45.0,
			hardness: 0.3,
			flow: 0.7,
			opacity: 0.9,
			mode: fx_core::BlendMode::Multiply,
			pressure_size: true,
			..Default::default()
		};
		let color = [65535, 20000, 0, 65535];
		for tool in [
			StrokeTool::Brush,
			StrokeTool::Eraser,
			StrokeTool::Clone {
				dx: 120.0,
				dy: 30.0,
				sample_all: true,
			},
		] {
			// Live: begin, then the samples in uneven batches.
			let prepared = prepare(&doc, layer, StrokeTarget::Pixels, &tool, &store).unwrap();
			let mut live = Stroke::begin(setup(&prepared, &doc, tool, brush, color), &store).unwrap();
			for chunk in samples().chunks(9) {
				live.add(chunk).unwrap();
			}
			let (live_image, live_offset) = live.finish().unwrap();
			// Replay through the command.
			let mut replayed = doc.clone();
			let ops = EngineOps::default();
			let mut ctx = CommandContext {
				tiles: &store,
				ops: Some(&ops as &dyn PixelOps),
			};
			Command::Stroke {
				layer: LayerRef::Id(layer),
				target: StrokeTarget::Pixels,
				tool,
				brush,
				color,
				samples: samples(),
			}
			.apply(&mut replayed, &mut ctx)
			.unwrap();
			let LayerKind::Pixel { image, offset } = &replayed.layer(layer).unwrap().kind else {
				panic!()
			};
			assert_eq!(*offset, live_offset, "{tool:?}");
			assert_eq!((image.width(), image.height()), (live_image.width(), live_image.height()));
			for ty in 0..image.grid(0).rows() {
				for tx in 0..image.grid(0).cols() {
					let (a, b) = (image.slot(0, tx, ty), live_image.slot(0, tx, ty));
					match (a, b) {
						(TileSlot::Data(x), TileSlot::Data(y)) => {
							assert!(store.get(x).unwrap().bytes() == store.get(y).unwrap().bytes(), "{tool:?} tile ({tx}, {ty})")
						}
						_ => assert!(a.same_as(b), "{tool:?} tile ({tx}, {ty}): {a:?} vs {b:?}"),
					}
				}
			}
		}
	}
}
