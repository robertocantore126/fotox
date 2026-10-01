//! Scratch health sampled away from render/engine disk access.
use std::path::Path;
// AUDIT-FIX(X1): independent comparison switch for scratch changes, read once at startup.
pub fn no_scratch_guards() -> bool {
	static OLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
	*OLD.get_or_init(|| std::env::var("FOTOX_NO_SCRATCH_GUARDS").is_ok_and(|v| v == "1"))
}
#[derive(Clone, Debug, Default)]
pub struct ScratchHealth {
	pub full: bool,
	pub error: Option<String>,
	pub free_bytes: u64,
	pub reserve_bytes: u64,
	pub path: String,
}
#[cfg(windows)]
pub fn disk_space(path: &Path) -> std::io::Result<Option<(u64, u64)>> {
	use std::os::windows::ffi::OsStrExt;
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn GetDiskFreeSpaceExW(path: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
	}
	let path = path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
	let (mut available, mut total, mut free) = (0, 0, 0);
	// SAFETY: NUL-terminated directory and live u64 output pointers match Win32 ABI.
	if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
		return Err(std::io::Error::last_os_error());
	}
	Ok(Some((available, total)))
}
#[cfg(not(windows))]
pub fn disk_space(_path: &Path) -> std::io::Result<Option<(u64, u64)>> {
	Ok(None)
}

// AUDIT-FIX(X1): probe only a unique owned file; never create a missing configured folder.
pub fn validate_folder(path: &Path) -> std::io::Result<Option<(u64, u64)>> {
	use std::io::Write;
	if !path.is_dir() {
		return Err(std::io::Error::other(format!("Scratch folder {} does not exist", path.display())));
	}
	static PROBE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	let probe = path.join(format!(
		".fotox-scratch-probe-{}-{}",
		std::process::id(),
		PROBE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
	));
	let mut options = std::fs::OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(windows)]
	{
		use std::os::windows::fs::OpenOptionsExt;
		options.custom_flags(0x04000000);
	}
	let mut file = options.open(&probe)?;
	let result = file.write_all(b"scratch probe").and_then(|_| file.sync_all());
	drop(file);
	#[cfg(not(windows))]
	let _ = std::fs::remove_file(&probe);
	result?;
	let space = disk_space(path)?;
	if let Some((free, total)) = space {
		if free < (5u64 << 30).max(total / 20) {
			return Err(std::io::Error::other(format!(
				"Scratch folder {} has {:.2} GB free, below its reserve",
				path.display(),
				free as f64 / 1e9
			)));
		}
	}
	Ok(space)
}
