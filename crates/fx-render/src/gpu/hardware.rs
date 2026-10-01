//! Adapter memory queried once during compositor construction.
// AUDIT-FIX(P1): match the selected adapter; do not size a GPU budget from another GPU's memory.
#[cfg(windows)]
pub fn video_memory(device: &wgpu::Device) -> Option<u64> {
	use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
	let info = device.adapter_info();
	// SAFETY: the factory enumeration/get-description APIs take no borrowed raw buffers.
	unsafe {
		let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
		for index in 0..32 {
			let Ok(adapter) = factory.EnumAdapters1(index) else { break };
			let Ok(desc) = adapter.GetDesc1() else { continue };
			if desc.VendorId == info.vendor && desc.DeviceId == info.device && desc.DedicatedVideoMemory > 0 {
				return Some(desc.DedicatedVideoMemory as u64);
			}
		}
	}
	None
}
#[cfg(not(windows))]
pub fn video_memory(_device: &wgpu::Device) -> Option<u64> {
	None
}

// AUDIT-FIX(P1): use half dedicated VRAM, capped at six GiB; unknown adapters get a conservative ceiling.
pub fn gpu_budget(device: &wgpu::Device) -> u64 {
	video_memory(device).map_or(1 << 30, |bytes| bytes / 2).clamp(64 << 20, 6 << 30)
}
