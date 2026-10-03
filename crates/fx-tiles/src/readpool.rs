//! Positional reads from many threads at once.
//!
//! Windows serialises all I/O on a synchronous handle: its file object has
//! one lock, so threads reading through the same `File` wait for each other
//! even with positioned reads. 12 threads read the scratch disk at 270 MiB/s
//! through one handle and at 900 MiB/s through one handle each
//! (`scratch::probe_parallel_reads`, 2026-10-03), and a cold `.fxd` open spent
//! 9 s reading tile headers one after the other.
//!
//! A [`ReadPool`] keeps more handles to the same file, each with its own file
//! object (`ReOpenFile`, so a renamed or replaced path cannot swap the file),
//! and gives every thread one of them. They are opened at the first read. If
//! reopening fails, every thread uses the original handle, as before.

use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

pub struct ReadPool {
	main: Arc<File>,
	extra: OnceLock<Vec<File>>,
}

impl ReadPool {
	pub fn new(main: Arc<File>) -> Self {
		Self {
			main,
			extra: OnceLock::new(),
		}
	}

	/// The original handle: writes, metadata, and reads that must see it.
	pub fn main(&self) -> &Arc<File> {
		&self.main
	}

	/// A handle for reading on the calling thread.
	pub fn reader(&self) -> &File {
		let extra = self.extra.get_or_init(|| reopen(&self.main));
		let slot = thread_slot() % (extra.len() + 1);
		crate::iostats::pool_read(slot, extra.len() + 1);
		match slot {
			0 => &self.main,
			i => &extra[i - 1],
		}
	}
}

impl std::fmt::Debug for ReadPool {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ReadPool").field("extra", &self.extra.get().map(Vec::len)).finish()
	}
}

/// A stable small number per thread, handed out in turn, so the worker
/// threads of a pool land on different handles.
fn thread_slot() -> usize {
	static NEXT: AtomicUsize = AtomicUsize::new(0);
	thread_local! {
		static SLOT: usize = NEXT.fetch_add(1, Ordering::Relaxed);
	}
	SLOT.with(|slot| *slot)
}

#[cfg(windows)]
fn reopen(main: &File) -> Vec<File> {
	use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn ReOpenFile(original: RawHandle, access: u32, share: u32, flags: u32) -> RawHandle;
	}
	const GENERIC_READ: u32 = 0x8000_0000;
	// FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE: the original
	// handle keeps writing (scratch, an appending save) and may delete on close.
	const SHARE_ALL: u32 = 0x7;
	const INVALID: isize = -1;
	// Comparison switch: the old single shared handle.
	if std::env::var_os("FOTOX_ONE_READ_HANDLE").is_some_and(|v| v == "1") {
		return Vec::new();
	}
	let count =std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16) - 1;
	let mut files = Vec::with_capacity(count);
	for _ in 0..count {
		// SAFETY: `main` is a live file handle for the duration of the call;
		// a valid returned handle is owned by nobody else, so `File` takes it.
		let handle = unsafe { ReOpenFile(main.as_raw_handle(), GENERIC_READ, SHARE_ALL, 0) };
		if handle.is_null() || handle as isize == INVALID {
			tracing::debug!("ReOpenFile failed ({}); reads share fewer handles", std::io::Error::last_os_error());
			break;
		}
		files.push(unsafe { File::from_raw_handle(handle) });
	}
	files
}

/// Elsewhere positioned reads (`pread`) do not queue on the descriptor.
#[cfg(not(windows))]
fn reopen(_main: &File) -> Vec<File> {
	Vec::new()
}

#[cfg(test)]
mod tests {
	use std::io::Write;

	use super::*;

	#[test]
	fn every_thread_reads_the_same_file() {
		let dir = std::env::temp_dir().join("fx-tiles-readpool-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join(format!("pool-{}.bin", std::process::id()));
		let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
		file.write_all(&[7u8; 4096]).unwrap();
		let pool = ReadPool::new(Arc::new(file));
		std::thread::scope(|s| {
			for _ in 0..8 {
				s.spawn(|| {
					let mut buf = [0u8; 16];
					crate::scratch::read_exact_at(pool.reader(), &mut buf, 100).unwrap();
					assert_eq!(buf, [7u8; 16]);
				});
			}
		});
		// A write through the original handle is visible through the others.
		crate::scratch::write_all_at(pool.main(), &[9u8; 16], 200).unwrap();
		std::thread::scope(|s| {
			for _ in 0..8 {
				s.spawn(|| {
					let mut buf = [0u8; 16];
					crate::scratch::read_exact_at(pool.reader(), &mut buf, 200).unwrap();
					assert_eq!(buf, [9u8; 16]);
				});
			}
		});
		drop(pool);
		let _ = std::fs::remove_file(&path);
	}
}
