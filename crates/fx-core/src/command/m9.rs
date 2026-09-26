//! M9's commands: channels and Quick Mask (T01), selections computed from
//! the pixels (T02–T06), Transform Selection (T02), Perspective Crop (T09).

use super::*;
use crate::channel::{Channel, canvas_aligned};

/// The name of the Quick Mask layer (M9-T01).
pub const QUICK_MASK: &str = "Quick Mask";

fn channel_index(doc: &Document, channel: usize) -> Result<usize, CommandError> {
	if channel < doc.channels.len() {
		Ok(channel)
	} else {
		Err(CommandError::InvalidValue {
			field: "channel",
			reason: format!("there is no channel {channel}"),
		})
	}
}

fn channel_effect(label: &str) -> CommandEffect {
	// Channels are saved: the step dirties the document, no layer changes.
	CommandEffect {
		label: label.into(),
		..Default::default()
	}
}

pub(super) fn save_selection(
	doc: &mut Document,
	channel: Option<usize>,
	name: Option<&str>,
	mode: SelectMode,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let size = (doc.width, doc.height);
	let depth = doc.color.depth;
	match channel {
		None => {
			let image = canvas_aligned(&selection, size, depth, ctx.tiles)?;
			let name = name.map(str::to_owned).unwrap_or_else(|| format!("Alpha {}", doc.channels.len() + 1));
			doc.channels.push(Channel::new(name, image));
		}
		Some(i) => {
			let i = channel_index(doc, i)?;
			let old = doc.channels[i].as_selection();
			let combined = selection::combine(size, Some(&old), &selection, mode, ctx.tiles)?;
			doc.channels[i].image = match combined {
				Some(s) => canvas_aligned(&s, size, depth, ctx.tiles)?,
				None => TiledImage::new(size.0, size.1, depth.gray_format()),
			};
		}
	}
	Ok(channel_effect("Save Selection"))
}

pub(super) fn load_selection(
	doc: &mut Document,
	channel: usize,
	invert: bool,
	mode: SelectMode,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let i = channel_index(doc, channel)?;
	let size = (doc.width, doc.height);
	let mut loaded = doc.channels[i].as_selection();
	if invert {
		loaded = match selection::invert(size, &loaded, ctx.tiles)? {
			Some(s) => s,
			None => Selection::empty(size, doc.color.depth),
		};
	}
	doc.selection = selection::combine(size, doc.selection.as_ref(), &loaded, mode, ctx.tiles)?;
	Ok(selection_effect("Load Selection"))
}

pub(super) fn delete_channel(doc: &mut Document, channel: usize) -> Result<CommandEffect, CommandError> {
	let i = channel_index(doc, channel)?;
	doc.channels.remove(i);
	Ok(channel_effect("Delete Channel"))
}

pub(super) fn duplicate_channel(doc: &mut Document, channel: usize) -> Result<CommandEffect, CommandError> {
	let i = channel_index(doc, channel)?;
	let mut copy = doc.channels[i].clone();
	copy.name = format!("{} copy", copy.name);
	doc.channels.insert(i + 1, copy);
	Ok(channel_effect("Duplicate Channel"))
}

pub(super) fn set_channel(
	doc: &mut Document,
	channel: usize,
	name: Option<&str>,
	color: Option<[u16; 4]>,
	opacity: Option<f32>,
) -> Result<CommandEffect, CommandError> {
	let i = channel_index(doc, channel)?;
	let c = &mut doc.channels[i];
	if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
		c.name = name.to_owned();
	}
	if let Some(color) = color {
		c.color = color;
	}
	if let Some(opacity) = opacity {
		c.opacity = opacity.clamp(0.0, 1.0);
	}
	Ok(channel_effect("Channel Options"))
}

/// The Quick Mask layer, if the document has one.
pub fn quick_mask_layer(doc: &Document) -> Option<LayerId> {
	doc.layers
		.iter()
		.rev()
		.find(|l| l.name == QUICK_MASK && matches!(l.kind, LayerKind::SolidFill { .. }) && l.mask.is_some())
		.map(|l| l.id)
}

/// Quick Mask (M9-T01).
///
/// FAST: the mask is a real layer on top of the stack — a red solid fill at
/// 50 % whose layer mask is the *unselected* amount — so display, mips and
/// painting (the paint tools' mask target, with the colour inverted by the
/// tool) all come for free. Export, merge and flatten would include it while
/// it is on.
pub(super) fn quick_mask(doc: &mut Document, on: bool, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let size = (doc.width, doc.height);
	let format = doc.color.depth.gray_format();
	if on {
		if quick_mask_layer(doc).is_some() {
			return Err(CommandError::NotAllowed("Quick Mask is already on".into()));
		}
		let image = match &doc.selection {
			Some(selection) => crate::pixels::mask_from_selection(selection, size, (0, 0), size, true, format, ctx.tiles)?,
			// Nothing selected: nothing is masked.
			None => TiledImage::new(size.0, size.1, format),
		};
		let id = doc.allocate_layer_id();
		let mut layer = Layer::new(id, QUICK_MASK, LayerKind::SolidFill { rgba: [65535, 0, 0, 65535] });
		layer.opacity = 0.5;
		layer.mask = Some(Mask {
			image,
			enabled: true,
			linked: false,
			outside_value: 0,
		});
		doc.layers.push(Arc::new(layer));
		doc.selected = vec![id];
		doc.selection = None;
		return Ok(CommandEffect {
			label: "Quick Mask".into(),
			structure_changed: true,
			..Default::default()
		});
	}
	let Some(id) = quick_mask_layer(doc) else {
		return Err(CommandError::NotAllowed("Quick Mask is off".into()));
	};
	let mask = doc.layer(id).and_then(|l| l.mask.clone()).expect("found with a mask");
	// The mask is the unselected amount: the selection is its inverse.
	let masked = Selection {
		image: mask.image,
		offset: (0, 0),
	};
	doc.selection = selection::invert(size, &masked, ctx.tiles)?;
	doc.layers.retain(|l| l.id != id);
	doc.selected = doc.layers.last().map(|l| vec![l.id]).unwrap_or_default();
	Ok(CommandEffect {
		label: "Quick Mask".into(),
		structure_changed: true,
		..Default::default()
	})
}

pub(super) fn select_by(
	doc: &mut Document,
	op: &crate::select_ops::SelectOp,
	mode: SelectMode,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let ops = pixel_ops(ctx, op.label())?;
	let size = (doc.width, doc.height);
	let found = ops.select_op(doc, op, ctx.tiles)?;
	// Grow, Similar and Select and Mask rework the selection itself.
	let mode = match op {
		crate::select_ops::SelectOp::Grow { .. } | crate::select_ops::SelectOp::Similar { .. } | crate::select_ops::SelectOp::Refine(_) => SelectMode::Replace,
		_ => mode,
	};
	doc.selection = match found {
		Some(found) => selection::combine(size, doc.selection.as_ref(), &found, mode, ctx.tiles)?,
		None => match mode {
			SelectMode::Replace | SelectMode::Intersect => None,
			SelectMode::Add | SelectMode::Subtract => doc.selection.take(),
		},
	};
	Ok(selection_effect(op.label()))
}

pub(super) fn transform_selection(doc: &mut Document, mapping: &Mapping, filter: Filter, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let ops = pixel_ops(ctx, "Transform Selection")?;
	let size = (doc.width, doc.height);
	// Canvas-aligned first, so image pixels are canvas pixels.
	let aligned = canvas_aligned(&selection, size, doc.color.depth, ctx.tiles)?;
	let image = ops.resample(&aligned, *mapping, size, filter, ctx.tiles)?;
	let moved = Selection { image, offset: (0, 0) };
	// Drop the empty result; a resample leaves mips dirty, level 0 is what counts.
	doc.selection = if moved.is_empty() { None } else { Some(moved) };
	Ok(selection_effect("Transform Selection"))
}

/// The inverse of an affine or projective mapping.
fn inverse(mapping: &Mapping) -> Option<Mapping> {
	let m = match mapping {
		Mapping::Affine([a, b, c, d, e, f]) => [*a, *c, *e, *b, *d, *f, 0.0, 0.0, 1.0],
		Mapping::Projective(m) => *m,
		Mapping::Warp(_) => return None,
	};
	let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
	if det.abs() < 1e-15 {
		return None;
	}
	let inv = [
		(m[4] * m[8] - m[5] * m[7]) / det,
		(m[2] * m[7] - m[1] * m[8]) / det,
		(m[1] * m[5] - m[2] * m[4]) / det,
		(m[5] * m[6] - m[3] * m[8]) / det,
		(m[0] * m[8] - m[2] * m[6]) / det,
		(m[2] * m[3] - m[0] * m[5]) / det,
		(m[3] * m[7] - m[4] * m[6]) / det,
		(m[1] * m[6] - m[0] * m[7]) / det,
		(m[0] * m[4] - m[1] * m[3]) / det,
	];
	Some(Mapping::Projective(inv.map(|v| v / inv[8])))
}

pub(super) fn perspective_crop(
	doc: &mut Document,
	quad: [(f64, f64); 4],
	width: u32,
	height: u32,
	ctx: &CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	if width == 0 || height == 0 || width > 300_000 || height > 300_000 {
		return Err(CommandError::InvalidValue {
			field: "width",
			reason: "the result must be 1..=300 000 px per side".into(),
		});
	}
	let rect = [0.0, 0.0, f64::from(width), f64::from(height)];
	// Rectangle → quad, inverted: canvas → rectangle.
	let to_quad = Mapping::from_quad(rect, quad).ok_or(CommandError::InvalidValue {
		field: "quad",
		reason: "the corners must form a convex quadrilateral".into(),
	})?;
	let mapping = inverse(&to_quad).ok_or(CommandError::InvalidValue {
		field: "quad",
		reason: "the quadrilateral is degenerate".into(),
	})?;
	let ops = pixel_ops(ctx, "Perspective Crop")?;
	let mut pixels_changed = resample_document(doc, ops, mapping, Filter::BicubicAutomatic, ctx.tiles)?;
	pixels_changed.extend(clip_document(doc, (0, 0, width, height), ctx.tiles)?);
	pixels_changed.sort_unstable();
	pixels_changed.dedup();
	let props_changed = set_canvas(doc, width, height);
	doc.selection = None;
	Ok(CommandEffect {
		label: "Perspective Crop".into(),
		pixels_changed,
		props_changed,
		..Default::default()
	})
}
