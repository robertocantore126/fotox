//! Free Transform's live preview (M6-T04).
//!
//! While the box is up, the transformed pixels are shown on the **visible**
//! tiles only, resampled at the **view level** L (one level coarser while a
//! handle is being dragged, refined when the pointer rests), into a display
//! image that the render snapshot puts in place of the layer. The work of one
//! update is bounded by the screen, never by the layer: the resampler reads
//! the source's mip level that matches the scale, and the preparation job has
//! made every level of the source valid once, at the start.
//!
//! With a selection the lifted pixels are what moves: the snapshot shows the
//! layer with the hole they left and a floating layer above it holding the
//! transformed pixels, exactly what the command will make of them.
//!
//! The latest request wins: a job compares its number with `latest` before
//! it starts and the engine drops a result that is not the newest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fx_core::pixels::{Placed, clear, extract, place_in};
use fx_core::{Document, Filter, LayerId, LayerKind, Mapping, Selection, dest_rect};
use fx_ops::resample::SourceInfo;
use fx_render::{ViewTransform, ViewportSize};
use fx_tiles::{PixelValue, TileClass, TileError, TileSlot, TileStore, TiledImage};

use crate::ops::ImageSource;

/// The layer id the floating selection gets in the render snapshot: never a
/// real layer's (ids are allocated upwards from 1).
pub const FLOATING_LAYER: LayerId = LayerId(u64::MAX);

/// Transformed pixels on display and the canvas pixel of their (0, 0).
pub type Shown = (TiledImage, (i32, i32));

/// What the transform moves, prepared once per session on a worker.
pub struct Prepared {
	/// The layer's content, or the lifted selection, with every mip level
	/// valid.
	pub source: TiledImage,
	/// Where `source` sits on the canvas.
	pub source_at: (i32, i32),
	/// With a selection: the layer's pixels with the hole the lifted ones left
	/// (at the layer's own offset).
	pub hole: Option<TiledImage>,
}

impl Prepared {
	/// Cut out what `layer` of `doc` moves: the selected pixels when there is
	/// a selection, else the layer's content (to its non-empty tiles), and
	/// compute every mip level of it so a preview at any zoom reads the level
	/// it needs. `None` when there is nothing to move.
	pub fn new(doc: &Document, layer: LayerId, store: &TileStore) -> Result<Option<Self>, TileError> {
		let Some(LayerKind::Pixel { image, offset }) = doc.layer(layer).map(|l| &l.kind) else {
			return Ok(None);
		};
		let canvas = (doc.width, doc.height);
		let placed = Placed { image, offset: *offset };
		let (mut source, source_at, hole) = match &doc.selection {
			Some(selection) => {
				let Some((x0, y0, x1, y1)) = selection.canvas_bounds(canvas) else {
					return Ok(None);
				};
				let at = (x0 as i32, y0 as i32);
				let lifted = extract(placed, Some(selection), canvas, store)?;
				let lifted = place_in(&lifted, *offset, at, Some((x1 - x0, y1 - y0)), PixelValue::TRANSPARENT, store)?;
				(lifted, at, Some(clear(placed, selection, canvas, store)?))
			}
			None => (image.clone(), *offset, None),
		};
		for level in 1..source.level_count() {
			let grid = source.grid(level).clone();
			for ty in 0..grid.rows() {
				for tx in 0..grid.cols() {
					crate::mips::ensure_mip(&mut source, store, level, tx, ty)?;
				}
			}
		}
		Ok(Some(Self { source, source_at, hole }))
	}
}

/// The preview state of a document with a transform box up.
pub struct TransformPreview {
	pub layer: LayerId,
	/// `None` until the preparation job is done: the layer shows unchanged.
	pub prepared: Option<Arc<Prepared>>,
	/// The transformed pixels on display, and where they sit.
	pub shown: Option<Shown>,
	/// The request whose result `shown` is (or is about to be).
	pub request: u64,
}

impl TransformPreview {
	/// Put the preview into a copy of the document (the render snapshot).
	pub fn apply(&self, doc: &mut Document) {
		let (Some(prepared), Some((shown, at))) = (&self.prepared, &self.shown) else {
			return;
		};
		match &prepared.hole {
			None => {
				if let Some(layer) = doc.layer_mut(self.layer) {
					if let LayerKind::Pixel { image, offset } = &mut layer.kind {
						*image = shown.clone();
						*offset = *at;
					}
					// A linked mask is transformed by the command, not by the
					// preview: showing it where it was would cut the moving pixels.
					if let Some(mask) = &mut layer.mask
						&& mask.linked
					{
						mask.enabled = false;
					}
				}
			}
			Some(hole) => {
				let Some(original) = doc.layer(self.layer).cloned() else {
					return;
				};
				if let Some(layer) = doc.layer_mut(self.layer)
					&& let LayerKind::Pixel { image, .. } = &mut layer.kind
				{
					*image = hole.clone();
				}
				// The lifted pixels float just above their layer, with its
				// blending but without its mask.
				let mut floating = original;
				floating.id = FLOATING_LAYER;
				floating.mask = None;
				floating.kind = LayerKind::Pixel {
					image: shown.clone(),
					offset: *at,
				};
				if let Some(path) = doc.path_of(self.layer) {
					let index = path.last().copied().unwrap_or(0);
					doc.siblings_mut(&path).insert(index + 1, Arc::new(floating));
				}
			}
		}
	}
}

/// One preview update; runs on the rayon pool.
pub struct PreviewJob {
	pub request: u64,
	pub latest: Arc<AtomicU64>,
	pub prepared: Arc<Prepared>,
	pub mapping: Mapping,
	pub filter: Filter,
	/// One level coarser than the view (a drag in progress: a quarter of the
	/// work).
	pub coarser: bool,
	pub view: ViewTransform,
	pub viewport: ViewportSize,
	pub canvas: (u32, u32),
}

impl PreviewJob {
	/// The transformed pixels on the visible tiles, or `None` when a newer
	/// request superseded this one or the mapping cannot be shown.
	pub fn run(self, store: &TileStore) -> Result<Option<Shown>, TileError> {
		let current = || self.latest.load(Ordering::Relaxed) == self.request;
		if !current() {
			return Ok(None);
		}
		let source = &self.prepared.source;
		let placed = self
			.mapping
			.after_translation(f64::from(self.prepared.source_at.0), f64::from(self.prepared.source_at.1));
		let Some((at, size)) = dest_rect(&placed, [0.0, 0.0, f64::from(source.width()), f64::from(source.height())]) else {
			return Ok(None);
		};
		let local = placed.after_destination_translation(-f64::from(at.0), -f64::from(at.1));
		let mut image = TiledImage::new(size.0, size.1, source.format());
		let top = image.level_count() - 1;
		let level = (self.view.mip_level(image.level_count()) + usize::from(self.coarser)).min(top);
		let Some((_, tiles)) = crate::filters::visible_tiles(&self.view, self.viewport, self.canvas, &image, at, level) else {
			return Ok(Some((image, at)));
		};
		let info = SourceInfo {
			size: (source.width(), source.height()),
			levels: source.level_count(),
		};
		let view = ImageSource { image: source, store };
		let done = fx_ops::resample::resample(&view, info, local, self.filter, level, &tiles)?;
		if !current() {
			return Ok(None);
		}
		for ((tx, ty), buffer) in done {
			if level == 0 {
				image.put_buffer(store, tx, ty, buffer);
			} else {
				// A preview tile is derived data: evictable, never on scratch.
				let slot = match buffer.uniform_value() {
					Some(value) if value.0 == [0; 4] => TileSlot::Empty,
					Some(value) => TileSlot::Solid(value),
					None => TileSlot::Data(store.insert(buffer, TileClass::Derived)),
				};
				image.set_derived_slot(level, tx, ty, slot);
			}
		}
		Ok(Some((image, at)))
	}
}

/// The box a transform starts with: the selection's exact bounds, or the
/// layer's content bounds (`[x0, y0, x1, y1]`, canvas pixels). `None` when
/// there is nothing to transform.
pub fn start_rect(doc: &Document, layer: LayerId, store: &TileStore) -> Result<Option<[f64; 4]>, TileError> {
	use fx_core::pixels::{Content, content_bounds};
	let bounds = match &doc.selection {
		Some(Selection { image, offset }) => content_bounds(Placed { image, offset: *offset }, Content::Opaque, store)?,
		None => match doc.layer(layer).map(|l| &l.kind) {
			Some(LayerKind::Pixel { image, offset }) => content_bounds(Placed { image, offset: *offset }, Content::Opaque, store)?,
			_ => None,
		},
	};
	Ok(bounds.map(|(x0, y0, x1, y1)| [f64::from(x0), f64::from(y0), f64::from(x1), f64::from(y1)]))
}
