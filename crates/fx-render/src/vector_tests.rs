//! Tests of the shape renderer (M6-T06): exact coverage at level 0, the same
//! shape covering the same area at a mip level, and the stroke alignments.

use fx_core::vector::{Paint, StrokeAlign, StrokeStyle, VectorShape};
use fx_tiles::{PixelFormat, TILE_PIXELS, TILE_SIZE, TileBuffer};

use crate::vector::{IDENTITY, render_shape_tile};

const RED: Paint = Paint::Solid {
	rgba: [u16::MAX, 0, 0, u16::MAX],
};
const BLUE: Paint = Paint::Solid {
	rgba: [0, 0, u16::MAX, u16::MAX],
};

/// The straight-alpha RGBA pixel `(x, y)` of a tile buffer.
fn pixel(buffer: &TileBuffer, x: u32, y: u32) -> [u8; 4] {
	let bytes = buffer.bytes();
	let at = ((y * TILE_SIZE + x) * 4) as usize;
	[bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

/// The alpha, 0..=255, of the tile buffer's pixel `(x, y)`.
fn alpha(buffer: &TileBuffer, x: u32, y: u32) -> u8 {
	pixel(buffer, x, y)[3]
}

/// A rectangle of `w × h` in local space, as the Rectangle tool builds it.
fn rect(w: f64, h: f64, radii: [f64; 4]) -> VectorShape {
	VectorShape::Rect { w, h, radii }
}

/// The identity placement moved to `(x, y)`.
fn at(x: f64, y: f64) -> [f64; 6] {
	[1.0, 0.0, 0.0, 1.0, x, y]
}

fn stroke(width: f64, align: StrokeAlign) -> StrokeStyle {
	StrokeStyle {
		width,
		align,
		paint: BLUE,
		dash: None,
	}
}

#[test]
fn a_rect_at_level_zero_covers_exactly_its_pixels() {
	// 100 × 50 at (10, 10): an axis-aligned edge on a pixel boundary has no
	// partial coverage, so every pixel is either fully opaque or empty.
	let shape = rect(100.0, 50.0, [0.0; 4]);
	let tile = render_shape_tile(&shape, Some(&RED), None, at(10.0, 10.0), 0, (0, 0), PixelFormat::Rgba8);
	for y in 0..TILE_SIZE {
		for x in 0..TILE_SIZE {
			let inside = (10..110).contains(&x) && (10..60).contains(&y);
			assert_eq!(alpha(&tile, x, y), if inside { 255 } else { 0 }, "pixel ({x},{y})");
			if inside {
				assert_eq!(pixel(&tile, x, y), [255, 0, 0, 255], "the fill is opaque red at ({x},{y})");
			}
		}
	}
}

#[test]
fn a_rect_covers_the_same_area_at_level_two() {
	// Level 2 is drawn from the geometry at 1/4 scale (never downsampled from
	// level 0), so the two renders agree on the area they cover within 2/255
	// of a tile's pixels — the higher level is only sharper along the edge.
	let shape = rect(180.0, 120.0, [0.0; 4]);
	let transform = at(37.0, 21.0);
	let level0 = render_shape_tile(&shape, Some(&RED), None, transform, 0, (0, 0), PixelFormat::Rgba8);
	let level2 = render_shape_tile(&shape, Some(&RED), None, transform, 2, (0, 0), PixelFormat::Rgba8);
	let area = |buffer: &TileBuffer| -> f64 {
		(0..TILE_SIZE)
			.map(|y| (0..TILE_SIZE).map(|x| f64::from(alpha(buffer, x, y)) / 255.0).sum::<f64>())
			.sum()
	};
	// One level-2 pixel is 4 × 4 document pixels, so the areas are comparable
	// once the level-0 one is scaled down.
	let covered = area(&level2);
	let scaled = area(&level0) / 16.0;
	let tolerance = TILE_PIXELS as f64 * (2.0 / 255.0);
	assert!(
		(covered - scaled).abs() <= tolerance,
		"level 2 covers {covered} level-2 pixels, level 0 covers {scaled} (tolerance {tolerance})"
	);
	assert!(scaled > 100.0, "the rect is 180 × 120 document pixels: an empty render is a bug");
}

#[test]
fn a_shape_outside_the_tile_draws_nothing() {
	let shape = VectorShape::Ellipse { w: 100.0, h: 100.0 };
	let tile = render_shape_tile(&shape, Some(&RED), None, at(1000.0, 1000.0), 0, (0, 0), PixelFormat::Rgba8);
	assert!(tile.uniform_value().is_some_and(|value| value.is_transparent(PixelFormat::Rgba8)));
}

#[test]
fn a_shape_without_paint_draws_nothing() {
	let shape = rect(100.0, 100.0, [0.0; 4]);
	let tile = render_shape_tile(&shape, None, None, IDENTITY, 0, (0, 0), PixelFormat::Rgba8);
	assert!(tile.uniform_value().is_some_and(|value| value.is_transparent(PixelFormat::Rgba8)));
}

#[test]
fn an_ellipse_is_round_and_anti_aliased() {
	// 100 × 100 at (10, 10): its box is (10, 10)…(110, 110).
	let shape = VectorShape::Ellipse { w: 100.0, h: 100.0 };
	let tile = render_shape_tile(&shape, Some(&RED), None, at(10.0, 10.0), 0, (0, 0), PixelFormat::Rgba8);
	assert_eq!(alpha(&tile, 60, 60), 255, "the centre is opaque");
	assert_eq!(alpha(&tile, 12, 12), 0, "a corner of the box is outside the ellipse");
	assert_eq!(alpha(&tile, 9, 60), 0, "a pixel left of the box is empty");
	assert_eq!(alpha(&tile, 60, 110), 0, "as is one below it");
	// The round edge has partial pixels: neither empty nor solid.
	let edge = (10..110)
		.filter(|x| {
			let a = alpha(&tile, *x, 10);
			a > 0 && a < 255
		})
		.count();
	assert!(edge > 0, "an anti-aliased ellipse edge must have partial pixels");
}

#[test]
fn a_center_stroke_spreads_on_both_sides_of_the_outline() {
	let shape = rect(100.0, 100.0, [0.0; 4]);
	// The outline is at 50 and 150; a 4 px centre stroke spans 48…152.
	let tile = render_shape_tile(
		&shape,
		None,
		Some(&stroke(4.0, StrokeAlign::Center)),
		at(50.0, 50.0),
		0,
		(0, 0),
		PixelFormat::Rgba8,
	);
	assert_eq!(alpha(&tile, 48, 100), 255, "2 px outside the outline");
	assert_eq!(alpha(&tile, 47, 100), 0, "and no further");
	assert_eq!(alpha(&tile, 100, 100), 0, "the inside of a stroke-only shape is empty");
	assert_eq!(pixel(&tile, 48, 100), [0, 0, 255, 255], "the stroke colour is the stroke's");
}

#[test]
fn an_outside_stroke_stays_outside_the_outline() {
	let shape = rect(100.0, 100.0, [0.0; 4]);
	let tile = render_shape_tile(
		&shape,
		None,
		Some(&stroke(10.0, StrokeAlign::Outside)),
		at(50.0, 50.0),
		0,
		(0, 0),
		PixelFormat::Rgba8,
	);
	assert_eq!(alpha(&tile, 100, 45), 255, "10 px outside the edge is covered");
	assert_eq!(alpha(&tile, 100, 39), 0, "and no further");
	assert_eq!(alpha(&tile, 100, 55), 0, "nothing is drawn inside the outline");
}

#[test]
fn an_inside_stroke_stays_inside_the_outline() {
	let shape = rect(100.0, 100.0, [0.0; 4]);
	let tile = render_shape_tile(
		&shape,
		None,
		Some(&stroke(10.0, StrokeAlign::Inside)),
		at(50.0, 50.0),
		0,
		(0, 0),
		PixelFormat::Rgba8,
	);
	assert_eq!(alpha(&tile, 100, 55), 255, "10 px inside the edge is covered");
	assert_eq!(alpha(&tile, 100, 61), 0, "and no further");
	assert_eq!(alpha(&tile, 100, 49), 0, "nothing is drawn outside the outline");
}

#[test]
fn a_line_is_a_filled_bar_of_its_weight() {
	let shape = VectorShape::Line { length: 100.0, width: 6.0 };
	let tile = render_shape_tile(&shape, Some(&RED), None, at(20.0, 30.0), 0, (0, 0), PixelFormat::Rgba8);
	assert_eq!(alpha(&tile, 70, 32), 255, "inside the bar");
	assert_eq!(alpha(&tile, 70, 37), 0, "past its weight");
	assert_eq!(alpha(&tile, 121, 32), 0, "past its length");
}

#[test]
fn a_rotated_line_is_drawn_along_its_angle() {
	let shape = VectorShape::Line { length: 100.0, width: 4.0 };
	// The Line tool turns the bar about the press point at (10, 10): 45°.
	let s = std::f64::consts::FRAC_1_SQRT_2;
	let tile = render_shape_tile(&shape, Some(&RED), None, [s, s, -s, s, 10.0, 10.0], 0, (0, 0), PixelFormat::Rgba8);
	assert_eq!(alpha(&tile, 45, 45), 255, "the segment runs through its midpoint");
	assert_eq!(alpha(&tile, 71, 51), 0, "and nowhere else");
}

#[test]
fn a_polygon_points_up_inside_its_local_box() {
	// The Polygon tool scales the 2 × 2 local box by half the drag's size.
	let shape = VectorShape::Polygon { sides: 5, star_inset: 0.0 };
	let tile = render_shape_tile(&shape, Some(&RED), None, [100.0, 0.0, 0.0, 100.0, 0.0, 0.0], 0, (0, 0), PixelFormat::Rgba8);
	assert_eq!(alpha(&tile, 100, 10), 255, "the top vertex is at the box's top edge");
	assert_eq!(alpha(&tile, 3, 3), 0, "the box's corner is outside the polygon");
	assert_eq!(alpha(&tile, 100, 195), 0, "the polygon's base is above y = 181");
	assert_eq!(alpha(&tile, 2, 100), 0, "and its sides are inside the box");
}

#[test]
fn a_rounded_rect_cuts_its_corners() {
	let sharp = render_shape_tile(&rect(100.0, 100.0, [0.0; 4]), Some(&RED), None, at(20.0, 20.0), 0, (0, 0), PixelFormat::Rgba8);
	let rounded = render_shape_tile(&rect(100.0, 100.0, [30.0; 4]), Some(&RED), None, at(20.0, 20.0), 0, (0, 0), PixelFormat::Rgba8);
	assert_eq!(alpha(&sharp, 20, 20), 255, "a sharp corner covers its corner pixel");
	assert_eq!(alpha(&rounded, 20, 20), 0, "a 30 px radius cuts it away");
	assert_eq!(alpha(&rounded, 70, 70), 255, "the middle is covered either way");
}

#[test]
fn tile_origins_place_the_shape_in_the_tile_it_belongs_to() {
	// A 300 × 10 bar from (100, 300) crosses the tile boundary at x = 256.
	let shape = rect(300.0, 10.0, [0.0; 4]);
	let transform = at(100.0, 300.0);
	let left = render_shape_tile(&shape, Some(&RED), None, transform, 0, (0, 1), PixelFormat::Rgba8);
	let right = render_shape_tile(&shape, Some(&RED), None, transform, 0, (1, 1), PixelFormat::Rgba8);
	// Tile (0, 1) starts at document (0, 256): its pixel (156, 49) is (156, 305).
	assert_eq!(alpha(&left, 156, 49), 255, "document x = 156 is inside the bar");
	assert_eq!(alpha(&left, 99, 49), 0, "document x = 99 is before it");
	// Tile (1, 1) starts at (256, 256): its pixel (0, 49) is (256, 305).
	assert_eq!(alpha(&right, 0, 49), 255, "document x = 256 is inside it too");
	assert_eq!(alpha(&right, 150, 49), 0, "document x = 406 is past its end");
}
