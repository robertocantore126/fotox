//! Audit 2026-10-01 — the Smart Object sampler's lock: `LazyMips::read`
//! holds the source image's `Mutex` while `ensure_mip` runs a rayon
//! `par_iter`; the caller (`fx_ops::resample::resample`) is itself a rayon
//! `par_iter` over destination tiles that each call `LazyMips::tile`. A
//! worker that waits inside the inner `par_iter` may steal an outer task,
//! which then locks the mutex its own thread already holds.
//!
//! This drives exactly that pair (the code path of `smart::draw`) on a
//! small dedicated pool, each attempt on a watched thread: an attempt that
//! does not finish in 30 s is reported as a hang.
//!
//!   cargo test --release -p fx-engine --test audit_lazy_mips -- --ignored --nocapture

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fx_engine::mips::LazyMips;
use fx_ops::resample::SourceInfo;
use fx_tiles::{PixelFormat, TileBuffer, TileStore, TileStoreConfig, TiledImage};

#[test]
#[ignore = "audit: may hang (the point); runs on watched threads"]
fn sampler_over_lazy_mips_does_not_deadlock() {
	let dir = std::env::temp_dir().join(format!("fx-audit-lazy-{}", std::process::id()));
	let mut config = TileStoreConfig::for_tests(dir.join("scratch"));
	config.hot_budget = 4 << 30;
	let store = TileStore::new(config).unwrap();
	let side = 8192u32;
	let attempts: usize = std::env::var("FOTOX_AUDIT_ATTEMPTS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
	let threads: usize = std::env::var("FOTOX_AUDIT_POOL").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
	let mut hangs = 0;
	let mut times = Vec::new();
	for attempt in 0..attempts {
		// A fresh composite each time: every mip is dirty, as after Convert.
		let mut image = TiledImage::new(side, side, PixelFormat::Rgba8);
		let mut seed = attempt as u64 + 1;
		for ty in 0..side / 256 {
			for tx in 0..side / 256 {
				let mut b = TileBuffer::zeroed(PixelFormat::Rgba8);
				for c in b.bytes_mut().chunks_mut(8) {
					seed ^= seed << 13;
					seed ^= seed >> 7;
					seed ^= seed << 17;
					c.copy_from_slice(&seed.to_le_bytes());
				}
				image.put_buffer(&store, tx, ty, b);
			}
		}
		let levels = image.level_count();
		let composite = Arc::new(Mutex::new(image));
		let (store2, composite2) = (store.clone(), composite.clone());
		let (done_tx, done_rx) = std::sync::mpsc::channel();
		let started = Instant::now();
		std::thread::spawn(move || {
			let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
			let result = pool.install(|| {
				let view = LazyMips::new(&composite2, &store2);
				// An instance scaled to 9 %: destination level 0 reads source level 3.
				let mapping = fx_core::Mapping::Affine([0.09, 0.0, 0.0, 0.09, 10.0, 10.0]);
				let dst: Vec<(u32, u32)> = (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).collect();
				fx_ops::resample::resample(&view, SourceInfo { size: (side, side), levels }, mapping, fx_core::Filter::Bicubic, 0, &dst).map(|v| v.len())
			});
			let _ = done_tx.send(result.map_err(|e| e.to_string()));
		});
		match done_rx.recv_timeout(Duration::from_secs(30)) {
			Ok(result) => {
				times.push(started.elapsed());
				println!("AUDIT lazy-mips attempt {attempt}: {:?} in {:.0} ms", result, started.elapsed().as_secs_f64() * 1000.0);
			}
			Err(_) => {
				hangs += 1;
				println!("AUDIT lazy-mips attempt {attempt}: HUNG (no result after 30 s; its threads are abandoned)");
			}
		}
	}
	println!("AUDIT lazy-mips: {hangs} hangs in {attempts} attempts (pool of {threads} threads)");
	assert_eq!(hangs, 0, "the Smart Object sampler deadlocked on its own source lock");
}
