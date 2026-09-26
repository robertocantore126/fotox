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
use fx_render::build_program;
use fx_render::program::TileRequest;
use fx_render::reference::render_tile;
use fx_tiles::{TILE_SIZE, TileStore};

/// The composite at the mip level whose long side is at most `max_side`:
/// straight RGBA `0..=1`, its size, and the level (document pixels per
/// working pixel = `2^level`).
pub struct Working {
	pub pixels: Vec<[f32; 4]>,
	pub width: usize,
	pub height: usize,
	pub level: usize,
}

impl Working {
	pub fn scale(&self) -> f64 {
		f64::from(1u32 << self.level)
	}
}

/// Serve the tiles a program is missing (the frame loop's work, inline).
pub fn serve(doc: &mut Document, store: &TileStore, requests: &[TileRequest]) {
	let mut shapes = Vec::new();
	let mut masks = Vec::new();
	let mut effects = Vec::new();
	for request in requests {
		match request {
			TileRequest::Mip(r) => {
				let Some(layer) = doc.layer_mut(r.layer) else { continue };
				let image = if r.mask {
					match layer.mask.as_mut() {
						Some(mask) => &mut mask.image,
						None => continue,
					}
				} else {
					match &mut layer.kind {
						fx_core::LayerKind::Pixel { image, .. } => image,
						_ => continue,
					}
				};
				if let Err(error) = crate::mips::ensure_mip(image, store, r.level, r.x, r.y) {
					tracing::warn!("mip {r:?} failed: {error}");
				}
			}
			TileRequest::Vector(r) if r.vector_mask => masks.push((r.layer, r.level, r.x, r.y)),
			TileRequest::Vector(r) => shapes.push((r.layer, r.level, r.x, r.y)),
			TileRequest::Effect(r) => effects.push((r.layer, r.effect, r.level, r.x, r.y)),
		}
	}
	if !shapes.is_empty() {
		crate::vector::draw_requests(doc, store, &shapes);
	}
	if !masks.is_empty() {
		crate::vector::draw_vector_mask_requests(doc, store, &masks);
	}
	if !effects.is_empty() {
		crate::effects::draw_effect_requests(doc, store, &effects);
	}
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
	for ty in cy0.div_euclid(t)..=(cy1 - 1).div_euclid(t) {
		for tx in cx0.div_euclid(t)..=(cx1 - 1).div_euclid(t) {
			let mut program = None;
			for _ in 0..4 {
				match build_program(doc, level, tx as u32, ty as u32, &mut |a| luts.get(a)) {
					Ok(p) => {
						program = Some(p);
						break;
					}
					Err(requests) => serve(doc, store, &requests),
				}
			}
			let program = program.ok_or_else(|| "the document's tiles could not be prepared".to_owned())?;
			let fetch = |handle: &fx_tiles::TileHandle| store.get(handle).expect("tile of a live document");
			let tile = render_tile(&program, &fetch);
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

/// A `mw × mh` mask covering the working image, placed on the canvas.
pub fn mask_over(width: usize, height: usize, level: usize, mw: u32, mh: u32, values: Vec<u8>, kind: ModelKind) -> ModelMask {
	let scale = f64::from(1u32 << level);
	let rect = (0, 0, (width as f64 * scale) as i64, (height as f64 * scale) as i64);
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
	})
}

/// The Object Selection decoder: a box and points in **canvas** pixels.
pub fn sam_mask(embedding: &Embedding, boxed: Option<[f64; 4]>, points: &[((f64, f64), bool)]) -> Result<ModelMask, AiError> {
	use fx_ai::models::EFFICIENT_SAM;
	let decoder = model(&EFFICIENT_SAM.path(EFFICIENT_SAM.files[1].file))?;
	let s = f64::from(1u32 << embedding.level);
	let boxed = boxed.map(|b| b.map(|v| (v / s) as f32));
	let points: Vec<((f32, f32), bool)> = points.iter().map(|((x, y), p)| (((x / s) as f32, (y / s) as f32), *p)).collect();
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
	let pixels = resize(&read, (lr.2 - lr.0) as usize, (lr.3 - lr.1) as usize, ow as usize, oh as usize);
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
