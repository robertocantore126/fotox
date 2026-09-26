//! Text layout and rasterising (M6-T07).
//!
//! A text layer holds a string and its formatting runs; the tiles it draws are
//! *derived* data, exactly like a shape layer's (D-055). This module is where
//! the string becomes glyphs and the glyphs become pixels:
//!
//! ```text
//! TextContent ──► parley: shape, wrap, align, caret geometry   (D-051)
//!        │
//!        ▼
//! skrifa: glyph outlines at the level-0 pixel size, in frame space
//!        │
//!        ▼
//! tiny-skia: one tile at the level being drawn                 (D-050)
//! ```
//!
//! **Frame space.** A text layer's placement matrix maps *frame space* to
//! document pixels. The frame's origin is what the Type tool's click means:
//! for point text it is the first line's baseline (Photoshop's insertion
//! point), and the block is aligned around it — Left puts the text's left edge
//! there, Center the middle, Right the right edge. For box text it is the box's
//! top-left corner, the text wraps to its width and the alignment applies
//! inside it. Everything this module reports (glyph outlines, line boxes,
//! carets, selections) is frame space, so the overlay and the tile rasteriser
//! only need the layer's matrix.
//!
//! **One layout per layer, many tiles.** Shaping, line breaking and outlining
//! are the expensive part and do not depend on the zoom, so [`Fonts::layout`]
//! runs them once at the level-0 pixel size and stores tiny-skia paths. A tile
//! then only appends the glyphs whose bounds meet it and fills them, which is
//! why text stays sharp at every level (the outlines are scaled, never
//! resampled) and why one tile of a page of text costs one tile of work.

use fx_core::text::{FontStyle, TextAlign, TextAntialias, TextContent, TextFrame, TextRun};
use fx_tiles::{PixelFormat, TILE_PIXELS, TILE_SIZE, TileBuffer};
use parley::editing::{Cursor, Selection};
use parley::{
	Affinity, Alignment, AlignmentOptions, FontContext, FontFamily, FontWeight, Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, Hinting, HintingInstance, HintingMode, OutlineGlyphCollection, OutlinePen};
use skrifa::{FontRef, GlyphId};
use tiny_skia::{FillRule, Path, PathBuilder, Pixmap, Transform};

/// The brush parley carries through a layout: the colour of the run a glyph
/// belongs to, in the document's 16-bit RGBA.
#[derive(Clone, PartialEq, Debug, Default)]
struct TextBrush {
	color: [u16; 4],
}

/// A rectangle in frame space.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TextRect {
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

impl TextRect {
	/// The smallest rectangle containing both.
	pub fn union(self, other: Self) -> Self {
		let x0 = self.x.min(other.x);
		let y0 = self.y.min(other.y);
		let x1 = (self.x + self.w).max(other.x + other.w);
		let y1 = (self.y + self.h).max(other.y + other.h);
		Self {
			x: x0,
			y: y0,
			w: (x1 - x0).max(0.0),
			h: (y1 - y0).max(0.0),
		}
	}

	/// Whether the rectangle covers no area.
	pub fn is_empty(self) -> bool {
		!(self.w > 0.0 && self.h > 0.0)
	}

	/// The rectangle as `[x0, y0, x1, y1]`, the form the commands and the tile
	/// cache take.
	pub fn as_box(self) -> [f64; 4] {
		[self.x, self.y, self.x + self.w, self.y + self.h]
	}
}

/// One line of a laid-out text, in frame space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextLine {
	/// The line's box, from its top to the bottom of its line height.
	pub rect: TextRect,
	/// The byte range of the text the line holds.
	pub text_range: (usize, usize),
}

/// A laid-out text layer: everything the renderer and the overlay need.
///
/// Plain data (plus the parley layout, which is plain data too), so it is
/// shared across the rayon workers with an `Arc` while they draw the tiles.
pub struct TextLayout {
	/// The shaped layout; caret and selection geometry come from it.
	layout: Layout<TextBrush>,
	/// Frame space → layout space: the frame's own offset.
	origin: (f64, f64),
	/// The glyphs, grouped by colour, in frame space.
	runs: Vec<RunOutlines>,
	lines: Vec<TextLine>,
	size: (f64, f64),
	/// How many glyphs the layout holds; `0` means nothing to draw.
	glyphs: usize,
}

/// The glyph outlines of one colour, in frame space.
struct RunOutlines {
	color: [u16; 4],
	/// One path per glyph. Keeping them apart lets a tile skip the glyphs it
	/// does not cover instead of walking the whole page's outline.
	glyphs: Vec<Path>,
}

impl TextLayout {
	/// The layout's size in frame space: the widest line and the block height.
	pub fn size(&self) -> (f64, f64) {
		self.size
	}

	/// Whether the layout drew nothing (empty text, or glyphs with no outline).
	pub fn is_empty(&self) -> bool {
		self.glyphs == 0
	}

	/// The lines, top to bottom.
	pub fn lines(&self) -> &[TextLine] {
		&self.lines
	}

	/// The box of every line, top to bottom: what the text inks.
	pub fn ink_box(&self) -> Option<TextRect> {
		self.lines.iter().map(|line| line.rect).reduce(TextRect::union)
	}

	/// The box of the lines covering `range`, in frame space.
	///
	/// Used to redraw only what an edit changed: the lines an insertion or a
	/// deletion touches are exactly the ones whose glyphs can differ. An empty
	/// range (a caret) still reports the line it sits on.
	pub fn box_for_range(&self, range: (usize, usize)) -> Option<TextRect> {
		let mut hit = None;
		for line in &self.lines {
			if covers(line.text_range, range) {
				hit = Some(hit.map_or(line.rect, |box_: TextRect| box_.union(line.rect)));
			}
		}
		hit
	}

	/// The box of `line` and everything below it, in frame space.
	///
	/// An edit that adds or removes a line shifts every later line down or up,
	/// so those lines have to be drawn again even though their text did not
	/// change.
	pub fn box_from_line(&self, line: usize) -> Option<TextRect> {
		self.lines.get(line..)?.iter().map(|line| line.rect).reduce(TextRect::union)
	}

	/// The box of the line holding `index` (or the last one, past the end).
	pub fn line_box_at(&self, index: usize) -> Option<TextRect> {
		self.line_index_at(index).and_then(|line| self.lines.get(line)).map(|line| line.rect)
	}

	/// The index of the line holding the byte `index`.
	pub fn line_index_at(&self, index: usize) -> Option<usize> {
		if self.lines.is_empty() {
			return None;
		}
		let found = self.lines.iter().position(|line| covers(line.text_range, (index, index)));
		Some(found.unwrap_or(self.lines.len() - 1))
	}

	/// How many lines the layout broke into.
	pub fn line_count(&self) -> usize {
		self.lines.len()
	}

	/// The caret before the byte `index`: a one pixel wide box at the right
	/// height for the line it sits on, in frame space.
	pub fn caret(&self, index: usize) -> TextRect {
		let cursor = Cursor::from_byte_index(&self.layout, index, Affinity::Downstream);
		let box_ = cursor.geometry(&self.layout, 1.0);
		self.frame_rect(box_.x0, box_.y0, box_.x1 - box_.x0, box_.y1 - box_.y0)
	}

	/// The boxes a selection paints, one per line it covers, in frame space.
	pub fn selection(&self, range: (usize, usize)) -> Vec<TextRect> {
		let (start, end) = (range.0.min(range.1), range.0.max(range.1));
		if start == end {
			return Vec::new();
		}
		let selection =
			Selection::from_byte_index(&self.layout, start, Affinity::Downstream).extend(Cursor::from_byte_index(&self.layout, end, Affinity::Downstream));
		selection
			.geometry(&self.layout)
			.into_iter()
			.map(|(box_, _)| self.frame_rect(box_.x0, box_.y0, box_.x1 - box_.x0, box_.y1 - box_.y0))
			.collect()
	}

	/// The byte index nearest the frame-space point `(x, y)`.
	///
	/// What a click in the text area means: parley picks the cluster and the
	/// side, so clicking the right half of a character puts the caret after it.
	pub fn index_at(&self, x: f64, y: f64) -> usize {
		let (lx, ly) = (x - self.origin.0, y - self.origin.1);
		Cursor::from_point(&self.layout, lx as f32, ly as f32).index()
	}

	/// A parley bounding box in frame space.
	fn frame_rect(&self, x: f64, y: f64, w: f64, h: f64) -> TextRect {
		TextRect {
			x: x + self.origin.0,
			y: y + self.origin.1,
			w,
			h,
		}
	}
}

/// Whether a line's text range meets `range` (an empty range is a caret).
///
/// A caret sits in the line whose text contains it, so a caret at a line break
/// belongs to the next line (that is where Photoshop draws it), and a caret on
/// an empty line belongs to that line alone.
fn covers(line: (usize, usize), range: (usize, usize)) -> bool {
	if range.0 == range.1 {
		return if line.1 > line.0 {
			range.0 >= line.0 && range.0 < line.1
		} else {
			range.0 == line.0
		};
	}
	range.0 < line.1 && line.0 < range.1
}

/// A font family and the styles the collection has for it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontEntry {
	/// The family name as the platform reports it.
	pub name: String,
	/// The styles of this family, in the option bar's order.
	pub styles: Vec<FontStyle>,
}

/// The font stack: the system font collection and the layout scratch space.
///
/// One per process (or per engine): fontique keeps a database and a source
/// cache, and parley's layout context reuses allocations between layouts.
pub struct Fonts {
	fonts: FontContext,
	layout: LayoutContext<TextBrush>,
}

impl Default for Fonts {
	fn default() -> Self {
		Self::new()
	}
}

impl Fonts {
	/// Open the platform's fonts. Enumerating them is lazy: the first
	/// [`Fonts::families`] or layout is what actually scans.
	pub fn new() -> Self {
		Self {
			fonts: FontContext::new(),
			layout: LayoutContext::new(),
		}
	}

	/// Every font family the platform offers, with the styles it has, sorted by
	/// name. What the Type option bar's font list shows.
	pub fn families(&mut self) -> Vec<FontEntry> {
		let names: Vec<String> = self.fonts.collection.family_names().map(str::to_owned).collect();
		let mut families: Vec<FontEntry> = names
			.into_iter()
			.filter_map(|name| {
				let family = self.fonts.collection.family_by_name(&name)?;
				let mut styles: Vec<FontStyle> = family
					.fonts()
					.iter()
					.map(|font| {
						let italic = !matches!(font.style(), parley::fontique::FontStyle::Normal);
						FontStyle::of(italic, font.weight().value() >= 600.0)
					})
					.collect();
				styles.sort_unstable_by_key(|style| FontStyle::all().iter().position(|s| s == style).unwrap_or(0));
				styles.dedup();
				if styles.is_empty() {
					styles.push(FontStyle::Regular);
				}
				Some(FontEntry {
					name: family.name().to_owned(),
					styles,
				})
			})
			.collect();
		families.sort_unstable_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
		families
	}

	/// Lay out a text layer at the document's resolution, with the frame's
	/// alignment applied.
	pub fn layout(&mut self, content: &TextContent, ppi: f32) -> TextLayout {
		let text = content.text.as_str();
		let default_run = content.run_or_default(0);
		let mut builder = self.layout.ranged_builder(&mut self.fonts, text, 1.0, false);
		builder.push_default(StyleProperty::FontFamily(FontFamily::named(&default_run.family)));
		builder.push_default(StyleProperty::FontSize(pixel_size(&default_run, ppi, content.antialias) as f32));
		builder.push_default(StyleProperty::FontWeight(weight_of(default_run.style)));
		builder.push_default(StyleProperty::FontStyle(style_of(default_run.style)));
		builder.push_default(StyleProperty::Brush(TextBrush { color: default_run.color }));
		for run in &content.runs {
			let range = run.range.0.min(text.len())..run.range.1.min(text.len());
			if range.is_empty() {
				continue;
			}
			push_run(&mut builder, run, ppi, content.antialias, range);
		}
		let mut layout = builder.build(text);
		let align = match content.frame {
			// Point text has nothing to align against: the shift below is the
			// whole of its alignment.
			TextFrame::Point => Alignment::Start,
			TextFrame::Box { .. } => match content.align {
				TextAlign::Left => Alignment::Start,
				TextAlign::Center => Alignment::Center,
				TextAlign::Right => Alignment::End,
				TextAlign::Justify => Alignment::Justify,
			},
		};
		layout.break_all_lines(content.frame.wrap_width().map(|w| w as f32));
		layout.align(align, AlignmentOptions::default());
		let width = f64::from(layout.width());
		let full = f64::from(layout.full_width());
		let height = f64::from(layout.height());
		let origin = match content.frame {
			TextFrame::Point => {
				// The click is the first line's baseline (Photoshop inserts
				// point text on the baseline) and the block hangs around it.
				let baseline = layout.get(0).map_or(0.0, |line| f64::from(line.metrics().baseline));
				let x = match content.align {
					TextAlign::Center => -full / 2.0,
					TextAlign::Right => -full,
					TextAlign::Left | TextAlign::Justify => 0.0,
				};
				(x, -baseline)
			}
			TextFrame::Box { .. } => (0.0, 0.0),
		};
		let lines = collect_lines(&layout, origin);
		let mut runs = collect_outlines(&layout, content, origin);
		// Warp Text (M10-T08): every outline point through the warp, over the
		// glyphs' own box. FAST: control points are moved, not re-fitted.
		if let Some(warp) = content.warp {
			warp_runs(&mut runs, &warp);
		}
		let glyphs = runs.iter().map(|run| run.glyphs.len()).sum();
		TextLayout {
			layout,
			origin,
			runs,
			lines,
			size: (width, height),
			glyphs,
		}
	}
}

/// Push one run's properties over its byte range.
fn push_run(builder: &mut parley::RangedBuilder<'_, TextBrush>, run: &TextRun, ppi: f32, antialias: TextAntialias, range: std::ops::Range<usize>) {
	builder.push(StyleProperty::FontFamily(FontFamily::named(&run.family)), range.clone());
	builder.push(StyleProperty::FontSize(pixel_size(run, ppi, antialias) as f32), range.clone());
	builder.push(StyleProperty::FontWeight(weight_of(run.style)), range.clone());
	builder.push(StyleProperty::FontStyle(style_of(run.style)), range.clone());
	builder.push(StyleProperty::Brush(TextBrush { color: run.color }), range.clone());
	if run.tracking != 0.0 {
		builder.push(StyleProperty::LetterSpacing(run.tracking as f32), range.clone());
	}
	if let Some(leading) = run.leading {
		builder.push(StyleProperty::LineHeight(LineHeight::Absolute(leading as f32)), range);
	}
}

/// A run's size in level-0 document pixels: points at the document's
/// resolution, which is Photoshop's rule (`size_pt × ppi / 72`).
///
/// Crisp snaps it to whole pixels, so every stem lands on the pixel grid — the
/// four anti-aliasing settings differ in how the outlines meet the grid, and
/// this is Crisp's way (see the T07 report).
fn pixel_size(run: &TextRun, ppi: f32, antialias: TextAntialias) -> f64 {
	let size = run.size_px(ppi);
	if antialias == TextAntialias::Crisp { size.round().max(1.0) } else { size }
}

fn weight_of(style: FontStyle) -> FontWeight {
	if style.bold() { FontWeight::BOLD } else { FontWeight::NORMAL }
}

fn style_of(style: FontStyle) -> parley::FontStyle {
	if style.italic() {
		parley::FontStyle::Italic
	} else {
		parley::FontStyle::Normal
	}
}

/// The lines of a layout, in frame space.
fn collect_lines(layout: &Layout<TextBrush>, origin: (f64, f64)) -> Vec<TextLine> {
	layout
		.lines()
		.map(|line| {
			let metrics = line.metrics();
			let range = line.text_range();
			TextLine {
				rect: TextRect {
					x: f64::from(metrics.inline_min_coord) + origin.0,
					y: f64::from(metrics.block_min_coord) + origin.1,
					w: f64::from(metrics.inline_max_coord - metrics.inline_min_coord),
					h: f64::from(metrics.block_max_coord - metrics.block_min_coord),
				},
				text_range: (range.start, range.end),
			}
		})
		.collect()
}

/// Outline every glyph of a layout, in frame space, grouped by colour.
///
/// The outlines are drawn at the level-0 pixel size: a tile at level `L` scales
/// them by `1 / 2ᴸ`, which is what keeps text sharp at every zoom. Hinting is
/// applied here, once, from the layer's anti-aliasing setting (see
/// `TextAntialias`): the four smooth settings differ in how the outlines are
/// grid-fitted, and `None` is the same outlines drawn without coverage.
fn collect_outlines(layout: &Layout<TextBrush>, content: &TextContent, origin: (f64, f64)) -> Vec<RunOutlines> {
	let mut runs: Vec<RunOutlines> = Vec::new();
	let hinting = hinting_mode(content.antialias);
	for line in layout.lines() {
		for item in line.items() {
			let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
			let run = glyph_run.run();
			let color = glyph_run.style().brush.color;
			let size = f64::from(run.font_size());
			let font_data = run.font();
			let Ok(font) = FontRef::from_index(font_data.data.data(), font_data.index) else {
				continue;
			};
			let outlines = OutlineGlyphCollection::new(&font);
			// Hinting follows the FreeType modes skrifa documents: Sharp is
			// `FT_LOAD_TARGET_NORMAL` with the horizontal metrics preserved (so
			// the layout's advances stay valid), Strong is the monochrome
			// target, which is the heavier grid fit Photoshop's Strong has.
			let instance = hinting
				.and_then(|mode| HintingInstance::new(&outlines, Size::new(size as f32), LocationRef::default(), mode).ok())
				.filter(HintingInstance::is_enabled);
			let mut memory: Vec<u8> = Vec::new();
			let placed: Vec<Path> = glyph_run
				.positioned_glyphs()
				.filter_map(|glyph| {
					let outline = outlines.get(GlyphId::new(glyph.id))?;
					// The outline is emitted straight into the tiny-skia path,
					// already moved to where the glyph sits in frame space.
					let mut pen = PathPen::new(f64::from(glyph.x) + origin.0, f64::from(glyph.y) + origin.1);
					let drawn = match &instance {
						Some(instance) => {
							let needed = outline.draw_memory_size(Hinting::Embedded);
							if memory.len() < needed {
								memory.resize(needed, 0);
							}
							outline
								.draw(DrawSettings::hinted(instance, false).with_memory(Some(&mut memory[..needed])), &mut pen)
								.is_ok()
						}
						None => outline
							.draw(DrawSettings::unhinted(Size::new(size as f32), LocationRef::default()), &mut pen)
							.is_ok(),
					};
					if !drawn {
						// A font that refuses to outline (a bitmap-only face,
						// say) draws nothing rather than a wrong glyph.
						return None;
					}
					pen.finish()
				})
				.collect();
			if placed.is_empty() {
				continue;
			}
			match runs.iter_mut().find(|known| known.color == color) {
				Some(known) => known.glyphs.extend(placed),
				None => runs.push(RunOutlines { color, glyphs: placed }),
			}
		}
	}
	runs
}

/// The glyphs' box in frame space, `[x0, y0, x1, y1]`.
fn runs_box(runs: &[RunOutlines]) -> Option<[f64; 4]> {
	let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
	for run in runs {
		for g in &run.glyphs {
			let r = g.bounds();
			b = [
				b[0].min(f64::from(r.left())),
				b[1].min(f64::from(r.top())),
				b[2].max(f64::from(r.right())),
				b[3].max(f64::from(r.bottom())),
			];
		}
	}
	b[0].is_finite().then_some(b)
}

/// A tiny-skia path with every point moved by `f`.
fn map_path(path: &Path, f: &dyn Fn(f64, f64) -> (f64, f64)) -> Option<Path> {
	use tiny_skia::PathSegment;
	let mut b = PathBuilder::new();
	let p = |pt: tiny_skia::Point| {
		let (x, y) = f(f64::from(pt.x), f64::from(pt.y));
		(x as f32, y as f32)
	};
	for seg in path.segments() {
		match seg {
			PathSegment::MoveTo(a) => {
				let (x, y) = p(a);
				b.move_to(x, y);
			}
			PathSegment::LineTo(a) => {
				let (x, y) = p(a);
				b.line_to(x, y);
			}
			PathSegment::QuadTo(c, a) => {
				let ((cx, cy), (x, y)) = (p(c), p(a));
				b.quad_to(cx, cy, x, y);
			}
			PathSegment::CubicTo(c1, c2, a) => {
				let ((x1, y1), (x2, y2), (x, y)) = (p(c1), p(c2), p(a));
				b.cubic_to(x1, y1, x2, y2, x, y);
			}
			PathSegment::Close => b.close(),
		}
	}
	b.finish()
}

fn warp_runs(runs: &mut [RunOutlines], warp: &fx_core::text::Warp) {
	let Some(bx) = runs_box(runs) else { return };
	for run in runs.iter_mut() {
		run.glyphs = run.glyphs.iter().filter_map(|g| map_path(g, &|x, y| warp.apply((x, y), bx))).collect();
	}
}

impl TextLayout {
	/// Every glyph outline as path elements in document coordinates, through
	/// the layer's `transform` (Type ▸ Create Work Path / Convert to Shape /
	/// type masks, M10-T08), and the first run's colour.
	pub fn outline_elements(&self, transform: [f64; 6]) -> (Vec<fx_core::vector::PathEl>, [u16; 4]) {
		use fx_core::vector::PathEl;
		use tiny_skia::PathSegment;
		let m = transform;
		let t = |pt: tiny_skia::Point| {
			let (x, y) = (f64::from(pt.x), f64::from(pt.y));
			[m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
		};
		let mut out = Vec::new();
		for run in &self.runs {
			for g in &run.glyphs {
				for seg in g.segments() {
					out.push(match seg {
						PathSegment::MoveTo(a) => PathEl::MoveTo(t(a)),
						PathSegment::LineTo(a) => PathEl::LineTo(t(a)),
						PathSegment::QuadTo(c, a) => PathEl::QuadTo(t(c), t(a)),
						PathSegment::CubicTo(c1, c2, a) => PathEl::CubicTo(t(c1), t(c2), t(a)),
						PathSegment::Close => PathEl::Close,
					});
				}
			}
		}
		(out, self.runs.first().map_or([0, 0, 0, 65535], |r| r.color))
	}
}

/// The hinting mode an anti-aliasing setting asks for, or `None` for the plain
/// unhinted outlines. `Crisp` snaps the size instead (see `collect_outlines`).
fn hinting_mode(antialias: TextAntialias) -> Option<HintingMode> {
	match antialias {
		TextAntialias::Sharp => Some(HintingMode::Smooth {
			lcd_subpixel: None,
			preserve_linear_metrics: true,
		}),
		TextAntialias::Strong => Some(HintingMode::Strong),
		TextAntialias::None | TextAntialias::Crisp | TextAntialias::Smooth => None,
	}
}

/// Builds a tiny-skia path from glyph outlines as they are emitted, moving
/// every point into frame space.
///
/// Glyph outline coordinates are relative to the pen position on the baseline,
/// which is where the layout says the glyph sits; `dx`/`dy` are that position
/// in frame space. Working through the pen keeps one allocation per glyph
/// instead of a point list per glyph.
struct PathPen {
	builder: PathBuilder,
	dx: f64,
	dy: f64,
	open: bool,
}

impl PathPen {
	fn new(dx: f64, dy: f64) -> Self {
		Self {
			builder: PathBuilder::new(),
			dx,
			dy,
			open: false,
		}
	}

	fn point(&self, x: f32, y: f32) -> (f32, f32) {
		((f64::from(x) + self.dx) as f32, (f64::from(y) + self.dy) as f32)
	}

	/// The glyph's path, or `None` when it has no outline at all (a space,
	/// say): there is nothing to fill, and no zero-area path to carry around.
	fn finish(self) -> Option<Path> {
		self.builder.finish()
	}
}

impl OutlinePen for PathPen {
	fn move_to(&mut self, x: f32, y: f32) {
		let (x, y) = self.point(x, y);
		self.builder.move_to(x, y);
		self.open = true;
	}

	fn line_to(&mut self, x: f32, y: f32) {
		let (x, y) = self.point(x, y);
		if self.open {
			self.builder.line_to(x, y);
		} else {
			// A contour that starts with a line still needs a starting point.
			self.builder.move_to(x, y);
			self.open = true;
		}
	}

	fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
		let (cx0, cy0) = self.point(cx0, cy0);
		let (x, y) = self.point(x, y);
		self.builder.quad_to(cx0, cy0, x, y);
	}

	fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
		let (cx0, cy0) = self.point(cx0, cy0);
		let (cx1, cy1) = self.point(cx1, cy1);
		let (x, y) = self.point(x, y);
		self.builder.cubic_to(cx0, cy0, cx1, cy1, x, y);
	}

	fn close(&mut self) {
		self.builder.close();
		self.open = false;
	}
}

/// Draw one tile of a laid-out text at `level`.
///
/// `transform` maps frame space to document pixels (the layer's matrix); `tile`
/// is the tile coordinate at `level`. Only the glyphs whose bounds meet the
/// tile are appended, so a tile of a full page costs a tile's worth of
/// outline work. The result is a `TILE_SIZE × TILE_SIZE` buffer in `format`.
pub fn render_text_tile(layout: &TextLayout, transform: [f64; 6], level: usize, tile: (u32, u32), format: PixelFormat, antialias: TextAntialias) -> TileBuffer {
	let pixels = TILE_PIXELS as u32;
	if layout.is_empty() {
		return TileBuffer::zeroed(format);
	}
	let Some(mut pixmap) = Pixmap::new(pixels, pixels) else {
		return TileBuffer::zeroed(format);
	};
	// frame space → document pixels → level space → this tile.
	let scale = f64::from(1u32 << level);
	let origin = (f64::from(tile.0) * f64::from(TILE_SIZE), f64::from(tile.1) * f64::from(TILE_SIZE));
	let level_to_tile = Transform::from_row(
		1.0 / scale as f32,
		0.0,
		0.0,
		1.0 / scale as f32,
		(-origin.0 / scale) as f32,
		(-origin.1 / scale) as f32,
	);
	let frame_to_doc = Transform::from_row(
		transform[0] as f32,
		transform[1] as f32,
		transform[2] as f32,
		transform[3] as f32,
		transform[4] as f32,
		transform[5] as f32,
	);
	let tile_transform = level_to_tile.pre_concat(frame_to_doc);
	// The tile's own box in tile space, to skip the glyphs that miss it. A
	// glyph that misses contributes nothing, so a tile of a page of text only
	// walks the glyphs on it.
	let Some(tile_box) = tiny_skia::Rect::from_ltrb(0.0, 0.0, pixels as f32, pixels as f32) else {
		return TileBuffer::zeroed(format);
	};
	for run in &layout.runs {
		let mut builder_path = PathBuilder::new();
		let mut any = false;
		for glyph in &run.glyphs {
			let Some(bounds) = glyph.bounds().transform(tile_transform) else { continue };
			if bounds.intersect(&tile_box).is_none() {
				continue;
			}
			builder_path.push_path(glyph);
			any = true;
		}
		if !any {
			continue;
		}
		let Some(path) = builder_path.finish() else { continue };
		let mut paint = tiny_skia::Paint::default();
		let rgba = run.color;
		paint.set_color_rgba8(
			(f64::from(rgba[0]) / f64::from(u16::MAX) * 255.0).round() as u8,
			(f64::from(rgba[1]) / f64::from(u16::MAX) * 255.0).round() as u8,
			(f64::from(rgba[2]) / f64::from(u16::MAX) * 255.0).round() as u8,
			(f64::from(rgba[3]) / f64::from(u16::MAX) * 255.0).round() as u8,
		);
		paint.anti_alias = antialias.anti_aliased();
		pixmap.fill_path(&path, &paint, FillRule::Winding, tile_transform, None);
	}
	convert(&pixmap.take_demultiplied(), format)
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
				*word = u16::from(straight.get(i).copied().unwrap_or(0)) * 257;
			}
		}
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
