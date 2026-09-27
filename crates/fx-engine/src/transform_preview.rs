//! Free Transform's live preview (M6-T04).
//!
//! While the box is up, the transformed pixels are shown on the **visible**
//! tiles only, resampled at the **view level** L (one level coarser while a
//! handle is being dragged, refined when the pointer rests), into a display
//! image that the render snapshot puts in place of the layer. The work of one
//! update is bounded by the screen, never by the layer: the resampler reads
//! the source's mip level that matches the scale, computing the mips it reads
//! the first time (code review 2026-09-27 R06: the preparation job used to
//! make every level valid up front, and a mip the trim then dropped failed
//! the preview).
//!
//! A Smart Object previews what the command will make of it: its source
//! composite through the box's mapping composed with its transform. With
//! Smart Filters on, it previews its rendered pixels instead (the filters run
//! on the canvas, after the transform).
//!
//! With a selection the lifted pixels are what moves: the snapshot shows the
//! layer with the hole they left and a floating layer above it holding the
//! transformed pixels, exactly what the command will make of them.
//!
//! The latest request wins: a job compares its number with `latest` before
//! it starts and the engine drops a result that is not the newest.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fx_core::pixels::{Placed, clear, extract, place_in};
use fx_core::{Document, Filter, LayerId, LayerKind, Mapping, Selection, dest_rect};
use fx_ops::resample::SourceInfo;
use fx_render::{ViewTransform, ViewportSize};
use fx_tiles::{PixelValue, TileClass, TileError, TileSlot, TileStore, TiledImage};

use crate::mips::LazyMips;

/// The layer id the floating selection gets in the render snapshot: never a
/// real layer's (ids are allocated upwards from 1).
pub const FLOATING_LAYER: LayerId = LayerId(u64::MAX);

/// Transformed pixels on display and the canvas pixel of their (0, 0).
pub type Shown = (TiledImage, (i32, i32));

/// What the transform moves, prepared once per session on a worker.
pub struct Prepared {
	/// The layer's content, or the lifted selection; its mips are computed
	/// as previews read them and kept for the next one.
	pub source: Mutex<TiledImage>,
	/// Where `source` sits on the canvas.
	pub source_at: (i32, i32),
	/// A Smart Object's transform: `source` is its composite, placed by this
	/// mapping (the box's mapping is composed with it, as the command does).
	pub placement: Option<Mapping>,
	/// With a selection: the layer's pixels with the hole the lifted ones left
	/// (at the layer's own offset).
	pub hole: Option<TiledImage>,
}

impl Prepared {
	/// Cut out what `layer` of `doc` moves: the selected pixels when there is
	/// a selection, else the layer's content (a Smart Object: its source and
	/// transform; the selection does not apply to one, as with the command).
	/// `None` when there is nothing to move.
	pub fn new(doc: &Document, layer: LayerId, store: &TileStore) -> Result<Option<Self>, TileError> {
		if let Some(LayerKind::Smart { smart, .. }) = doc.layer(layer).map(|l| &l.kind) {
			let filtered = smart.filters_enabled && smart.filters.iter().any(|f| f.enabled);
			let (source, placement) = if filtered {
				// The filters run after the transform: preview the rendered
				// pixels (drawn here, on the worker).
				let content = crate::derived::layer_content(doc, store, layer).map_err(|e| TileError::Io(std::io::Error::other(e.to_string())))?;
				(content, None)
			} else {
				(smart.source.composite.clone(), Some(smart.transform))
			};
			return Ok(Some(Self {
				source: Mutex::new(source),
				source_at: (0, 0),
				placement,
				hole: None,
			}));
		}
		let (image, offset) = match doc.layer(layer).map(|l| &l.kind) {
			Some(LayerKind::Pixel { image, offset }) => (image, offset),
			_ => return Ok(None),
		};
		let canvas = (doc.width, doc.height);
		let placed = Placed { image, offset: *offset };
		let (source, source_at, hole) = match &doc.selection {
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
		Ok(Some(Self {
			source: Mutex::new(source),
			source_at,
			placement: None,
			hole,
		}))
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
					match &mut layer.kind {
						LayerKind::Pixel { image, offset } => {
							*image = shown.clone();
							*offset = *at;
						}
						// A Smart Object shows the transformed pixels in place
						// of its cache, with its own blending (it used to stay
						// still until the transform was committed).
						LayerKind::Smart { .. } => {
							layer.kind = LayerKind::Pixel {
								image: shown.clone(),
								offset: *at,
							};
						}
						_ => {}
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
		let (width, height, levels, format) = {
			let source = self.prepared.source.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
			(source.width(), source.height(), source.level_count(), source.format())
		};
		let placed = match self.prepared.placement {
			Some(inner) => {
				let Some(placed) = fx_core::command::m12::compose(self.mapping, inner) else {
					return Ok(None);
				};
				placed
			}
			None => self
				.mapping
				.after_translation(f64::from(self.prepared.source_at.0), f64::from(self.prepared.source_at.1)),
		};
		let Some((at, size)) = dest_rect(&placed, [0.0, 0.0, f64::from(width), f64::from(height)]) else {
			return Ok(None);
		};
		let local = placed.after_destination_translation(-f64::from(at.0), -f64::from(at.1));
		let mut image = TiledImage::new(size.0, size.1, format);
		// The frame reads the preview at the canvas's level, which a small
		// preview's own pyramid may not reach.
		image.ensure_levels(fx_tiles::level_count_for(self.canvas.0, self.canvas.1));
		let top = image.level_count() - 1;
		let level = (self.view.mip_level(image.level_count()) + usize::from(self.coarser)).min(top);
		let Some((_, tiles)) = crate::filters::visible_tiles(&self.view, self.viewport, self.canvas, &image, at, level) else {
			return Ok(Some((image, at)));
		};
		let info = SourceInfo { size: (width, height), levels };
		let view = LazyMips::new(&self.prepared.source, store);
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
			// A Smart Object's box is its source through its transform (M12-T01).
			Some(LayerKind::Smart { smart, .. }) => smart.bounds().map(|((x, y), (w, h))| (x, y, x + w as i32, y + h as i32)),
			_ => None,
		},
	};
	Ok(bounds.map(|(x0, y0, x1, y1)| [f64::from(x0), f64::from(y0), f64::from(x1), f64::from(y1)]))
}
