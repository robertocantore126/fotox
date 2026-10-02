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
//! module is reloaded.
//!
//! A plugin is a `.wasm`, or a single `.rs` source that Fotox builds itself
//! ([`script`]): ask an AI for the file (`plugins/AI-PROMPT.md`), drop it in
//! the folder, and the tool appears.
//!
//! Protection against a buggy plugin, which matters for AI-written code:
//! * it runs sandboxed — no imports at all, so no files, network or system;
//! * its memory is capped ([`MEMORY_LIMIT`]);
//! * a trap (panic, bad index), a call stuck past the 1 s deadline, or a call
//!   slower than [`SLOW_CALL`] **stops** the plugin: its strokes paint
//!   nothing, the tool says "(stopped)" and why, until the file is saved
//!   again;
//! * a pixel it returns with NaN or infinity is ignored (the pixel is kept);
//! * a manifest with a bad or already used id, or too many params, is
//!   refused with a message;
//! * and every stroke is one undo step.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use wasmtime::{Config, Engine, Instance, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

pub mod script;

/// f32 words before the values: params `0..16`, colour `16..20`, flags `20`.
pub const HEADER_WORDS: usize = 32;
/// Option-bar params a plugin can read.
pub const MAX_PARAMS: usize = 16;

/// The epoch ticks every `TICK`; a call still running after
/// `DEADLINE_TICKS` ticks is stopped (a plugin stuck in a loop must not hang
/// a rayon worker for good).
const TICK: Duration = Duration::from_millis(50);
const DEADLINE_TICKS: u64 = 20;
/// The most linear memory one plugin instance may grow to.
pub const MEMORY_LIMIT: usize = 256 << 20;
/// A call slower than this stops the plugin: a brush that takes half a
/// second per tile makes painting unusable (native ops take a few ms).
pub const SLOW_CALL: Duration = Duration::from_millis(500);
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
	/// How the stroke engine paints for this plugin, where it differs from a
	/// native brush.
	#[serde(default)]
	pub brush: BrushOverrides,
}

/// A plugin's brush settings that the option bar does not show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrushOverrides {
	/// The round tip's fall-off: `classic`, `gaussian` or `feather`. Set, it
	/// also forces the round tip (no sampled tip from Brush Settings).
	#[serde(default)]
	pub profile: Option<String>,
	/// How dabs add up: `build_up` (native) or `max`.
	#[serde(default)]
	pub accumulate: Option<String>,
	/// Dab spacing as a fraction of the diameter, unless the bar has a
	/// `Spacing` field.
	#[serde(default)]
	pub spacing: Option<f32>,
	/// The option-bar key of a feather width in pixels: the tip becomes
	/// `Size` of solid core plus that much soft edge on each side (diameter
	/// `Size + 2 × feather`, hardness `Size / diameter`).
	#[serde(default)]
	pub feather_from: Option<String>,
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
	/// Why the plugin was stopped (it crashed, hung or was too slow); `None`
	/// while it works. A reload starts it fresh.
	stopped: Mutex<Option<String>>,
}

impl Plugin {
	/// Why the plugin is stopped, if it is.
	pub fn stopped(&self) -> Option<String> {
		self.stopped.lock().unwrap_or_else(PoisonError::into_inner).clone()
	}

	/// Stop it; `true` the first time.
	fn stop(&self, reason: String) -> bool {
		let mut stopped = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
		if stopped.is_some() {
			return false;
		}
		*stopped = Some(reason);
		true
	}
}

/// What a plugin's manifest must satisfy.
fn validate(m: &Manifest) -> Result<(), String> {
	let id_ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_';
	if m.id.is_empty() || m.id.len() > 64 || !m.id.bytes().all(id_ok) {
		return Err(format!("the id \"{}\" must be 1-64 characters of a-z, 0-9, - or _", m.id));
	}
	if m.name.trim().is_empty() {
		return Err("the manifest has no name".into());
	}
	if m.params.len() > MAX_PARAMS {
		return Err(format!("more than {MAX_PARAMS} params"));
	}
	if !(m.options.is_null() || m.options.is_array()) {
		return Err("options must be a list of option-bar fields".into());
	}
	Ok(())
}

/// A store with the memory cap and the deadline.
fn new_store(engine: &Engine) -> Store<StoreLimits> {
	let limits = StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).instances(1).memories(1).build();
	let mut store = Store::new(engine, limits);
	store.limiter(|limits| limits);
	store.set_epoch_deadline(DEADLINE_TICKS);
	store
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

/// Whether `path` is a single-file source plugin.
fn is_script(path: &Path) -> bool {
	path.extension().is_some_and(|e| e.eq_ignore_ascii_case("rs"))
}

/// Load the plugin in `path` — a `.wasm`, or a `.rs` built first — and
/// register it (replacing what the same file held before). Returns it, or
/// why it could not load.
pub fn load_file(path: &Path) -> Result<Arc<Plugin>, String> {
	let bytes = if is_script(path) {
		script::build(path)?
	} else {
		std::fs::read(path).map_err(|e| format!("plugin {}: {e}", path.display()))?
	};
	load_bytes(&bytes, path)
}

/// Register the wasm module `bytes`, loaded from (or built from) `path`.
fn load_bytes(bytes: &[u8], path: &Path) -> Result<Arc<Plugin>, String> {
	let file = path.display();
	let registry = registry();
	let module = Module::new(&registry.engine, bytes).map_err(|e| format!("plugin {file} does not compile: {e:#}"))?;
	if let Some(import) = module.imports().next() {
		return Err(format!(
			"plugin {file} imports {}::{} — build it for wasm32-unknown-unknown with the fotox-plugin SDK only",
			import.module(),
			import.name()
		));
	}
	// Read the manifest and check the exports in a throwaway instance.
	let mut store = new_store(&registry.engine);
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
	validate(&manifest).map_err(|e| format!("plugin {file}: {e}"))?;
	if let Some(other) = get(key_of(&manifest.id)).filter(|other| other.path != path) {
		return Err(format!(
			"plugin {file}: the id \"{}\" is already used by {}; give this plugin another id",
			manifest.id,
			other.path.display()
		));
	}
	let plugin = Arc::new(Plugin {
		key: key_of(&manifest.id),
		manifest,
		path: path.to_path_buf(),
		module,
		generation: registry.generation.fetch_add(1, Ordering::Relaxed),
		has_gray,
		stopped: Mutex::new(None),
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

/// Every `.wasm` and `.rs` in `dir`, sorted.
fn plugin_files(dir: &Path) -> Vec<PathBuf> {
	let wanted = |p: &PathBuf| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wasm") || e.eq_ignore_ascii_case("rs"));
	let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
		.map(|entries| entries.flatten().map(|e| e.path()).filter(wanted).collect())
		.unwrap_or_default();
	files.sort();
	files
}

/// Whether [`load_dir`] loads `path` at once: a `.wasm`, or a `.rs` whose
/// build is up to date. A `.rs` that needs building is left to the watcher,
/// so the app does not wait for a compiler at start.
fn loads_at_start(path: &Path) -> bool {
	!is_script(path) || script::cached(path).is_some()
}

/// Load every plugin in `dir` that is ready; a missing folder is no
/// plugins. Failures go to [`take_errors`].
pub fn load_dir(dir: &Path) -> Vec<Arc<Plugin>> {
	plugin_files(dir)
		.iter()
		.filter(|path| loads_at_start(path))
		.filter_map(|path| load_file(path).map_err(report).ok())
		.collect()
}

/// What tells a file changed: time, length and a hash of the content. The
/// hash catches an edit that keeps the length within the file system's time
/// step (exFAT's is coarse); plugin files are a few KB, so reading them
/// every poll costs nothing.
type Stamp = (SystemTime, u64, u64);

fn stamp(path: &Path) -> Option<Stamp> {
	let meta = std::fs::metadata(path).ok()?;
	let bytes = std::fs::read(path).ok()?;
	let hash = bytes
		.iter()
		.fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
	Some((meta.modified().ok()?, meta.len(), hash))
}

/// Set by [`request_reload`]; the watcher's next poll reloads everything.
static RELOAD: AtomicBool = AtomicBool::new(false);

/// Reload every plugin at the watcher's next poll (Plugins ▸ Reload
/// Plugins), changed or not: a stopped plugin starts fresh. Returns at once;
/// the result arrives as one [`Change::Reloaded`].
pub fn request_reload() {
	RELOAD.store(true, Ordering::Relaxed);
}

/// What the watcher did.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
	/// Everything was reloaded on request: the names now loaded.
	Reloaded {
		names: Vec<String>,
	},
	/// A `.rs` plugin is being built (a few seconds).
	Building {
		file: String,
	},
	Loaded {
		id: String,
		name: String,
	},
	Unloaded {
		id: String,
		name: String,
	},
	Failed(String),
}

/// Watch `dir` from now on (call after [`load_dir`]): a plugin file that
/// changed and then stayed the same for one poll (its writer has finished)
/// is reloaded — a `.rs` is built first; a removed one is unloaded.
/// `on_change` runs on the watcher thread after each scan that changed
/// something, and before a build starts.
pub fn watch(dir: PathBuf, on_change: impl Fn(Vec<Change>) + Send + 'static) {
	let spawned = std::thread::Builder::new().name("fx-plugin watch".into()).spawn(move || {
		let mut loaded: HashMap<PathBuf, Stamp> = plugin_files(&dir)
			.into_iter()
			.filter(|p| loads_at_start(p))
			.filter_map(|p| Some((p.clone(), stamp(&p)?)))
			.collect();
		let mut seen = loaded.clone();
		loop {
			std::thread::sleep(POLL);
			let mut changes = Vec::new();
			let files = plugin_files(&dir);
			let reload = RELOAD.swap(false, Ordering::Relaxed);
			if reload {
				// Every file counts as changed and settled.
				loaded.clear();
				for path in &files {
					if let Some(now) = stamp(path) {
						seen.insert(path.clone(), now);
					}
				}
			}
			for path in &files {
				let Some(now) = stamp(path) else { continue };
				let stable = seen.get(path) == Some(&now);
				seen.insert(path.clone(), now);
				if !stable || loaded.get(path) == Some(&now) {
					continue;
				}
				loaded.insert(path.clone(), now);
				if is_script(path) && script::cached(path).is_none() {
					let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
					on_change(vec![Change::Building { file }]);
				}
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
			if reload {
				// One summary instead of a toast per plugin; failures stay.
				let mut names: Vec<String> = changes
					.iter()
					.filter_map(|c| match c {
						Change::Loaded { name, .. } => Some(name.clone()),
						_ => None,
					})
					.collect();
				names.sort();
				changes.retain(|c| !matches!(c, Change::Loaded { .. }));
				changes.insert(0, Change::Reloaded { names });
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
	store: Store<StoreLimits>,
	memory: Memory,
	alloc: TypedFunc<u32, u32>,
	rect: RectFn,
	gray: Option<RectFn>,
}

impl Live {
	fn new(plugin: &Plugin) -> Result<Self, String> {
		let id = &plugin.manifest.id;
		let mut store = new_store(&registry().engine);
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

	/// Copy in, call, copy back. `values` has `w·h·channels` floats. A pixel
	/// the plugin returns with a NaN or an infinity keeps its input; returns
	/// how many did.
	fn run(&mut self, gray: bool, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), values: &mut [f32], k: &[f32]) -> Result<usize, String> {
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
		let channels = if gray { 1 } else { 4 };
		let out = &self.memory.data(&self.store)[v0..k0];
		let mut invalid = 0;
		for (dst, src) in values.chunks_exact_mut(channels).zip(out.chunks_exact(4 * channels)) {
			let mut pixel = [0.0f32; 4];
			for (c, bytes) in pixel.iter_mut().zip(src.chunks_exact(4)) {
				*c = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
			}
			if pixel[..channels].iter().all(|c| c.is_finite()) {
				dst.copy_from_slice(&pixel[..channels]);
			} else {
				invalid += 1;
			}
		}
		Ok(invalid)
	}
}

thread_local! {
	static LIVE: RefCell<HashMap<u64, Live>> = RefCell::new(HashMap::new());
}

/// Stop `plugin` for `why` and say so once.
fn stop(plugin: &Plugin, why: String) {
	if plugin.stop(why.clone()) {
		report(format!(
			"Plugin \u{201c}{}\u{201d} stopped: {why}. Its strokes paint nothing until its file is saved again.",
			plugin.manifest.name
		));
	}
}

fn call(plugin: &Plugin, gray: bool, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), values: &mut [f32], k: &[f32]) -> Result<(), String> {
	if let Some(why) = plugin.stopped() {
		return Err(why);
	}
	LIVE.with(|cell| {
		let mut live = cell.borrow_mut();
		if live.get(&plugin.key).is_none_or(|l| l.generation != plugin.generation) {
			match Live::new(plugin) {
				Ok(instance) => live.insert(plugin.key, instance),
				Err(e) => {
					stop(plugin, e.clone());
					return Err(e);
				}
			};
		}
		let instance = live.get_mut(&plugin.key).expect("inserted above");
		let started = Instant::now();
		let result = instance.run(gray, header, at, size, values, k);
		let took = started.elapsed();
		match result {
			Ok(invalid) => {
				if invalid > 0 {
					report(format!(
						"Plugin \u{201c}{}\u{201d} returned invalid values (NaN or infinity); those pixels were left unchanged",
						plugin.manifest.name
					));
				}
				if took > SLOW_CALL {
					stop(plugin, format!("too slow: {} ms for a {}x{} px area", took.as_millis(), size.0, size.1));
				}
				Ok(())
			}
			Err(e) => {
				// A trap can leave the instance in any state: start fresh next time.
				live.remove(&plugin.key);
				let why = if e.contains("interrupt") || e.contains("epoch") {
					"it ran too long (an endless loop?)".to_string()
				} else {
					format!("it crashed ({})", e.lines().next().unwrap_or(&e))
				};
				stop(plugin, why.clone());
				Err(why)
			}
		}
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
/// error the pixels are unchanged; a plugin that crashed, hung or was too
/// slow is stopped, and [`take_errors`] says so once.
pub fn rect(key: u64, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), pixels: &mut [[f32; 4]], k: &[f32]) -> Result<(), String> {
	let plugin = loaded(key)?;
	// `run` copies back only after a successful call.
	call(&plugin, false, header, at, size, pixels.as_flattened_mut(), k)
}

/// [`rect`] for a mask's grey values. `Ok(false)` when the plugin has no
/// mask function.
pub fn gray(key: u64, header: &[f32; HEADER_WORDS], at: (i32, i32), size: (u32, u32), values: &mut [f32], k: &[f32]) -> Result<bool, String> {
	let plugin = loaded(key)?;
	if !plugin.has_gray {
		return Ok(false);
	}
	call(&plugin, true, header, at, size, values, k).map(|()| true)
}
