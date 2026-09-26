//! M8's pixel commands: Paint Bucket and Magic Eraser (T02); gradient,
//! pattern and red-eye commands join them in later cards.

use super::*;
use fx_tiles::TILE_SIZE;

use crate::fill::FillSource;

/// The wand's region at `params`, limited by the selection. `None` when the
/// wand finds nothing there.
fn wand_region(doc: &Document, params: &WandParams, ctx: &CommandContext<'_>, what: &str) -> Result<Option<Selection>, CommandError> {
	if !(0.0..=255.0).contains(&params.tolerance) {
		return Err(CommandError::InvalidValue {
			field: "tolerance",
			reason: format!("{} is not in 0..=255", params.tolerance),
		});
	}
	let ops = pixel_ops(ctx, what)?;
	let size = (doc.width, doc.height);
	let Some(found) = ops.magic_wand(doc, params, ctx.tiles)? else {
		return Ok(None);
	};
	Ok(match &doc.selection {
		Some(selection) => selection::combine(size, Some(selection), &found, SelectMode::Intersect, ctx.tiles)?,
		None => Some(found),
	})
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bucket_fill(
	doc: &mut Document,
	layer: &LayerRef,
	params: &WandParams,
	source: &FillSource,
	mode: BlendMode,
	opacity: f64,
	preserve_transparency: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if !(0.0..=1.0).contains(&opacity) {
		return Err(CommandError::InvalidValue {
			field: "opacity",
			reason: format!("{opacity} is not in 0..=1"),
		});
	}
	let (id, image, offset) = pixel_target(doc, layer)?;
	// FAST: the wand reads the active layer; the bucket fills `layer`, which
	// the tool always sends as the active one.
	let Some(region) = wand_region(doc, params, ctx, "the Paint Bucket")? else {
		return Err(CommandError::NotAllowed("nothing to fill there".into()));
	};
	let locked_alpha = doc.layer(id).is_some_and(|l| l.locked_transparency);
	let placed = crate::pixels::Placed { image: &image, offset };
	let canvas = (doc.width, doc.height);
	let (filled, offset) = match source {
		FillSource::Color { rgba } => {
			let spec = crate::pixels::FillSpec {
				color: *rgba,
				mode,
				opacity,
				preserve_transparency: preserve_transparency || locked_alpha,
			};
			crate::pixels::fill(placed, Some(&region), canvas, &spec, ctx.tiles)?
		}
		FillSource::Pattern { pattern } => {
			let paint = pattern_paint(doc, *pattern)?;
			crate::pixels::fill_with(
				placed,
				Some(&region),
				canvas,
				mode,
				opacity,
				preserve_transparency || locked_alpha,
				&paint,
				ctx.tiles,
			)?
		}
	};
	set_pixels(doc, id, filled, offset);
	Ok(CommandEffect {
		label: "Paint Bucket".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

/// A document pattern as a paint function (canvas pixel → straight RGBA).
pub(super) fn pattern_paint(doc: &Document, id: u64) -> Result<impl Fn(i64, i64) -> [f64; 4] + Sync + use<>, CommandError> {
	let pattern = doc
		.patterns
		.iter()
		.find(|p| p.id == id)
		.cloned()
		.ok_or_else(|| CommandError::NotAllowed("the pattern is not in this document".into()))?;
	Ok(move |x: i64, y: i64| pattern.at(x, y))
}

pub(super) fn magic_erase(
	doc: &mut Document,
	layer: &LayerRef,
	params: &WandParams,
	opacity: f64,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let (id, image, offset) = pixel_target(doc, layer)?;
	let Some(region) = wand_region(doc, params, ctx, "the Magic Eraser")? else {
		return Err(CommandError::NotAllowed("nothing to erase there".into()));
	};
	// FAST: opacity below 100 % erases fully (the coverage is not scaled).
	let _ = opacity;
	let layer_ref = doc.layer(id).expect("resolved id exists");
	let locked_alpha = layer_ref.locked_transparency;
	let background = layer_ref.name == "Background" && layer_ref.locked_position;
	let placed = crate::pixels::Placed { image: &image, offset };
	let canvas = (doc.width, doc.height);
	let erased = if locked_alpha {
		// Photoshop paints the background colour then; FAST: nothing happens.
		return Err(CommandError::NotAllowed("the layer's transparency is locked".into()));
	} else {
		crate::pixels::clear(placed, &region, canvas, ctx.tiles)?
	};
	set_pixels(doc, id, erased, offset);
	let mut effect = CommandEffect {
		label: "Magic Eraser".into(),
		pixels_changed: vec![id],
		..Default::default()
	};
	if background {
		let target = doc.layer_mut(id).expect("resolved id exists");
		target.name = "Layer 0".into();
		target.locked_position = false;
		effect.props_changed.push(id);
		effect.structure_changed = true;
	}
	Ok(effect)
}

pub(super) fn fill_gradient(
	doc: &mut Document,
	layer: &LayerRef,
	fill: &crate::gradient::GradientFill,
	mode: BlendMode,
	opacity: f64,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if !(0.0..=1.0).contains(&opacity) {
		return Err(CommandError::InvalidValue {
			field: "opacity",
			reason: format!("{opacity} is not in 0..=1"),
		});
	}
	if fill.gradient.colors.is_empty() {
		return Err(CommandError::InvalidValue {
			field: "gradient",
			reason: "a gradient needs a colour stop".into(),
		});
	}
	let (id, image, offset) = pixel_target(doc, layer)?;
	let locked_alpha = doc.layer(id).is_some_and(|l| l.locked_transparency);
	let placed = crate::pixels::Placed { image: &image, offset };
	let paint = |x: i64, y: i64| fill.color_at(x, y);
	let (filled, offset) = crate::pixels::fill_with(
		placed,
		doc.selection.as_ref(),
		(doc.width, doc.height),
		mode,
		opacity,
		locked_alpha,
		&paint,
		ctx.tiles,
	)?;
	set_pixels(doc, id, filled, offset);
	Ok(CommandEffect {
		label: "Gradient".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

pub(super) fn fill_pattern(
	doc: &mut Document,
	layer: &LayerRef,
	pattern: u64,
	mode: BlendMode,
	opacity: f64,
	preserve_transparency: bool,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let (id, image, offset) = pixel_target(doc, layer)?;
	let locked_alpha = doc.layer(id).is_some_and(|l| l.locked_transparency);
	let paint = pattern_paint(doc, pattern)?;
	let placed = crate::pixels::Placed { image: &image, offset };
	let (filled, offset) = crate::pixels::fill_with(
		placed,
		doc.selection.as_ref(),
		(doc.width, doc.height),
		mode,
		opacity.clamp(0.0, 1.0),
		preserve_transparency || locked_alpha,
		&paint,
		ctx.tiles,
	)?;
	set_pixels(doc, id, filled, offset);
	Ok(CommandEffect {
		label: "Fill".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

pub(super) fn set_fill_layer(doc: &mut Document, layer: &LayerRef, content: &crate::fill::FillLayer) -> Result<CommandEffect, CommandError> {
	let id = resolve(doc, layer)?;
	let (w, h, format) = (doc.width, doc.height, doc.color.depth.rgba_format());
	let target = doc.layer_mut(id).expect("resolved id exists");
	let LayerKind::FillLayer { content: old, cache } = &mut target.kind else {
		return Err(CommandError::NotAllowed("the layer is not a gradient or pattern fill".into()));
	};
	*old = content.clone();
	// Every tile is drawn again from the new parameters.
	*cache = TiledImage::derived(w, h, format);
	Ok(CommandEffect {
		label: "Change Fill Layer".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}

pub(super) fn define_pattern(doc: &mut Document, pattern: &crate::pattern::Pattern) -> Result<CommandEffect, CommandError> {
	let max = crate::pattern::MAX_SIDE;
	if pattern.width == 0 || pattern.height == 0 || pattern.width > max || pattern.height > max {
		return Err(CommandError::InvalidValue {
			field: "pattern",
			reason: format!("a pattern is 1..={max} px per side"),
		});
	}
	if pattern.pixels.len() != (pattern.width * pattern.height) as usize {
		return Err(CommandError::InvalidValue {
			field: "pattern",
			reason: "the pixel count does not match the size".into(),
		});
	}
	if !doc.patterns.iter().any(|p| p.id == pattern.id) {
		doc.patterns.push(pattern.clone());
	}
	Ok(CommandEffect {
		label: "Define Pattern".into(),
		history_only: true,
		..Default::default()
	})
}

/// How far from the click the Red Eye tool looks, in pixels.
// FAST: a fixed window; Photoshop scales with the image.
const RED_EYE_REACH: i64 = 120;

pub(super) fn red_eye(
	doc: &mut Document,
	layer: &LayerRef,
	point: (f64, f64),
	pupil_size: f64,
	darken: f64,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let (id, mut image, offset) = pixel_target(doc, layer)?;
	let format = image.format();
	if matches!(format, PixelFormat::Gray8 | PixelFormat::Gray16) {
		return Err(CommandError::NotAllowed("the Red Eye tool needs colour pixels".into()));
	}
	let tile = i64::from(TILE_SIZE);
	// The window in layer pixels.
	let (px, py) = (point.0.floor() as i64 - i64::from(offset.0), point.1.floor() as i64 - i64::from(offset.1));
	let (iw, ih) = (i64::from(image.width()), i64::from(image.height()));
	let (x0, y0) = ((px - RED_EYE_REACH).max(0), (py - RED_EYE_REACH).max(0));
	let (x1, y1) = ((px + RED_EYE_REACH).min(iw - 1), (py + RED_EYE_REACH).min(ih - 1));
	if x0 > x1 || y0 > y1 {
		return Err(CommandError::NotAllowed("the click is outside the layer".into()));
	}
	let (w, h) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
	// Read the window's tiles.
	let mut tiles: std::collections::HashMap<(u32, u32), Vec<[f32; 4]>> = std::collections::HashMap::new();
	for ty in y0 / tile..=y1 / tile {
		for tx in x0 / tile..=x1 / tile {
			let pixels = match image.slot(0, tx as u32, ty as u32) {
				TileSlot::Empty => vec![[0.0; 4]; TILE_PIXELS],
				TileSlot::Solid(v) => vec![v.0.map(|c| f32::from(c) / 65535.0); TILE_PIXELS],
				TileSlot::Data(handle) => crate::pixels::decode(ctx.tiles.get(handle)?.as_ref(), format),
			};
			tiles.insert((tx as u32, ty as u32), pixels);
		}
	}
	let at = |tiles: &std::collections::HashMap<(u32, u32), Vec<[f32; 4]>>, x: i64, y: i64| -> [f32; 4] {
		tiles[&((x / tile) as u32, (y / tile) as u32)][((y % tile) * tile + x % tile) as usize]
	};
	let redness = |p: [f32; 4]| -> f32 { (p[0] - p[1].max(p[2])) * p[3] };
	let red: Vec<f32> = (0..w * h).map(|i| redness(at(&tiles, x0 + (i % w) as i64, y0 + (i / w) as i64))).collect();
	// The seed: the reddest pixel within 15 px of the click.
	let (cx, cy) = ((px - x0) as i64, (py - y0) as i64);
	let mut seed = None;
	let mut best = 0.08f32;
	for dy in -15..=15i64 {
		for dx in -15..=15i64 {
			let (x, y) = (cx + dx, cy + dy);
			if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || dx * dx + dy * dy > 225 {
				continue;
			}
			let r = red[y as usize * w + x as usize];
			if r > best {
				best = r;
				seed = Some((x as usize, y as usize));
			}
		}
	}
	let Some(seed) = seed else {
		return Err(CommandError::NotAllowed("no red eye was found there".into()));
	};
	// Flood the red blob; a bigger pupil size takes paler red too (VERIFY).
	let threshold = best * (0.6 - 0.45 * pupil_size.clamp(0.0, 1.0) as f32);
	let mut mask = vec![0.0f32; w * h];
	let mut stack = vec![seed];
	mask[seed.1 * w + seed.0] = 1.0;
	while let Some((x, y)) = stack.pop() {
		for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
			if nx >= w || ny >= h || mask[ny * w + nx] > 0.0 || red[ny * w + nx] < threshold {
				continue;
			}
			mask[ny * w + nx] = 1.0;
			stack.push((nx, ny));
		}
	}
	// Feather: two 3 × 3 box passes.
	for _ in 0..2 {
		let src = mask.clone();
		for y in 0..h {
			for x in 0..w {
				let mut sum = 0.0;
				let mut n = 0.0;
				for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
					let (xx, yy) = (x as i64 + dx, y as i64 + dy);
					if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
						sum += src[yy as usize * w + xx as usize];
						n += 1.0;
					}
				}
				mask[y * w + x] = sum / n;
			}
		}
	}
	// Desaturate the red and darken, through the mask.
	let dark = (1.0 - 0.85 * darken.clamp(0.0, 1.0)) as f32;
	for y in 0..h {
		for x in 0..w {
			let m = mask[y * w + x];
			if m <= 0.0 {
				continue;
			}
			let (lx, ly) = (x0 + x as i64, y0 + y as i64);
			let key = ((lx / tile) as u32, (ly / tile) as u32);
			let i = ((ly % tile) * tile + lx % tile) as usize;
			let p = tiles[&key][i];
			// Red takes the green-blue average (VERIFY: Photoshop's formula).
			let q = [(p[1] + p[2]) / 2.0 * dark, p[1] * dark, p[2] * dark];
			let out = [0, 1, 2].map(|c| p[c] + (q[c] - p[c]) * m);
			tiles.get_mut(&key).expect("read above")[i] = [out[0], out[1], out[2], p[3]];
		}
	}
	for ((tx, ty), pixels) in tiles {
		image.put_buffer(ctx.tiles, tx, ty, crate::pixels::encode(&pixels, format));
	}
	set_pixels(doc, id, image, offset);
	Ok(CommandEffect {
		label: "Red Eye".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}
