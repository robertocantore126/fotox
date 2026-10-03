//! The scratch file: where authoritative tiles go when RAM is full.
//!
//! * One file per store, deleted when the store drops. On Windows it is opened
//!   with `FILE_FLAG_DELETE_ON_CLOSE`, so even a crash does not leak it.
//! * Space is handed out in 4 KiB-aligned extents from a free list
//!   (best fit, coalescing), else appended at the end, up to `limit` bytes.
//! * All I/O is positioned (`pread`/`pwrite` style): no shared cursor, no
//!   lock held during I/O. The allocator lock only guards the free list.
//! * Reads go through a [`ReadPool`]: one handle per thread, so parallel
//!   readers (a save, the render loads) do not queue on one handle.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::readpool::ReadPool;

const ALIGN: u64 = 4096;

/// A region of the scratch file holding one compressed tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Extent {
	pub offset: u64,
	/// Exact length of the data.
	pub len: u32,
	/// Allocated length (multiple of 4 KiB, ≥ `len`).
	pub alloc: u64,
	// AUDIT-FIX(X1): integrity stored in each live extent, no disk format change.
	pub crc32: u32,
}

/// Free-list allocator. Pure bookkeeping, no I/O, unit-tested on its own.
#[derive(Debug, Default)]
pub(crate) struct Allocator {
	by_offset: BTreeMap<u64, u64>,
	by_size: BTreeSet<(u64, u64)>,
	end: u64,
	limit: u64,
}

impl Allocator {
	pub fn new(limit: u64) -> Self {
		Self { limit, ..Default::default() }
	}

	/// Bytes currently allocated (end of file minus free space).
	pub fn used(&self) -> u64 {
		self.end - self.by_offset.values().sum::<u64>()
	}

	pub fn alloc(&mut self, len: u32) -> Option<Extent> {
		let size = (len as u64).div_ceil(ALIGN).max(1) * ALIGN;
		if let Some(&(free_size, offset)) = self.by_size.range((size, 0)..).next() {
			self.remove_free(offset, free_size);
			if free_size > size {
				self.insert_free(offset + size, free_size - size);
			}
			return Some(Extent {
				offset,
				len,
				alloc: size,
				crc32: 0,
			});
		}
		if self.end + size > self.limit {
			return None;
		}
		let offset = self.end;
		self.end += size;
		Some(Extent {
			offset,
			len,
			alloc: size,
			crc32: 0,
		})
	}

	pub fn free(&mut self, extent: Extent) {
		let mut offset = extent.offset;
		let mut size = extent.alloc;
		// merge with the previous free extent
		if let Some((&prev_off, &prev_size)) = self.by_offset.range(..offset).next_back()
			&& prev_off + prev_size == offset
		{
			self.remove_free(prev_off, prev_size);
			offset = prev_off;
			size += prev_size;
		}
		// merge with the next free extent
		if let Some(&next_size) = self.by_offset.get(&(offset + size)) {
			self.remove_free(offset + size, next_size);
			size += next_size;
		}
		if offset + size == self.end {
			// Free space at the end of the file just shrinks the used range.
			self.end = offset;
		} else {
			self.insert_free(offset, size);
		}
	}

	fn insert_free(&mut self, offset: u64, size: u64) {
		self.by_offset.insert(offset, size);
		self.by_size.insert((size, offset));
	}

	fn remove_free(&mut self, offset: u64, size: u64) {
		self.by_offset.remove(&offset);
		self.by_size.remove(&(size, offset));
	}
}

pub(crate) struct ScratchFile {
	file: ReadPool,
	path: PathBuf,
	allocator: Mutex<Allocator>,
}

impl ScratchFile {
	pub fn create(dir: &Path, limit: u64) -> std::io::Result<Self> {
		std::fs::create_dir_all(dir)?;
		static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
		let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
		let path = dir.join(format!("fotox-{}-{n}.scratch", std::process::id()));
		let mut options = std::fs::OpenOptions::new();
		options.read(true).write(true).create_new(true);
		#[cfg(windows)]
		{
			use std::os::windows::fs::OpenOptionsExt;
			const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
			options.custom_flags(FILE_FLAG_DELETE_ON_CLOSE);
		}
		let file = options.open(&path)?;
		Ok(Self {
			file: ReadPool::new(std::sync::Arc::new(file)),
			path,
			allocator: Mutex::new(Allocator::new(limit)),
		})
	}

	/// Reserve space and write `data` there. `None` = the limit is reached.
	pub fn write(&self, data: &[u8]) -> std::io::Result<Option<Extent>> {
		let len = u32::try_from(data.len()).map_err(|_| std::io::Error::other("block larger than 4 GiB"))?;
		// AUDIT-FIX(X1): allocator's logical end may shrink while physical EOF does not.
		// Conservatively admit every write, including reused extents, against volume reserve.
		if !crate::health::no_scratch_guards() {
			if let Some((free, total)) = crate::health::disk_space(self.path.parent().unwrap_or(Path::new(".")))? {
				let reserve = (5u64 << 30).max(total / 20);
				let growth = (len as u64).div_ceil(ALIGN).max(1) * ALIGN;
				if free < reserve.saturating_add(growth) {
					return Ok(None);
				}
			}
		}

		let Some(mut extent) = self.allocator.lock().alloc(len) else {
			return Ok(None);
		};
		if let Err(e) = write_all_at(self.file.main(), data, extent.offset) {
			self.allocator.lock().free(extent);
			return Err(e);
		}
		if !crate::health::no_scratch_guards() {
			extent.crc32 = crc32fast::hash(data);
		}
		Ok(Some(extent))
	}

	pub fn read(&self, extent: Extent) -> Result<Vec<u8>, crate::TileError> {
		let mut buf = Vec::new();
		self.read_into(extent, &mut buf)?;
		Ok(buf)
	}

	/// [`Self::read`] into a buffer the caller reuses (resized to the block).
	pub fn read_into(&self, extent: Extent, buf: &mut Vec<u8>) -> Result<(), crate::TileError> {
		buf.resize(extent.len as usize, 0);
		crate::iostats::size(0, buf.len());
		let in_flight = crate::iostats::in_flight();
		let t = crate::iostats::start();
		read_exact_at(self.file.reader(), buf, extent.offset)?;
		t.stop(crate::iostats::Phase::ScratchIo, buf.len());
		drop(in_flight);
		// AUDIT-FIX(X1): verify compressed bytes before any LZ4 decode.
		let t = crate::iostats::start();
		let corrupt = !crate::health::no_scratch_guards() && crc32fast::hash(buf) != extent.crc32;
		t.stop(crate::iostats::Phase::ScratchCrc, buf.len());
		if corrupt {
			return Err(crate::TileError::Corrupt(format!("scratch CRC mismatch at offset {}", extent.offset)));
		}
		Ok(())
	}

	pub fn free(&self, extent: Extent) {
		self.allocator.lock().free(extent);
	}

	pub fn used(&self) -> u64 {
		self.allocator.lock().used()
	}
}

impl Drop for ScratchFile {
	fn drop(&mut self) {
		// On Windows the file is already gone (delete-on-close); elsewhere remove it.
		if !cfg!(windows) {
			let _ = std::fs::remove_file(&self.path);
		}
	}
}

#[cfg(unix)]
pub(crate) fn write_all_at(file: &File, data: &[u8], offset: u64) -> std::io::Result<()> {
	std::os::unix::fs::FileExt::write_all_at(file, data, offset)
}

#[cfg(unix)]
pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
	std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
pub(crate) fn write_all_at(file: &File, mut data: &[u8], mut offset: u64) -> std::io::Result<()> {
	use std::os::windows::fs::FileExt;
	while !data.is_empty() {
		let n = file.seek_write(data, offset)?;
		if n == 0 {
			return Err(std::io::ErrorKind::WriteZero.into());
		}
		data = &data[n..];
		offset += n as u64;
	}
	Ok(())
}

#[cfg(windows)]
pub(crate) fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
	use std::os::windows::fs::FileExt;
	while !buf.is_empty() {
		let n = file.seek_read(buf, offset)?;
		if n == 0 {
			return Err(std::io::ErrorKind::UnexpectedEof.into());
		}
		buf = &mut buf[n..];
		offset += n as u64;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// PERF probe: do parallel reads through the one scratch handle queue up?
	/// (Windows serialises I/O on a synchronous handle.) Same reads through
	/// one shared handle, the scratch's [`ReadPool`], and one handle opened
	/// per thread. `FOTOX_SCRATCH_PROBE` =
	/// a directory on the real scratch disk; `FOTOX_SCRATCH_PROBE_MB` = data
	/// written (default 4096, more than the free RAM, so reads reach the disk).
	#[test]
	#[ignore = "probe: needs FOTOX_SCRATCH_PROBE"]
	fn probe_parallel_reads() {
		let Some(dir) = std::env::var_os("FOTOX_SCRATCH_PROBE").map(PathBuf::from) else { return };
		let mb: u64 = std::env::var("FOTOX_SCRATCH_PROBE_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(4096);
		let scratch = ScratchFile::create(&dir, 1 << 40).unwrap();
		let block = 96 * 1024;
		let mut data = vec![0u8; block];
		let mut x = 0x2545_F491_u32;
		for b in &mut data {
			x ^= x << 13;
			x ^= x >> 17;
			x ^= x << 5;
			*b = x as u8;
		}
		let count = (mb << 20) / block as u64;
		let extents: Vec<Extent> = (0..count)
			.map(|i| {
				data[0] = i as u8;
				scratch.write(&data).unwrap().unwrap()
			})
			.collect();
		let threads = std::thread::available_parallelism().map_or(8, |n| n.get());
		let run = |mode: &str| {
			let t = std::time::Instant::now();
			std::thread::scope(|s| {
				for k in 0..threads {
					let (scratch, extents) = (&scratch, &extents);
					s.spawn(move || {
						let own = (mode == "own").then(|| File::open(&scratch.path).unwrap());
						let file: &File = match mode {
							"own" => own.as_ref().unwrap(),
							"pool" => scratch.file.reader(),
							_ => scratch.file.main(),
						};
						let mut buf = vec![0u8; block];
						// Strided, so neighbouring threads hit far-apart offsets.
						for e in extents.iter().skip(k).step_by(threads) {
							read_exact_at(file, &mut buf[..e.len as usize], e.offset).unwrap();
						}
					});
				}
			});
			let secs = t.elapsed().as_secs_f64();
			println!(
				"PROBE {threads} threads, {mode} handles: {count} reads of 96 KiB in {secs:.2} s = {:.0} MiB/s",
				(count * block as u64) as f64 / 1048576.0 / secs
			);
		};
		for mode in ["shared", "pool", "own", "shared", "pool", "own"] {
			run(mode);
		}
	}

	#[test]
	fn alloc_reuses_and_coalesces() {
		let mut a = Allocator::new(1 << 20);
		let x = a.alloc(100).unwrap();
		let y = a.alloc(5000).unwrap();
		let z = a.alloc(4096).unwrap();
		assert_eq!((x.offset, y.offset, z.offset), (0, 4096, 12288));
		assert_eq!(a.used(), 16384);

		a.free(x);
		a.free(y); // coalesces with x → one free extent of 12 KiB at 0
		let w = a.alloc(12000).unwrap();
		assert_eq!(w.offset, 0, "coalesced hole is reused");

		a.free(z); // tail extent: the file end shrinks
		assert_eq!(a.used(), 12288);
		a.free(w);
		assert_eq!(a.used(), 0);
		assert_eq!(a.end, 0, "everything freed collapses the file");
	}

	#[test]
	fn alloc_respects_limit() {
		let mut a = Allocator::new(8192);
		assert!(a.alloc(4096).is_some());
		assert!(a.alloc(4096).is_some());
		assert!(a.alloc(1).is_none());
	}

	#[test]
	fn best_fit_splits_larger_holes() {
		let mut a = Allocator::new(1 << 20);
		let big = a.alloc(40000).unwrap(); // 40 KiB
		let _tail = a.alloc(1).unwrap();
		a.free(big);
		let small = a.alloc(10).unwrap();
		assert_eq!(small.offset, 0);
		let next = a.alloc(10).unwrap();
		assert_eq!(next.offset, 4096, "remainder of the split hole is used next");
	}

	#[test]
	fn file_roundtrip() {
		let dir = std::env::temp_dir().join("fx-scratch-test");
		let scratch = ScratchFile::create(&dir, 1 << 20).unwrap();
		let a = scratch.write(b"hello tiles").unwrap().unwrap();
		let b = scratch.write(&[7u8; 9000]).unwrap().unwrap();
		assert_eq!(scratch.read(a).unwrap(), b"hello tiles");
		assert_eq!(scratch.read(b).unwrap(), vec![7u8; 9000]);
		let path = scratch.path.clone();
		drop(scratch);
		assert!(!path.exists(), "scratch file is removed on drop");
	}
}
