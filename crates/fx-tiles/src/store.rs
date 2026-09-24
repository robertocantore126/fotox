//! The tile store: ownership, reference counting and residency of tiles.
//!
//! STATUS: complete for M1 (hot / warm / cold tiers, LRU trim, background
//! trim thread, scratch file). Written by Claude; M3 adds a `backed` copy
//! (tiles that live in an opened native file).
//!
//! Design: a [`TileHandle`] is an `Arc<TileEntry>`. Cloning/dropping a handle
//! is a single atomic operation, so snapshots of whole layers (tens of
//! thousands of handles) are cheap. The store keeps a *weak* registry of all
//! entries so the trimming code can find eviction candidates.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex as StdMutex, Weak};
use std::time::Duration;

use parking_lot::Mutex;

use crate::format::{PixelFormat, PixelValue};
use crate::scratch::{Extent, ScratchFile};

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
	/// Hard cap for the scratch file. When it is reached, tiles simply stay in
	/// RAM over budget and [`TileStoreStats::scratch_full`] is set; nothing is
	/// ever dropped or corrupted.
	pub scratch_limit: u64,
	/// Run the background trim thread. Tests turn it off to be deterministic
	/// and call [`TileStore::trim`] themselves.
	pub background_trim: bool,
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
			background_trim: true,
		}
	}

	/// Tiny budgets (4 RGBA16 tiles each) so tests exercise every tier with a
	/// handful of tiles. No background thread.
	pub fn for_tests(scratch_dir: PathBuf) -> Self {
		Self {
			hot_budget: 4 * 512 * 1024,
			warm_budget: 4 * 512 * 1024,
			scratch_dir,
			scratch_limit: 1 << 30,
			background_trim: false,
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum TileError {
	#[error("tile buffer has wrong size: expected {expected} bytes, got {got}")]
	WrongSize { expected: usize, got: usize },
	#[error("derived tile was evicted and must be regenerated")]
	Evicted,
	#[error("scratch file I/O error: {0}")]
	Io(#[from] std::io::Error),
	#[error("corrupted tile data: {0}")]
	Corrupt(String),
}

/// Whether a tile can be regenerated (and therefore dropped under pressure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileClass {
	/// Real document content (level-0 layer pixels, masks, undo data).
	/// Never lost: compressed in RAM, then written to the scratch file.
	Authoritative,
	/// Mip levels, composite caches, previews. Dropped under pressure.
	Derived,
}

/// Counters of the store. A tile can have several copies at once (e.g. hot
/// *and* cold after being read back from disk), so tile counts per tier do
/// not add up to `live_tiles`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileStoreStats {
	/// Tiles with at least one live handle.
	pub live_tiles: u64,
	pub hot_tiles: u64,
	pub hot_bytes: u64,
	pub warm_tiles: u64,
	pub warm_bytes: u64,
	pub cold_tiles: u64,
	/// Exact compressed bytes of cold tiles (allocation overhead not included).
	pub cold_bytes: u64,
	/// Derived tiles that were dropped and must be regenerated.
	pub evicted_tiles: u64,
	/// The scratch limit was hit during the last trim: RAM is over budget.
	pub scratch_full: bool,
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

/// The copies of one tile that currently exist. Tiles are immutable, so every
/// copy that exists is valid forever: keeping the cold copy after reading a
/// tile back means evicting it again later costs nothing.
///
/// Invariant: an authoritative tile always has at least one copy.
#[derive(Default)]
struct Copies {
	hot: Option<Arc<TileBuffer>>,
	/// LZ4 block of `format.tile_bytes()` bytes.
	warm: Option<Arc<[u8]>>,
	cold: Option<Extent>,
}

impl Copies {
	fn is_empty(&self) -> bool {
		self.hot.is_none() && self.warm.is_none() && self.cold.is_none()
	}
}

struct TileEntry {
	id: TileId,
	format: PixelFormat,
	class: TileClass,
	copies: Mutex<Copies>,
	/// Store clock value at last access; drives LRU trimming.
	last_use: AtomicU64,
	store: Weak<StoreInner>,
}

impl Drop for TileEntry {
	fn drop(&mut self) {
		let Some(store) = self.store.upgrade() else { return };
		let copies = std::mem::take(self.copies.get_mut());
		store.stats.live_tiles.fetch_sub(1, Ordering::Relaxed);
		store.account_hot(self.format, copies.hot.is_some(), false);
		if let Some(block) = &copies.warm {
			store.account_warm(block.len(), false);
		}
		if let Some(extent) = copies.cold {
			store.account_cold(extent, false);
			if let Some(scratch) = &store.scratch {
				scratch.free(extent);
			}
		}
		if copies.is_empty() {
			store.stats.evicted_tiles.fetch_sub(1, Ordering::Relaxed);
		}
		store.registry_shard(self.id).lock().remove(&self.id);
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

#[derive(Default)]
struct Counters {
	live_tiles: AtomicU64,
	hot_tiles: AtomicU64,
	hot_bytes: AtomicU64,
	warm_tiles: AtomicU64,
	warm_bytes: AtomicU64,
	cold_tiles: AtomicU64,
	cold_bytes: AtomicU64,
	evicted_tiles: AtomicU64,
	scratch_full: AtomicBool,
}

/// Wakes the background trim thread.
#[derive(Default)]
struct TrimSignal {
	pending: StdMutex<bool>,
	condvar: Condvar,
}

impl TrimSignal {
	fn notify(&self) {
		let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
		if !*pending {
			*pending = true;
			self.condvar.notify_one();
		}
	}

	/// Wait for a notification or the timeout.
	fn wait(&self, timeout: Duration) {
		let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
		let (mut pending, _) = self.condvar.wait_timeout_while(pending, timeout, |p| !*p).unwrap_or_else(|e| e.into_inner());
		*pending = false;
	}
}

struct StoreInner {
	config: TileStoreConfig,
	next_id: AtomicU64,
	clock: AtomicU64,
	registry: Vec<Mutex<HashMap<TileId, Weak<TileEntry>>>>,
	stats: Counters,
	/// `None` if the scratch file could not be created: then nothing spills
	/// to disk and RAM goes over budget (reported via `scratch_full`).
	scratch: Option<ScratchFile>,
	/// Only one trim at a time (background thread vs explicit calls).
	trim_lock: Mutex<()>,
	signal: Arc<TrimSignal>,
}

impl Drop for StoreInner {
	fn drop(&mut self) {
		// Wake the trim thread so it notices the store is gone and exits.
		self.signal.notify();
	}
}

impl StoreInner {
	fn registry_shard(&self, id: TileId) -> &Mutex<HashMap<TileId, Weak<TileEntry>>> {
		&self.registry[(id.get() as usize) % SHARDS]
	}

	fn tick(&self) -> u64 {
		self.clock.fetch_add(1, Ordering::Relaxed)
	}

	fn account_hot(&self, format: PixelFormat, present: bool, add: bool) {
		if !present {
			return;
		}
		let bytes = format.tile_bytes() as u64;
		if add {
			self.stats.hot_tiles.fetch_add(1, Ordering::Relaxed);
			self.stats.hot_bytes.fetch_add(bytes, Ordering::Relaxed);
		} else {
			self.stats.hot_tiles.fetch_sub(1, Ordering::Relaxed);
			self.stats.hot_bytes.fetch_sub(bytes, Ordering::Relaxed);
		}
	}

	fn account_warm(&self, len: usize, add: bool) {
		if add {
			self.stats.warm_tiles.fetch_add(1, Ordering::Relaxed);
			self.stats.warm_bytes.fetch_add(len as u64, Ordering::Relaxed);
		} else {
			self.stats.warm_tiles.fetch_sub(1, Ordering::Relaxed);
			self.stats.warm_bytes.fetch_sub(len as u64, Ordering::Relaxed);
		}
	}

	fn account_cold(&self, extent: Extent, add: bool) {
		if add {
			self.stats.cold_tiles.fetch_add(1, Ordering::Relaxed);
			self.stats.cold_bytes.fetch_add(extent.len as u64, Ordering::Relaxed);
		} else {
			self.stats.cold_tiles.fetch_sub(1, Ordering::Relaxed);
			self.stats.cold_bytes.fetch_sub(extent.len as u64, Ordering::Relaxed);
		}
	}

	fn over_budget(&self) -> bool {
		self.stats.hot_bytes.load(Ordering::Relaxed) > self.config.hot_budget || self.stats.warm_bytes.load(Ordering::Relaxed) > self.config.warm_budget
	}

	/// Snapshot of all live entries (weak refs upgraded). Shards are locked one
	/// at a time and only while copying pointers.
	fn live_entries(&self) -> Vec<Arc<TileEntry>> {
		let mut out = Vec::new();
		for shard in &self.registry {
			out.extend(shard.lock().values().filter_map(Weak::upgrade));
		}
		out
	}
}

/// Thread-safe tile store. Cheap to clone (it is an `Arc`).
/// One store per process, shared by all open documents.
///
/// Locking rules (reviewers: check these on every change):
/// * Each tile has its own small mutex (`copies`). It is held while *that
///   tile* is decompressed or read from disk in [`TileStore::get`], so two
///   readers of the same tile do not both do the work. It is never held while
///   another tile's lock or a registry shard lock is taken.
/// * Trimming compresses and writes to disk **without** holding the tile's
///   lock: it clones the copy it needs, releases the lock, does the work, then
///   re-locks and installs the result only if nothing changed meanwhile.
/// * Registry shard locks are held only to insert/remove/copy pointers.
#[derive(Clone)]
pub struct TileStore(Arc<StoreInner>);

impl TileStore {
	pub fn new(config: TileStoreConfig) -> Result<Self, TileError> {
		let scratch = match ScratchFile::create(&config.scratch_dir, config.scratch_limit) {
			Ok(file) => Some(file),
			Err(e) => {
				tracing::error!("cannot create scratch file in {:?}: {e}; tiles will stay in RAM", config.scratch_dir);
				None
			}
		};
		let signal = Arc::new(TrimSignal::default());
		let inner = Arc::new(StoreInner {
			next_id: AtomicU64::new(1),
			clock: AtomicU64::new(0),
			registry: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
			stats: Counters::default(),
			scratch,
			trim_lock: Mutex::new(()),
			signal: signal.clone(),
			config,
		});
		if inner.config.background_trim {
			let weak = Arc::downgrade(&inner);
			std::thread::Builder::new()
				.name("fx-tile-trim".into())
				.spawn(move || trim_thread(weak, signal))
				.map_err(TileError::Io)?;
		}
		Ok(Self(inner))
	}

	pub fn config(&self) -> &TileStoreConfig {
		&self.0.config
	}

	/// Insert a tile. It starts hot. Callers should first check
	/// [`TileBuffer::uniform_value`] and store uniform tiles as
	/// `TileSlot::Empty`/`Solid` instead (see [`crate::TiledImage::put_buffer`]).
	/// Never blocks on trimming: if RAM goes over budget, the background
	/// thread is woken.
	pub fn insert(&self, buffer: TileBuffer, class: TileClass) -> TileHandle {
		let id = TileId(NonZeroU64::new(self.0.next_id.fetch_add(1, Ordering::Relaxed)).expect("tile id overflow"));
		let format = buffer.format();
		self.0.stats.live_tiles.fetch_add(1, Ordering::Relaxed);
		self.0.account_hot(format, true, true);
		let entry = Arc::new(TileEntry {
			id,
			format,
			class,
			copies: Mutex::new(Copies {
				hot: Some(Arc::new(buffer)),
				..Default::default()
			}),
			last_use: AtomicU64::new(self.0.tick()),
			store: Arc::downgrade(&self.0),
		});
		self.0.registry_shard(id).lock().insert(id, Arc::downgrade(&entry));
		if self.0.over_budget() {
			self.0.signal.notify();
		}
		TileHandle(entry)
	}

	/// Get the pixels of a tile, bringing it back to RAM if needed.
	///
	/// May block on decompression (warm) or disk I/O (cold). Never call this
	/// on the render thread for tiles that are not hot: use [`Self::try_get_hot`]
	/// there and schedule a load instead.
	pub fn get(&self, handle: &TileHandle) -> Result<Arc<TileBuffer>, TileError> {
		let entry = &handle.0;
		debug_assert!(entry.store.ptr_eq(&Arc::downgrade(&self.0)), "handle belongs to another store");
		entry.last_use.store(self.0.tick(), Ordering::Relaxed);
		let mut copies = entry.copies.lock();
		if let Some(buffer) = &copies.hot {
			return Ok(buffer.clone());
		}
		let tile_bytes = entry.format.tile_bytes();
		let decompressed = if let Some(block) = &copies.warm {
			decompress(block, tile_bytes)?
		} else if let Some(extent) = copies.cold {
			let scratch = self
				.0
				.scratch
				.as_ref()
				.ok_or_else(|| TileError::Corrupt("cold tile without scratch file".into()))?;
			let block = scratch.read(extent)?;
			decompress(&block, tile_bytes)?
		} else {
			return Err(TileError::Evicted);
		};
		let buffer = Arc::new(TileBuffer::from_bytes(entry.format, decompressed.into_boxed_slice())?);
		copies.hot = Some(buffer.clone());
		self.0.account_hot(entry.format, true, true);
		drop(copies);
		if self.0.over_budget() {
			self.0.signal.notify();
		}
		Ok(buffer)
	}

	/// Non-blocking: the pixels if the tile is hot, otherwise `None`.
	pub fn try_get_hot(&self, handle: &TileHandle) -> Option<Arc<TileBuffer>> {
		let copies = handle.0.copies.try_lock()?;
		let buffer = copies.hot.clone()?;
		handle.0.last_use.store(self.0.tick(), Ordering::Relaxed);
		Some(buffer)
	}

	/// True if [`Self::get`] would not block (the tile is hot).
	pub fn is_hot(&self, handle: &TileHandle) -> bool {
		handle.0.copies.try_lock().is_some_and(|c| c.hot.is_some())
	}

	/// Bring RAM usage back under budget:
	/// 1. least-recently-used hot **derived** tiles → dropped (`Evicted`);
	/// 2. least-recently-used hot **authoritative** tiles → LZ4 (`warm`), or
	///    simply dropped from RAM if a warm/cold copy already exists;
	/// 3. oldest warm tiles → scratch file (`cold`), or dropped from RAM if a
	///    cold copy already exists.
	///
	/// Each tier is trimmed to 90 % of its budget (hysteresis, so the
	/// background thread does not run constantly). A hot tile whose
	/// `Arc<TileBuffer>` is still borrowed elsewhere is skipped: someone is
	/// using it right now.
	///
	/// Normally run by the background trim thread; public for tests and
	/// benchmarks. Cost: one scan over all live tiles.
	pub fn trim(&self) {
		let inner = &*self.0;
		let _guard = inner.trim_lock.lock();
		let hot_target = inner.config.hot_budget / 10 * 9;
		let warm_target = inner.config.warm_budget / 10 * 9;
		let hot_over = inner.stats.hot_bytes.load(Ordering::Relaxed) > inner.config.hot_budget;
		let warm_over = inner.stats.warm_bytes.load(Ordering::Relaxed) > inner.config.warm_budget;
		if !hot_over && !warm_over {
			return;
		}

		// Snapshot `last_use` before sorting: readers keep updating it, and
		// sorting by a key that changes mid-sort is not a total order (std
		// panics on that). A slightly stale LRU order is fine.
		let mut keyed: Vec<(u64, Arc<TileEntry>)> = inner.live_entries().into_iter().map(|e| (e.last_use.load(Ordering::Relaxed), e)).collect();
		keyed.sort_unstable_by_key(|(key, _)| *key);
		let entries: Vec<Arc<TileEntry>> = keyed.into_iter().map(|(_, e)| e).collect();

		if hot_over {
			// Derived tiles first: dropping them is free.
			for pass_class in [TileClass::Derived, TileClass::Authoritative] {
				for entry in entries.iter().filter(|e| e.class == pass_class) {
					if inner.stats.hot_bytes.load(Ordering::Relaxed) <= hot_target {
						break;
					}
					self.demote_hot(entry);
				}
			}
		}

		// Demoting hot tiles may have pushed warm over budget.
		if inner.stats.warm_bytes.load(Ordering::Relaxed) > inner.config.warm_budget {
			let mut scratch_full = false;
			for entry in &entries {
				if inner.stats.warm_bytes.load(Ordering::Relaxed) <= warm_target {
					break;
				}
				if !self.demote_warm(entry) {
					scratch_full = true;
					break;
				}
			}
			inner.stats.scratch_full.store(scratch_full, Ordering::Relaxed);
		} else {
			inner.stats.scratch_full.store(false, Ordering::Relaxed);
		}
	}

	/// Remove the hot copy of one tile (compressing it first if it is the
	/// only copy of an authoritative tile).
	fn demote_hot(&self, entry: &Arc<TileEntry>) {
		let inner = &*self.0;
		let (buffer, seen) = {
			let mut copies = entry.copies.lock();
			let Some(buffer) = copies.hot.clone() else { return };
			// Ours + the entry's = 2. More means someone holds the pixels right now.
			if Arc::strong_count(&buffer) > 2 {
				return;
			}
			if entry.class == TileClass::Derived || copies.warm.is_some() || copies.cold.is_some() {
				copies.hot = None;
				inner.account_hot(entry.format, true, false);
				if copies.is_empty() {
					inner.stats.evicted_tiles.fetch_add(1, Ordering::Relaxed);
				}
				return;
			}
			(buffer, entry.last_use.load(Ordering::Relaxed))
		};

		// Compress without holding the tile's lock.
		let block: Arc<[u8]> = lz4_flex::block::compress(buffer.bytes()).into();

		let mut copies = entry.copies.lock();
		let unchanged = copies.hot.as_ref().is_some_and(|b| Arc::ptr_eq(b, &buffer)) && entry.last_use.load(Ordering::Relaxed) == seen;
		// Two strong refs: `buffer` (ours) and the entry's.
		if !unchanged || Arc::strong_count(&buffer) > 2 {
			return; // used meanwhile: it is not least-recently-used any more
		}
		inner.account_warm(block.len(), true);
		copies.warm = Some(block);
		copies.hot = None;
		inner.account_hot(entry.format, true, false);
	}

	/// Remove the warm copy of one tile (writing it to scratch first if it is
	/// the only copy). Returns `false` if the scratch file is full.
	fn demote_warm(&self, entry: &Arc<TileEntry>) -> bool {
		let inner = &*self.0;
		let block = {
			let mut copies = entry.copies.lock();
			let Some(block) = copies.warm.clone() else { return true };
			if copies.cold.is_some() {
				copies.warm = None;
				inner.account_warm(block.len(), false);
				return true;
			}
			block
		};
		let Some(scratch) = &inner.scratch else { return false };

		// Write without holding the tile's lock.
		let extent = match scratch.write(&block) {
			Ok(Some(extent)) => extent,
			Ok(None) => return false,
			Err(e) => {
				tracing::error!("scratch write failed: {e}");
				return false;
			}
		};

		let mut copies = entry.copies.lock();
		if copies.cold.is_some() {
			// Cannot happen while trims are serialised, but never leak space.
			scratch.free(extent);
			return true;
		}
		inner.account_cold(extent, true);
		copies.cold = Some(extent);
		if copies.warm.as_ref().is_some_and(|w| Arc::ptr_eq(w, &block)) {
			copies.warm = None;
			inner.account_warm(block.len(), false);
		}
		true
	}

	pub fn stats(&self) -> TileStoreStats {
		let s = &self.0.stats;
		TileStoreStats {
			live_tiles: s.live_tiles.load(Ordering::Relaxed),
			hot_tiles: s.hot_tiles.load(Ordering::Relaxed),
			hot_bytes: s.hot_bytes.load(Ordering::Relaxed),
			warm_tiles: s.warm_tiles.load(Ordering::Relaxed),
			warm_bytes: s.warm_bytes.load(Ordering::Relaxed),
			cold_tiles: s.cold_tiles.load(Ordering::Relaxed),
			cold_bytes: s.cold_bytes.load(Ordering::Relaxed),
			evicted_tiles: s.evicted_tiles.load(Ordering::Relaxed),
			scratch_full: s.scratch_full.load(Ordering::Relaxed),
		}
	}

	/// Bytes currently allocated in the scratch file.
	pub fn scratch_used(&self) -> u64 {
		self.0.scratch.as_ref().map_or(0, ScratchFile::used)
	}
}

fn decompress(block: &[u8], tile_bytes: usize) -> Result<Vec<u8>, TileError> {
	let out = lz4_flex::block::decompress(block, tile_bytes).map_err(|e| TileError::Corrupt(e.to_string()))?;
	if out.len() != tile_bytes {
		return Err(TileError::Corrupt(format!("decompressed {} bytes, expected {tile_bytes}", out.len())));
	}
	Ok(out)
}

/// Background trimming: runs when woken by an over-budget insert/get, and
/// every 250 ms. Holds only a weak reference, so it exits once the store is
/// dropped (never joined: the last strong reference may be dropped by this
/// very thread).
fn trim_thread(store: Weak<StoreInner>, signal: Arc<TrimSignal>) {
	loop {
		signal.wait(Duration::from_millis(250));
		let Some(inner) = store.upgrade() else { return };
		if inner.over_budget() {
			TileStore(inner).trim();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn store() -> TileStore {
		TileStore::new(TileStoreConfig::for_tests(std::env::temp_dir().join("fx-tiles-tests"))).unwrap()
	}

	/// Incompressible pseudo-random content (xorshift), like photographic noise.
	fn noise(format: PixelFormat, seed: u8) -> TileBuffer {
		let mut buffer = TileBuffer::zeroed(format);
		let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ (seed as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
		for chunk in buffer.bytes_mut().chunks_exact_mut(8) {
			state ^= state << 13;
			state ^= state >> 7;
			state ^= state << 17;
			chunk.copy_from_slice(&state.to_le_bytes());
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

	#[test]
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
	fn trim_skips_tiles_in_use() {
		let store = store();
		let handles: Vec<_> = (0..8).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Derived)).collect();
		let borrowed = store.get(&handles[0]).unwrap(); // someone is reading tile 0
		store.trim();
		assert!(store.try_get_hot(&handles[0]).is_some(), "a borrowed tile must stay hot");
		drop(borrowed);
	}

	#[test]
	fn spill_to_scratch_and_back() {
		let store = store(); // warm budget is tiny too
		let handles: Vec<_> = (0..40).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect();
		store.trim();
		assert!(store.stats().cold_tiles > 0);
		for (i, handle) in handles.iter().enumerate() {
			assert_eq!(store.get(handle).unwrap().bytes(), noise(PixelFormat::Rgba16, i as u8).bytes());
		}
	}

	#[test]
	fn accounting_is_exact_through_all_tiers() {
		let store = store();
		let handles: Vec<_> = (0..40).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect();
		store.trim();
		// read half back (hot again, cold copy kept), trim again
		for h in handles.iter().step_by(2) {
			store.get(h).unwrap();
		}
		store.trim();
		let stats = store.stats();
		assert!(stats.hot_bytes <= store.config().hot_budget);
		assert!(stats.warm_bytes <= store.config().warm_budget);
		assert_eq!(
			stats.cold_bytes,
			handles.iter().map(|h| h.0.copies.lock().cold.map_or(0, |e| e.len as u64)).sum::<u64>()
		);
		assert!(store.scratch_used() > 0);
		drop(handles);
		let stats = store.stats();
		assert_eq!(stats, TileStoreStats::default(), "everything freed: {stats:?}");
		assert_eq!(store.scratch_used(), 0, "all scratch extents returned");
	}

	#[test]
	fn scratch_full_keeps_data_in_ram() {
		let mut config = TileStoreConfig::for_tests(std::env::temp_dir().join("fx-tiles-tests"));
		config.scratch_limit = 3 * 1024 * 1024; // room for ~5 compressed noise tiles
		let store = TileStore::new(config).unwrap();
		let handles: Vec<_> = (0..30).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect();
		store.trim();
		assert!(store.stats().scratch_full);
		for (i, handle) in handles.iter().enumerate() {
			assert_eq!(store.get(handle).unwrap().bytes(), noise(PixelFormat::Rgba16, i as u8).bytes(), "tile {i}");
		}
	}

	/// Readers on 8 threads while two trimmers run: no deadlock, every read exact.
	#[test]
	fn concurrent_get_and_trim() {
		let store = store();
		let handles: Arc<Vec<_>> = Arc::new((0..64).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect());
		let expected: Arc<Vec<Vec<u8>>> = Arc::new((0..64).map(|i| noise(PixelFormat::Rgba16, i).bytes().to_vec()).collect());
		let stop = Arc::new(AtomicBool::new(false));
		let mut threads = Vec::new();
		for t in 0..8 {
			let (store, handles, expected, stop) = (store.clone(), handles.clone(), expected.clone(), stop.clone());
			threads.push(std::thread::spawn(move || {
				let mut i = t * 7;
				let mut reads = 0u64;
				while !stop.load(Ordering::Relaxed) {
					let k = i % handles.len();
					assert_eq!(store.get(&handles[k]).unwrap().bytes(), &expected[k][..]);
					i += 13;
					reads += 1;
				}
				reads
			}));
		}
		for _ in 0..2 {
			let (store, stop) = (store.clone(), stop.clone());
			threads.push(std::thread::spawn(move || {
				while !stop.load(Ordering::Relaxed) {
					store.trim();
				}
				0
			}));
		}
		std::thread::sleep(Duration::from_millis(1500));
		stop.store(true, Ordering::Relaxed);
		let reads: u64 = threads.into_iter().map(|t| t.join().unwrap()).sum();
		assert!(reads > 100, "readers made progress ({reads} reads)");
	}

	#[test]
	fn background_thread_trims_and_exits() {
		let mut config = TileStoreConfig::for_tests(std::env::temp_dir().join("fx-tiles-tests"));
		config.background_trim = true;
		let store = TileStore::new(config).unwrap();
		let handles: Vec<_> = (0..20).map(|i| store.insert(noise(PixelFormat::Rgba16, i), TileClass::Authoritative)).collect();
		let deadline = std::time::Instant::now() + Duration::from_secs(5);
		while store.stats().hot_bytes > store.config().hot_budget {
			assert!(std::time::Instant::now() < deadline, "background trim did not run");
			std::thread::sleep(Duration::from_millis(10));
		}
		drop(handles);
		drop(store); // the thread must notice and exit (no hang at test end)
	}
}
