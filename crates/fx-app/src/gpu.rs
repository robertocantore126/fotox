//! The wgpu context the app renders with.
//!
//! Created through `wgpu_sync` so the queue is a `wgpu_sync::Queue`. CEF
//! delivers UI frames on another thread, and the locks in that crate are what
//! stop a surface reconfiguration from racing a submit or a present (see the
//! `wgpu-sync` crate docs and `docs/GRAPHITE.md` §1).

use anyhow::{Context, Result};

/// Everything the app needs to render: one `wgpu_sync` instance, its chosen
/// adapter, and the device and queue created from it. All four share one lock.
pub(crate) struct Gpu {
	pub(crate) instance: wgpu_sync::Instance,
	pub(crate) adapter: wgpu_sync::Adapter,
	pub(crate) device: wgpu::Device,
	pub(crate) queue: wgpu_sync::Queue,
}

/// Environment variable that overrides adapter selection with an index, as
/// printed by the adapter listing below.
const ADAPTER_OVERRIDE_ENV: &str = "FOTOX_WGPU_ADAPTER";

/// Create the instance, adapter, device and queue.
///
/// Takes the event loop's display handle because a window surface cannot be
/// created without one: `create_surface` fails outright with "no
/// `DisplayHandle` is available" on an instance that has none. That is why the
/// event loop is created before this, rather than after as in the reference.
///
/// DX12 is the only backend requested, because that is what this project
/// targets (decision D-003). The adapter's own limits are requested rather than
/// wgpu's defaults, because the tile atlas needs 2048 texture array layers and
/// the default limit is far below that (M0-T03 step 1). `IMMEDIATES` is
/// required because the composite pass passes its uniforms as immediates.
pub(crate) fn create(display_handle: winit::event_loop::OwnedDisplayHandle) -> Result<Gpu> {
	let descriptor = wgpu::InstanceDescriptor {
		backends: wgpu::Backends::DX12,
		..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(display_handle))
	};
	let instance = wgpu_sync::Instance::new(wgpu::Instance::new(descriptor));

	// Every adapter is listed so that an index in `FOTOX_WGPU_ADAPTER` is
	// actionable without a second run to find out what the indices mean.
	let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
	for (index, adapter) in adapters.iter().enumerate() {
		let info = adapter.get_info();
		tracing::info!("adapter {index}: {} ({:?})", info.name, info.device_type);
	}
	let Some(first) = adapters.first().cloned() else {
		anyhow::bail!("no DX12 adapter available for Fotox");
	};

	let adapter = match std::env::var(ADAPTER_OVERRIDE_ENV) {
		Ok(value) => {
			let index: usize = value
				.parse()
				.with_context(|| format!("{ADAPTER_OVERRIDE_ENV} must be an adapter index, got {value:?}"))?;
			let picked = adapters
				.get(index)
				.with_context(|| format!("{ADAPTER_OVERRIDE_ENV}={index}, but only {} adapter(s) were found", adapters.len()))?;
			tracing::info!("{ADAPTER_OVERRIDE_ENV}={index} selects {}", picked.get_info().name);
			picked.clone()
		}
		// No override: prefer a discrete GPU, which is what the reference machine
		// has and what a large-document editor wants.
		Err(_) => adapters
			.iter()
			.find(|adapter| adapter.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
			.cloned()
			.unwrap_or(first),
	};

	let info = adapter.get_info();
	tracing::info!("using adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);

	let features = wgpu::Features::IMMEDIATES;
	let unsupported = features - adapter.features();
	if !unsupported.is_empty() {
		anyhow::bail!("the {} adapter does not support {unsupported:?}, which the composite pass needs", info.name);
	}

	let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
		label: Some("fotox-app"),
		required_features: features,
		required_limits: adapter.limits(),
		..Default::default()
	}))
	.context("failed to create the wgpu device")?;

	Ok(Gpu {
		instance,
		adapter,
		device,
		queue,
	})
}
