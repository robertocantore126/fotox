//! Text layout and rasterising tests (M6-T07).
//!
//! The card asks for a bundled OFL font so glyph positions are stable. The
//! repository has no font files and this work was done without a network to
//! fetch one, so these tests use whatever the platform offers and assert the
//! *shape* of the result rather than exact coordinates: a layout that holds
//! glyphs, one line where there is no wrapping, more than one where there is,
//! carets that move with the text, tiles that only meet the glyphs on them.
//! The exact-position test the card describes still belongs here — it needs a
//! checked-in font (`crates/fx-render/tests/data/`), which the T07 report
//! records as the one test the card asks for that is not in yet.

use fx_core::text::{FontStyle, TextAlign, TextAntialias, TextContent, TextFrame, TextRun};
use fx_tiles::{PixelFormat, TILE_SIZE};

use crate::text::{Fonts, TextLayout, render_text_tile};

/// A point text of one run in `family` at `size_pt`.
fn point_text(text: &str, family: &str, size_pt: f64, x: f64, y: f64) -> TextContent {
	TextContent {
		text: text.into(),
		runs: vec![TextRun {
			range: (0, text.len()),
			family: family.into(),
			style: FontStyle::Regular,
			size_pt,
			color: [0, 0, 0, 65_535],
			tracking: 0.0,
			leading: None,
		}],
		transform: [1.0, 0.0, 0.0, 1.0, x, y],
		..Default::default()
	}
}

/// The font stack, and a family to write in: the first one the platform
/// offers, or `None` on a machine with no fonts installed at all.
fn stack() -> (Fonts, Option<String>) {
	let mut fonts = Fonts::new();
	let family = fonts.families().first().map(|entry| entry.name.clone());
	(fonts, family)
}

/// The same, skipping the test when the machine has no fonts: a layout without
/// a font is empty for a reason that has nothing to do with the code.
macro_rules! stack {
	() => {
		match stack() {
			(fonts, Some(family)) => (fonts, family),
			(_, None) => return,
		}
	};
}

fn layout_of(fonts: &mut Fonts, content: &TextContent, ppi: f32) -> TextLayout {
	fonts.layout(content, ppi)
}

/// Whether a tile buffer holds any ink.
fn any_ink(buffer: &fx_tiles::TileBuffer) -> bool {
	buffer.as_u16().iter().any(|word| *word != 0)
}

#[test]
fn the_platform_reports_its_fonts_with_their_styles() {
	let mut fonts = Fonts::new();
	let families = fonts.families();
	assert!(!families.is_empty(), "the platform has fonts to enumerate");
	assert!(
		families.windows(2).all(|pair| pair[0].name.to_lowercase() <= pair[1].name.to_lowercase()),
		"the list is sorted by name, which is what the option bar shows"
	);
	assert!(families.iter().all(|entry| !entry.styles.is_empty()), "every family offers at least one style");
}

#[test]
fn a_layout_shapes_the_text_and_reports_where_it_is() {
	let (mut fonts, family) = stack!();
	let content = point_text("Hamburgefonstiv", &family, 12.0, 100.0, 50.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	assert!(!layout.is_empty(), "the text shaped into glyphs");
	assert_eq!(layout.line_count(), 1, "point text does not wrap");
	let (width, height) = layout.size();
	assert!(width > 0.0 && height > 0.0, "a laid-out line has a size: {width} × {height}");
	let ink = layout.ink_box().expect("the line has a box");
	assert!(ink.w > 0.0 && ink.h > 0.0, "and it covers the text: {ink:?}");
	// The frame's origin is the first baseline, so the block sits above it.
	assert!(ink.y < 50.0 && ink.y + ink.h > 40.0, "the baseline is at y = 50: {ink:?}");
	// A caret is a box at the text's height, and it moves with the index.
	let first = layout.caret(0);
	let last = layout.caret(content.text.len());
	assert!(first.h > 0.0 && first.w > 0.0, "a caret has a height: {first:?}");
	assert!(last.x > first.x, "the caret moves to the right with the text: {first:?} {last:?}");
}

#[test]
fn a_size_in_points_is_a_size_in_pixels_at_the_document_resolution() {
	let (mut fonts, family) = stack!();
	let content = point_text("Hxy", &family, 12.0, 0.0, 0.0);
	let small = layout_of(&mut fonts, &content, 72.0);
	let large = layout_of(&mut fonts, &content, 144.0);
	let ratio = large.size().1 / small.size().1;
	assert!(
		(1.8..=2.2).contains(&ratio),
		"twice the resolution is twice the text: {ratio} ({} vs {})",
		small.size().1,
		large.size().1
	);
	assert!(
		(0.5..=3.0).contains(&(small.size().1 / 12.0)),
		"a 12 pt line is around 12 px tall, not {} px",
		small.size().1
	);
}

#[test]
fn an_unknown_family_still_draws_with_the_fallback() {
	let (mut fonts, _) = stack!();
	// A family no platform has: the font stack falls back rather than drawing
	// nothing, which is what the option bar relies on when a document made on
	// another machine names a font this one does not have.
	let content = point_text("Fallback", "No Such Font 4711", 16.0, 0.0, 0.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	assert!(!layout.is_empty(), "the fallback font draws the text");
}

#[test]
fn box_text_wraps_and_point_text_does_not() {
	let (mut fonts, family) = stack!();
	let words = "the quick brown fox jumps over the lazy dog";
	let point = point_text(words, &family, 24.0, 0.0, 0.0);
	let point = layout_of(&mut fonts, &point, 72.0);
	assert_eq!(point.line_count(), 1, "point text never wraps");

	let mut boxed = point_text(words, &family, 24.0, 0.0, 0.0);
	boxed.frame = TextFrame::Box { w: 120.0, h: 300.0 };
	let boxed = layout_of(&mut fonts, &boxed, 72.0);
	assert!(boxed.line_count() > 1, "a 120 px box wraps 24 pt text");
	assert!(
		boxed.lines().iter().all(|line| line.rect.w <= 120.0),
		"no line is wider than the box: {:?}",
		boxed.lines()
	);
	// Alignment moves the lines inside the box, not the box itself.
	let mut centred = point_text(words, &family, 24.0, 0.0, 0.0);
	centred.frame = TextFrame::Box { w: 120.0, h: 300.0 };
	centred.align = TextAlign::Center;
	let centred = layout_of(&mut fonts, &centred, 72.0);
	assert!(centred.lines()[0].rect.x > boxed.lines()[0].rect.x, "a centred line starts further in");
}

#[test]
fn an_edit_knows_which_lines_it_changed() {
	let (mut fonts, family) = stack!();
	let mut content = point_text("one\ntwo\nthree", &family, 24.0, 0.0, 0.0);
	content.frame = TextFrame::Box { w: 400.0, h: 300.0 };
	let before = layout_of(&mut fonts, &content, 72.0);
	assert_eq!(before.line_count(), 3, "three hard lines");

	// A character typed on the second line: only that line's box changes.
	content.text = "one\ntwo!\nthree".into();
	content.runs[0].range = (0, content.text.len());
	let after = layout_of(&mut fonts, &content, 72.0);
	let changed = before.box_for_range((4, 4)).zip(after.box_for_range((4, 5))).map(|(old, new)| (old, new));
	let (old, new) = changed.expect("the second line has a box in both layouts");
	assert!((old.y - new.y).abs() < 1e-9, "the line did not move: {old:?} {new:?}");
	assert!(old.y > 0.0, "and it is not the first line");
	// The lines below it moved, because the second line got longer only in x:
	// a longer line still cannot push the next one down here, so the box below
	// is the same. Adding a line is what moves them.
	let above = before.box_from_line(2).expect("the old third line");
	let below = after.box_from_line(2).expect("the new third line");
	assert!((above.y - below.y).abs() < 1e-9, "the third line stays put: {above:?} {below:?}");
}

#[test]
fn hit_testing_finds_the_index_under_the_point() {
	let (mut fonts, family) = stack!();
	let content = point_text("Hello world", &family, 24.0, 0.0, 0.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	let ink = layout.ink_box().expect("a box");
	let start = layout.caret(0);
	let end = layout.caret(content.text.len());
	assert_eq!(layout.index_at(start.x - 50.0, start.y + start.h / 2.0), 0, "left of the text is its start");
	assert_eq!(
		layout.index_at(end.x + 500.0, end.y + end.h / 2.0),
		content.text.len(),
		"right of the text is its end"
	);
	let middle = layout.index_at(start.x + (end.x - start.x) / 2.0, ink.y + ink.h / 2.0);
	assert!(middle > 0 && middle < content.text.len(), "half way in is half way through: {middle}");
}

#[test]
fn an_empty_text_layer_draws_nothing() {
	let (mut fonts, family) = stack!();
	let content = point_text("", &family, 12.0, 0.0, 0.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	assert!(layout.is_empty());
	assert!(layout.ink_box().is_none(), "nothing to ink");
	let tile = render_text_tile(&layout, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, (0, 0), PixelFormat::Rgba16, TextAntialias::Smooth);
	assert!(!any_ink(&tile), "and nothing to draw");
}

#[test]
fn a_tile_only_draws_the_glyphs_that_meet_it() {
	let (mut fonts, family) = stack!();
	let content = point_text("H", &family, 96.0, 5.0, 5.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	let format = PixelFormat::Rgba16;
	let here = render_text_tile(&layout, content.transform, 0, (0, 0), format, TextAntialias::Smooth);
	assert!(any_ink(&here), "the glyph is in the first tile");
	let far = render_text_tile(&layout, content.transform, 0, (40, 40), format, TextAntialias::Smooth);
	assert!(!any_ink(&far), "and nothing is drawn ten thousand pixels away");
	// At a coarser level the same glyphs are smaller, so a tile still has ink
	// (the outlines are scaled, never resampled from the level above).
	let coarse = render_text_tile(&layout, content.transform, 2, (0, 0), format, TextAntialias::Smooth);
	assert!(any_ink(&coarse), "text is drawn at every level");
	let pixels = |buffer: &fx_tiles::TileBuffer| buffer.as_u16().chunks(4).filter(|px| px[3] != 0).count();
	assert!(pixels(&here) > 0, "the first tile has covered pixels");
	assert!(pixels(&here) <= (TILE_SIZE * TILE_SIZE) as usize);
}

#[test]
fn anti_aliasing_settings_change_the_edges() {
	let (mut fonts, family) = stack!();
	let content = point_text("Hamburgefonstiv", &family, 18.0, 3.0, 28.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	let format = PixelFormat::Rgba16;
	let smooth = render_text_tile(&layout, content.transform, 0, (0, 0), format, TextAntialias::Smooth);
	let aliased = render_text_tile(&layout, content.transform, 0, (0, 0), format, TextAntialias::None);
	assert!(any_ink(&smooth) && any_ink(&aliased), "both draw the text");
	assert!(smooth.as_u16() != aliased.as_u16(), "coverage off is not the same pixels as coverage on");
	// The hinted settings draw too (a font without instructions falls back to
	// the unhinted outlines, so this is about the path being exercised).
	for setting in [TextAntialias::Sharp, TextAntialias::Crisp, TextAntialias::Strong] {
		let hinted = render_text_tile(&layout, content.transform, 0, (0, 0), format, setting);
		assert!(any_ink(&hinted), "{setting:?} draws the text");
	}
}

#[test]
fn a_turning_layer_matrix_turns_the_text() {
	let (mut fonts, family) = stack!();
	let content = point_text("L", &family, 48.0, 40.0, 60.0);
	let layout = layout_of(&mut fonts, &content, 72.0);
	let format = PixelFormat::Rgba16;
	let upright = render_text_tile(&layout, content.transform, 0, (0, 0), format, TextAntialias::Smooth);
	// A quarter turn about (40, 60): (x, y) → (40 - (y - 60), 60 + (x - 40)).
	let turned = [0.0, 1.0, -1.0, 0.0, 100.0, 20.0];
	let turned = render_text_tile(&layout, turned, 0, (0, 0), format, TextAntialias::Smooth);
	assert!(any_ink(&upright) && any_ink(&turned), "the glyph is drawn in both");
	assert!(upright.as_u16() != turned.as_u16(), "and not in the same pixels");
}
