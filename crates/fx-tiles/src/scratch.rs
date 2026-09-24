//! The scratch file: where authoritative tiles go when RAM is full.
//!
//! * One file per store, deleted when the store drops. On Windows it is opened
//!   with `FILE_FLAG_DELETE_ON_CLOSE`, so even a crash does not leak it.
//! * Space is handed out in 4 KiB-aligned extents from a free list
//!   (best fit, coalescing), else appended at the end, up to `limit` bytes.
//! * All I/O is positioned (`pread`/`pwrite` style): no shared cursor, no
//!   lock held during I/O. The allocator lock only guards the free list.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

const ALIGN: u64 = 4096;

/// A region of the scratch file holding one compressed tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Extent {
	pub offset: u64,
	/// Exact length of the data.
	pub len: u32,
	/// Allocated length (multiple of 4 KiB, ≥ `len`).
	pub alloc: u64,
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
			return Some(Extent { offset, len, alloc: size });
		}
		if self.end + size > self.limit {
			return None;
		}
		let offset = self.end;
		self.end += size;
		Some(Extent { offset, len, alloc: size })
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
	file: File,
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
			file,
			path,
			allocator: Mutex::new(Allocator::new(limit)),
		})
	}

	/// Reserve space and write `data` there. `None` = the limit is reached.
	pub fn write(&self, data: &[u8]) -> std::io::Result<Option<Extent>> {
		let len = u32::try_from(data.len()).map_err(|_| std::io::Error::other("block larger than 4 GiB"))?;
		let Some(extent) = self.allocator.lock().alloc(len) else {
			return Ok(None);
		};
		if let Err(e) = write_all_at(&self.file, data, extent.offset) {
			self.allocator.lock().free(extent);
			return Err(e);
		}
		Ok(Some(extent))
	}

	pub fn read(&self, extent: Extent) -> std::io::Result<Vec<u8>> {
		let mut buf = vec![0u8; extent.len as usize];
		read_exact_at(&self.file, &mut buf, extent.offset)?;
		Ok(buf)
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
fn write_all_at(file: &File, data: &[u8], offset: u64) -> std::io::Result<()> {
	std::os::unix::fs::FileExt::write_all_at(file, data, offset)
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
	std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut data: &[u8], mut offset: u64) -> std::io::Result<()> {
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
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
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
