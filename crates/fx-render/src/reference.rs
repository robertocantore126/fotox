//! CPU reference compositor: executes a [`TileProgram`] per pixel in f64.
//!
//! Slow and obviously correct. It defines what the GPU compositor must
//! produce (tests compare them) and is the fallback for exact results.

use std::collections::HashMap;
use std::sync::Arc;

use fx_core::BlendMode;
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileHandle, TileId, TileSlot};

use crate::blend::{Premul, composite, unpremultiply};
use crate::program::{AdjustKind, MaskRef, Op, Quad, QuadSlot, Source, TileProgram};

/// Fetches the pixels of a tile handle (normally `TileStore::get`).
pub type Fetch<'a> = &'a dyn Fn(&TileHandle) -> Arc<TileBuffer>;

/// Render a program into 256×256 premultiplied RGBA (row-major).
pub fn render_tile(program: &TileProgram, fetch: Fetch<'_>) -> Vec<Premul> {
	// Resolve all tiles up front so the per-pixel loop is simple.
	let mut buffers: HashMap<TileId, Arc<TileBuffer>> = HashMap::new();
	for op in &program.ops {
		for quad in op.quads() {
			for slot in &quad.slots {
				if let QuadSlot::Slot(TileSlot::Data(h)) = slot {
					buffers.entry(h.id()).or_insert_with(|| fetch(h));
				}
			}
		}
	}
	let origin = (program.tx * TILE_SIZE, program.ty * TILE_SIZE);
	let mut out = Vec::with_capacity((TILE_SIZE * TILE_SIZE) as usize);
	let mut stack: Vec<Premul> = Vec::with_capacity(8);
	for py in 0..TILE_SIZE {
		for px in 0..TILE_SIZE {
			stack.clear();
			stack.push([0.0; 4]);
			for op in &program.ops {
				execute(op, px, py, origin, &buffers, &mut stack);
			}
			debug_assert_eq!(stack.len(), 1, "unbalanced group ops");
			out.push(stack[0]);
		}
	}
	out
}

fn execute(op: &Op, px: u32, py: u32, origin: (u32, u32), buffers: &HashMap<TileId, Arc<TileBuffer>>, stack: &mut Vec<Premul>) {
	match op {
		Op::Layer {
			layer,
			source,
			blend,
			alpha,
			mask,
			clip,
		} => {
			let (cs, content_alpha) = match source {
				Source::Solid(c) => ([c[0] as f64, c[1] as f64, c[2] as f64], c[3] as f64),
				Source::Tiles(quad) => sample_rgba(quad, px, py, buffers),
			};
			let m = mask.as_ref().map_or(1.0, |m| sample_mask(m, px, py, buffers));
			let mut alpha_s = content_alpha * *alpha as f64 * m;
			let mut mode = *blend;
			if mode == BlendMode::Dissolve {
				let threshold = dissolve_hash(origin.0 + px, origin.1 + py, layer.0 as u32);
				alpha_s = if threshold < alpha_s { 1.0 } else { 0.0 };
				mode = BlendMode::Normal;
			}
			let top = stack.last_mut().expect("stack never empty");
			*top = composite(mode, *top, cs, alpha_s, *clip);
		}
		Op::Adjust {
			adjust, blend, alpha, mask, ..
		} => {
			let m = mask.as_ref().map_or(1.0, |m| sample_mask(m, px, py, buffers));
			let top = stack.last_mut().expect("stack never empty");
			let cb = unpremultiply(*top);
			let f = match adjust {
				AdjustKind::Lut(lut) => lut.apply(cb),
				AdjustKind::HueSaturation { .. } => todo!("M2-T04: Hue/Saturation"),
			};
			*top = composite(*blend, *top, f, *alpha as f64 * m, true);
		}
		Op::BeginIsolated => stack.push([0.0; 4]),
		Op::BeginPassThrough => {
			let top = *stack.last().expect("stack never empty");
			stack.push(top);
		}
		Op::EndIsolated { blend, alpha, mask, clip } => {
			let group = stack.pop().expect("balanced");
			let m = mask.as_ref().map_or(1.0, |m| sample_mask(m, px, py, buffers));
			let top = stack.last_mut().expect("stack never empty");
			*top = composite(*blend, *top, unpremultiply(group), group[3] * *alpha as f64 * m, *clip);
		}
		Op::EndPassThrough { alpha, mask } => {
			let result = stack.pop().expect("balanced");
			let m = mask.as_ref().map_or(1.0, |m| sample_mask(m, px, py, buffers));
			let t = *alpha as f64 * m;
			let top = stack.last_mut().expect("stack never empty");
			for i in 0..4 {
				top[i] += (result[i] - top[i]) * t;
			}
		}
	}
}

/// Straight RGB + alpha of a pixel layer at output pixel `(px, py)`.
fn sample_rgba(quad: &Quad, px: u32, py: u32, buffers: &HashMap<TileId, Arc<TileBuffer>>) -> ([f64; 3], f64) {
	let (index, x, y) = quad.locate(px, py);
	match &quad.slots[index] {
		QuadSlot::Outside | QuadSlot::Slot(TileSlot::Empty) => ([0.0; 3], 0.0),
		QuadSlot::Slot(TileSlot::Solid(v)) => {
			let c = v.0.map(|c| c as f64 / 65535.0);
			([c[0], c[1], c[2]], c[3])
		}
		QuadSlot::Slot(TileSlot::Data(h)) => {
			let c = read_pixel(&buffers[&h.id()], x, y);
			([c[0], c[1], c[2]], c[3])
		}
	}
}

fn sample_mask(mask: &MaskRef, px: u32, py: u32, buffers: &HashMap<TileId, Arc<TileBuffer>>) -> f64 {
	let (index, x, y) = mask.quad.locate(px, py);
	match &mask.quad.slots[index] {
		QuadSlot::Outside => mask.outside as f64,
		QuadSlot::Slot(TileSlot::Empty) => 0.0,
		QuadSlot::Slot(TileSlot::Solid(v)) => v.0[0] as f64 / 65535.0,
		QuadSlot::Slot(TileSlot::Data(h)) => read_pixel(&buffers[&h.id()], x, y)[0],
	}
}

/// One pixel as 0..1 values `[r, g, b, a]` (gray formats: `[v, 0, 0, 0]`).
pub fn read_pixel(buffer: &TileBuffer, x: u32, y: u32) -> [f64; 4] {
	let i = (y * TILE_SIZE + x) as usize;
	match buffer.format() {
		PixelFormat::Rgba8 => {
			let b = &buffer.bytes()[i * 4..i * 4 + 4];
			[b[0], b[1], b[2], b[3]].map(|v| v as f64 / 255.0)
		}
		PixelFormat::Rgba16 => {
			let s = &buffer.as_u16()[i * 4..i * 4 + 4];
			[s[0], s[1], s[2], s[3]].map(|v| v as f64 / 65535.0)
		}
		PixelFormat::Gray8 => [buffer.bytes()[i] as f64 / 255.0, 0.0, 0.0, 0.0],
		PixelFormat::Gray16 => [buffer.as_u16()[i] as f64 / 65535.0, 0.0, 0.0, 0.0],
	}
}

/// Position-stable hash in 0..1 for Dissolve. Must match `composite.wgsl`.
pub fn dissolve_hash(x: u32, y: u32, seed: u32) -> f64 {
	let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
	// lowbias32 (Chris Wellons)
	h ^= h >> 16;
	h = h.wrapping_mul(0x7FEB_352D);
	h ^= h >> 15;
	h = h.wrapping_mul(0x846C_A68B);
	h ^= h >> 16;
	h as f64 / 4_294_967_296.0
}
