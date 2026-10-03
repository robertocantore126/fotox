//! `FOTOX_IO_STATS=1`: where a save's time goes (2026-10-03 baseline).
//!
//! * Phases measured on the threads that run them: thread time summed over
//!   threads (it can exceed wall time), bytes, calls.
//! * The size of every scratch read and every chunk payload written.
//! * How many scratch reads were in flight at once, and which [`ReadPool`]
//!   handle each read used.
//! * One-off wall-clock events of a save (collect, flush, rename, …).
//!
//! Off by default: then every hook costs one relaxed load. Nothing here
//! changes what is read, written or kept.
//!
//! [`ReadPool`]: crate::ReadPool

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub enum Phase {
	/// `get_streaming` on one tile: lock + whatever it took (all below).
	Get,
	/// Tile already hot: no read.
	HotHit,
	/// LZ4 decode of a warm (RAM-compressed) tile.
	WarmDecode,
	/// Positioned read of a cold tile's bytes from the scratch file.
	ScratchIo,
	/// CRC32 of a cold tile's compressed bytes.
	ScratchCrc,
	/// LZ4 decode of a cold tile.
	ColdDecode,
	/// Read + decode of a tile backed by an open `.fxd`.
	BackedRead,
	/// zstd compression of one tile.
	Compress,
	/// Building a chunk payload (8-byte tile header + copy of the bytes).
	ChunkBuild,
	/// CRC32 of a chunk payload.
	ChunkCrc,
	/// The positioned writes of one chunk (header + payload).
	ChunkWrite,
	/// Creating a zstd compression context.
	ZstdContext,
	/// Writer bookkeeping per chunk: chunk table insert, written list.
	WriteRefs,
	/// Freeing a compressed tile's buffer after it is written (bytes =
	/// its capacity).
	WriteDrop,
}

const PHASES: usize = 14;
const NAMES: [&str; PHASES] = [
	"get_streaming (per tile, total)",
	"  hot hit",
	"  warm: LZ4 decode",
	"  cold: scratch read I/O",
	"  cold: scratch CRC32",
	"  cold: LZ4 decode",
	"  backed .fxd read+decode",
	"zstd compress",
	"chunk payload build (copy)",
	"chunk CRC32",
	"chunk write (2 positioned writes)",
	"zstd context created",
	"writer: chunk table + written list",
	"writer: free compressed buffer (capacity)",
];

pub fn enabled() -> bool {
	static ON: OnceLock<bool> = OnceLock::new();
	*ON.get_or_init(|| std::env::var("FOTOX_IO_STATS").is_ok_and(|v| v == "1"))
}

static NANOS: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];
static BYTES: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];
static CALLS: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];
/// Sizes: 0 = scratch reads, 1 = chunk payloads written.
static SIZES: Mutex<[Vec<u32>; 2]> = Mutex::new([Vec::new(), Vec::new()]);
static EVENTS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static MAX_ACTIVE: AtomicUsize = AtomicUsize::new(0);
const SLOTS: usize = 17;
static SLOT_READS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static SINGLE_HANDLE_READS: AtomicU64 = AtomicU64::new(0);

/// A running measurement; [`Timer::stop`] records it.
pub struct Timer(Option<Instant>);

pub fn start() -> Timer {
	Timer(enabled().then(Instant::now))
}

impl Timer {
	pub fn stop(self, phase: Phase, bytes: usize) {
		if let Some(t) = self.0 {
			record(phase, t.elapsed().as_nanos() as u64, bytes);
		}
	}
}

pub fn record(phase: Phase, nanos: u64, bytes: usize) {
	if !enabled() {
		return;
	}
	let i = phase as usize;
	NANOS[i].fetch_add(nanos, Ordering::Relaxed);
	BYTES[i].fetch_add(bytes as u64, Ordering::Relaxed);
	CALLS[i].fetch_add(1, Ordering::Relaxed);
}

/// The size of one scratch read (`kind` 0) or one chunk payload (`kind` 1).
pub fn size(kind: usize, bytes: usize) {
	if enabled() {
		SIZES.lock().expect("iostats poisoned")[kind].push(bytes as u32);
	}
}

/// A wall-clock step of a save, in order.
pub fn event(name: &str, ms: f64) {
	if enabled() {
		EVENTS.lock().expect("iostats poisoned").push((name.to_owned(), ms));
	}
}

/// Count a scratch read in flight until the guard drops.
pub fn in_flight() -> Option<InFlight> {
	enabled().then(|| {
		let now = ACTIVE.fetch_add(1, Ordering::Relaxed) + 1;
		MAX_ACTIVE.fetch_max(now, Ordering::Relaxed);
		InFlight
	})
}

pub struct InFlight;

impl Drop for InFlight {
	fn drop(&mut self) {
		ACTIVE.fetch_sub(1, Ordering::Relaxed);
	}
}

/// A read went through pool handle `slot` (0 = the original) of a pool with
/// `handles` handles in all.
pub(crate) fn pool_read(slot: usize, handles: usize) {
	if enabled() {
		SLOT_READS[slot.min(SLOTS - 1)].fetch_add(1, Ordering::Relaxed);
		if handles == 1 {
			SINGLE_HANDLE_READS.fetch_add(1, Ordering::Relaxed);
		}
	}
}

fn percentile(sorted: &[u32], p: f64) -> u32 {
	if sorted.is_empty() {
		return 0;
	}
	sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Everything since the last call, as text; resets the counters.
pub fn take_report(wall_ms: f64) -> String {
	let mut out = String::new();
	let _ = writeln!(out, "  phase                                   thread-ms   %wall    calls      MiB   MiB/s/thread");
	for i in 0..PHASES {
		let calls = CALLS[i].swap(0, Ordering::Relaxed);
		let nanos = NANOS[i].swap(0, Ordering::Relaxed);
		let bytes = BYTES[i].swap(0, Ordering::Relaxed);
		if calls == 0 {
			continue;
		}
		let ms = nanos as f64 / 1e6;
		let mib = bytes as f64 / 1048576.0;
		let rate = if ms > 0.0 { mib / (ms / 1e3) } else { 0.0 };
		let _ = writeln!(
			out,
			"  {:<38} {:>10.1} {:>6.1}% {:>8} {:>8.1} {:>10.0}",
			NAMES[i],
			ms,
			100.0 * ms / wall_ms.max(1e-9),
			calls,
			mib,
			rate
		);
	}
	let sizes = std::mem::take(&mut *SIZES.lock().expect("iostats poisoned"));
	for (name, mut list) in ["scratch read size", "chunk payload size"].into_iter().zip(sizes) {
		if list.is_empty() {
			continue;
		}
		list.sort_unstable();
		let mean = list.iter().map(|&v| v as f64).sum::<f64>() / list.len() as f64;
		let _ = writeln!(
			out,
			"  {name}: n {}, mean {:.1} KiB, p50 {:.1}, p95 {:.1}, p99 {:.1}, max {:.1} KiB",
			list.len(),
			mean / 1024.0,
			percentile(&list, 0.5) as f64 / 1024.0,
			percentile(&list, 0.95) as f64 / 1024.0,
			percentile(&list, 0.99) as f64 / 1024.0,
			*list.last().expect("non-empty") as f64 / 1024.0
		);
	}
	let max = MAX_ACTIVE.swap(0, Ordering::Relaxed);
	let slots: Vec<u64> = SLOT_READS.iter().map(|s| s.swap(0, Ordering::Relaxed)).collect();
	let single = SINGLE_HANDLE_READS.swap(0, Ordering::Relaxed);
	let used: Vec<String> = slots.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("{i}:{n}")).collect();
	let _ = writeln!(
		out,
		"  scratch reads in flight at once: max {max}; reads per pool handle (0 = original) [{}]; reads with no extra handle {single}",
		used.join(" ")
	);
	let events = std::mem::take(&mut *EVENTS.lock().expect("iostats poisoned"));
	for (name, ms) in events {
		let _ = writeln!(out, "  wall {name:<36} {ms:>10.1} ms {:>6.1}%", 100.0 * ms / wall_ms.max(1e-9));
	}
	out
}
