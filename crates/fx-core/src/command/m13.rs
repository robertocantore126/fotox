//! M13's commands: Remove Background (T02) and the generative layers of
//! Generative Fill / Expand (T06).
//!
//! The models run before the command, on the engine's AI job; the command
//! carries their outputs at the model's resolution (bounded, rule 2), so a
//! replay needs neither the model nor ComfyUI.

use fx_tiles::TILE_SIZE;

use super::*;
use crate::select_ops::{ModelMask, SelectOp};

/// One generated image: `width × height` straight RGBA8, row-major.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variation {
	pub width: u32,
	pub height: u32,
	pub rgba: Vec<u8>,
}

impl Variation {
	/// Straight RGBA `0..=1` at `(u, v)` in image pixels (centres at +0.5),
	/// bilinear, clamped at the edges.
	fn sample(&self, u: f64, v: f64) -> [f32; 4] {
		let (w, h) = (self.width as usize, self.height as usize);
		let fx = (u - 0.5).clamp(0.0, (w - 1) as f64);
		let fy = (v - 0.5).clamp(0.0, (h - 1) as f64);
		let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
		let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
		let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
		let px = |x: usize, y: usize| {
			let i = (y * w + x) * 4;
			[0, 1, 2, 3].map(|c| f32::from(self.rgba[i + c]) / 255.0)
		};
		let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
		[0, 1, 2, 3].map(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
	}
}

/// The selection a model mask makes, through the engine's `select_op`.
fn model_selection(doc: &Document, mask: &ModelMask, ctx: &CommandContext<'_>, what: &str) -> Result<Selection, CommandError> {
	let ops = pixel_ops(ctx, what)?;
	ops.select_op(doc, &SelectOp::Model(mask.clone()), ctx.tiles)?
		.ok_or_else(|| CommandError::NotAllowed("the model found nothing".into()))
}

/// Properties ▸ Remove Background (M13-T02): the model's subject mask as a
/// layer mask on `layer`. One history step; the selection is left alone.
pub(super) fn mask_from_model(doc: &mut Document, layer: &LayerRef, mask: &ModelMask, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let found = model_selection(doc, mask, ctx, "Remove Background")?;
	let id = resolve(doc, layer)?;
	if doc.layer(id).is_some_and(|l| l.mask.is_some()) {
		// FAST: Photoshop intersects with the existing mask; this refuses.
		return Err(CommandError::NotAllowed("the layer already has a mask".into()));
	}
	let kept = doc.selection.replace(found);
	let result = add_mask(doc, &LayerRef::Id(id), MaskFill::RevealSelection, ctx.tiles);
	doc.selection = kept;
	let mut effect = result?;
	effect.label = "Remove Background".into();
	Ok(effect)
}

/// Generative Fill / Expand (M13-T06): a group named after the prompt with
/// one pixel layer per variation (the first visible), each masked by the
/// selection (or `mask`, for Expand). The images cover the canvas
/// rectangle `rect`, stretched bilinearly.
#[allow(clippy::too_many_arguments)]
pub(super) fn generative_layer(
	doc: &mut Document,
	name: &str,
	rect: (i64, i64, i64, i64),
	variations: &[Variation],
	mask: Option<&ModelMask>,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if variations.is_empty() {
		return Err(CommandError::NotAllowed("nothing was generated".into()));
	}
	for v in variations {
		if v.width == 0 || v.height == 0 || v.rgba.len() != v.width as usize * v.height as usize * 4 {
			return Err(CommandError::NotAllowed("a generated image is malformed".into()));
		}
	}
	let selection = match mask {
		Some(mask) => model_selection(doc, mask, ctx, "Generative Expand")?,
		None => doc
			.selection
			.clone()
			.ok_or_else(|| CommandError::NotAllowed("Generative Fill needs a selection".into()))?,
	};
	let canvas = (doc.width, doc.height);
	let format = doc.color.depth.rgba_format();
	let gray = doc.color.depth.gray_format();
	let tile = i64::from(TILE_SIZE);
	// Canvas tiles under the rectangle.
	let (x0, y0) = (rect.0.max(0), rect.1.max(0));
	let (x1, y1) = (rect.2.min(i64::from(canvas.0)), rect.3.min(i64::from(canvas.1)));
	if x0 >= x1 || y0 >= y1 {
		return Err(CommandError::NotAllowed("the generated area is off the canvas".into()));
	}
	let (rw, rh) = ((rect.2 - rect.0).max(1) as f64, (rect.3 - rect.1).max(1) as f64);
	// One mask image, shared by every variation (tiles are shared handles).
	let mask_image = crate::pixels::mask_from_selection(&selection, canvas, (0, 0), canvas, false, gray, ctx.tiles)?;
	let mut children = Vec::with_capacity(variations.len());
	for (i, variation) in variations.iter().enumerate() {
		let mut image = TiledImage::new(canvas.0, canvas.1, format);
		let (sx, sy) = (f64::from(variation.width) / rw, f64::from(variation.height) / rh);
		let tiles: Vec<(i64, i64)> = (y0.div_euclid(tile)..=(y1 - 1).div_euclid(tile))
			.flat_map(|ty| (x0.div_euclid(tile)..=(x1 - 1).div_euclid(tile)).map(move |tx| (tx, ty)))
			.collect();
		let buffers: Vec<((i64, i64), TileBuffer)> = tiles
			.par_iter()
			.map(|&(tx, ty)| {
				let mut pixels = vec![[0.0f32; 4]; TILE_PIXELS];
				for py in 0..tile {
					let cy = ty * tile + py;
					if cy < y0 || cy >= y1 {
						continue;
					}
					for px in 0..tile {
						let cx = tx * tile + px;
						if cx < x0 || cx >= x1 {
							continue;
						}
						let u = (cx - rect.0) as f64 + 0.5;
						let v = (cy - rect.1) as f64 + 0.5;
						let mut p = variation.sample(u * sx, v * sy);
						// FAST: the generated alpha is ignored (the mask cuts).
						p[3] = 1.0;
						pixels[(py * tile + px) as usize] = p;
					}
				}
				((tx, ty), crate::pixels::encode(&pixels, format))
			})
			.collect();
		for ((tx, ty), buffer) in buffers {
			image.put_buffer(ctx.tiles, tx as u32, ty as u32, buffer);
		}
		let id = doc.allocate_layer_id();
		let mut layer = Layer::new(id, format!("Variation {}", i + 1), LayerKind::Pixel { image, offset: (0, 0) });
		layer.visible = i == 0;
		layer.mask = Some(Mask {
			image: mask_image.clone(),
			enabled: true,
			linked: true,
			outside_value: 0,
		});
		children.push(Arc::new(layer));
	}
	// Children are bottom → top: the first variation on top, visible.
	children.reverse();
	let group_id = doc.allocate_layer_id();
	let group = Layer::new(group_id, name, LayerKind::Group { children, expanded: true });
	insert_above_active(doc, Arc::new(group));
	doc.selected = vec![group_id];
	Ok(CommandEffect {
		label: if mask.is_some() { "Generative Expand" } else { "Generative Fill" }.into(),
		structure_changed: true,
		..Default::default()
	})
}
