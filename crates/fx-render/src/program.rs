//! Tile programs: *what* to composite for one output tile.
//!
//! A [`TileProgram`] is built from an immutable document snapshot for one
//! tile `(level, tx, ty)`. It is a flat list of [`Op`]s executed per pixel on
//! a small stack of premultiplied accumulators (groups push/pop). The same
//! program is executed by the CPU reference ([`crate::reference`]) and by the
//! GPU compositor, which is how the GPU is tested.
//!
//! The builder drops everything that cannot contribute to this tile: hidden
//! layers, layers whose tiles here are all empty, fully hidden masks, empty
//! groups. A sparse layer therefore costs nothing where it has no pixels.
//!
//! The program's [`TileProgram::key`] hashes the *identity* of every input
//! (tile ids, solid values, parameters). Tiles are immutable, so equal keys
//! mean equal results: this is the composite-cache key, no manual
//! invalidation anywhere.
//!
//! Written by Claude (M1-T07/M2-T03 core). Extend, don't restructure.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use fx_core::layer::Adjustment;
use fx_core::{BlendMode, Document, Layer, LayerId, LayerKind, Mask};
use fx_tiles::{TILE_SIZE, TileSlot, TiledImage};

use crate::adjust::Lut;

/// One tile position of a layer (or mask) grid, as seen by a program.
#[derive(Clone, Debug)]
pub enum QuadSlot {
	/// Outside the image grid: transparent for pixels, `outside` for masks.
	Outside,
	Slot(TileSlot),
}

/// The up-to-4 source tiles one output tile reads from, when the source is
/// shifted by a non-multiple of the tile size.
///
/// Output pixel `p` (0..256 per axis) reads source pixel `p + shift` of the
/// 512×512 area made of `slots` = `[top_left, top_right, bottom_left, bottom_right]`.
/// Slots that cannot be read (shift 0 on that axis) are `Outside`.
#[derive(Clone, Debug)]
pub struct Quad {
	pub shift: (u32, u32),
	pub slots: [QuadSlot; 4],
}

impl Quad {
	/// Which of the 4 slots a pixel reads, and where inside it.
	pub fn locate(&self, px: u32, py: u32) -> (usize, u32, u32) {
		let x = px + self.shift.0;
		let y = py + self.shift.1;
		let index = (y / TILE_SIZE) as usize * 2 + (x / TILE_SIZE) as usize;
		(index, x % TILE_SIZE, y % TILE_SIZE)
	}

	fn all_empty(&self) -> bool {
		self.slots.iter().all(|s| matches!(s, QuadSlot::Outside | QuadSlot::Slot(TileSlot::Empty)))
	}
}

#[derive(Clone, Debug)]
pub enum Source {
	/// Straight RGBA, 0..1 (solid fill layers).
	Solid([f32; 4]),
	Tiles(Quad),
}

#[derive(Clone, Debug)]
pub struct MaskRef {
	pub quad: Quad,
	/// Mask value (0..1) outside the mask image.
	pub outside: f32,
}

#[derive(Clone, Debug)]
pub enum AdjustKind {
	/// Per-channel curve (Invert, Levels, Curves, Exposure, Brightness/Contrast).
	Lut(Arc<Lut>),
	/// Per-pixel HSL adjustment (M2-T04).
	HueSaturation {
		hue: f32,
		saturation: f32,
		lightness: f32,
		colorize: bool,
	},
}

#[derive(Clone, Debug)]
pub enum Op {
	/// Composite a layer's content. `alpha` = opacity × fill (× constant mask).
	Layer {
		layer: LayerId,
		source: Source,
		blend: BlendMode,
		alpha: f32,
		mask: Option<MaskRef>,
		/// Source-atop (clipped layer): keeps the backdrop alpha.
		clip: bool,
	},
	/// Adjustment layer: `Cs = f(Cb)`, always composited source-atop.
	Adjust {
		layer: LayerId,
		adjust: AdjustKind,
		blend: BlendMode,
		alpha: f32,
		mask: Option<MaskRef>,
	},
	/// Push a transparent accumulator (isolated group / clipping group).
	BeginIsolated,
	/// Push a copy of the current accumulator (pass-through group with opacity/mask).
	BeginPassThrough,
	/// Pop the group result and composite it onto the new top.
	EndIsolated {
		blend: BlendMode,
		alpha: f32,
		mask: Option<MaskRef>,
		clip: bool,
	},
	/// Pop the result `R`; new top = lerp(top, R, alpha × mask).
	EndPassThrough { alpha: f32, mask: Option<MaskRef> },
}

impl Op {
	/// Every tile quad this op reads (source and mask).
	pub fn quads(&self) -> Vec<&Quad> {
		fn mask_quad(mask: &Option<MaskRef>) -> Option<&Quad> {
			mask.as_ref().map(|m| &m.quad)
		}
		let mut quads = Vec::new();
		match self {
			Op::Layer { source, mask, .. } => {
				if let Source::Tiles(q) = source {
					quads.push(q);
				}
				quads.extend(mask_quad(mask));
			}
			Op::Adjust { mask, .. } | Op::EndIsolated { mask, .. } | Op::EndPassThrough { mask, .. } => quads.extend(mask_quad(mask)),
			Op::BeginIsolated | Op::BeginPassThrough => {}
		}
		quads
	}
}

#[derive(Clone, Debug)]
pub struct TileProgram {
	pub level: usize,
	pub tx: u32,
	pub ty: u32,
	pub ops: Vec<Op>,
	/// Hash of every input: equal keys ⇒ identical output.
	pub key: u64,
}

impl TileProgram {
	/// Nothing to draw: the tile is fully transparent.
	pub fn is_empty(&self) -> bool {
		self.ops.is_empty()
	}
}

/// A mip tile that must be computed before this program can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MipRequest {
	pub layer: LayerId,
	pub mask: bool,
	pub level: usize,
	pub x: u32,
	pub y: u32,
}

/// Build the program for output tile `(level, tx, ty)` of `doc`.
///
/// `luts` resolves adjustment parameters to baked LUTs (cached by the caller,
/// see [`crate::adjust::LutCache`]).
///
/// Errors with the list of dirty mip tiles if any input is not computed yet.
pub fn build_program(doc: &Document, level: usize, tx: u32, ty: u32, luts: &mut dyn FnMut(&Adjustment) -> Arc<Lut>) -> Result<TileProgram, Vec<MipRequest>> {
	let mut builder = Builder {
		level,
		origin: (tx as i64 * TILE_SIZE as i64, ty as i64 * TILE_SIZE as i64),
		missing: Vec::new(),
		luts,
	};
	let ops = builder.list(&doc.layers);
	if !builder.missing.is_empty() {
		return Err(builder.missing);
	}
	let mut hasher = std::collections::hash_map::DefaultHasher::new();
	(level, tx, ty).hash(&mut hasher);
	for op in &ops {
		hash_op(op, &mut hasher);
	}
	Ok(TileProgram {
		level,
		tx,
		ty,
		ops,
		key: hasher.finish(),
	})
}

struct Builder<'a> {
	level: usize,
	/// Output tile origin in level-space pixels.
	origin: (i64, i64),
	missing: Vec<MipRequest>,
	luts: &'a mut dyn FnMut(&Adjustment) -> Arc<Lut>,
}

enum MaskEval {
	/// Mask is 0 everywhere in this tile: the layer is invisible here.
	Hidden,
	/// Mask is this constant here: folded into alpha.
	Constant(f32),
	Varying(MaskRef),
}

impl Builder<'_> {
	/// Ops for a sibling list (bottom → top), resolving clipping groups.
	fn list(&mut self, layers: &[Arc<Layer>]) -> Vec<Op> {
		let mut ops = Vec::new();
		let mut i = 0;
		while i < layers.len() {
			let base = &layers[i];
			let mut j = i + 1;
			while j < layers.len() && layers[j].clipped {
				j += 1;
			}
			let clipped = &layers[i + 1..j];
			let can_be_base = !matches!(base.kind, LayerKind::Adjustment(_));
			if clipped.is_empty() || !can_be_base {
				ops.extend(self.layer(base, false));
				for layer in clipped {
					// No valid base: composite normally (docs/BLEND_MODES.md §6).
					ops.extend(self.layer(layer, false));
				}
			} else {
				ops.extend(self.clipping_group(base, clipped));
			}
			i = j;
		}
		ops
	}

	/// Base + clipped layers: isolated; base drawn Normal with its fill and
	/// mask; clipped layers source-atop; the result blended with the base's
	/// mode and opacity (Photoshop: "clipped layers take the base's opacity
	/// and mode").
	fn clipping_group(&mut self, base: &Layer, clipped: &[Arc<Layer>]) -> Vec<Op> {
		if !base.visible || base.opacity <= 0.0 {
			return Vec::new();
		}
		let base_ops = match &base.kind {
			LayerKind::Group { children, .. } => {
				let inner = self.list(children);
				if inner.is_empty() {
					return Vec::new();
				}
				match self.mask(base) {
					MaskEval::Hidden => return Vec::new(),
					MaskEval::Constant(m) => wrap_isolated(inner, BlendMode::Normal, m, None, false),
					MaskEval::Varying(mask) => wrap_isolated(inner, BlendMode::Normal, 1.0, Some(mask), false),
				}
			}
			_ => self.content(base, BlendMode::Normal, base.fill, false),
		};
		if base_ops.is_empty() {
			return Vec::new(); // base transparent here → the whole clipping group is invisible
		}
		let mut inner = base_ops;
		for layer in clipped {
			inner.extend(self.layer(layer, true));
		}
		wrap_isolated(inner, normal_if_pass(base.blend), base.opacity, None, false)
	}

	/// Ops for one layer (and its subtree).
	fn layer(&mut self, layer: &Layer, clip: bool) -> Vec<Op> {
		if !layer.visible || layer.opacity <= 0.0 {
			return Vec::new();
		}
		match &layer.kind {
			LayerKind::Group { children, .. } => {
				let inner = self.list(children);
				if inner.is_empty() {
					return Vec::new();
				}
				let (alpha, mask) = match self.mask(layer) {
					MaskEval::Hidden => return Vec::new(),
					MaskEval::Constant(m) => (layer.opacity * m, None),
					MaskEval::Varying(mask) => (layer.opacity, Some(mask)),
				};
				if layer.blend == BlendMode::PassThrough && !clip {
					if alpha >= 1.0 && mask.is_none() {
						return inner; // exactly equivalent, cheaper
					}
					let mut ops = Vec::with_capacity(inner.len() + 2);
					ops.push(Op::BeginPassThrough);
					ops.extend(inner);
					ops.push(Op::EndPassThrough { alpha, mask });
					return ops;
				}
				wrap_isolated(inner, normal_if_pass(layer.blend), alpha, mask, clip)
			}
			_ => self.content(layer, layer.blend, layer.opacity * layer.fill, clip),
		}
	}

	/// A non-group layer's own op, with its mask.
	fn content(&mut self, layer: &Layer, blend: BlendMode, alpha: f32, clip: bool) -> Vec<Op> {
		if alpha <= 0.0 {
			return Vec::new();
		}
		let (alpha, mask) = match self.mask(layer) {
			MaskEval::Hidden => return Vec::new(),
			MaskEval::Constant(m) if m <= 0.0 => return Vec::new(),
			MaskEval::Constant(m) => (alpha * m, None),
			MaskEval::Varying(mask) => (alpha, Some(mask)),
		};
		let op = match &layer.kind {
			LayerKind::Pixel { image, offset } => {
				let Some(quad) = self.quad(image, *offset, layer.id, false) else {
					return Vec::new();
				};
				if quad.all_empty() {
					return Vec::new();
				}
				Op::Layer {
					layer: layer.id,
					source: Source::Tiles(quad),
					blend,
					alpha,
					mask,
					clip,
				}
			}
			LayerKind::SolidFill { rgba } => Op::Layer {
				layer: layer.id,
				source: Source::Solid(rgba.map(|v| v as f32 / 65535.0)),
				blend,
				alpha,
				mask,
				clip,
			},
			LayerKind::Adjustment(adjustment) => {
				let adjust = match adjustment {
					Adjustment::HueSaturation {
						hue,
						saturation,
						lightness,
						colorize,
					} => AdjustKind::HueSaturation {
						hue: *hue,
						saturation: *saturation,
						lightness: *lightness,
						colorize: *colorize,
					},
					other => AdjustKind::Lut((self.luts)(other)),
				};
				Op::Adjust {
					layer: layer.id,
					adjust,
					blend,
					alpha,
					mask,
				}
			}
			LayerKind::Group { .. } => unreachable!("groups are handled by `layer`"),
		};
		vec![op]
	}

	fn mask(&mut self, layer: &Layer) -> MaskEval {
		let Some(mask @ Mask { enabled: true, .. }) = &layer.mask else {
			return MaskEval::Constant(1.0);
		};
		let offset = match (&layer.kind, mask.linked) {
			(LayerKind::Pixel { offset, .. }, true) => *offset,
			_ => (0, 0),
		};
		let outside = mask.outside_value as f32 / 65535.0;
		let Some(quad) = self.quad(&mask.image, offset, layer.id, true) else {
			return MaskEval::Constant(1.0);
		};
		// Constant if every slot this tile reads is uniform with the same value.
		let mut constant: Option<f32> = None;
		let mut varying = false;
		for slot in &quad.slots {
			let value = match slot {
				QuadSlot::Outside => outside,
				QuadSlot::Slot(TileSlot::Empty) => 0.0,
				QuadSlot::Slot(TileSlot::Solid(v)) => v.0[0] as f32 / 65535.0,
				QuadSlot::Slot(TileSlot::Data(_)) => {
					varying = true;
					break;
				}
			};
			match constant {
				None => constant = Some(value),
				Some(c) if c != value => {
					varying = true;
					break;
				}
				_ => {}
			}
		}
		if varying {
			return MaskEval::Varying(MaskRef { quad, outside });
		}
		// Unused slots are `Outside`; if they were all unused, the tile reads
		// only the top-left slot — covered by the loop above.
		match constant.unwrap_or(outside) {
			v if v <= 0.0 => MaskEval::Hidden,
			v => MaskEval::Constant(v),
		}
	}

	/// The source quad of `image` (shifted by `offset` document pixels) for
	/// this output tile. Records dirty mips; `None` if any are dirty.
	fn quad(&mut self, image: &TiledImage, offset: (i32, i32), layer: LayerId, is_mask: bool) -> Option<Quad> {
		// Document invariant: every image in a document has the document's
		// size, hence the same number of levels (offsets express placement).
		debug_assert!(self.level < image.level_count(), "image smaller than the document");
		let level = self.level.min(image.level_count() - 1);
		let scale = 1i64 << level;
		// Offsets are rounded to whole pixels of this level (exact at level 0).
		let off = |o: i32| (o as i64 * 2 + scale).div_euclid(scale * 2);
		let pos = (self.origin.0 - off(offset.0), self.origin.1 - off(offset.1));
		let t = TILE_SIZE as i64;
		let base = (pos.0.div_euclid(t), pos.1.div_euclid(t));
		let shift = (pos.0.rem_euclid(t) as u32, pos.1.rem_euclid(t) as u32);
		let grid = image.grid(level);
		let mut dirty = false;
		let slots = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
			// Slots never read with this shift stay Outside.
			if (dx == 1 && shift.0 == 0) || (dy == 1 && shift.1 == 0) {
				return QuadSlot::Outside;
			}
			let (gx, gy) = (base.0 + dx, base.1 + dy);
			if gx < 0 || gy < 0 || gx >= grid.cols() as i64 || gy >= grid.rows() as i64 {
				return QuadSlot::Outside;
			}
			let (gx, gy) = (gx as u32, gy as u32);
			if image.is_dirty(level, gx, gy) {
				dirty = true;
				self.missing.push(MipRequest {
					layer,
					mask: is_mask,
					level,
					x: gx,
					y: gy,
				});
				return QuadSlot::Outside;
			}
			QuadSlot::Slot(image.slot(level, gx, gy).clone())
		});
		(!dirty).then_some(Quad { shift, slots })
	}
}

fn normal_if_pass(mode: BlendMode) -> BlendMode {
	if mode == BlendMode::PassThrough { BlendMode::Normal } else { mode }
}

fn wrap_isolated(inner: Vec<Op>, blend: BlendMode, alpha: f32, mask: Option<MaskRef>, clip: bool) -> Vec<Op> {
	let mut ops = Vec::with_capacity(inner.len() + 2);
	ops.push(Op::BeginIsolated);
	ops.extend(inner);
	ops.push(Op::EndIsolated { blend, alpha, mask, clip });
	ops
}

// ---------------------------------------------------------------------------
// Hashing (composite-cache key)
// ---------------------------------------------------------------------------

/// Hash a sequence of ops (prefix-cache keys).
pub fn hash_ops(ops: &[Op], h: &mut impl Hasher) {
	for op in ops {
		hash_op(op, h);
	}
}

fn hash_op(op: &Op, h: &mut impl Hasher) {
	std::mem::discriminant(op).hash(h);
	match op {
		Op::Layer {
			layer,
			source,
			blend,
			alpha,
			mask,
			clip,
		} => {
			layer.hash(h);
			hash_source(source, h);
			blend.hash(h);
			alpha.to_bits().hash(h);
			hash_mask(mask, h);
			clip.hash(h);
		}
		Op::Adjust {
			layer,
			adjust,
			blend,
			alpha,
			mask,
		} => {
			layer.hash(h);
			match adjust {
				AdjustKind::Lut(lut) => lut.key.hash(h),
				AdjustKind::HueSaturation {
					hue,
					saturation,
					lightness,
					colorize,
				} => {
					[hue, saturation, lightness].map(|v| v.to_bits()).hash(h);
					colorize.hash(h);
				}
			}
			blend.hash(h);
			alpha.to_bits().hash(h);
			hash_mask(mask, h);
		}
		Op::BeginIsolated | Op::BeginPassThrough => {}
		Op::EndIsolated { blend, alpha, mask, clip } => {
			blend.hash(h);
			alpha.to_bits().hash(h);
			hash_mask(mask, h);
			clip.hash(h);
		}
		Op::EndPassThrough { alpha, mask } => {
			alpha.to_bits().hash(h);
			hash_mask(mask, h);
		}
	}
}

fn hash_source(source: &Source, h: &mut impl Hasher) {
	match source {
		Source::Solid(c) => {
			0u8.hash(h);
			c.map(f32::to_bits).hash(h);
		}
		Source::Tiles(quad) => {
			1u8.hash(h);
			hash_quad(quad, h);
		}
	}
}

fn hash_mask(mask: &Option<MaskRef>, h: &mut impl Hasher) {
	match mask {
		None => 0u8.hash(h),
		Some(m) => {
			1u8.hash(h);
			m.outside.to_bits().hash(h);
			hash_quad(&m.quad, h);
		}
	}
}

fn hash_quad(quad: &Quad, h: &mut impl Hasher) {
	quad.shift.hash(h);
	for slot in &quad.slots {
		match slot {
			QuadSlot::Outside => 0u8.hash(h),
			QuadSlot::Slot(TileSlot::Empty) => 1u8.hash(h),
			QuadSlot::Slot(TileSlot::Solid(v)) => {
				2u8.hash(h);
				v.hash(h);
			}
			QuadSlot::Slot(TileSlot::Data(handle)) => {
				3u8.hash(h);
				handle.id().hash(h);
			}
		}
	}
}
