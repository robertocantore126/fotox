//! The render thread: draws the viewport texture whenever the engine thread
//! asks, and hands it to the shell (docs/ARCHITECTURE.md §2.2).
//!
//! * No document open → the procedural test pattern of the virtual M0
//!   document (`fx_render::test_pattern`).
//! * A document → the tile pipeline (M1-T07): plan the frame, build programs
//!   for the tiles it needs, composite them on the GPU, draw the plan with the
//!   viewport renderer.
//!
//! Rules:
//! * **Never block.** The render thread never calls `TileStore::get`: tiles
//!   the compositor reports missing are loaded by jobs on the rayon pool, which
//!   wake the thread when done. Dirty mip tiles go back to the engine thread.
//! * Only render when something changed or work is pending; a burst of
//!   requests collapses into one frame for the newest one.
//! * Every queue submission goes through the [`wgpu_sync::Queue`], so it cannot
//!   race the shell's surface reconfiguration.
//! * Double-buffered: the texture the shell is showing is never the one being
//!   drawn into.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use fx_core::{Document, LayerId};
use fx_protocol::DocId;
use fx_render::adjust::LutCache;
use fx_render::gpu::{CompositorConfig, GpuCompositor, TileOutcome, ViewportRenderer};
use fx_render::{FramePlan, MipRequest, TestPatternRenderer, TileKey, TileProgram, VIEWPORT_FORMAT, ViewTransform, ViewportSize, build_program, plan_frame};
use fx_tiles::{TILE_SIZE, TileId, TileStore};

use crate::{EngineOutput, OutputSink};

/// Programs built per frame, at most (the rest wait for the next frame).
const MAX_REQUESTS_PER_FRAME: usize = 256;

/// `plan_frame` has no notion of "ready but fully transparent": such tiles
/// are reported with this slot and their draws are dropped before rendering.
const EMPTY_SLOT: u32 = u32::MAX;

/// What to draw.
#[derive(Clone)]
pub(crate) struct Frame {
	pub view: ViewTransform,
	pub viewport: ViewportSize,
	/// The active document, or `None` for the M0 test pattern.
	pub doc: Option<(DocId, Arc<Document>)>,
	/// Size of the virtual document when `doc` is `None`.
	pub virtual_doc: (u32, u32),
}

/// Work for the render thread.
pub(crate) enum RenderRequest {
	Frame(Frame),
	/// A tile load finished, or the last frame left work: draw again.
	Wake,
	Stop,
}

/// Dirty mip tiles a frame needed, for the engine thread to compute.
pub(crate) struct MipWork {
	pub doc: DocId,
	pub revision: u64,
	pub requests: Vec<MipRequest>,
}

/// Channels and shared state the render thread works with.
pub(crate) struct RenderContext {
	pub device: wgpu::Device,
	pub queue: wgpu_sync::Queue,
	pub store: Arc<TileStore>,
	pub requests: Receiver<RenderRequest>,
	/// For tile loaders and for itself (to schedule a follow-up frame).
	pub wake: Sender<RenderRequest>,
	pub mips: Sender<MipWork>,
	pub output: OutputSink,
}

/// Body of the render thread. Returns on [`RenderRequest::Stop`] or when the
/// engine thread goes away.
pub(crate) fn run(ctx: RenderContext) {
	let pattern = TestPatternRenderer::new(&ctx.device, VIEWPORT_FORMAT);
	let mut tiles: Option<TilePipeline> = None;
	let mut textures: [Option<wgpu::Texture>; 2] = [None, None];
	let mut next = 0;
	let mut frame: Option<Frame> = None;

	while let Ok(first) = ctx.requests.recv() {
		// Only the newest frame matters; wakes just mean "draw again".
		let mut stop = false;
		for request in std::iter::once(first).chain(ctx.requests.try_iter()) {
			match request {
				RenderRequest::Frame(f) => frame = Some(f),
				RenderRequest::Wake => {}
				RenderRequest::Stop => stop = true,
			}
		}
		if stop {
			break;
		}
		let Some(f) = &frame else { continue };
		let viewport = f.viewport;
		if viewport.width == 0 || viewport.height == 0 {
			continue;
		}

		let reuse = textures[next]
			.as_ref()
			.is_some_and(|t| t.width() == viewport.width && t.height() == viewport.height);
		if !reuse {
			textures[next] = Some(create_viewport_texture(&ctx.device, viewport));
		}
		let texture = textures[next].as_ref().expect("created above when missing or resized");
		let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
		let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("fx-viewport-frame"),
		});

		let mut again = false;
		match &f.doc {
			None => pattern.render(&ctx.queue, &mut encoder, &target, viewport, &f.view, f.virtual_doc),
			Some((id, doc)) => {
				let pipeline = tiles.get_or_insert_with(|| TilePipeline::new(&ctx));
				match pipeline.frame(&ctx, &mut encoder, &target, &f.view, viewport, *id, doc) {
					Ok(more) => again = more,
					Err(error) => {
						tracing::error!("cannot composite the document: {error}");
						// Show the pattern rather than a stale frame.
						pattern.render(&ctx.queue, &mut encoder, &target, viewport, &f.view, (doc.width, doc.height));
					}
				}
			}
		}
		ctx.queue.submit(std::iter::once(encoder.finish()));
		(ctx.output)(EngineOutput::ViewportFrame(texture.clone()));
		next ^= 1;
		if again {
			let _ = ctx.wake.send(RenderRequest::Wake);
		}
	}
	tracing::debug!("render thread finished");
}

/// Whether a planned tile is composited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ready {
	Slot(u32),
	Empty,
}

/// GPU compositor state for the documents (M1-T07).
struct TilePipeline {
	compositor: GpuCompositor,
	renderer: ViewportRenderer,
	luts: LutCache,
	/// Tiles composited for the current document revision.
	ready: HashMap<TileKey, Ready>,
	/// Programs built for the current document revision (reused every frame).
	programs: HashMap<TileKey, TileProgram>,
	/// Which document / revision / snapshot `ready` and `programs` belong to.
	current: Option<(DocId, u64)>,
	snapshot: Option<Arc<Document>>,
	/// Mip tiles already sent to the engine for this snapshot.
	mips_sent: HashSet<(LayerId, bool, usize, u32, u32)>,
	/// Tiles being loaded by rayon jobs.
	loading: Arc<Mutex<HashSet<TileId>>>,
}

impl TilePipeline {
	fn new(ctx: &RenderContext) -> Self {
		Self {
			compositor: GpuCompositor::new(&ctx.device, &ctx.queue, CompositorConfig::default()),
			renderer: ViewportRenderer::new(&ctx.device, &ctx.queue, VIEWPORT_FORMAT),
			luts: LutCache::default(),
			ready: HashMap::new(),
			programs: HashMap::new(),
			current: None,
			snapshot: None,
			mips_sent: HashSet::new(),
			loading: Arc::new(Mutex::new(HashSet::new())),
		}
	}

	/// Draw one frame of `doc`. Returns whether another frame should follow
	/// straight away (progress was made but the view is not complete yet).
	#[allow(clippy::too_many_arguments)]
	fn frame(
		&mut self,
		ctx: &RenderContext,
		encoder: &mut wgpu::CommandEncoder,
		target: &wgpu::TextureView,
		view: &ViewTransform,
		viewport: ViewportSize,
		id: DocId,
		doc: &Arc<Document>,
	) -> Result<bool, fx_render::gpu::CompositeError> {
		// A new document or revision invalidates everything cached for it; a
		// new snapshot of the same revision (mips committed) only allows the
		// failed tiles to be retried.
		if self.current != Some((id, doc.revision)) {
			self.current = Some((id, doc.revision));
			self.ready.clear();
			self.programs.clear();
		}
		if !self.snapshot.as_ref().is_some_and(|s| Arc::ptr_eq(s, doc)) {
			self.snapshot = Some(doc.clone());
			self.mips_sent.clear();
		}

		self.compositor.begin_frame();
		let levels = level_count(doc.width, doc.height);

		// Plan with what is ready, remembering every ready tile the plan looked
		// at: those must be composited (a cache hit) again this frame, or their
		// slots could be reused by other tiles while still on screen.
		let touched = RefCell::new(Vec::new());
		let plan = {
			let lookup = |key: TileKey| {
				let ready = self.ready.get(&key).copied();
				if ready.is_some() {
					touched.borrow_mut().push(key);
				}
				ready.map(slot_of)
			};
			plan_frame(view, viewport, doc.width, doc.height, levels, &lookup)
		};
		let mut keys: Vec<TileKey> = plan.requests.iter().copied().take(MAX_REQUESTS_PER_FRAME).collect();
		let mut seen: HashSet<TileKey> = keys.iter().copied().collect();
		keys.extend(touched.into_inner().into_iter().filter(|k| seen.insert(*k)));

		// Programs: cached per revision; tiles with dirty mips go to the engine.
		let mut mips = Vec::new();
		let mut programs = Vec::with_capacity(keys.len());
		let mut program_keys = Vec::with_capacity(keys.len());
		for key in keys {
			if !self.programs.contains_key(&key) {
				let luts = &mut self.luts;
				match build_program(doc, key.level, key.tx, key.ty, &mut |a| luts.get(a)) {
					Ok(program) => {
						self.programs.insert(key, program);
					}
					Err(missing) => {
						mips.extend(missing.into_iter().filter(|m| self.mips_sent.insert((m.layer, m.mask, m.level, m.x, m.y))));
						continue;
					}
				}
			}
			programs.push(self.programs[&key].clone());
			program_keys.push(key);
		}
		if !mips.is_empty() {
			let _ = ctx.mips.send(MipWork {
				doc: id,
				revision: doc.revision,
				requests: mips,
			});
		}

		// Composite. `hot` never blocks: RAM-resident tiles only.
		let store = &ctx.store;
		let outcomes = self.compositor.composite(&programs, &|handle| store.try_get_hot(handle))?;
		let mut progressed = false;
		let mut budget_spent = false;
		for (key, outcome) in program_keys.into_iter().zip(outcomes) {
			match outcome {
				TileOutcome::Ready { slot } => progressed |= self.ready.insert(key, Ready::Slot(slot)) != Some(Ready::Slot(slot)),
				TileOutcome::Empty => progressed |= self.ready.insert(key, Ready::Empty) != Some(Ready::Empty),
				TileOutcome::Deferred { missing } => {
					self.ready.remove(&key);
					if missing.is_empty() {
						budget_spent = true;
					}
					for handle in missing {
						self.load(ctx, handle);
					}
				}
			}
		}

		// Plan again with this frame's results, and draw.
		let mut plan: FramePlan = plan_frame(view, viewport, doc.width, doc.height, levels, &|key| self.ready.get(&key).copied().map(slot_of));
		plan.draws.retain(|d| d.slot != EMPTY_SLOT);
		self.renderer.render(
			encoder,
			target,
			(viewport.width, viewport.height),
			&plan,
			view.zoom,
			self.compositor.composite_view(),
		);
		Ok(!plan.complete && (progressed || budget_spent))
	}

	/// Bring a missing tile into RAM on the rayon pool, then wake the render
	/// thread. Each tile is loaded once, however often it is asked for.
	fn load(&self, ctx: &RenderContext, handle: fx_tiles::TileHandle) {
		let id = handle.id();
		if !self.loading.lock().expect("loader set poisoned").insert(id) {
			return;
		}
		let (store, loading, wake) = (ctx.store.clone(), self.loading.clone(), ctx.wake.clone());
		rayon::spawn(move || {
			if let Err(error) = store.get(&handle) {
				tracing::warn!("loading tile {id:?} failed: {error}");
			}
			loading.lock().expect("loader set poisoned").remove(&id);
			let _ = wake.send(RenderRequest::Wake);
		});
	}
}

fn slot_of(ready: Ready) -> u32 {
	match ready {
		Ready::Slot(slot) => slot,
		Ready::Empty => EMPTY_SLOT,
	}
}

/// Mip levels of a `width × height` document (same rule as `TiledImage::new`).
fn level_count(width: u32, height: u32) -> usize {
	let (mut w, mut h, mut levels) = (width, height, 1);
	while w > TILE_SIZE || h > TILE_SIZE {
		w = w.div_ceil(2);
		h = h.div_ceil(2);
		levels += 1;
	}
	levels
}

fn create_viewport_texture(device: &wgpu::Device, viewport: ViewportSize) -> wgpu::Texture {
	device.create_texture(&wgpu::TextureDescriptor {
		label: Some("fx-viewport"),
		size: wgpu::Extent3d {
			width: viewport.width,
			height: viewport.height,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: VIEWPORT_FORMAT,
		usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
		view_formats: &[],
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_tiles::{PixelFormat, TiledImage};

	#[test]
	fn level_count_matches_tiled_image() {
		for (w, h) in [(1, 1), (256, 256), (257, 10), (30_000, 30_000), (1000, 70_000)] {
			assert_eq!(level_count(w, h), TiledImage::new(w, h, PixelFormat::Rgba8).level_count(), "{w} × {h}");
		}
	}
}
