//! The tile store: ownership, reference counting and residency of tiles.
//!
//! STATUS
//! * Implemented (reference implementation, keep its structure):
//!   insertion, RAII handles, hot residency, accounting, stats.
//! * M1-T03: warm tier (LZ4 compression of cold-ish hot tiles) + `trim`.
//! * M1-T04: cold tier (scratch file) + background trimming thread.
//! * M3:     `Backed` residency (tiles that live in an opened native file).
//!
//! Design: a [`TileHandle`] is an `Arc<TileEntry>`. Cloning/dropping a handle
//! is a single atomic operation, so snapshots of whole layers (tens of
//! thousands of handles) are cheap. The store keeps a *weak* registry of all
//! entries so the trimming code can find eviction candidates.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

use crate::format::{PixelFormat, PixelValue};

// ---------------------------------------------------------------------------
// Buffers
// ---------------------------------------------------------------------------

/// Uncompressed pixels of one tile. Length is always `format.tile_bytes()`.
///
/// Pixels outside the image (right/bottom edge tiles) exist in the buffer and
/// must be kept fully transparent (RGBA) or 0 (gray). Code that reads edge
/// tiles may rely on that.
#[derive(Clone, PartialEq, Eq)]
pub struct TileBuffer {
	format: PixelFormat,
	bytes: Box<[u8]>,
}

impl std::fmt::Debug for TileBuffer {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("TileBuffer")
			.field("format", &self.format)
			.field("len", &self.bytes.len())
			.finish()
	}
}

impl TileBuffer {
	/// A buffer with every byte 0 (transparent black / gray 0).
	pub fn zeroed(format: PixelFormat) -> Self {
		Self {
			format,
			bytes: vec![0u8; format.tile_bytes()].into_boxed_slice(),
		}
	}

	/// A buffer where every pixel equals `value`.
	pub fn filled(format: PixelFormat, value: PixelValue) -> Self {
		let mut buffer = Self::zeroed(format);
		let px = encode_pixel(format, value);
		for chunk in buffer.bytes.chunks_exact_mut(format.bytes_per_pixel()) {
			chunk.copy_from_slice(&px[..format.bytes_per_pixel()]);
		}
		buffer
	}

	pub fn from_bytes(format: PixelFormat, bytes: Box<[u8]>) -> Result<Self, TileError> {
		if bytes.len() != format.tile_bytes() {
			return Err(TileError::WrongSize {
				expected: format.tile_bytes(),
				got: bytes.len(),
			});
		}
		Ok(Self { format, bytes })
	}

	pub fn format(&self) -> PixelFormat {
		self.format
	}

	pub fn bytes(&self) -> &[u8] {
		&self.bytes
	}

	pub fn bytes_mut(&mut self) -> &mut [u8] {
		&mut self.bytes
	}

	/// 16-bit view of an `Rgba16`/`Gray16` buffer. Panics for 8-bit formats.
	pub fn as_u16(&self) -> &[u16] {
		assert_eq!(self.format.bytes_per_channel(), 2, "as_u16 on an 8-bit tile");
		bytemuck_cast_u16(&self.bytes)
	}

	/// If every pixel has the same value, return it. Used to collapse tiles
	/// into [`crate::TileSlot::Empty`] / [`crate::TileSlot::Solid`] before
	/// inserting them. Must be fast: it runs on every tile an operation writes.
	pub fn uniform_value(&self) -> Option<PixelValue> {
		let bpp = self.format.bytes_per_pixel();
		let first = &self.bytes[..bpp];
		if self.bytes.chunks_exact(bpp).all(|px| px == first) {
			Some(decode_pixel(self.format, first))
		} else {
			None
		}
	}
}

fn bytemuck_cast_u16(bytes: &[u8]) -> &[u16] {
	// Boxed slices from the global allocator are at least 2-aligned in practice,
	// but we do not rely on it silently.
	assert!(bytes.as_ptr().align_offset(std::mem::align_of::<u16>()) == 0, "tile buffer not 2-aligned");
	// SAFETY: alignment checked above, length is even for 16-bit formats, u16 has no invalid bit patterns.
	unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), bytes.len() / 2) }
}

fn encode_pixel(format: PixelFormat, value: PixelValue) -> [u8; 8] {
	let mut out = [0u8; 8];
	match format {
		PixelFormat::Rgba8 => {
			for (o, v) in out.iter_mut().zip(value.0) {
				*o = (v / 257) as u8;
			}
		}
		PixelFormat::Rgba16 => {
			for c in 0..4 {
				out[c * 2..c * 2 + 2].copy_from_slice(&value.0[c].to_ne_bytes());
			}
		}
		PixelFormat::Gray8 => out[0] = (value.0[0] / 257) as u8,
		PixelFormat::Gray16 => out[..2].copy_from_slice(&value.0[0].to_ne_bytes()),
	}
	out
}

fn decode_pixel(format: PixelFormat, px: &[u8]) -> PixelValue {
	match format {
		PixelFormat::Rgba8 => PixelValue::rgba8(px[0], px[1], px[2], px[3]),
		PixelFormat::Rgba16 => {
			let c = |i: usize| u16::from_ne_bytes([px[i * 2], px[i * 2 + 1]]);
			PixelValue::rgba16(c(0), c(1), c(2), c(3))
		}
		PixelFormat::Gray8 => PixelValue::gray16(px[0] as u16 * 257),
		PixelFormat::Gray16 => PixelValue::gray16(u16::from_ne_bytes([px[0], px[1]])),
	}
}

// ---------------------------------------------------------------------------
// Config, errors, stats
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TileStoreConfig {
	/// Max bytes of uncompressed tiles kept in RAM before trimming starts.
	pub hot_budget: u64,
	/// Max bytes of LZ4-compressed tiles kept in RAM before spilling to disk.
	pub warm_budget: u64,
	/// Directory for the scratch file. Must be on the fastest local disk.
	pub scratch_dir: PathBuf,
	/// Hard cap for the scratch file. Beyond it, inserting authoritative tiles
	/// fails with [`TileError::ScratchFull`] instead of filling the disk.
	pub scratch_limit: u64,
}

impl TileStoreConfig {
	/// Defaults for the reference machine (16 GB RAM, NVMe with <100 GB free).
	/// See docs/PERFORMANCE.md §2. Overridable from the preferences file.
	pub fn reference_machine(scratch_dir: PathBuf) -> Self {
		const GIB: u64 = 1 << 30;
		Self {
			hot_budget: 5 * GIB,
			warm_budget: 3 * GIB,
			scratch_dir,
			scratch_limit: 60 * GIB,
		}
	}

	/// Tiny budgets so tests exercise eviction with a handful of tiles.
	pub fn for_tests(scratch_dir: PathBuf) -> Self {
		Self {
			hot_budget: 4 * 512 * 1024,
			warm_budget: 4 * 512 * 1024,
			scratch_dir,
			scratch_limit: 1 << 30,
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum TileError {
	#[error("tile buffer has wrong size: expected {expected} bytes, got {got}")]
	WrongSize { expected: usize, got: usize },
	#[error("derived tile was evicted and must be regenerated")]
	Evicted,
	#[error("scratch disk limit reached ({limit} bytes)")]
	ScratchFull { limit: u64 },
	#[error("scratch file I/O error: {0}")]
	Io(#[from] std::io::Error),
	#[error("corrupted tile data: {0}")]
	Corrupt(String),
}

/// Whether a tile can be regenerated (and therefore dropped under pressure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileClass {
	/// Real document content (level-0 layer pixels, masks, undo data).
	/// Never lost: evicted to compressed RAM, then to the scratch file.
	Authoritative,
	/// Mip levels, composite caches, previews. Dropped under pressure.
	Derived,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileStoreStats {
	pub live_tiles: u64,
	pub hot_tiles: u64,
	pub hot_bytes: u64,
	pub warm_tiles: u64,
	pub warm_bytes: u64,
	pub cold_tiles: u64,
	pub cold_bytes: u64,
	pub evicted_tiles: u64,
}

// ---------------------------------------------------------------------------
// Entries and handles
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId(NonZeroU64);

impl TileId {
	pub fn get(self) -> u64 {
		self.0.get()
	}
}

enum Residency {
	Hot(Arc<TileBuffer>),
	/// M1-T03: LZ4 block of `format.tile_bytes()` bytes.
	#[allow(dead_code)]
	Warm(Box<[u8]>),
	/// M1-T04: location of a compressed block inside the scratch file.
	#[allow(dead_code)]
	Cold {
		offset: u64,
		len: u32,
	},
	/// Derived tile dropped under memory pressure.
	Evicted,
}

struct TileEntry {
	id: TileId,
	format: PixelFormat,
	class: TileClass,
	residency: Mutex<Residency>,
	/// Store clock value at last access; drives LRU trimming.
	last_use: AtomicU64,
	store: Weak<StoreInner>,
}

impl Drop for TileEntry {
	fn drop(&mut self) {
		let Some(store) = self.store.upgrade() else { return };
		let residency = std::mem::replace(self.residency.get_mut(), Residency::Evicted);
		store.account_remove(self.format, &residency);
		store.registry_shard(self.id).lock().remove(&self.id);
		// M1-T04: return the scratch-file extent of a Cold tile to the free list.
	}
}

/// Shared, reference-counted reference to one immutable tile.
/// Size: one pointer. Clone/drop: one atomic op.
#[derive(Clone)]
pub struct TileHandle(Arc<TileEntry>);

impl TileHandle {
	pub fn id(&self) -> TileId {
		self.0.id
	}

	pub fn format(&self) -> PixelFormat {
		self.0.format
	}

	pub fn class(&self) -> TileClass {
		self.0.class
	}

	/// Two handles point at the same tile (cheap identity check, used to skip
	/// work when a tile did not change between two snapshots).
	pub fn same_tile(&self, other: &TileHandle) -> bool {
		Arc::ptr_eq(&self.0, &other.0)
	}
}

impl std::fmt::Debug for TileHandle {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "TileHandle({}, {:?})", self.0.id.get(), self.0.format)
	}
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

const SHARDS: usize = 64;

struct StoreInner {
	config: TileStoreConfig,
	next_id: AtomicU64,
	clock: AtomicU64,
	registry: Vec<Mutex<HashMap<TileId, Weak<TileEntry>>>>,
	hot_tiles: AtomicU64,
	hot_bytes: AtomicU64,
	warm_tiles: AtomicU64,
	warm_bytes: AtomicU64,
	cold_tiles: AtomicU64,
	cold_bytes: AtomicU64,
	evicted_tiles: AtomicU64,
}

impl StoreInner {
	fn registry_shard(&self, id: TileId) -> &Mutex<HashMap<TileId, Weak<TileEntry>>> {
		&self.registry[(id.get() as usize) % SHARDS]
	}

	fn tick(&self) -> u64 {
		self.clock.fetch_add(1, Ordering::Relaxed)
	}

	fn account_add(&self, format: PixelFormat, residency: &Residency) {
		match residency {
			Residency::Hot(_) => {
				self.hot_tiles.fetch_add(1, Ordering::Relaxed);
				self.hot_bytes.fetch_add(format.tile_bytes() as u64, Ordering::Relaxed);
			}
			Residency::Warm(block) => {
				self.warm_tiles.fetch_add(1, Ordering::Relaxed);
				self.warm_bytes.fetch_add(block.len() as u64, Ordering::Relaxed);
			}
			Residency::Cold { len, .. } => {
				self.cold_tiles.fetch_add(1, Ordering::Relaxed);
				self.cold_bytes.fetch_add(*len as u64, Ordering::Relaxed);
			}
			Residency::Evicted => {
				self.evicted_tiles.fetch_add(1, Ordering::Relaxed);
			}
		}
	}

	fn account_remove(&self, format: PixelFormat, residency: &Residency) {
		match residency {
			Residency::Hot(_) => {
				self.hot_tiles.fetch_sub(1, Ordering::Relaxed);
				self.hot_bytes.fetch_sub(format.tile_bytes() as u64, Ordering::Relaxed);
			}
			Residency::Warm(block) => {
				self.warm_tiles.fetch_sub(1, Ordering::Relaxed);
				self.warm_bytes.fetch_sub(block.len() as u64, Ordering::Relaxed);
			}
			Residency::Cold { len, .. } => {
				self.cold_tiles.fetch_sub(1, Ordering::Relaxed);
				self.cold_bytes.fetch_sub(*len as u64, Ordering::Relaxed);
			}
			Residency::Evicted => {
				self.evicted_tiles.fetch_sub(1, Ordering::Relaxed);
			}
		}
	}
}

/// Thread-safe tile store. Cheap to clone (it is an `Arc`).
/// One store per process, shared by all open documents.
#[derive(Clone)]
pub struct TileStore(Arc<StoreInner>);

impl TileStore {
	pub fn new(config: TileStoreConfig) -> Result<Self, TileError> {
		// M1-T04: create/open the scratch file in `config.scratch_dir` here.
		Ok(Self(Arc::new(StoreInner {
			config,
			next_id: AtomicU64::new(1),
			clock: AtomicU64::new(0),
			registry: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
			hot_tiles: AtomicU64::new(0),
			hot_bytes: AtomicU64::new(0),
			warm_tiles: AtomicU64::new(0),
			warm_bytes: AtomicU64::new(0),
			cold_tiles: AtomicU64::new(0),
			cold_bytes: AtomicU64::new(0),
			evicted_tiles: AtomicU64::new(0),
		})))
	}

	pub fn config(&self) -> &TileStoreConfig {
		&self.0.config
	}

	/// Insert a tile. It starts hot. Callers should first check
	/// [`TileBuffer::uniform_value`] and store uniform tiles as
	/// `TileSlot::Empty`/`Solid` instead (see [`crate::TiledImage::put_buffer`]).
	pub fn insert(&self, buffer: TileBuffer, class: TileClass) -> TileHandle {
		let id = TileId(NonZeroU64::new(self.0.next_id.fetch_add(1, Ordering::Relaxed)).expect("tile id overflow"));
		let format = buffer.format();
		let residency = Residency::Hot(Arc::new(buffer));
		self.0.account_add(format, &residency);
		let entry = Arc::new(TileEntry {
			id,
			format,
			class,
			residency: Mutex::new(residency),
			last_use: AtomicU64::new(self.0.tick()),
			store: Arc::downgrade(&self.0),
		});
		self.0.registry_shard(id).lock().insert(id, Arc::downgrade(&entry));
		// M1-T04: if hot_bytes > hot_budget, wake the trimming thread (never trim inline here).
		TileHandle(entry)
	}

	/// Get the pixels of a tile, bringing it back to RAM if needed.
	///
	/// May block on decompression (warm) or disk I/O (cold). Never call this
	/// on the render thread for tiles that are not hot: use [`Self::try_get_hot`]
	/// there and schedule a load instead.
	pub fn get(&self, handle: &TileHandle) -> Result<Arc<TileBuffer>, TileError> {
		debug_assert!(handle.0.store.ptr_eq(&Arc::downgrade(&self.0)), "handle belongs to another store");
		handle.0.last_use.store(self.0.tick(), Ordering::Relaxed);
		let residency = handle.0.residency.lock();
		match &*residency {
			Residency::Hot(buffer) => Ok(buffer.clone()),
			Residency::Warm(_) => todo!("M1-T03: decompress, promote to Hot, fix accounting"),
			Residency::Cold { .. } => todo!("M1-T04: read from scratch file, decompress, promote to Hot"),
			Residency::Evicted => Err(TileError::Evicted),
		}
	}

	/// Non-blocking: the pixels if the tile is hot, otherwise `None`.
	pub fn try_get_hot(&self, handle: &TileHandle) -> Option<Arc<TileBuffer>> {
		let residency = handle.0.residency.try_lock()?;
		match &*residency {
			Residency::Hot(buffer) => {
				handle.0.last_use.store(self.0.tick(), Ordering::Relaxed);
				Some(buffer.clone())
			}
			_ => None,
		}
	}

	/// Bring RAM usage back under budget:
	/// 1. least-recently-used hot **derived** tiles → `Evicted`;
	/// 2. least-recently-used hot **authoritative** tiles → `Warm` (LZ4);
	/// 3. (M1-T04) oldest warm tiles → `Cold` (scratch file).
	///
	/// A hot tile whose `Arc<TileBuffer>` is still borrowed elsewhere
	/// (`Arc::strong_count > 1`) is skipped: someone is using it right now.
	///
	/// Normally run by the background trimming thread; public for tests and
	/// for `fx-cli bench`.
	pub fn trim(&self) {
		todo!("M1-T03: implement LRU trimming as documented above")
	}

	pub fn stats(&self) -> TileStoreStats {
		let s = &self.0;
		let hot_tiles = s.hot_tiles.load(Ordering::Relaxed);
		let warm_tiles = s.warm_tiles.load(Ordering::Relaxed);
		let cold_tiles = s.cold_tiles.load(Ordering::Relaxed);
		let evicted_tiles = s.evicted_tiles.load(Ordering::Relaxed);
		TileStoreStats {
			live_tiles: hot_tiles + warm_tiles + cold_tiles + evicted_tiles,
			hot_tiles,
			hot_bytes: s.hot_bytes.load(Ordering::Relaxed),
			warm_tiles,
			warm_bytes: s.warm_bytes.load(Ordering::Relaxed),
			cold_tiles,
			cold_bytes: s.cold_bytes.load(Ordering::Relaxed),
			evicted_tiles,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn store() -> TileStore {
		TileStore::new(TileStoreConfig::for_tests(std::env::temp_dir().join("fx-tiles-tests"))).unwrap()
	}

	fn noise(format: PixelFormat, seed: u8) -> TileBuffer {
		let mut buffer = TileBuffer::zeroed(format);
		for (i, b) in buffer.bytes_mut().iter_mut().enumerate() {
			*b = (i as u8).wrapping_mul(31).wrapping_add(seed);
		}
		buffer
	}

	#[test]
	fn insert_get_and_free() {
		let store = store();
		let handle = store.insert(noise(PixelFormat::Rgba8, 1), TileClass::Authoritative);
		assert_eq!(store.get(&handle).unwrap().bytes(), noise(PixelFormat::Rgba8, 1).bytes());
		assert_eq!(store.stats().hot_bytes, PixelFormat::Rgba8.tile_bytes() as u64);

		let copy = handle.clone();
		drop(handle);
		assert_eq!(store.stats().live_tiles, 1, "a clone keeps the tile alive");
		drop(copy);
		assert_eq!(store.stats(), TileStoreStats::default(), "last handle frees the tile");
	}

	#[test]
	fn uniform_detection() {
		let red = PixelValue::rgba8(255, 0, 0, 255);
		assert_eq!(TileBuffer::filled(PixelFormat::Rgba16, red).uniform_value(), Some(red));
		assert_eq!(TileBuffer::zeroed(PixelFormat::Rgba8).uniform_value(), Some(PixelValue::TRANSPARENT));
		assert_eq!(noise(PixelFormat::Rgba8, 3).uniform_value(), None);
	}

	#[test]
	fn wrong_size_rejected() {
		assert!(TileBuffer::from_bytes(PixelFormat::Rgba8, vec![0; 10].into_boxed_slice()).is_err());
	}

	// ---- Specification tests for upcoming tasks. Remove `#[ignore]` when implementing. ----

	#[test]
	#[ignore = "M1-T03"]
	fn trim_evicts_derived_and_compresses_authoritative() {
		let store = store(); // hot budget = 4 RGBA16 tiles
		let keep: Vec<_> = (0..6).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect();
		let derived: Vec<_> = (0..6).map(|i| store.insert(noise(PixelFormat::Rgba16, 100 + i), TileClass::Derived)).collect();
		store.trim();
		let stats = store.stats();
		assert!(stats.hot_bytes <= store.config().hot_budget);
		assert!(stats.evicted_tiles >= 1, "derived tiles go first");
		// authoritative content always comes back bit-exact
		for (i, handle) in keep.iter().enumerate() {
			assert_eq!(store.get(handle).unwrap().bytes(), noise(PixelFormat::Rgba16, i as u8).bytes());
		}
		// evicted derived tiles report Evicted, never wrong data
		for (i, handle) in derived.iter().enumerate() {
			match store.get(handle) {
				Ok(buffer) => assert_eq!(buffer.bytes(), noise(PixelFormat::Rgba16, 100 + i as u8).bytes()),
				Err(TileError::Evicted) => {}
				Err(e) => panic!("unexpected error {e}"),
			}
		}
	}

	#[test]
	#[ignore = "M1-T03"]
	fn trim_skips_tiles_in_use() {
		let store = store();
		let handles: Vec<_> = (0..8).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Derived)).collect();
		let borrowed = store.get(&handles[0]).unwrap(); // someone is reading tile 0
		store.trim();
		assert!(store.try_get_hot(&handles[0]).is_some(), "a borrowed tile must stay hot");
		drop(borrowed);
	}

	#[test]
	#[ignore = "M1-T04"]
	fn spill_to_scratch_and_back() {
		let store = store(); // warm budget is tiny too
		let handles: Vec<_> = (0..40).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect();
		store.trim();
		assert!(store.stats().cold_tiles > 0);
		for (i, handle) in handles.iter().enumerate() {
			assert_eq!(store.get(handle).unwrap().bytes(), noise(PixelFormat::Rgba16, i as u8).bytes());
		}
	}
}
