//! The engine side of the local AI (M13-T01): the document's composite read
//! at a bounded working resolution (from the mips, never level 0 of a big
//! document), and the model manager's actions.
//!
//! Also: the models run here (T02 Select Subject, T04 Object Selection) and
//! the input Generative Fill / Expand send to ComfyUI (T06). The masks leave
//! at the model's resolution; `SelectOp::Model` upsamples and refines them
//! in the edge band (M9-T05).
//!
//! FAST: the missing tiles a program asks for are served here on the
//! calling thread (mips, shape / vector-mask / effect tiles), up to three
//! rounds.

use std::sync::{Arc, Mutex};

use fx_ai::AiError;
use fx_ai::session::{Input, Model, Tensor32};
use fx_core::Document;
use fx_core::select_ops::{ModelKind, ModelMask};
use fx_render::adjust::LutCache;
use fx_render::blend::unpremultiply;
use fx_render::program::TileRequest;
use fx_tiles::{TILE_SIZE, TileStore};

/// The composite at the mip level whose long side is at most `max_side`:
/// straight RGBA `0..=1`, its size, and the level (document pixels per
/// working pixel = `2^level`).
pub struct Working {
	pub pixels: Vec<[f32; 4]>,
	pub width: usize,
	pub height: usize,
	pub level: usize,
	/// Canvas pixel of the working image's top-left corner (a crop's).
	pub origin: (i64, i64),
}

impl Working {
	pub fn scale(&self) -> f64 {
		f64::from(1u32 << self.level)
	}
}

/// Serve the tiles a program is missing (the frame loop's work, inline).
pub fn serve(doc: &mut Document, store: &TileStore, requests: &[TileRequest]) {
	crate::derived::fulfil(doc, store, requests);
}

/// Read the composite at a working resolution (M13-T01): the number of tiles
/// read is bounded by `max_side²`, whatever the document size.
pub fn working_composite(doc: &mut Document, store: &TileStore, max_side: u32) -> Result<Working, String> {
	let mut level = 0usize;
	while (doc.width.max(doc.height) >> level) > max_side && level < 16 {
		level += 1;
	}
	let (w, h) = (doc.width.div_ceil(1 << level).max(1) as usize, doc.height.div_ceil(1 << level).max(1) as usize);
	let pixels = composite_rect(doc, store, level, (0, 0, w as i64, h as i64))?;
	Ok(Working {
		pixels,
		width: w,
		height: h,
		level,
		origin: (0, 0),
	})
}

/// The composite of the canvas rectangle `rect` (`x0, y0, x1, y1`, clamped
/// to the canvas) at the finest mip level whose long side is at most
/// `max_side`: a crop read at up to full resolution, so a model sees a small
/// object with all its detail instead of a shrunk whole document.
pub fn working_crop(doc: &mut Document, store: &TileStore, rect: (i64, i64, i64, i64), max_side: u32) -> Result<Working, String> {
	let (w, h) = (i64::from(doc.width), i64::from(doc.height));
	let (x0, y0, x1, y1) = (rect.0.clamp(0, w), rect.1.clamp(0, h), rect.2.clamp(0, w), rect.3.clamp(0, h));
	if x1 <= x0 || y1 <= y0 {
		return Err("the area is outside the canvas".into());
	}
	let mut level = 0usize;
	while ((x1 - x0).max(y1 - y0) >> level) > i64::from(max_side) && level < 16 {
		level += 1;
	}
	let step = 1i64 << level;
	// Whole mip pixels, rounded outward.
	let (mx0, my0) = (x0.div_euclid(step), y0.div_euclid(step));
	let (mx1, my1) = ((x1 + step - 1).div_euclid(step), (y1 + step - 1).div_euclid(step));
	let pixels = composite_rect(doc, store, level, (mx0, my0, mx1, my1))?;
	Ok(Working {
		pixels,
		width: (mx1 - mx0) as usize,
		height: (my1 - my0) as usize,
		level,
		origin: (mx0 * step, my0 * step),
	})
}

/// The composite of the rectangle `(x0, y0, x1, y1)` (pixels of mip
/// `level`, exclusive; it may reach past the canvas, which reads
/// transparent): straight RGBA `0..=1`, row-major. The caller bounds it.
pub fn composite_rect(doc: &mut Document, store: &TileStore, level: usize, rect: (i64, i64, i64, i64)) -> Result<Vec<[f32; 4]>, String> {
	let (x0, y0, x1, y1) = rect;
	let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
	let mut pixels = vec![[0.0f32; 4]; w * h];
	let t = i64::from(TILE_SIZE);
	let lw = i64::from(doc.width.div_ceil(1 << level).max(1));
	let lh = i64::from(doc.height.div_ceil(1 << level).max(1));
	let (cx0, cy0, cx1, cy1) = (x0.max(0), y0.max(0), x1.min(lw), y1.min(lh));
	if cx0 >= cx1 || cy0 >= cy1 {
		return Ok(pixels);
	}
	let mut luts = LutCache::default();
	let mut tiles = Vec::new();
	for ty in cy0.div_euclid(t)..=(cy1 - 1).div_euclid(t) {
		for tx in cx0.div_euclid(t)..=(cx1 - 1).div_euclid(t) {
			tiles.push((tx as u32, ty as u32));
		}
	}
	// Code review 2026-09-27 R01: the derived inputs are computed (again, if
	// the trim dropped them) and held while each tile renders.
	let rendered =
		crate::derived::render_tiles(doc, store, level, &tiles, &mut luts).map_err(|e| format!("the document's tiles could not be prepared: {e}"))?;
	for tile in rendered {
		let (tx, ty) = (i64::from(tile.tile.0), i64::from(tile.tile.1));
		{
			let Some(tile) = tile.pixels else { continue };
			for py in 0..t {
				let y = ty * t + py;
				if y < cy0 || y >= cy1 {
					continue;
				}
				for px in 0..t {
					let x = tx * t + px;
					if x < cx0 || x >= cx1 {
						continue;
					}
					let p = tile[(py * t + px) as usize];
					let rgb = unpremultiply(p);
					pixels[(y - y0) as usize * w + (x - x0) as usize] = [rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, p[3] as f32];
				}
			}
		}
	}
	Ok(pixels)
}

/// `doc` reduced to layer `id`'s own content (visible, opaque, Normal, no
/// masks or styles: [`fx_core::command::content_alone`]) over neutral grey,
/// so the Object Selection tool sees the whole layer, the parts other layers
/// cover included. The grey stands in for transparency: a straight read of
/// it is black, where a dark object would vanish. `None` for an adjustment
/// layer (it has no content of its own) or an unknown id.
pub fn layer_alone(doc: &Document, id: fx_core::LayerId) -> Option<Document> {
	if matches!(doc.layer(id)?.kind, fx_core::LayerKind::Adjustment(_)) {
		return None;
	}
	let mut solo = fx_core::command::content_alone(doc, id)?;
	let backdrop = fx_core::Layer::new(
		solo.allocate_layer_id(),
		"backdrop",
		fx_core::LayerKind::SolidFill {
			rgba: [32768, 32768, 32768, 65535],
		},
	);
	solo.layers.insert(0, Arc::new(backdrop));
	Some(solo)
}

/// Bilinear resize of straight RGBA (pixel centres aligned).
pub fn resize(pixels: &[[f32; 4]], w: usize, h: usize, ow: usize, oh: usize) -> Vec<[f32; 4]> {
	let mut out = vec![[0.0f32; 4]; ow * oh];
	if w == 0 || h == 0 {
		return out;
	}
	for y in 0..oh {
		let fy = ((y as f32 + 0.5) * h as f32 / oh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
		let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
		let y1 = (y0 + 1).min(h - 1);
		for x in 0..ow {
			let fx = ((x as f32 + 0.5) * w as f32 / ow as f32 - 0.5).clamp(0.0, (w - 1) as f32);
			let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
			let x1 = (x0 + 1).min(w - 1);
			let (a, b, c, d) = (pixels[y0 * w + x0], pixels[y0 * w + x1], pixels[y1 * w + x0], pixels[y1 * w + x1]);
			out[y * ow + x] = [0, 1, 2, 3].map(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty);
		}
	}
	out
}

/// Resize the requested canvas rectangle from a mip read whose origin was
/// rounded outward to whole mip pixels.
#[allow(clippy::too_many_arguments)]
fn resize_canvas_rect(
	pixels: &[[f32; 4]],
	w: usize,
	h: usize,
	read_origin: (i64, i64),
	step: i64,
	rect: (i64, i64, i64, i64),
	ow: usize,
	oh: usize,
) -> Vec<[f32; 4]> {
	let mut out = vec![[0.0; 4]; ow * oh];
	if w == 0 || h == 0 || ow == 0 || oh == 0 || step <= 0 {
		return out;
	}
	let (rw, rh) = ((rect.2 - rect.0) as f64, (rect.3 - rect.1) as f64);
	for y in 0..oh {
		let doc_y = rect.1 as f64 + (y as f64 + 0.5) * rh / oh as f64;
		let fy = (doc_y / step as f64 - 0.5 - read_origin.1 as f64).clamp(0.0, (h - 1) as f64);
		let (y0, ty) = (fy.floor() as usize, (fy - fy.floor()) as f32);
		let y1 = (y0 + 1).min(h - 1);
		for x in 0..ow {
			let doc_x = rect.0 as f64 + (x as f64 + 0.5) * rw / ow as f64;
			let fx = (doc_x / step as f64 - 0.5 - read_origin.0 as f64).clamp(0.0, (w - 1) as f64);
			let (x0, tx) = (fx.floor() as usize, (fx - fx.floor()) as f32);
			let x1 = (x0 + 1).min(w - 1);
			let (a, b, c, d) = (pixels[y0 * w + x0], pixels[y0 * w + x1], pixels[y1 * w + x0], pixels[y1 * w + x1]);
			out[y * ow + x] =
				[0, 1, 2, 3].map(|channel| (a[channel] * (1.0 - tx) + b[channel] * tx) * (1.0 - ty) + (c[channel] * (1.0 - tx) + d[channel] * tx) * ty);
		}
	}
	out
}

/// Reads a selection's coverage at canvas points in increasing rows, keeping
/// one row of tiles (never the whole selection).
pub struct CoverageReader<'a> {
	selection: &'a fx_core::Selection,
	store: &'a TileStore,
	canvas: (u32, u32),
	row: Option<u32>,
	tiles: std::collections::HashMap<u32, fx_core::selection::TileCoverage>,
}

impl<'a> CoverageReader<'a> {
	pub fn new(selection: &'a fx_core::Selection, store: &'a TileStore, canvas: (u32, u32)) -> Self {
		Self {
			selection,
			store,
			canvas,
			row: None,
			tiles: Default::default(),
		}
	}

	pub fn at(&mut self, x: i64, y: i64) -> f32 {
		if x < 0 || y < 0 || x >= i64::from(self.canvas.0) || y >= i64::from(self.canvas.1) {
			return 0.0;
		}
		let t = i64::from(TILE_SIZE);
		let (tx, ty) = ((x / t) as u32, (y / t) as u32);
		if self.row != Some(ty) {
			self.row = Some(ty);
			self.tiles.clear();
		}
		let (selection, store) = (self.selection, self.store);
		let tile = self
			.tiles
			.entry(tx)
			.or_insert_with(|| selection.tile_coverage(store, tx, ty).unwrap_or(fx_core::selection::TileCoverage::Uniform(0.0)));
		tile.at((x % t) as u32, (y % t) as u32)
	}
}

/// A model loaded once per process and kept (M13-T01).
pub fn model(path: &std::path::Path) -> Result<Arc<Model>, AiError> {
	static MODELS: Mutex<Vec<(std::path::PathBuf, Arc<Model>)>> = Mutex::new(Vec::new());
	let mut models = MODELS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
	if let Some((_, m)) = models.iter().find(|(p, _)| p == path) {
		return Ok(m.clone());
	}
	// FAST: loaded under the lock, so two first uses wait for each other.
	let m = Arc::new(Model::load(path)?);
	models.push((path.to_owned(), m.clone()));
	Ok(m)
}

/// Load ONNX Runtime and every installed model on a background thread at
/// start, and run BiRefNet once on a blank image, so the first Select
/// Subject / Object Selection does not wait for the runtime, the DirectML
/// sessions and their first-run shader compilation.
pub fn warm() {
	use fx_ai::models::{BIREFNET, EFFICIENT_SAM};
	let spawned = std::thread::Builder::new().name("ai-warm".into()).spawn(|| {
		if !fx_ai::runtime::available() {
			return;
		}
		let started = std::time::Instant::now();
		if BIREFNET.installed() {
			let result = model(&BIREFNET.path(BIREFNET.files[0].file)).and_then(|m| {
				const SIDE: usize = 1024;
				m.run(vec![Tensor32::new(vec![1, 3, SIDE, SIDE], vec![0.0; 3 * SIDE * SIDE]).into()])
			});
			if let Err(error) = result {
				tracing::warn!("AI warm-up, {}: {error}", BIREFNET.name);
			}
		}
		if EFFICIENT_SAM.installed() {
			for file in EFFICIENT_SAM.files {
				if let Err(error) = model(&EFFICIENT_SAM.path(file.file)) {
					tracing::warn!("AI warm-up, {}: {error}", file.file);
				}
			}
		}
		tracing::info!("AI warm-up done in {:.2} s", started.elapsed().as_secs_f64());
	});
	if let Err(error) = spawned {
		tracing::warn!("AI warm-up thread: {error}");
	}
}

/// The inputs in the model's order, matched by name.
pub fn ordered(model: &Model, mut named: Vec<(&str, Input)>) -> Result<Vec<Input>, AiError> {
	let mut out = Vec::with_capacity(model.inputs.len());
	for name in &model.inputs {
		let i = named
			.iter()
			.position(|(n, _)| n == name)
			.ok_or_else(|| AiError::Inference(format!("the model wants an input named {name}")))?;
		out.push(named.swap_remove(i).1);
	}
	Ok(out)
}

/// Logits → `0..=255`.
pub fn to_u8(logits: &[f32]) -> Vec<u8> {
	logits.iter().map(|v| (fx_ai::image::sigmoid(*v) * 255.0).round() as u8).collect()
}

/// A `mw × mh` mask covering the working image (`width × height` at mip
/// `level`, its corner at canvas `origin`), placed on the canvas.
#[allow(clippy::too_many_arguments)]
pub fn mask_over(width: usize, height: usize, level: usize, origin: (i64, i64), mw: u32, mh: u32, values: Vec<u8>, kind: ModelKind) -> ModelMask {
	let scale = f64::from(1u32 << level);
	let rect = (
		origin.0,
		origin.1,
		origin.0 + (width as f64 * scale) as i64,
		origin.1 + (height as f64 * scale) as i64,
	);
	// The guided filter's window: about one and a half mask cells.
	let cell = (rect.2.max(rect.3) as f64 / f64::from(mw.max(mh).max(1))) as f32;
	ModelMask {
		kind,
		rect,
		width: mw,
		height: mh,
		values,
		refine_radius: (cell * 1.5).clamp(2.0, 32.0),
	}
}

/// Select Subject / Remove Background (M13-T02): BiRefNet on the working
/// image → a mask over the canvas rectangle the working image covers.
pub fn subject_mask(work: &Working) -> Result<ModelMask, AiError> {
	use fx_ai::models::BIREFNET;
	if !BIREFNET.installed() {
		return Err(AiError::Missing(BIREFNET.name.into()));
	}
	let model = model(&BIREFNET.path(BIREFNET.files[0].file))?;
	const SIDE: usize = 1024;
	let input = fx_ai::image::planar_rgb(
		&work.pixels,
		work.width,
		work.height,
		SIDE,
		SIDE,
		fx_ai::image::IMAGENET_MEAN,
		fx_ai::image::IMAGENET_STD,
	);
	let out = model.run(vec![Tensor32::new(vec![1, 3, SIDE, SIDE], input).into()])?;
	// The finest output (BiRefNet exports may list several scales).
	let best = out
		.iter()
		.rev()
		.find(|t| t.data.len() == SIDE * SIDE)
		.ok_or_else(|| AiError::Inference("BiRefNet gave no 1024² mask".into()))?;
	Ok(mask_over(
		work.width,
		work.height,
		work.level,
		work.origin,
		SIDE as u32,
		SIDE as u32,
		to_u8(&best.data),
		ModelKind::Subject,
	))
}

/// EfficientSAM's image embedding of a working image (M13-T04), cached by the
/// engine per document content.
pub struct Embedding {
	pub tensor: Tensor32,
	pub width: usize,
	pub height: usize,
	pub level: usize,
	/// Canvas pixel of the working image's corner (a crop's; else 0, 0).
	pub origin: (i64, i64),
}

pub fn sam_embedding(work: &Working) -> Result<Embedding, AiError> {
	use fx_ai::models::EFFICIENT_SAM;
	if !EFFICIENT_SAM.installed() {
		return Err(AiError::Missing(EFFICIENT_SAM.name.into()));
	}
	let encoder = model(&EFFICIENT_SAM.path(EFFICIENT_SAM.files[0].file))?;
	let (w, h) = (work.width, work.height);
	let input = fx_ai::image::planar_rgb(&work.pixels, w, h, w, h, [0.0; 3], [1.0; 3]);
	let out = encoder.run(vec![Tensor32::new(vec![1, 3, h, w], input).into()])?;
	let tensor = out.into_iter().next().ok_or_else(|| AiError::Inference("no embedding".into()))?;
	Ok(Embedding {
		tensor,
		width: w,
		height: h,
		level: work.level,
		origin: work.origin,
	})
}

/// The canvas area Object Selection reads for a box (`x0, y0, x1, y1`): the
/// box and a margin of context around it, clamped to the canvas. The model
/// then sees the object at up to full resolution, as Photoshop analyses the
/// region drawn, instead of a whole document shrunk to 1024 px (where a
/// 300 px object was 75 px wide and its outline a guess).
pub fn object_crop(boxed: [f64; 4], canvas: (u32, u32)) -> (i64, i64, i64, i64) {
	let (x0, x1) = (boxed[0].min(boxed[2]), boxed[0].max(boxed[2]));
	let (y0, y1) = (boxed[1].min(boxed[3]), boxed[1].max(boxed[3]));
	// VERIFY: how much context helps; a fifth of the box's long side.
	let margin = ((x1 - x0).max(y1 - y0) * 0.2).max(32.0);
	(
		((x0 - margin).floor() as i64).max(0),
		((y0 - margin).floor() as i64).max(0),
		((x1 + margin).ceil() as i64).min(i64::from(canvas.0)),
		((y1 + margin).ceil() as i64).min(i64::from(canvas.1)),
	)
}

/// The canvas box around a mask's selected part (values ≥ 128); `None`
/// when nothing is selected.
pub fn mask_bounds(mask: &ModelMask) -> Option<[f64; 4]> {
	let (w, h) = (mask.width as usize, mask.height as usize);
	let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
	for y in 0..h {
		for x in 0..w {
			if mask.values[y * w + x] >= 128 {
				(x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
			}
		}
	}
	if x0 >= x1 {
		return None;
	}
	let (rx0, ry0, rx1, ry1) = mask.rect;
	let (sx, sy) = ((rx1 - rx0) as f64 / w as f64, (ry1 - ry0) as f64 / h as f64);
	Some([
		rx0 as f64 + x0 as f64 * sx,
		ry0 as f64 + y0 as f64 * sy,
		rx0 as f64 + x1 as f64 * sx,
		ry0 as f64 + y1 as f64 * sy,
	])
}

/// Object Selection with BiRefNet (the model Select Subject uses) instead of
/// EfficientSAM: the area around the box is read at up to full resolution,
/// BiRefNet finds what stands out in it, and whatever lies outside the
/// prompt (the box, or the lasso's outline, with a small tolerance) is cut
/// away. Much finer edges than the small SAM (hair, holes, thin parts), at
/// the price of needing an object that stands out from its surroundings.
pub fn birefnet_object(doc: &mut Document, store: &TileStore, boxed: [f64; 4], lasso: &[(f64, f64)], max_side: u32) -> Result<ModelMask, String> {
	let area = object_crop(boxed, (doc.width, doc.height));
	let work = working_crop(doc, store, area, max_side)?;
	let mut mask = subject_mask(&work).map_err(|e| e.to_string())?;
	mask.kind = ModelKind::Object;
	clip_to_prompt(&mut mask, boxed, lasso);
	if mask.values.iter().all(|&v| v < 128) {
		return Err("No object found there: draw the box or lasso closer around it".into());
	}
	Ok(mask)
}

/// A click with BiRefNet: the whole document says where the object under the
/// click is (the salient part connected to the click), then that object is
/// read again as a crop at up to full resolution and only the part
/// connected to the click is kept. `None` when nothing salient is near.
pub fn birefnet_click(doc: &mut Document, store: &TileStore, at: (f64, f64), max_side: u32) -> Result<Option<ModelMask>, String> {
	let work = working_composite(doc, store, max_side)?;
	let coarse = subject_mask(&work).map_err(|e| e.to_string())?;
	let Some([x0, y0, x1, y1]) = component_bounds(&coarse, at) else { return Ok(None) };
	let (cw, ch) = (f64::from(doc.width), f64::from(doc.height));
	let pad = (x1 - x0).max(y1 - y0) * 0.08;
	let boxed = [(x0 - pad).max(0.0), (y0 - pad).max(0.0), (x1 + pad).min(cw), (y1 + pad).min(ch)];
	let area = object_crop(boxed, (doc.width, doc.height));
	let work = working_crop(doc, store, area, max_side)?;
	let mut mask = subject_mask(&work).map_err(|e| e.to_string())?;
	mask.kind = ModelKind::Object;
	clip_to_prompt(&mut mask, boxed, &[]);
	keep_component(&mut mask, at);
	Ok(mask.values.iter().any(|&v| v >= 128).then_some(mask))
}

/// The mask cell holding canvas `(x, y)`, clamped into the mask.
fn cell_of(mask: &ModelMask, (x, y): (f64, f64)) -> (usize, usize) {
	let (rx0, ry0, rx1, ry1) = mask.rect;
	let (w, h) = (mask.width as usize, mask.height as usize);
	let cx = ((x - rx0 as f64) / (rx1 - rx0).max(1) as f64 * w as f64).floor();
	let cy = ((y - ry0 as f64) / (ry1 - ry0).max(1) as f64 * h as f64).floor();
	((cx.max(0.0) as usize).min(w.saturating_sub(1)), (cy.max(0.0) as usize).min(h.saturating_sub(1)))
}

/// The selected cell (≥ 128) nearest to `at` within a few percent of the
/// mask, so a click just off a thin object still finds it.
fn seed_near(mask: &ModelMask, at: (f64, f64)) -> Option<(usize, usize)> {
	let (w, h) = (mask.width as usize, mask.height as usize);
	let (sx, sy) = cell_of(mask, at);
	let radius = (w.max(h) / 40).max(2) as i64;
	let mut best: Option<((usize, usize), i64)> = None;
	for dy in -radius..=radius {
		for dx in -radius..=radius {
			let (x, y) = (sx as i64 + dx, sy as i64 + dy);
			if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
				continue;
			}
			let d = dx * dx + dy * dy;
			if mask.values[y as usize * w + x as usize] >= 128 && best.is_none_or(|(_, b)| d < b) {
				best = Some(((x as usize, y as usize), d));
			}
		}
	}
	best.map(|(p, _)| p)
}

/// The cells connected to `seed` whose value is at least `floor`.
fn flood(mask: &ModelMask, seed: (usize, usize), floor: u8) -> Vec<bool> {
	let (w, h) = (mask.width as usize, mask.height as usize);
	let mut inside = vec![false; w * h];
	let mut stack = vec![seed];
	while let Some((x, y)) = stack.pop() {
		let i = y * w + x;
		if inside[i] || mask.values[i] < floor {
			continue;
		}
		inside[i] = true;
		if x > 0 {
			stack.push((x - 1, y));
		}
		if x + 1 < w {
			stack.push((x + 1, y));
		}
		if y > 0 {
			stack.push((x, y - 1));
		}
		if y + 1 < h {
			stack.push((x, y + 1));
		}
	}
	inside
}

/// The canvas box of the salient part connected to `at`.
fn component_bounds(mask: &ModelMask, at: (f64, f64)) -> Option<[f64; 4]> {
	let seed = seed_near(mask, at)?;
	let inside = flood(mask, seed, 128);
	let mut part = ModelMask {
		values: inside.iter().map(|&b| if b { 255 } else { 0 }).collect(),
		..mask.clone()
	};
	part.kind = mask.kind;
	mask_bounds(&part)
}

/// Keep only the part connected to `at` (its soft edge included).
fn keep_component(mask: &mut ModelMask, at: (f64, f64)) {
	let Some(seed) = seed_near(mask, at) else {
		mask.values.fill(0);
		return;
	};
	// A low floor keeps the anti-aliased fringe around the object.
	let inside = flood(mask, seed, 8);
	for (v, keep) in mask.values.iter_mut().zip(inside) {
		if !keep {
			*v = 0;
		}
	}
}

/// Cut away what lies outside the prompt: the box (a little tolerance), or
/// the lasso's outline (with the same tolerance, so an outline drawn a bit
/// tight does not slice the object).
fn clip_to_prompt(mask: &mut ModelMask, boxed: [f64; 4], lasso: &[(f64, f64)]) {
	let (x0, x1) = (boxed[0].min(boxed[2]), boxed[0].max(boxed[2]));
	let (y0, y1) = (boxed[1].min(boxed[3]), boxed[1].max(boxed[3]));
	let tol = ((x1 - x0).max(y1 - y0) * 0.03).max(4.0);
	// At most ~256 vertices: the tolerance hides the rest.
	let step = lasso.len().div_ceil(256).max(1);
	let poly: Vec<(f64, f64)> = lasso.iter().step_by(step).copied().collect();
	let (rx0, ry0, rx1, ry1) = mask.rect;
	let (w, h) = (mask.width as usize, mask.height as usize);
	let (sx, sy) = ((rx1 - rx0) as f64 / w as f64, (ry1 - ry0) as f64 / h as f64);
	let tol2 = tol * tol;
	for cy in 0..h {
		let y = ry0 as f64 + (cy as f64 + 0.5) * sy;
		for cx in 0..w {
			let i = cy * w + cx;
			if mask.values[i] == 0 {
				continue;
			}
			let x = rx0 as f64 + (cx as f64 + 0.5) * sx;
			let keep = if poly.len() >= 3 {
				point_in_polygon(&poly, x, y) || near_outline(&poly, x, y, tol2)
			} else {
				x >= x0 - tol && x <= x1 + tol && y >= y0 - tol && y <= y1 + tol
			};
			if !keep {
				mask.values[i] = 0;
			}
		}
	}
}

fn point_in_polygon(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
	let mut inside = false;
	let mut j = poly.len() - 1;
	for i in 0..poly.len() {
		let ((xi, yi), (xj, yj)) = (poly[i], poly[j]);
		if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
			inside = !inside;
		}
		j = i;
	}
	inside
}

fn near_outline(poly: &[(f64, f64)], x: f64, y: f64, tol2: f64) -> bool {
	let mut j = poly.len() - 1;
	for i in 0..poly.len() {
		let ((ax, ay), (bx, by)) = (poly[j], poly[i]);
		let (dx, dy) = (bx - ax, by - ay);
		let len2 = dx * dx + dy * dy;
		let t = if len2 > 0.0 { (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
		let (px, py) = (ax + t * dx - x, ay + t * dy - y);
		if px * px + py * py <= tol2 {
			return true;
		}
		j = i;
	}
	false
}

/// A click's second pass (Object Selection): the first mask, from the whole
/// document at ≤ `max_side`, only says *where* the object is; the object is
/// then read again as a crop around it, at up to full resolution, with the
/// same clicks and its box as the prompt. The coarse mask is kept when it
/// covers most of the canvas (a crop would gain nothing).
pub fn click_refine(doc: &mut Document, store: &TileStore, coarse: ModelMask, points: &[((f64, f64), bool)], max_side: u32) -> Result<ModelMask, String> {
	let Some([x0, y0, x1, y1]) = mask_bounds(&coarse) else { return Ok(coarse) };
	let (cw, ch) = (f64::from(doc.width), f64::from(doc.height));
	if (x1 - x0) * (y1 - y0) > 0.6 * cw * ch {
		return Ok(coarse);
	}
	// The coarse outline can miss thin parts: a looser box.
	let pad = (x1 - x0).max(y1 - y0) * 0.08;
	let boxed = [(x0 - pad).max(0.0), (y0 - pad).max(0.0), (x1 + pad).min(cw), (y1 + pad).min(ch)];
	let area = object_crop(boxed, (doc.width, doc.height));
	let work = working_crop(doc, store, area, max_side)?;
	let embedding = sam_embedding(&work).map_err(|e| e.to_string())?;
	sam_mask(&embedding, Some(boxed), points).map_err(|e| e.to_string())
}

/// The Object Selection decoder: a box and points in **canvas** pixels.
pub fn sam_mask(embedding: &Embedding, boxed: Option<[f64; 4]>, points: &[((f64, f64), bool)]) -> Result<ModelMask, AiError> {
	use fx_ai::models::EFFICIENT_SAM;
	let decoder = model(&EFFICIENT_SAM.path(EFFICIENT_SAM.files[1].file))?;
	let s = f64::from(1u32 << embedding.level);
	let (ox, oy) = (embedding.origin.0 as f64, embedding.origin.1 as f64);
	let boxed = boxed.map(|[x0, y0, x1, y1]| [((x0 - ox) / s) as f32, ((y0 - oy) / s) as f32, ((x1 - ox) / s) as f32, ((y1 - oy) / s) as f32]);
	let points: Vec<((f32, f32), bool)> = points.iter().map(|((x, y), p)| ((((x - ox) / s) as f32, ((y - oy) / s) as f32), *p)).collect();
	let (coords, labels, size) = fx_ai::sam::prompt(boxed, &points, (embedding.width, embedding.height));
	let inputs = ordered(
		&decoder,
		vec![
			("image_embeddings", Input::F32(embedding.tensor.clone())),
			("batched_point_coords", coords),
			("batched_point_labels", labels),
			("orig_im_size", size),
		],
	)?;
	let out = decoder.run(inputs)?;
	let find = |name: &str| decoder.outputs.iter().position(|o| o == name).map(|i| &out[i]);
	let (masks, iou) = match (find("output_masks"), find("iou_predictions")) {
		(Some(m), Some(i)) => (m, i),
		_ if out.len() >= 2 => (&out[0], &out[1]),
		_ => return Err(AiError::Inference("EfficientSAM's decoder gave too few outputs".into())),
	};
	let best = fx_ai::sam::best_mask(masks, iou).ok_or_else(|| AiError::Inference("EfficientSAM gave no mask".into()))?;
	let (mh, mw) = match masks.shape.as_slice() {
		[.., h, w] => (*h, *w),
		_ => (embedding.height, embedding.width),
	};
	Ok(mask_over(
		embedding.width,
		embedding.height,
		embedding.level,
		embedding.origin,
		mw as u32,
		mh as u32,
		to_u8(&best),
		ModelKind::Object,
	))
}

/// What Generative Fill / Expand send to ComfyUI (M13-T06): the area around
/// the selection at the model's resolution, alpha 0 where new content goes.
pub struct GenInput {
	pub image: fx_ai::comfy::Image,
	/// The canvas rectangle the image covers.
	pub rect: (i64, i64, i64, i64),
	/// Expand: the new area as a mask (Fill uses the selection).
	pub mask: Option<ModelMask>,
}

/// The model's size for a `w × h` area: long side `long`, multiples of 8.
pub fn model_size(w: i64, h: i64, long: u32) -> (u32, u32) {
	let (w, h) = (w.max(1) as f64, h.max(1) as f64);
	let s = f64::from(long) / w.max(h);
	let r8 = |v: f64| ((v / 8.0).round() as u32 * 8).max(64);
	(r8(w * s), r8(h * s))
}

/// Read `rect` (canvas) at the model's size; `coverage(x, y)` says how much
/// of each canvas point is to be generated (`1` = new content).
pub fn generative_input(
	doc: &mut Document,
	store: &TileStore,
	rect: (i64, i64, i64, i64),
	long: u32,
	coverage: &mut dyn FnMut(i64, i64) -> f32,
	plain_mask: bool,
) -> Result<GenInput, String> {
	let (rw, rh) = (rect.2 - rect.0, rect.3 - rect.1);
	let (ow, oh) = model_size(rw, rh, long);
	// Read at the mip level that is at most twice the model's size.
	let mut level = 0usize;
	while (rw.max(rh) >> level) > i64::from(long) * 2 && level < 16 {
		level += 1;
	}
	let step = 1i64 << level;
	let lr = (
		rect.0.div_euclid(step),
		rect.1.div_euclid(step),
		(rect.2 + step - 1).div_euclid(step),
		(rect.3 + step - 1).div_euclid(step),
	);
	let read = composite_rect(doc, store, level, lr)?;
	let pixels = resize_canvas_rect(
		&read,
		(lr.2 - lr.0) as usize,
		(lr.3 - lr.1) as usize,
		(lr.0, lr.1),
		step,
		rect,
		ow as usize,
		oh as usize,
	);
	let mut rgba = Vec::with_capacity(pixels.len() * 4);
	let mut values = Vec::with_capacity(pixels.len());
	for y in 0..oh {
		let cy = rect.1 + ((f64::from(y) + 0.5) * rh as f64 / f64::from(oh)) as i64;
		for x in 0..ow {
			let cx = rect.0 + ((f64::from(x) + 0.5) * rw as f64 / f64::from(ow)) as i64;
			let p = pixels[(y * ow + x) as usize];
			let k = coverage(cx, cy).clamp(0.0, 1.0);
			// Transparent pixels are new content too (an empty area).
			let keep = (1.0 - k) * p[3].clamp(0.0, 1.0);
			let over_grey = |c: f32| c * p[3] + 0.5 * (1.0 - p[3]);
			rgba.extend([over_grey(p[0]), over_grey(p[1]), over_grey(p[2]), keep].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
			values.push((k * 255.0).round() as u8);
		}
	}
	Ok(GenInput {
		image: fx_ai::comfy::Image { width: ow, height: oh, rgba },
		rect,
		mask: plain_mask.then_some(ModelMask {
			kind: ModelKind::Plain,
			rect,
			width: ow,
			height: oh,
			values,
			refine_radius: 0.0,
		}),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn generative_fill_resampling_accounts_for_outward_rounded_mip_origin() {
		let pixels: Vec<[f32; 4]> = [10.0, 20.0, 30.0, 40.0].into_iter().map(|value| [value; 4]).collect();
		let resized = resize_canvas_rect(&pixels, 4, 1, (1, 0), 4, (5, 0, 13, 4), 2, 1);
		assert!((resized[0][0] - 12.5).abs() < 0.001);
		assert!((resized[1][0] - 22.5).abs() < 0.001);
	}
}
