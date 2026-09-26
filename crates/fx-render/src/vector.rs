//! Rasterising a shape layer's geometry into tiles (M6-T06).
//!
//! A shape layer has no authoritative pixels: its tiles are *derived* from the
//! geometry, at whatever mip level is being drawn, by the renderer this module
//! wraps — tiny-skia (D-050), a CPU Skia subset. Nothing here touches wgpu, so
//! the same function serves the screen, export and `raster:shape`.
//!
//! The tile is a `TILE_SIZE × TILE_SIZE` `Pixmap` covering level-space
//! `[tx·T, tx·T + T)` — that is, document pixels `[tx·T·2ᴸ, …)`. Shape-local
//! coordinates reach it through the layer's affine transform composed with the
//! level scale and the tile origin, one `tiny_skia::Transform`.
//!
//! Anti-aliasing is tiny-skia's analytic coverage, in 8 bits: `Pixmap` is
//! premultiplied RGBA8, and the tile is converted to straight alpha in the
//! document's format (D-050 accepts the 8-bit coverage; Photoshop's own shape
//! anti-aliasing is 8-bit too).

use fx_core::vector::{Paint, PathEl, StrokeAlign, StrokeStyle, VectorShape};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer};
use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Path, PathBuilder, Pixmap, Stroke, Transform};

/// Draw one tile of `level` of a shape.
///
/// `transform` maps shape-local coordinates to document pixels (see
/// `fx_core::vector`); `tile` is the tile coordinate at `level`. The result is
/// a `TILE_SIZE × TILE_SIZE` buffer in `format` (straight alpha), ready for
/// `TileSlot::Data`/`Solid`/`Empty`.
pub fn render_shape_tile(
	shape: &VectorShape,
	fill: Option<&Paint>,
	stroke: Option<&StrokeStyle>,
	transform: [f64; 6],
	level: usize,
	tile: (u32, u32),
	format: PixelFormat,
) -> TileBuffer {
	let origin = (f64::from(tile.0) * f64::from(TILE_SIZE), f64::from(tile.1) * f64::from(TILE_SIZE));
	// Level-space → document pixels: 2^level is the mip scale.
	let scale = f64::from(1u32 << level);
	let pixels = TILE_SIZE;
	let Some(mut pixmap) = Pixmap::new(pixels, pixels) else {
		return TileBuffer::zeroed(format);
	};
	let Some(path) = build_path(&shape.outline()) else {
		return convert(&pixmap.take_demultiplied(), format);
	};
	// local → document → level space → this tile.
	let level_to_tile = Transform::from_row(
		(1.0 / scale) as f32,
		0.0,
		0.0,
		(1.0 / scale) as f32,
		(-origin.0 / scale) as f32,
		(-origin.1 / scale) as f32,
	);
	let local_to_doc = Transform::from_row(
		transform[0] as f32,
		transform[1] as f32,
		transform[2] as f32,
		transform[3] as f32,
		transform[4] as f32,
		transform[5] as f32,
	);
	let tile_transform = level_to_tile.pre_concat(local_to_doc);

	if let Some(paint) = fill {
		let mut skia = skia_paint(*paint);
		skia.anti_alias = true;
		pixmap.fill_path(&path, &skia, FillRule::Winding, tile_transform, None);
	}
	if let Some(stroke) = stroke {
		draw_stroke(&mut pixmap, &path, stroke, scale, tile_transform);
	}
	convert(&pixmap.take_demultiplied(), format)
}

/// The stroke band, honouring Photoshop's Align.
///
/// Center is what tiny-skia strokes. Inside and Outside are the same stroke
/// drawn at twice the width and clipped to the covered / uncovered side of the
/// outline (the visible half is then exactly `width` wide on that side), which
/// is how every vector editor implements them without offsetting the path.
fn draw_stroke(pixmap: &mut Pixmap, path: &Path, stroke: &StrokeStyle, scale: f64, transform: Transform) {
	let width = stroke.width.max(0.0);
	if width <= 0.0 {
		return;
	}
	let mut paint = skia_paint(stroke.paint);
	paint.anti_alias = true;
	let (pixmap_width, clipped) = match stroke.align {
		StrokeAlign::Center => (width, false),
		StrokeAlign::Inside | StrokeAlign::Outside => (width * 2.0, true),
	};
	let mut skia = Stroke {
		width: (pixmap_width / scale) as f32,
		line_cap: LineCap::Butt,
		line_join: LineJoin::Miter,
		..Default::default()
	};
	// A dash pattern needs at least two non-negative values; anything else is
	// refused by tiny-skia and drawn solid.
	skia.dash = stroke
		.dash
		.as_ref()
		.and_then(|pattern| tiny_skia::StrokeDash::new(pattern.iter().map(|v| (v / scale) as f32).collect(), 0.0));
	if !clipped {
		pixmap.stroke_path(path, &paint, &skia, transform, None);
		return;
	}
	let pixels = TILE_SIZE;
	let Some(mut cover) = Mask::new(pixels, pixels) else {
		return;
	};
	cover.fill_path(path, FillRule::Winding, true, transform);
	if stroke.align == StrokeAlign::Outside {
		cover.invert();
	}
	pixmap.stroke_path(path, &paint, &skia, transform, Some(&cover));
}

/// A tiny-skia paint of one solid colour, anti-aliased by the caller.
fn skia_paint(paint: Paint) -> tiny_skia::Paint<'static> {
	let rgba = paint.rgba_f32();
	let mut out = tiny_skia::Paint::default();
	out.set_color_rgba8(
		(rgba[0] * 255.0).round() as u8,
		(rgba[1] * 255.0).round() as u8,
		(rgba[2] * 255.0).round() as u8,
		(rgba[3] * 255.0).round() as u8,
	);
	out
}

/// Straight RGBA8 (tiny-skia's `take_demultiplied`) into the tile's format.
fn convert(straight: &[u8], format: PixelFormat) -> TileBuffer {
	let mut out = TileBuffer::zeroed(format);
	match format {
		PixelFormat::Rgba8 => {
			let bytes = out.bytes_mut();
			let n = bytes.len().min(straight.len());
			bytes[..n].copy_from_slice(&straight[..n]);
		}
		PixelFormat::Rgba16 => {
			let words = out.as_u16_mut();
			for (i, word) in words.iter_mut().enumerate() {
				// 8-bit coverage scaled to 16: 0xFF → 0xFFFF.
				*word = u16::from(straight.get(i).copied().unwrap_or(0)) * 257;
			}
		}
		// Grey formats carry the red channel; a shape cache is RGBA in
		// practice (the document's own `rgba_format`).
		PixelFormat::Gray8 => {
			let bytes = out.bytes_mut();
			for (i, byte) in bytes.iter_mut().enumerate() {
				*byte = straight.get(i * 4).copied().unwrap_or(0);
			}
		}
		PixelFormat::Gray16 => {
			let words = out.as_u16_mut();
			for (i, word) in words.iter_mut().enumerate() {
				*word = u16::from(straight.get(i * 4).copied().unwrap_or(0)) * 257;
			}
		}
	}
	out
}

/// A tiny-skia path from shape-local path elements.
fn build_path(elements: &[PathEl]) -> Option<Path> {
	let mut builder = PathBuilder::new();
	let mut open = false;
	for element in elements {
		match *element {
			PathEl::MoveTo(p) => {
				builder.move_to(p[0] as f32, p[1] as f32);
				open = true;
			}
			PathEl::LineTo(p) => {
				// A path that starts with a line still needs a starting point.
				if open {
					builder.line_to(p[0] as f32, p[1] as f32);
				} else {
					builder.move_to(p[0] as f32, p[1] as f32);
					open = true;
				}
			}
			PathEl::QuadTo(c, p) => {
				builder.quad_to(c[0] as f32, c[1] as f32, p[0] as f32, p[1] as f32);
			}
			PathEl::CubicTo(c1, c2, p) => {
				builder.cubic_to(c1[0] as f32, c1[1] as f32, c2[0] as f32, c2[1] as f32, p[0] as f32, p[1] as f32);
			}
			PathEl::Close => {
				builder.close();
				open = false;
			}
		}
	}
	builder.finish()
}

/// The identity transform, for callers that place the shape themselves.
pub const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// One tile of a vector mask's coverage (M10-T06): the path (document
/// coordinates) filled at `level`, `1 − density` outside it, as grey
/// `format`. FAST: Feather is not applied (Photoshop blurs the coverage).
pub fn render_vector_mask_tile(elements: &[fx_core::vector::PathEl], density: f32, level: usize, tile: (u32, u32), format: PixelFormat) -> TileBuffer {
	let shape = VectorShape::Path { elements: elements.to_vec() };
	let white = Paint::Solid { rgba: [65535; 4] };
	let rgba = render_shape_tile(&shape, Some(&white), None, fx_core::vector::IDENTITY, level, tile, PixelFormat::Rgba8);
	let mut out = TileBuffer::zeroed(format);
	let alpha = rgba.bytes();
	let outside = 1.0 - density.clamp(0.0, 1.0);
	for y in 0..TILE_SIZE {
		for x in 0..TILE_SIZE {
			let a = f32::from(alpha[((y * TILE_SIZE + x) * 4 + 3) as usize]) / 255.0;
			fx_core::selection::set_gray(&mut out, format, x, y, outside + (1.0 - outside) * a);
		}
	}
	out
}
