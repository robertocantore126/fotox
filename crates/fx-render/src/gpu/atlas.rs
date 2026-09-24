//! GPU tile atlas: source tiles (and cached prefix composites) resident on
//! the GPU, in up to 8 pages of `Rgba16Float` 2D-array textures.
//!
//! * Source tiles are stored **straight** (not premultiplied) f16; masks as
//!   `(v, 0, 0, 1)`. Prefix composites are stored premultiplied (they are read
//!   by `K_LOAD_PREFIX` only).
//! * Slots are recycled with a clock (second-chance) sweep over "last frame
//!   used": a slot used in the current frame is never evicted.
//! * Conversion to f16 runs on the rayon pool; the upload is one
//!   `write_texture` per tile.

use std::collections::HashMap;
use std::sync::Arc;

use fx_tiles::{PixelFormat, TILE_PIXELS, TILE_SIZE, TileBuffer, TileId};
use half::f16;
use rayon::prelude::*;

pub const MAX_PAGES: usize = 8;
pub const TILE_BYTES_F16: u64 = (TILE_PIXELS * 8) as u64;
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AtlasKey {
	Tile(TileId),
	/// Cached composite of a program prefix (premultiplied).
	Prefix(u64),
}

pub struct TileAtlas {
	pages: Vec<wgpu::Texture>,
	views: Vec<wgpu::TextureView>,
	layers_per_page: u32,
	capacity: u32,
	map: HashMap<AtlasKey, u32>,
	/// Per slot: key and last frame used.
	slots: Vec<Option<(AtlasKey, u64)>>,
	free: Vec<u32>,
	hand: u32,
	frame: u64,
}

impl TileAtlas {
	/// `budget_bytes` / 512 KiB slots, split into pages of at most
	/// `max_texture_array_layers` (≤ 2048) layers, at most 8 pages.
	pub fn new(device: &wgpu::Device, budget_bytes: u64) -> Self {
		let layers_per_page = device.limits().max_texture_array_layers.clamp(1, 2048);
		let wanted = (budget_bytes / TILE_BYTES_F16).max(1);
		let pages = wanted.div_ceil(layers_per_page as u64).min(MAX_PAGES as u64) as u32;
		let capacity = (wanted as u32).min(pages * layers_per_page);
		let mut page_textures = Vec::new();
		let mut views = Vec::new();
		for p in 0..pages {
			let layers = (capacity - p * layers_per_page).min(layers_per_page);
			let texture = device.create_texture(&wgpu::TextureDescriptor {
				label: Some("fx-atlas-page"),
				size: wgpu::Extent3d {
					width: TILE_SIZE,
					height: TILE_SIZE,
					depth_or_array_layers: layers,
				},
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				format: FORMAT,
				usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
				view_formats: &[],
			});
			views.push(texture.create_view(&wgpu::TextureViewDescriptor {
				dimension: Some(wgpu::TextureViewDimension::D2Array),
				..Default::default()
			}));
			page_textures.push(texture);
		}
		Self {
			pages: page_textures,
			views,
			layers_per_page,
			capacity,
			map: HashMap::new(),
			slots: vec![None; capacity as usize],
			free: (0..capacity).rev().collect(),
			hand: 0,
			frame: 1,
		}
	}

	pub fn capacity(&self) -> u32 {
		self.capacity
	}

	pub fn layers_per_page(&self) -> u32 {
		self.layers_per_page
	}

	pub fn begin_frame(&mut self) {
		self.frame += 1;
	}

	/// Views for the 8 page bindings (missing pages repeat page 0).
	pub fn page_views(&self) -> [&wgpu::TextureView; MAX_PAGES] {
		std::array::from_fn(|i| self.views.get(i).unwrap_or(&self.views[0]))
	}

	/// Slot of a resident key, marked as used this frame.
	pub fn lookup(&mut self, key: AtlasKey) -> Option<u32> {
		let slot = *self.map.get(&key)?;
		if let Some(entry) = &mut self.slots[slot as usize] {
			entry.1 = self.frame;
		}
		Some(slot)
	}

	pub fn contains(&self, key: AtlasKey) -> bool {
		self.map.contains_key(&key)
	}

	/// Reserve a slot for `key` (evicting the least recently used slot not
	/// used this frame). `None` if every slot is in use this frame.
	pub fn allocate(&mut self, key: AtlasKey) -> Option<u32> {
		if let Some(slot) = self.lookup(key) {
			return Some(slot);
		}
		let slot = match self.free.pop() {
			Some(slot) => slot,
			None => self.evict()?,
		};
		self.slots[slot as usize] = Some((key, self.frame));
		self.map.insert(key, slot);
		Some(slot)
	}

	/// Clock sweep: a slot used in an earlier frame gets one "second chance"
	/// pass by being aged; the first slot found older than the previous frame
	/// is evicted. Slots used in the current frame are never evicted.
	fn evict(&mut self) -> Option<u32> {
		for _ in 0..2 * self.capacity {
			let slot = self.hand;
			self.hand = (self.hand + 1) % self.capacity;
			if let Some((key, last)) = self.slots[slot as usize]
				&& last + 1 < self.frame
			{
				self.map.remove(&key);
				self.slots[slot as usize] = None;
				return Some(slot);
			}
		}
		// Only slots used in this or the previous frame remain: take one from the previous frame.
		for _ in 0..self.capacity {
			let slot = self.hand;
			self.hand = (self.hand + 1) % self.capacity;
			if let Some((key, last)) = self.slots[slot as usize]
				&& last < self.frame
			{
				self.map.remove(&key);
				self.slots[slot as usize] = None;
				return Some(slot);
			}
		}
		None
	}

	/// Page texture and layer of a slot (for copies into the atlas).
	pub fn location(&self, slot: u32) -> (&wgpu::Texture, u32) {
		(&self.pages[(slot / self.layers_per_page) as usize], slot % self.layers_per_page)
	}

	/// Convert (in parallel) and upload tiles. Returns the slot of each, or
	/// `None` for tiles that did not fit (atlas full this frame).
	pub fn upload(&mut self, queue: &wgpu_sync::Queue, tiles: &[(TileId, Arc<TileBuffer>)]) -> Vec<Option<u32>> {
		let slots: Vec<Option<u32>> = tiles.iter().map(|(id, _)| self.allocate(AtlasKey::Tile(*id))).collect();
		let converted: Vec<Option<Vec<u8>>> = tiles
			.par_iter()
			.zip(&slots)
			.map(|((_, buffer), slot)| slot.map(|_| to_f16_straight(buffer)))
			.collect();
		for (slot, data) in slots.iter().zip(converted) {
			if let (Some(slot), Some(data)) = (slot, data) {
				let (texture, layer) = self.location(*slot);
				queue.write_texture(
					wgpu::TexelCopyTextureInfo {
						texture,
						mip_level: 0,
						origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
						aspect: wgpu::TextureAspect::All,
					},
					&data,
					wgpu::TexelCopyBufferLayout {
						offset: 0,
						bytes_per_row: Some(TILE_SIZE * 8),
						rows_per_image: Some(TILE_SIZE),
					},
					wgpu::Extent3d {
						width: TILE_SIZE,
						height: TILE_SIZE,
						depth_or_array_layers: 1,
					},
				);
			}
		}
		slots
	}
}

/// Tile pixels → straight RGBA f16 bytes (masks: `(v, 0, 0, 1)`).
pub fn to_f16_straight(buffer: &TileBuffer) -> Vec<u8> {
	let mut out = vec![0u8; TILE_PIXELS * 8];
	let write = |out: &mut [u8], i: usize, v: [f32; 4]| {
		for c in 0..4 {
			out[i * 8 + c * 2..i * 8 + c * 2 + 2].copy_from_slice(&f16::from_f32(v[c]).to_le_bytes());
		}
	};
	match buffer.format() {
		PixelFormat::Rgba8 => {
			for (i, px) in buffer.bytes().chunks_exact(4).enumerate() {
				write(&mut out, i, [px[0], px[1], px[2], px[3]].map(|v| v as f32 / 255.0));
			}
		}
		PixelFormat::Rgba16 => {
			for (i, px) in buffer.as_u16().chunks_exact(4).enumerate() {
				write(&mut out, i, [px[0], px[1], px[2], px[3]].map(|v| v as f32 / 65535.0));
			}
		}
		PixelFormat::Gray8 => {
			for (i, v) in buffer.bytes().iter().enumerate() {
				write(&mut out, i, [*v as f32 / 255.0, 0.0, 0.0, 1.0]);
			}
		}
		PixelFormat::Gray16 => {
			for (i, v) in buffer.as_u16().iter().enumerate() {
				write(&mut out, i, [*v as f32 / 65535.0, 0.0, 0.0, 1.0]);
			}
		}
	}
	out
}
