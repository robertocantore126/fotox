//! Startup machine budgets and comparison switch.
use std::sync::OnceLock;
// AUDIT-FIX(P1): environment and machine memory are sampled once, not per tile.
pub fn old_budgets() -> bool {
	static OLD: OnceLock<bool> = OnceLock::new();
	*OLD.get_or_init(|| std::env::var("FOTOX_OLD_BUDGETS").is_ok_and(|v| v == "1"))
}
pub fn total_ram() -> u64 {
	static RAM: OnceLock<u64> = OnceLock::new();
	*RAM.get_or_init(|| physical_ram().unwrap_or(16 << 30))
}
#[cfg(windows)]
fn physical_ram() -> Option<u64> {
	#[repr(C)]
	struct MemoryStatus {
		length: u32,
		load: u32,
		total: u64,
		available: u64,
		page_total: u64,
		page_available: u64,
		virtual_total: u64,
		virtual_available: u64,
		extended: u64,
	}
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
	}
	let mut status = MemoryStatus {
		length: std::mem::size_of::<MemoryStatus>() as u32,
		load: 0,
		total: 0,
		available: 0,
		page_total: 0,
		page_available: 0,
		virtual_total: 0,
		virtual_available: 0,
		extended: 0,
	};
	// SAFETY: correctly sized writable MEMORYSTATUSEX with initialized length.
	(unsafe { GlobalMemoryStatusEx(&mut status) } != 0 && status.total > 0).then_some(status.total)
}
#[cfg(not(windows))]
fn physical_ram() -> Option<u64> {
	None
}
