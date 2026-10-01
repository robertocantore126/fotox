//! Durable replacement and bounded disk admission shared by save/export and settings.
#[cfg(not(windows))]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

// AUDIT-FIX(D6+D10): unique sibling files are cleaned on every ordinary error/cancel path.
pub struct PartGuard(pub PathBuf);
impl Drop for PartGuard {
	fn drop(&mut self) {
		let _ = std::fs::remove_file(&self.0);
	}
}
pub fn unique_part(path: &Path) -> PathBuf {
	static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	let job = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	let mut name = path.file_stem().unwrap_or_default().to_os_string();
	name.push(format!(".{}-{job}.fxd.part", std::process::id()));
	path.with_file_name(name)
}

// AUDIT-FIX(D6): Windows replacement includes write-through; no remove-then-rename gap.
#[cfg(windows)]
pub fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
	use std::os::windows::ffi::OsStrExt;
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
	}
	let from = from.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
	let to = to.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
	// SAFETY: both strings are live NUL-terminated UTF-16; flags are REPLACE_EXISTING|WRITE_THROUGH.
	if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(())
}
#[cfg(not(windows))]
pub fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
	std::fs::rename(from, to)?;
	if let Some(parent) = to.parent() {
		File::open(parent)?.sync_all()?;
	}
	Ok(())
}

// AUDIT-FIX(D10): translate common Windows save failures without suggesting false success.
pub fn error_text(error: &io::Error) -> String {
	match error.raw_os_error() {
		Some(5 | 183) => format!("Cannot replace or write the target: it may be read-only, unwritable, or held open by another program ({error})"),
		Some(32 | 33) => format!("The file is in use by another program; close it or choose another path ({error})"),
		Some(39 | 112) => format!("The disk is full; free space or choose another drive ({error})"),
		_ => error.to_string(),
	}
}

// AUDIT-FIX(D10): disk checks use bytes available to this user on the target volume.
#[cfg(windows)]
pub fn disk_space(path: &Path) -> io::Result<Option<(u64, u64)>> {
	use std::os::windows::ffi::OsStrExt;
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn GetDiskFreeSpaceExW(path: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
	}
	let path = path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
	let (mut available, mut total, mut free) = (0, 0, 0);
	// SAFETY: pointers refer to live ABI-compatible integers and a NUL-terminated directory name.
	if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(Some((available, total)))
}
#[cfg(not(windows))]
pub fn disk_space(_path: &Path) -> io::Result<Option<(u64, u64)>> {
	Ok(None)
}

// AUDIT-FIX(D10): estimate changed/fresh bytes plus reserve before writing, preserving dirty state on refusal.
pub fn require_space(path: &Path, estimate: u64) -> io::Result<()> {
	let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
	if let Some((available, _)) = disk_space(parent)? {
		let required = estimate.saturating_add((estimate / 20).max(64 << 20));
		if required > available {
			return Err(io::Error::other(format!(
				"Not enough space on {} (needs {:.2} GB, {:.2} GB free)",
				parent.display(),
				required as f64 / 1e9,
				available as f64 / 1e9
			)));
		}
	}
	Ok(())
}

// AUDIT-FIX(D7): sync the new JSON and a valid previous JSON backup before replacing either name.
pub fn write_json(path: &Path, bytes: &[u8]) -> io::Result<()> {
	let _lease = crate::fxd::PathWriteLock::acquire(path);
	fn write_one(path: &Path, bytes: &[u8]) -> io::Result<()> {
		let part = PartGuard(unique_part(path));
		let mut file = OpenOptions::new().write(true).create_new(true).open(&part.0)?;
		file.write_all(bytes)?;
		file.sync_all()?;
		drop(file);
		atomic_replace(&part.0, path)
	}
	if let Ok(old) = std::fs::read(path) {
		if serde_json::from_slice::<serde_json::Value>(&old).is_ok() {
			write_one(&backup_path(path), &old)?;
		}
	}
	write_one(path, bytes)
}
pub fn backup_path(path: &Path) -> PathBuf {
	let mut name = path.as_os_str().to_os_string();
	name.push(".bak");
	PathBuf::from(name)
}
// AUDIT-FIX(D7): malformed main JSON falls back to the synced previous version.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
	[path.to_path_buf(), backup_path(path)]
		.into_iter()
		.find_map(|path| std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()))
}

// AUDIT-FIX(D10): only Fotox's named parts, older than one day and belonging to dead processes, are swept.
#[cfg(windows)]
pub fn sweep_parts(recent: &[PathBuf]) {
	use std::collections::HashSet;
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
		fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
	}
	let folders = recent.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect::<HashSet<_>>();
	for folder in folders {
		let Ok(entries) = std::fs::read_dir(folder) else { continue };
		for entry in entries.flatten() {
			let name = entry.file_name().to_string_lossy().into_owned();
			let Some(base) = name.strip_suffix(".fxd.part") else { continue };
			let Some((_, owner)) = base.rsplit_once('.') else { continue };
			let Some((pid, job)) = owner.split_once('-') else { continue };
			let (Ok(pid), Ok(_)) = (pid.parse::<u32>(), job.parse::<u64>()) else {
				continue;
			};
			if !entry
				.metadata()
				.ok()
				.and_then(|m| m.modified().ok())
				.and_then(|t| t.elapsed().ok())
				.is_some_and(|age| age.as_secs() > 86400)
			{
				continue;
			}
			// SAFETY: process handle is used only to check liveness and closed immediately.
			let handle = unsafe { OpenProcess(0x1000, 0, pid) };
			if !handle.is_null() {
				unsafe {
					CloseHandle(handle);
				}
				continue;
			}
			if io::Error::last_os_error().raw_os_error() == Some(87) {
				let _ = std::fs::remove_file(entry.path());
			}
		}
	}
}
#[cfg(not(windows))]
pub fn sweep_parts(_recent: &[PathBuf]) {}
