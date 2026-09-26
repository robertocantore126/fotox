//! M8's pixel commands: Paint Bucket and Magic Eraser (T02); gradient,
//! pattern and red-eye commands join them in later cards.

use super::*;
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
