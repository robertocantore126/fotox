//! Read profiler (`FOTOX_READ_STATS=1`): counts the tile reads that leave
//! RAM's hot tier — a warm tile decompressed, a cold one read from the
//! scratch file — by tier and class, and samples who asked (one backtrace in
//! [`SAMPLE`] reads, reduced to the first Fotox functions above the store).
//! Off by default: then [`record`] is one relaxed load.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::store::TileClass;

/// One backtrace per this many reads.
const SAMPLE: u64 = 256;

fn enabled() -> bool {
	static ON: OnceLock<bool> = OnceLock::new();
	*ON.get_or_init(|| std::env::var_os("FOTOX_READ_STATS").is_some())
}

#[derive(Default)]
struct Stats {
	/// (tier, class) → (reads, bytes decompressed).
	counts: HashMap<(&'static str, &'static str), (u64, u64)>,
	/// Caller signature → sampled reads.
	callers: HashMap<String, u64>,
}

static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
static READS: AtomicU64 = AtomicU64::new(0);

fn stats() -> &'static Mutex<Stats> {
	STATS.get_or_init(|| Mutex::new(Stats::default()))
}

/// The first Fotox functions of the current stack, outside the store.
fn caller() -> String {
	let trace = std::backtrace::Backtrace::force_capture().to_string();
	let mut frames = Vec::new();
	for line in trace.lines() {
		let line = line.trim();
		// "12: fx_engine::mips::ensure_mip" (not the "at file:line" lines).
		let Some((index, symbol)) = line.split_once(": ") else { continue };
		if index.parse::<u32>().is_err() {
			continue;
		}
		let symbol = symbol.split("::h").next().unwrap_or(symbol);
		if !symbol.starts_with("fx_") || symbol.starts_with("fx_tiles::store") || symbol.starts_with("fx_tiles::readstats") {
			continue;
		}
		let short: String = symbol.split('<').next().unwrap_or(symbol).to_string();
		if frames.last() != Some(&short) {
			frames.push(short);
		}
		if frames.len() == 4 {
			break;
		}
	}
	if frames.is_empty() { "(no Fotox frame)".into() } else { frames.join(" <- ") }
}

/// A read that left the hot tier.
pub(crate) fn record(tier: &'static str, class: TileClass, bytes: usize) {
	if !enabled() {
		return;
	}
	let class = match class {
		TileClass::Authoritative => "pixels",
		TileClass::Derived => "derived",
	};
	let n = READS.fetch_add(1, Ordering::Relaxed);
	let who = (n % SAMPLE == 0).then(caller);
	let mut s = stats().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
	let c = s.counts.entry((tier, class)).or_default();
	c.0 += 1;
	c.1 += bytes as u64;
	if let Some(who) = who {
		*s.callers.entry(who).or_default() += 1;
	}
}

/// What was recorded since the last call, then reset. Empty when off.
pub fn take_report() -> String {
	if !enabled() {
		return String::new();
	}
	let s = std::mem::take(&mut *stats().lock().unwrap_or_else(std::sync::PoisonError::into_inner));
	let mut out = String::new();
	let mut counts: Vec<_> = s.counts.into_iter().collect();
	counts.sort();
	for ((tier, class), (reads, bytes)) in counts {
		out.push_str(&format!("  reads {tier:<5} {class:<8} {reads:>9} tiles {:>8} MiB\n", bytes >> 20));
	}
	let total: u64 = s.callers.values().sum();
	let mut callers: Vec<_> = s.callers.into_iter().collect();
	callers.sort_by(|a, b| b.1.cmp(&a.1));
	for (who, n) in callers.into_iter().take(8) {
		out.push_str(&format!("  {:>5.1} %  {who}\n", 100.0 * n as f64 / total.max(1) as f64));
	}
	out
}
