//! Brush plugins (D-096): WebAssembly modules that decide what a stroke does
//! to the pixels under it, loaded at run time and reloaded when their file
//! changes — edit, save, and the next stroke uses the new code.
//!
//! The plugin side is `plugins/sdk` (its doc is the ABI). The stroke engine
//! (`fx_ops::brush::stroke`) calls a plugin **once per rectangle** — a dirty
//! part of one layer tile — with the pixels before the stroke and the build-up
//! `k`, never once per pixel: a call into wasm costs tens of nanoseconds, a
//! pixel costs a few.
//!
//! Each rayon thread keeps its own instance of each plugin (a wasmtime
//! `Store` is single-threaded), created on first use and replaced when the
//! module is reloaded. A plugin that traps, loops (2 s epoch deadline) or
//! returns an error fails that call only: the caller falls back, the message
//! waits in [`take_errors`] for the UI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use wasmtime::{Config, Engine, Instance, Memory, Module, Store, TypedFunc};

/// f32 words before the values: params `0..16`, colour `16..20`, flags `20`.
pub const HEADER_WORDS: usize = 32;
/// Option-bar params a plugin can read.
pub const MAX_PARAMS: usize = 16;

/// The epoch ticks every `TICK`; a call still running after
/// `DEADLINE_TICKS` ticks is stopped (a plugin stuck in a loop must not hang
/// a rayon worker for good).
const TICK: Duration = Duration::from_millis(50);
const DEADLINE_TICKS: u64 = 40;
/// How often the watcher looks at the folder.
const POLL: Duration = Duration::from_millis(250);

/// Which swatch a plugin paints with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaintColor {
	#[default]
	Foreground,
	Background,
}

/// What a plugin says about itself (`fx_manifest`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
	/// Stable id; the UI tool id is `plugin:<id>`.
	pub id: String,
	/// The tool's name in the toolbar and the History panel.
	pub name: String,
	/// The toolbar slot whose flyout gets the tool (`eraser`, `brush`…).
	#[serde(default = "default_slot")]
	pub slot: String,
	/// An icon of the UI's sprite (`i-eraser`).
	#[serde(default = "default_icon")]
	pub icon: String,
	#[serde(default)]
	pub color: PaintColor,
	/// The option bar, in `ui/js/data/options.js`'s format.
	#[serde(default)]
	pub options: serde_json::Value,
	/// The option-bar keys (field text without the colon) passed to the
	/// plugin as params, in order. At most [`MAX_PARAMS`].
	#[serde(default)]
	pub params: Vec<String>,
}

fn default_slot() -> String {
	"brush".into()
}

fn default_icon() -> String {
	"i-brush".into()
}

/// A loaded plugin.
pub struct Plugin {
	/// [`key_of`] the id: what `StrokeTool::Plugin` carries.
	pub key: u64,
	pub manifest: Manifest,
	pub path: PathBuf,
	module: Module,
	/// Bumped on every load: instances of an older generation are replaced.
	generation: u64,
	pub has_gray: bool,
}

/// The stable 64-bit key of a plugin id (FNV-1a).
pub fn key_of(id: &str) -> u64 {
	let mut h: u64 = 0xcbf2_9ce4_8422_2325;
	for b in id.bytes() {
		h ^= u64::from(b);
		h = h.wrapping_mul(0x0100_0000_01b3);
	}
	h
}

struct Registry {
	engine: Engine,
	plugins: RwLock<HashMap<u64, Arc<Plugin>>>,
	errors: Mutex<Vec<String>>,
	generation: AtomicU64,
}

fn registry() -> &'static Registry {
	static REGISTRY: OnceLock<Registry> = OnceLock::new();
	REGISTRY.get_or_init(|| {
		let mut config = Config::new();
		config.epoch_interruption(true);
		let engine = Engine::new(&config).expect("the default wasmtime configuration is valid");
		let ticker = engine.clone();
		let _ = std::thread::Builder::new().name("fx-plugin epoch".into()).spawn(move || {
			loop {
				std::thread::sleep(TICK);
				ticker.increment_epoch();
			}
		});
		Registry {
			engine,
			plugins: RwLock::new(HashMap::new()),
			errors: Mutex::new(Vec::new()),
			generation: AtomicU64::new(1),
		}
	})
}

fn report(message: String) {
	tracing::warn!("{message}");
	let mut errors = registry().errors.lock().unwrap_or_else(PoisonError::into_inner);
	// One plugin failing on every tile of a stroke is one message.
	if !errors.contains(&message) {
		errors.push(message);
	}
}

/// Plugin failures since the last call, for the UI.
pub fn take_errors() -> Vec<String> {
	std::mem::take(&mut *registry().errors.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The plugin with key `key`, if loaded.
pub fn get(key: u64) -> Option<Arc<Plugin>> {
	registry().plugins.read().unwrap_or_else(PoisonError::into_inner).get(&key).cloned()
}

/// The plugin with id `id`, if loaded.
pub fn by_id(id: &str) -> Option<Arc<Plugin>> {
	get(key_of(id))
}

/// Every loaded plugin, by name.
pub fn plugins() -> Vec<Arc<Plugin>> {
	let mut all: Vec<_> = registry().plugins.read().unwrap_or_else(PoisonError::into_inner).values().cloned().collect();
	all.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
	all
}

/// Compile and register the plugin in `path` (replacing one with the same
/// id). Returns it, or why it could not load.
pub fn load_file(path: &Path) -> Result<Arc<Plugin>, String> {
	let file = path.display();
	let bytes = std::fs::read(path).map_err(|e| format!("plugin {file}: {e}"))?;
	let registry = registry();
	let module = Module::new(&registry.engine, &bytes).map_err(|e| format!("plugin {file} does not compile: {e:#}"))?;
	if let Some(import) = module.imports().next() {
		return Err(format!(
			"plugin {file} imports {}::{} — build it for wasm32-unknown-unknown with the fotox-plugin SDK only",
			import.module(),
			import.name()
		));
	}
	// Read the manifest and check the exports in a throwaway instance.
	let mut store = Store::new(&registry.engine, ());
	store.set_epoch_deadline(DEADLINE_TICKS);
	let instance = Instance::new(&mut store, &module, &[]).map_err(|e| format!("plugin {file}: {e:#}"))?;
	let manifest_fn = instance
		.get_typed_func::<(), u64>(&mut store, "fx_manifest")
		.map_err(|e| format!("plugin {file}: fx_manifest: {e:#}"))?;
	instance
		.get_typed_func::<u32, u32>(&mut store, "fx_alloc")
		.map_err(|e| format!("plugin {file}: fx_alloc: {e:#}"))?;
	instance
		.get_typed_func::<(u32, u32, u32, i32, i32), i32>(&mut store, "fx_rect")
		.map_err(|e| format!("plugin {file}: fx_rect: {e:#}"))?;
	let has_gray = instance.get_typed_func::<(u32, u32, u32, i32, i32), i32>(&mut store, "fx_gray").is_ok();
	let memory = instance
		.get_memory(&mut store, "memory")
		.ok_or_else(|| format!("plugin {file} exports no memory"))?;
	let packed = manifest_fn.call(&mut store, ()).map_err(|e| format!("plugin {file}: fx_manifest: {e:#}"))?;
	let (ptr, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
	let json = memory
		.data(&store)
		.get(ptr..ptr + len)
		.ok_or_else(|| format!("plugin {file}: the manifest is outside the memory"))?;
	let manifest: Manifest = serde_json::from_slice(json).map_err(|e| format!("plugin {file}: bad manifest: {e}"))?;
	if manifest.params.len() > MAX_PARAMS {
		return Err(format!("plugin {file}: more than {MAX_PARAMS} params"));
	}
	let plugin = Arc::new(Plugin {
		key: key_of(&manifest.id),
		manifest,
		path: path.to_path_buf(),
		module,
		generation: registry.generation.fetch_add(1, Ordering::Relaxed),
		has_gray,
	});
	let mut plugins = registry.plugins.write().unwrap_or_else(PoisonError::into_inner);
	// The file may now hold a plugin with another id: the old one is gone.
	plugins.retain(|_, p| p.path != path);
	plugins.insert(plugin.key, plugin.clone());
	drop(plugins);
	tracing::info!("plugin {} loaded from {file}", plugin.manifest.id);
	Ok(plugin)
}

/// Drop the plugins loaded from `path`.
pub fn unload_path(path: &Path) -> Vec<Arc<Plugin>> {
	let mut plugins = registry().plugins.write().unwrap_or_else(PoisonError::into_inner);
	let gone: Vec<u64> = plugins.values().filter(|p| p.path == path).map(|p| p.key).collect();
	gone.iter().filter_map(|key| plugins.remove(key)).collect()
}

/// Every `.wasm` in `dir`, sorted.
fn wasm_files(dir: &Path) -> Vec<PathBuf> {
	let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
		.map(|entries| {
			entries
				.flatten()
				.map(|e| e.path())
				.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wasm")))
				.collect()
		})
		.unwrap_or_default();
	files.sort();
	files
}

/// Load every plugin in `dir`; a missing folder is no plugins. Failures go
/// to [`take_errors`].
pub fn load_dir(dir: &Path) -> Vec<Arc<Plugin>> {
	wasm_files(dir).iter().filter_map(|path| load_file(path).map_err(report).ok()).collect()
}

type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> Option<Stamp> {
	let meta = std::fs::metadata(path).ok()?;
	Some((meta.modified().ok()?, meta.len()))
}

/// What the watcher did.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
	Loaded { id: String, name: String },
	Unloaded { id: String, name: String },
	Failed(String),
}

/// Watch `dir` from now on (call after [`load_dir`]): a `.wasm` that changed
/// and then stayed the same for one poll (the build has finished writing it)
/// is reloaded; a removed one is unloaded. `on_change` runs on the watcher
/// thread after each scan that changed something.
pub fn watch(dir: PathBuf, on_change: impl Fn(Vec<Change>) + Send + 'static) {
	let spawned = std::thread::Builder::new().name("fx-plugin watch".into()).spawn(move || {
		let mut loaded: HashMap<PathBuf, Stamp> = wasm_files(&dir).into_iter().filter_map(|p| Some((p.clone(), stamp(&p)?))).collect();
		let mut seen = loaded.clone();
		loop {
			std::thread::sleep(POLL);
			let mut changes = Vec::new();
			let files = wasm_files(&dir);
			for path in &files {
				let Some(now) = stamp(path) else { continue };
				let stable = seen.get(path) == Some(&now);
				seen.insert(path.clone(), now);
				if !stable || loaded.get(path) == Some(&now) {
					continue;
				}
				loaded.insert(path.clone(), now);
				changes.push(match load_file(path) {
					Ok(plugin) => Change::Loaded {
						id: plugin.manifest.id.clone(),
						name: plugin.manifest.name.clone(),
					},
					// Reported through `on_change`, not `take_errors`: a stroke
					// ending later must not repeat it.
					Err(message) => {
						tracing::warn!("{message}");
						Change::Failed(message)
					}
				});
			}
			let removed: Vec<PathBuf> = loaded.keys().filter(|p| !files.contains(p)).cloned().collect();
			for path in removed {
				loaded.remove(&path);
				seen.remove(&path);
				for plugin in unload_path(&path) {
					changes.push(Change::Unloaded {
						id: plugin.manifest.id.clone(),
						name: plugin.manifest.name.clone(),
					});
				}
			}
			if !changes.is_empty() {
				on_change(changes);
			}
		}
	});
	if let Err(e) = spawned {
		report(format!("the plugin watcher did not start: {e}"));
	}
}

/// The header of a call: params (missing ones 0), the paint colour
/// (straight RGBA `0..=1`), the lock-alpha flag.
pub fn header(params: &[f32], color: [f32; 4], lock_alpha: bool) -> [f32; HEADER_WORDS] {
	let mut h = [0.0f32; HEADER_WORDS];
	for (o, p) in h.iter_mut().zip(params.iter().take(MAX_PARAMS)) {
		*o = *p;
	}
	h[16..20].copy_from_slice(&color);
	h[20] = if lock_alpha { 1.0 } else { 0.0 };
	h
}

type RectFn = TypedFunc<(u32, u32, u32, i32, i32), i32>;

/// One plugin instantiated on one thread.
struct Live {
	generation: u64,
	store: Store<()>,
	memory: Memory,
	alloc: TypedFunc<u32, u32>,
	rect: RectFn,
	gray: Option<RectFn>,
}

impl Live {
	fn new(plugin: &Plugin) -> Result<Self, String> {
		let id = &plugin.manifest.id;
		let mut store = Store::new(&registry().engine, ());
		store.set_epoch_deadline(DEADLINE_TICKS);
		let instance = Instance::new(&mut store, &plugin.module, &[]).map_err(|e| format!("plugin {id}: {e:#}"))?;
		let memory = instance
			.get_memory(&mut store, "memory")
			.ok_or_else(|| format!("plugin {id} exports no memory"))?;
		let alloc = instance.get_typed_func(&mut store, "fx_alloc").map_err(|e| format!("plugin {id}: {e:#}"))?;
		let rect = instance.get_typed_func(&mut store, "fx_rect").map_err(|e| format!("plugin {id}: {e:#}"))?;
		let gray = instance.get_typed_func(&mut store, "fx_gray").ok();
		Ok(Self {
			generation: plugin.generation,
			store,
			memory,
			alloc,
			rect,
			gray,
		})
	}

	/// Copy in, call, copy back. `values` has `w·h·channels` floats.
	fn run(&mut self, gray: bool, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), values: &mut [f32], k: &[f32]) -> Result<(), String> {
		let n = size.0 as usize * size.1 as usize;
		debug_assert_eq!(k.len(), n);
		let bytes = (HEADER_WORDS + values.len() + n) * 4;
		self.store.set_epoch_deadline(DEADLINE_TICKS);
		let ptr = self.alloc.call(&mut self.store, bytes as u32).map_err(|e| format!("fx_alloc: {e:#}"))? as usize;
		let v0 = ptr + HEADER_WORDS * 4;
		let k0 = v0 + values.len() * 4;
		{
			let memory = self.memory.data_mut(&mut self.store);
			if memory.len() < ptr + bytes {
				return Err("fx_alloc returned a buffer outside the memory".into());
			}
			memory[ptr..v0].copy_from_slice(bytemuck::cast_slice(header));
			memory[v0..k0].copy_from_slice(bytemuck::cast_slice(values));
			memory[k0..ptr + bytes].copy_from_slice(bytemuck::cast_slice(k));
		}
		let func = if gray { self.gray.as_ref().ok_or("no fx_gray")? } else { &self.rect };
		let code = func
			.call(&mut self.store, (ptr as u32, size.0, size.1, at.0, at.1))
			.map_err(|e| format!("{e:#}"))?;
		if code != 0 {
			return Err(format!("returned error {code}"));
		}
		bytemuck::cast_slice_mut(values).copy_from_slice(&self.memory.data(&self.store)[v0..k0]);
		Ok(())
	}
}

thread_local! {
	static LIVE: RefCell<HashMap<u64, Live>> = RefCell::new(HashMap::new());
}

fn call(plugin: &Plugin, gray: bool, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), values: &mut [f32], k: &[f32]) -> Result<(), String> {
	LIVE.with(|cell| {
		let mut live = cell.borrow_mut();
		if live.get(&plugin.key).is_none_or(|l| l.generation != plugin.generation) {
			live.insert(plugin.key, Live::new(plugin)?);
		}
		let instance = live.get_mut(&plugin.key).expect("inserted above");
		let result = instance.run(gray, header, at, size, values, k);
		if result.is_err() {
			// A trap can leave the instance in any state: start fresh next time.
			live.remove(&plugin.key);
		}
		result.map_err(|e| format!("plugin {} failed: {e}", plugin.manifest.id))
	})
}

/// The plugin with key `key`, or (reported) why not: it was removed while a
/// stroke was using it.
fn loaded(key: u64) -> Result<Arc<Plugin>, String> {
	get(key).ok_or_else(|| {
		let message = "a brush plugin was removed while it was painting".to_string();
		report(message.clone());
		message
	})
}

/// Run plugin `key` on a rectangle of premultiplied RGBA `pixels` (row-major,
/// `size.0 × size.1`, first pixel at canvas `at`) with build-up `k`. On an
/// error the pixels are unchanged and the message is also kept for
/// [`take_errors`].
pub fn rect(key: u64, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), pixels: &mut [[f32; 4]], k: &[f32]) -> Result<(), String> {
	let plugin = loaded(key)?;
	// `run` copies back only after a successful call.
	call(&plugin, false, header, at, size, pixels.as_flattened_mut(), k).inspect_err(|e| report(e.clone()))
}

/// [`rect`] for a mask's grey values. `Ok(false)` when the plugin has no
/// mask function.
pub fn gray(key: u64, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), values: &mut [f32], k: &[f32]) -> Result<bool, String> {
	let plugin = loaded(key)?;
	if !plugin.has_gray {
		return Ok(false);
	}
	call(&plugin, true, header, at, size, values, k)
		.map(|()| true)
		.inspect_err(|e| report(e.clone()))
}
