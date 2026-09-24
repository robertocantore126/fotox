//! GPU compositor: runs [`TileProgram`]s for many tiles in one dispatch and
//! caches results by program key.
//!
//! Per frame (render thread):
//! ```text
//! begin_frame()
//! composite(programs, hot)  →  per tile: Empty | Ready{slot} | Deferred{missing}
//! viewport pass samples `composite_view()` at the returned slots
//! ```
//! * `Ready` — composited now or earlier (cache hit on the program key).
//! * `Deferred` — some source tiles are not in RAM (`missing`: load them on a
//!   worker with `TileStore::get`, then ask again), or this frame's upload
//!   budget / cache capacity is spent. Draw a coarser level meanwhile.
//!
//! **Prefix cache** (the "stack split" of ARCHITECTURE.md §4.6): while a layer
//! is being edited (`set_hot_layer`), the composite of everything below it —
//! the program prefix up to the root-level segment that contains the layer —
//! is cached in the atlas. Each frame then only runs the ops from that layer up.
//!
//! Written by Claude. Tests: gpu/tests.rs (GPU vs CPU reference).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use fx_core::{BlendMode, LayerId};
use fx_tiles::{TILE_SIZE, TileBuffer, TileHandle, TileId, TileSlot};

use super::atlas::{AtlasKey, FORMAT, MAX_PAGES, TileAtlas};
use crate::adjust::{LUT_SIZE, Lut};
use crate::program::{AdjustKind, MaskRef, Op, Quad, QuadSlot, Source, TileProgram};

const EMPTY: u32 = 0xFFFF_FFFF;
const SOLID: u32 = 0xFFFF_FFFE;
const OUTSIDE: u32 = 0xFFFF_FFFD;

const K_LAYER: u32 = 0;
const K_ADJUST_LUT: u32 = 1;
const K_BEGIN_ISOLATED: u32 = 3;
const K_BEGIN_PASS: u32 = 4;
const K_END_ISOLATED: u32 = 5;
const K_END_PASS: u32 = 6;
const K_LOAD_PREFIX: u32 = 7;

const F_CLIP: u32 = 1;
const F_MASK: u32 = 2;
const F_DISSOLVE: u32 = 4;

/// Max nesting of groups (clipping groups count) the shader supports.
pub const MAX_DEPTH: usize = 11;

/// Max tile jobs per `composite` call (dispatch z limit is 65 535).
const MAX_JOBS: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GpuOp {
	kind: u32,
	blend: u32,
	flags: u32,
	alpha: f32,
	src: [u32; 4],
	mask: [u32; 4],
	src_shift: [u32; 2],
	mask_shift: [u32; 2],
	lut_row: u32,
	seed: u32,
	mask_outside: f32,
	_pad: f32,
	src_solid: [[f32; 4]; 4],
	mask_solid: [f32; 4],
	params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GpuJob {
	op_start: u32,
	op_count: u32,
	out_slot: u32,
	_pad0: u32,
	origin: [u32; 2],
	_pad1: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Globals {
	layers_per_page: u32,
	lut_size: u32,
	_pad: [u32; 2],
}

#[derive(Clone, Debug)]
pub struct CompositorConfig {
	/// VRAM for source tiles (default 6 GiB on the reference GPU).
	pub atlas_budget: u64,
	/// Composite cache slots (visible tiles + margin). 512 KiB each.
	pub composite_slots: u32,
	/// Max source tiles uploaded per `composite` call.
	pub upload_budget: usize,
}

impl Default for CompositorConfig {
	fn default() -> Self {
		Self {
			atlas_budget: 6 << 30,
			composite_slots: 1024,
			upload_budget: 48,
		}
	}
}

#[derive(Clone, Debug)]
pub enum TileOutcome {
	/// Nothing to draw (fully transparent).
	Empty,
	/// Composite available in `composite_view()` layer `slot`.
	Ready { slot: u32 },
	/// Not now. `missing` tiles are not in RAM: load them, then retry.
	/// Empty `missing` = budget/capacity spent this frame: retry next frame.
	Deferred { missing: Vec<TileHandle> },
}

#[derive(Debug, thiserror::Error)]
pub enum CompositeError {
	#[error("groups nested deeper than {MAX_DEPTH} levels")]
	TooDeep,
	#[error("not supported on the GPU yet: {0}")]
	Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositorStats {
	pub cache_hits: u64,
	pub composited: u64,
	pub uploads: u64,
	pub prefix_hits: u64,
	pub deferred: u64,
}

/// `(level, tx, ty)` of an output tile.
type TileCoord = (usize, u32, u32);

struct CacheEntry {
	key: u64,
	slot: u32,
	last_frame: u64,
}

pub struct GpuCompositor {
	device: wgpu::Device,
	queue: wgpu_sync::Queue,
	pipeline: wgpu::ComputePipeline,
	layout: wgpu::BindGroupLayout,
	atlas: TileAtlas,
	// composite cache
	composite_texture: wgpu::Texture,
	composite_view: wgpu::TextureView,
	composite_capacity: u32,
	cache: HashMap<TileCoord, CacheEntry>,
	composite_slot_owner: Vec<Option<(TileCoord, u64)>>,
	composite_free: Vec<u32>,
	composite_hand: u32,
	// LUTs
	lut_texture: wgpu::Texture,
	lut_view: wgpu::TextureView,
	lut_rows: HashMap<u64, u32>,
	lut_store: HashMap<u64, Arc<Lut>>,
	lut_capacity: u32,
	// buffers
	ops_buffer: wgpu::Buffer,
	jobs_buffer: wgpu::Buffer,
	globals: wgpu::Buffer,
	config: CompositorConfig,
	frame: u64,
	hot_layer: Option<LayerId>,
	stats: CompositorStats,
}

impl GpuCompositor {
	pub fn new(device: &wgpu::Device, queue: &wgpu_sync::Queue, config: CompositorConfig) -> Self {
		let module = device.create_shader_module(wgpu::include_wgsl!("composite.wgsl"));
		let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..MAX_PAGES as u32)
			.map(|binding| wgpu::BindGroupLayoutEntry {
				binding,
				visibility: wgpu::ShaderStages::COMPUTE,
				ty: wgpu::BindingType::Texture {
					sample_type: wgpu::TextureSampleType::Float { filterable: false },
					view_dimension: wgpu::TextureViewDimension::D2Array,
					multisampled: false,
				},
				count: None,
			})
			.collect();
		entries.push(wgpu::BindGroupLayoutEntry {
			binding: 8,
			visibility: wgpu::ShaderStages::COMPUTE,
			ty: wgpu::BindingType::Texture {
				sample_type: wgpu::TextureSampleType::Float { filterable: false },
				view_dimension: wgpu::TextureViewDimension::D2,
				multisampled: false,
			},
			count: None,
		});
		for binding in [9, 10] {
			entries.push(wgpu::BindGroupLayoutEntry {
				binding,
				visibility: wgpu::ShaderStages::COMPUTE,
				ty: wgpu::BindingType::Buffer {
					ty: wgpu::BufferBindingType::Storage { read_only: true },
					has_dynamic_offset: false,
					min_binding_size: None,
				},
				count: None,
			});
		}
		entries.push(wgpu::BindGroupLayoutEntry {
			binding: 11,
			visibility: wgpu::ShaderStages::COMPUTE,
			ty: wgpu::BindingType::StorageTexture {
				access: wgpu::StorageTextureAccess::WriteOnly,
				format: FORMAT,
				view_dimension: wgpu::TextureViewDimension::D2Array,
			},
			count: None,
		});
		entries.push(wgpu::BindGroupLayoutEntry {
			binding: 12,
			visibility: wgpu::ShaderStages::COMPUTE,
			ty: wgpu::BindingType::Buffer {
				ty: wgpu::BufferBindingType::Uniform,
				has_dynamic_offset: false,
				min_binding_size: None,
			},
			count: None,
		});
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("fx-composite"),
			entries: &entries,
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("fx-composite"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
			label: Some("fx-composite"),
			layout: Some(&pipeline_layout),
			module: &module,
			entry_point: Some("main"),
			compilation_options: Default::default(),
			cache: None,
		});

		let atlas = TileAtlas::new(device, config.atlas_budget);
		let composite_capacity = config.composite_slots.clamp(1, device.limits().max_texture_array_layers);
		let composite_texture = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("fx-composites"),
			size: wgpu::Extent3d {
				width: TILE_SIZE,
				height: TILE_SIZE,
				depth_or_array_layers: composite_capacity,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: FORMAT,
			usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
			view_formats: &[],
		});
		let composite_view = composite_texture.create_view(&wgpu::TextureViewDescriptor {
			dimension: Some(wgpu::TextureViewDimension::D2Array),
			..Default::default()
		});
		let lut_capacity = 16;
		let (lut_texture, lut_view) = create_lut_texture(device, lut_capacity);
		let globals = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("fx-composite-globals"),
			size: std::mem::size_of::<Globals>() as u64,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		queue.write_buffer(
			&globals,
			0,
			bytemuck::bytes_of(&Globals {
				layers_per_page: atlas.layers_per_page(),
				lut_size: LUT_SIZE as u32,
				_pad: [0; 2],
			}),
		);
		Self {
			device: device.clone(),
			queue: queue.clone(),
			pipeline,
			layout,
			atlas,
			composite_texture,
			composite_view,
			composite_capacity,
			cache: HashMap::new(),
			composite_slot_owner: vec![None; composite_capacity as usize],
			composite_free: (0..composite_capacity).rev().collect(),
			composite_hand: 0,
			lut_texture,
			lut_view,
			lut_rows: HashMap::new(),
			lut_store: HashMap::new(),
			lut_capacity,
			ops_buffer: storage_buffer(device, "fx-composite-ops", 64 * std::mem::size_of::<GpuOp>() as u64),
			jobs_buffer: storage_buffer(device, "fx-composite-jobs", 64 * std::mem::size_of::<GpuJob>() as u64),
			globals,
			config,
			frame: 1,
			hot_layer: None,
			stats: CompositorStats::default(),
		}
	}

	/// Array view of composited tiles (premultiplied Rgba16Float), indexed by `Ready.slot`.
	pub fn composite_view(&self) -> &wgpu::TextureView {
		&self.composite_view
	}

	pub fn stats(&self) -> CompositorStats {
		self.stats
	}

	pub fn begin_frame(&mut self) {
		self.frame += 1;
		self.atlas.begin_frame();
	}

	/// The layer currently being edited (enables the prefix cache), or `None`.
	pub fn set_hot_layer(&mut self, layer: Option<LayerId>) {
		self.hot_layer = layer;
	}

	/// Composite a batch of tiles. `hot` must never block: return the pixels
	/// only if they are already in RAM (`TileStore::try_get_hot`).
	pub fn composite(&mut self, programs: &[TileProgram], hot: &dyn Fn(&TileHandle) -> Option<Arc<TileBuffer>>) -> Result<Vec<TileOutcome>, CompositeError> {
		let mut outcomes: Vec<Option<TileOutcome>> = vec![None; programs.len()];
		let mut uploads: Vec<(TileId, Arc<TileBuffer>)> = Vec::new();
		let mut upload_ids: HashSet<TileId> = HashSet::new();
		let mut runnable: Vec<usize> = Vec::new();

		for (i, program) in programs.iter().enumerate() {
			check_supported(program)?;
			if program.is_empty() {
				outcomes[i] = Some(TileOutcome::Empty);
				continue;
			}
			let coord = (program.level, program.tx, program.ty);
			if let Some(entry) = self.cache.get_mut(&coord)
				&& entry.key == program.key
			{
				entry.last_frame = self.frame;
				self.composite_slot_owner[entry.slot as usize] = Some((coord, self.frame));
				self.stats.cache_hits += 1;
				outcomes[i] = Some(TileOutcome::Ready { slot: entry.slot });
				continue;
			}
			// Which inputs are missing from the atlas?
			let mut missing = Vec::new();
			let mut needed: Vec<(TileId, Arc<TileBuffer>)> = Vec::new();
			for handle in program_tiles(program) {
				let key = AtlasKey::Tile(handle.id());
				if self.atlas.lookup(key).is_some() || upload_ids.contains(&handle.id()) {
					continue;
				}
				match hot(handle) {
					Some(buffer) => needed.push((handle.id(), buffer)),
					None => missing.push(handle.clone()),
				}
			}
			// Upload what fits in this frame's budget even if the program must
			// wait: partial progress guarantees every tile converges.
			let room = self.config.upload_budget.saturating_sub(uploads.len());
			let complete = needed.len() <= room;
			for (id, buffer) in needed.into_iter().take(room) {
				upload_ids.insert(id);
				uploads.push((id, buffer));
			}
			if !missing.is_empty() || !complete {
				self.stats.deferred += 1;
				outcomes[i] = Some(TileOutcome::Deferred { missing });
				continue;
			}
			runnable.push(i);
		}

		let slots = self.atlas.upload(&self.queue, &uploads);
		self.stats.uploads += slots.iter().filter(|s| s.is_some()).count() as u64;
		if slots.iter().any(Option::is_none) {
			// Atlas full this frame: defer every program that needed one of the failed tiles.
			let failed: HashSet<TileId> = uploads.iter().zip(&slots).filter(|(_, s)| s.is_none()).map(|((id, _), _)| *id).collect();
			runnable.retain(|&i| {
				let blocked = program_tiles(&programs[i]).iter().any(|h| failed.contains(&h.id()));
				if blocked {
					outcomes[i] = Some(TileOutcome::Deferred { missing: Vec::new() });
				}
				!blocked
			});
		}

		// LUTs first: the table may grow, which must not happen mid-encode.
		let luts: Vec<Arc<Lut>> = runnable
			.iter()
			.flat_map(|&i| programs[i].ops.iter())
			.filter_map(|op| match op {
				Op::Adjust {
					adjust: AdjustKind::Lut(lut), ..
				} => Some(lut.clone()),
				_ => None,
			})
			.collect();
		self.ensure_luts(&luts);

		// Encode jobs.
		let mut gpu_ops: Vec<GpuOp> = Vec::new();
		let mut jobs: Vec<GpuJob> = Vec::new();
		let mut prefix_copies: Vec<(u32, u32)> = Vec::new(); // (composite slot, atlas slot)
		let mut temps: Vec<u32> = Vec::new();
		for &i in &runnable {
			let program = &programs[i];
			let coord = (program.level, program.tx, program.ty);
			if jobs.len() + 2 > MAX_JOBS {
				outcomes[i] = Some(TileOutcome::Deferred { missing: Vec::new() });
				continue;
			}
			let Some(out_slot) = self.allocate_composite(coord, program.key) else {
				outcomes[i] = Some(TileOutcome::Deferred { missing: Vec::new() });
				continue;
			};
			let origin = [program.tx * TILE_SIZE, program.ty * TILE_SIZE];
			let mut main_ops: &[Op] = &program.ops;
			let mut load_prefix: Option<u32> = None;

			if let Some((split, prefix_key)) = self.prefix_split(program) {
				if let Some(slot) = self.atlas.lookup(AtlasKey::Prefix(prefix_key)) {
					self.stats.prefix_hits += 1;
					load_prefix = Some(slot);
					main_ops = &program.ops[split..];
				} else if let Some(temp) = self.take_composite_slot()
					&& let Some(atlas_slot) = self.atlas.allocate(AtlasKey::Prefix(prefix_key))
				{
					// Compute the prefix alone this frame and keep it for the next ones.
					temps.push(temp);
					let start = gpu_ops.len() as u32;
					for op in &program.ops[..split] {
						gpu_ops.push(self.encode(op));
					}
					jobs.push(GpuJob {
						op_start: start,
						op_count: split as u32,
						out_slot: temp,
						origin,
						..Default::default()
					});
					prefix_copies.push((temp, atlas_slot));
				}
			}

			let start = gpu_ops.len() as u32;
			if let Some(slot) = load_prefix {
				gpu_ops.push(GpuOp {
					kind: K_LOAD_PREFIX,
					src: [slot, EMPTY, EMPTY, EMPTY],
					..Default::default()
				});
			}
			for op in main_ops {
				gpu_ops.push(self.encode(op));
			}
			jobs.push(GpuJob {
				op_start: start,
				op_count: gpu_ops.len() as u32 - start,
				out_slot,
				origin,
				..Default::default()
			});
			self.stats.composited += 1;
			outcomes[i] = Some(TileOutcome::Ready { slot: out_slot });
		}

		if !jobs.is_empty() {
			self.dispatch(&gpu_ops, &jobs, &prefix_copies);
		}
		// Temporary prefix slots are free again once the copies are recorded
		// (GPU work is ordered: a later write cannot overtake this copy).
		self.composite_free.extend(temps);
		Ok(outcomes.into_iter().map(|o| o.expect("every program gets an outcome")).collect())
	}

	/// Split point for the prefix cache: start of the root-level segment
	/// containing an op of the hot layer, if at least 2 ops precede it.
	fn prefix_split(&self, program: &TileProgram) -> Option<(usize, u64)> {
		let hot = self.hot_layer?;
		let mut depth = 0usize;
		let mut segment_start = 0usize;
		let mut split = None;
		for (i, op) in program.ops.iter().enumerate() {
			if depth == 0 {
				segment_start = i;
			}
			match op {
				Op::BeginIsolated | Op::BeginPassThrough => depth += 1,
				Op::EndIsolated { .. } | Op::EndPassThrough { .. } => depth -= 1,
				Op::Layer { layer, .. } | Op::Adjust { layer, .. } if *layer == hot => {
					split = Some(segment_start);
					break;
				}
				_ => {}
			}
		}
		let split = split.filter(|s| *s >= 2)?;
		let mut hasher = std::collections::hash_map::DefaultHasher::new();
		(program.level, program.tx, program.ty, split).hash(&mut hasher);
		crate::program::hash_ops(&program.ops[..split], &mut hasher);
		Some((split, hasher.finish()))
	}

	fn allocate_composite(&mut self, coord: TileCoord, key: u64) -> Option<u32> {
		let slot = match self.cache.get(&coord) {
			Some(entry) => entry.slot, // overwrite this tile's own previous result
			None => self.take_composite_slot()?,
		};
		self.cache.insert(
			coord,
			CacheEntry {
				key,
				slot,
				last_frame: self.frame,
			},
		);
		self.composite_slot_owner[slot as usize] = Some((coord, self.frame));
		Some(slot)
	}

	fn take_composite_slot(&mut self) -> Option<u32> {
		if let Some(slot) = self.composite_free.pop() {
			return Some(slot);
		}
		for _ in 0..self.composite_capacity {
			let slot = self.composite_hand;
			self.composite_hand = (self.composite_hand + 1) % self.composite_capacity;
			if let Some((coord, last)) = self.composite_slot_owner[slot as usize]
				&& last < self.frame
			{
				self.cache.remove(&coord);
				self.composite_slot_owner[slot as usize] = None;
				return Some(slot);
			}
		}
		None
	}

	/// Make sure every LUT has a row, growing the table (and re-uploading all
	/// known LUTs) if needed. Called before encoding a batch.
	fn ensure_luts(&mut self, luts: &[Arc<Lut>]) {
		let new: Vec<&Arc<Lut>> = luts.iter().filter(|l| !self.lut_rows.contains_key(&l.key)).collect();
		if new.is_empty() {
			return;
		}
		if self.lut_rows.len() + new.len() > self.lut_capacity as usize {
			// Keep only LUTs still referenced elsewhere, grow if needed, re-upload.
			self.lut_store.retain(|_, lut| Arc::strong_count(lut) > 1);
			let needed = (self.lut_store.len() + new.len()) as u32;
			while self.lut_capacity < needed {
				self.lut_capacity *= 2;
			}
			let (texture, view) = create_lut_texture(&self.device, self.lut_capacity);
			self.lut_texture = texture;
			self.lut_view = view;
			self.lut_rows.clear();
			let keep: Vec<Arc<Lut>> = self.lut_store.values().cloned().collect();
			for lut in keep {
				self.upload_lut(&lut);
			}
		}
		for lut in new {
			if !self.lut_rows.contains_key(&lut.key) {
				self.upload_lut(lut);
			}
		}
	}

	fn upload_lut(&mut self, lut: &Arc<Lut>) {
		let row = self.lut_rows.len() as u32;
		let data: Vec<f32> = lut.entries.iter().flat_map(|e| [e[0], e[1], e[2], 1.0]).collect();
		self.queue.write_texture(
			wgpu::TexelCopyTextureInfo {
				texture: &self.lut_texture,
				mip_level: 0,
				origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
				aspect: wgpu::TextureAspect::All,
			},
			bytemuck::cast_slice(&data),
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(LUT_SIZE as u32 * 16),
				rows_per_image: Some(1),
			},
			wgpu::Extent3d {
				width: LUT_SIZE as u32,
				height: 1,
				depth_or_array_layers: 1,
			},
		);
		self.lut_rows.insert(lut.key, row);
		self.lut_store.insert(lut.key, lut.clone());
	}

	fn encode(&mut self, op: &Op) -> GpuOp {
		let mut g = GpuOp::default();
		match op {
			Op::Layer {
				layer,
				source,
				blend,
				alpha,
				mask,
				clip,
			} => {
				g.kind = K_LAYER;
				g.blend = blend.shader_id();
				g.alpha = *alpha;
				g.seed = layer.0 as u32;
				if *blend == BlendMode::Dissolve {
					g.flags |= F_DISSOLVE;
				}
				if *clip {
					g.flags |= F_CLIP;
				}
				match source {
					Source::Solid(c) => {
						g.src = [SOLID; 4];
						g.src_solid = [*c; 4];
					}
					Source::Tiles(quad) => self.encode_src(quad, &mut g),
				}
				self.encode_mask(mask, &mut g);
			}
			Op::Adjust {
				adjust, blend, alpha, mask, ..
			} => {
				g.blend = blend.shader_id();
				g.alpha = *alpha;
				match adjust {
					AdjustKind::Lut(lut) => {
						g.kind = K_ADJUST_LUT;
						g.lut_row = *self.lut_rows.get(&lut.key).expect("ensure_luts ran before encoding");
					}
					AdjustKind::HueSaturation { .. } => unreachable!("rejected by check_supported"),
				}
				self.encode_mask(mask, &mut g);
			}
			Op::BeginIsolated => g.kind = K_BEGIN_ISOLATED,
			Op::BeginPassThrough => g.kind = K_BEGIN_PASS,
			Op::EndIsolated { blend, alpha, mask, clip } => {
				g.kind = K_END_ISOLATED;
				g.blend = blend.shader_id();
				g.alpha = *alpha;
				if *clip {
					g.flags |= F_CLIP;
				}
				self.encode_mask(mask, &mut g);
			}
			Op::EndPassThrough { alpha, mask } => {
				g.kind = K_END_PASS;
				g.alpha = *alpha;
				self.encode_mask(mask, &mut g);
			}
		}
		g
	}

	fn encode_src(&mut self, quad: &Quad, g: &mut GpuOp) {
		g.src_shift = [quad.shift.0, quad.shift.1];
		for (i, slot) in quad.slots.iter().enumerate() {
			g.src[i] = match slot {
				QuadSlot::Outside | QuadSlot::Slot(TileSlot::Empty) => EMPTY,
				QuadSlot::Slot(TileSlot::Solid(v)) => {
					g.src_solid[i] = v.0.map(|c| c as f32 / 65535.0);
					SOLID
				}
				QuadSlot::Slot(TileSlot::Data(h)) => self.atlas.lookup(AtlasKey::Tile(h.id())).expect("inputs are resident before encoding"),
			};
		}
	}

	fn encode_mask(&mut self, mask: &Option<MaskRef>, g: &mut GpuOp) {
		let Some(mask) = mask else { return };
		g.flags |= F_MASK;
		g.mask_shift = [mask.quad.shift.0, mask.quad.shift.1];
		g.mask_outside = mask.outside;
		for (i, slot) in mask.quad.slots.iter().enumerate() {
			g.mask[i] = match slot {
				QuadSlot::Outside => OUTSIDE,
				QuadSlot::Slot(TileSlot::Empty) => {
					g.mask_solid[i] = 0.0;
					SOLID
				}
				QuadSlot::Slot(TileSlot::Solid(v)) => {
					g.mask_solid[i] = v.0[0] as f32 / 65535.0;
					SOLID
				}
				QuadSlot::Slot(TileSlot::Data(h)) => self.atlas.lookup(AtlasKey::Tile(h.id())).expect("inputs are resident before encoding"),
			};
		}
	}

	fn dispatch(&mut self, ops: &[GpuOp], jobs: &[GpuJob], prefix_copies: &[(u32, u32)]) {
		let ops_bytes = bytemuck::cast_slice(ops);
		let jobs_bytes = bytemuck::cast_slice(jobs);
		if self.ops_buffer.size() < ops_bytes.len() as u64 {
			self.ops_buffer = storage_buffer(&self.device, "fx-composite-ops", (ops_bytes.len() as u64).next_power_of_two());
		}
		if self.jobs_buffer.size() < jobs_bytes.len() as u64 {
			self.jobs_buffer = storage_buffer(&self.device, "fx-composite-jobs", (jobs_bytes.len() as u64).next_power_of_two());
		}
		self.queue.write_buffer(&self.ops_buffer, 0, ops_bytes);
		self.queue.write_buffer(&self.jobs_buffer, 0, jobs_bytes);

		let pages = self.atlas.page_views();
		let mut entries: Vec<wgpu::BindGroupEntry> = pages
			.iter()
			.enumerate()
			.map(|(i, view)| wgpu::BindGroupEntry {
				binding: i as u32,
				resource: wgpu::BindingResource::TextureView(view),
			})
			.collect();
		entries.push(wgpu::BindGroupEntry {
			binding: 8,
			resource: wgpu::BindingResource::TextureView(&self.lut_view),
		});
		entries.push(wgpu::BindGroupEntry {
			binding: 9,
			resource: self.ops_buffer.as_entire_binding(),
		});
		entries.push(wgpu::BindGroupEntry {
			binding: 10,
			resource: self.jobs_buffer.as_entire_binding(),
		});
		entries.push(wgpu::BindGroupEntry {
			binding: 11,
			resource: wgpu::BindingResource::TextureView(&self.composite_view),
		});
		entries.push(wgpu::BindGroupEntry {
			binding: 12,
			resource: self.globals.as_entire_binding(),
		});
		let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("fx-composite"),
			layout: &self.layout,
			entries: &entries,
		});

		let mut encoder = self
			.device
			.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx-composite") });
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("fx-composite"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&self.pipeline);
			pass.set_bind_group(0, &bind_group, &[]);
			pass.dispatch_workgroups(TILE_SIZE / 16, TILE_SIZE / 16, jobs.len() as u32);
		}
		for &(temp, atlas_slot) in prefix_copies {
			let (texture, layer) = self.atlas.location(atlas_slot);
			encoder.copy_texture_to_texture(
				wgpu::TexelCopyTextureInfo {
					texture: &self.composite_texture,
					mip_level: 0,
					origin: wgpu::Origin3d { x: 0, y: 0, z: temp },
					aspect: wgpu::TextureAspect::All,
				},
				wgpu::TexelCopyTextureInfo {
					texture,
					mip_level: 0,
					origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
					aspect: wgpu::TextureAspect::All,
				},
				wgpu::Extent3d {
					width: TILE_SIZE,
					height: TILE_SIZE,
					depth_or_array_layers: 1,
				},
			);
		}
		self.queue.submit([encoder.finish()]);
	}

	/// Read one composited tile back (premultiplied RGBA f32). Blocking:
	/// tests, thumbnails and debugging only — never on the render path.
	pub fn read_tile(&self, slot: u32) -> Vec<[f32; 4]> {
		let size = (TILE_SIZE * TILE_SIZE * 8) as u64;
		let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("fx-readback"),
			size,
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		let mut encoder = self
			.device
			.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx-readback") });
		encoder.copy_texture_to_buffer(
			wgpu::TexelCopyTextureInfo {
				texture: &self.composite_texture,
				mip_level: 0,
				origin: wgpu::Origin3d { x: 0, y: 0, z: slot },
				aspect: wgpu::TextureAspect::All,
			},
			wgpu::TexelCopyBufferInfo {
				buffer: &buffer,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(TILE_SIZE * 8),
					rows_per_image: Some(TILE_SIZE),
				},
			},
			wgpu::Extent3d {
				width: TILE_SIZE,
				height: TILE_SIZE,
				depth_or_array_layers: 1,
			},
		);
		self.queue.submit([encoder.finish()]);
		let slice = buffer.slice(..);
		slice.map_async(wgpu::MapMode::Read, |r| r.expect("readback map failed"));
		self.device.poll(wgpu::PollType::wait_indefinitely()).expect("device lost during readback");
		let data = slice.get_mapped_range();
		let halves: &[u16] = bytemuck::cast_slice(&data);
		halves
			.chunks_exact(4)
			.map(|c| [0, 1, 2, 3].map(|i| half::f16::from_bits(c[i]).to_f32()))
			.collect()
	}
}

fn check_supported(program: &TileProgram) -> Result<(), CompositeError> {
	let mut depth = 0usize;
	for op in &program.ops {
		match op {
			Op::BeginIsolated | Op::BeginPassThrough => {
				depth += 1;
				if depth > MAX_DEPTH {
					return Err(CompositeError::TooDeep);
				}
			}
			Op::EndIsolated { .. } | Op::EndPassThrough { .. } => depth = depth.saturating_sub(1),
			Op::Adjust {
				adjust: AdjustKind::HueSaturation { .. },
				..
			} => return Err(CompositeError::Unsupported("Hue/Saturation (M2-T04)")),
			_ => {}
		}
	}
	Ok(())
}

/// All stored tiles a program reads (sources and masks).
fn program_tiles(program: &TileProgram) -> Vec<&TileHandle> {
	let mut out = Vec::new();
	for op in &program.ops {
		for quad in op.quads() {
			for slot in &quad.slots {
				if let QuadSlot::Slot(TileSlot::Data(h)) = slot {
					out.push(h);
				}
			}
		}
	}
	out
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some(label),
		size: size.max(256),
		usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
		mapped_at_creation: false,
	})
}

fn create_lut_texture(device: &wgpu::Device, rows: u32) -> (wgpu::Texture, wgpu::TextureView) {
	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("fx-luts"),
		size: wgpu::Extent3d {
			width: LUT_SIZE as u32,
			height: rows,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: wgpu::TextureFormat::Rgba32Float,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	});
	let view = texture.create_view(&Default::default());
	(texture, view)
}
