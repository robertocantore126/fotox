//! Text layers (M6-T07): the string, the formatting runs over it and the frame
//! it is laid out in.
//!
//! Like a shape layer, a text layer is *geometry*, not pixels: the string and
//! its runs are the truth (D-055), and the layer's tiles are laid out and
//! rendered on demand at the level being drawn — by the D-051 stack (parley
//! lays out, skrifa outlines) into the D-050 renderer's paths. Editing the text
//! therefore never resamples anything and is sharp at every zoom.
//!
//! Sizes are Photoshop's: `size_pt` is in points and the pixel size is
//! `size_pt × ppi / 72`, so the same text layer keeps its physical size when
//! the document's resolution changes. `tracking` and `leading` are in pixels at
//! level 0, like every other length in the document.

use serde::{Deserialize, Serialize};

/// A family, a style in it and everything the Type tool's option bar sets for a
/// stretch of the text. `range` is a byte range into the layer's text; runs are
/// stored in order and cover the text they apply to (gaps take the first run's
/// settings, as in the UI).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
	/// Byte range `start..end` of the text this run formats.
	pub range: (usize, usize),
	/// The family name, as the font list reports it.
	pub family: String,
	pub style: FontStyle,
	/// Size in points; the pixel size is `size_pt × ppi / 72` (Photoshop).
	pub size_pt: f64,
	/// 16-bit RGBA, like every other colour in the document.
	pub color: [u16; 4],
	/// Letter spacing in document pixels, added after every glyph.
	pub tracking: f64,
	/// Line spacing in document pixels; `None` uses the font's own line height.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub leading: Option<f64>,
}

impl TextRun {
	/// Whether `index` (a byte offset) is inside this run.
	pub fn covers(&self, index: usize) -> bool {
		index >= self.range.0 && index < self.range.1
	}

	/// The pixel size of the run's text at `ppi`, which is what the layout and
	/// the renderer work in.
	pub fn size_px(&self, ppi: f32) -> f64 {
		self.size_pt * f64::from(ppi) / 72.0
	}
}

/// The four styles a family is offered in (the Type option bar's list, and what
/// `fontique` reports).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontStyle {
	Regular,
	Bold,
	Italic,
	BoldItalic,
}

impl FontStyle {
	/// The label Photoshop's style list shows.
	pub fn label(self) -> &'static str {
		match self {
			FontStyle::Regular => "Regular",
			FontStyle::Bold => "Bold",
			FontStyle::Italic => "Italic",
			FontStyle::BoldItalic => "Bold Italic",
		}
	}

	/// The style a fontique face is, by its italic flag and weight.
	pub fn of(italic: bool, bold: bool) -> Self {
		match (bold, italic) {
			(false, false) => FontStyle::Regular,
			(true, false) => FontStyle::Bold,
			(false, true) => FontStyle::Italic,
			(true, true) => FontStyle::BoldItalic,
		}
	}

	/// Whether the style asks for a bold face.
	pub fn bold(self) -> bool {
		matches!(self, FontStyle::Bold | FontStyle::BoldItalic)
	}

	/// Whether the style asks for an italic face.
	pub fn italic(self) -> bool {
		matches!(self, FontStyle::Italic | FontStyle::BoldItalic)
	}

	/// Every style, in the order the option bar lists them.
	pub fn all() -> [FontStyle; 4] {
		[FontStyle::Regular, FontStyle::Bold, FontStyle::Italic, FontStyle::BoldItalic]
	}
}

/// How a text layer is framed: a single point (the text grows around it) or a
/// box (paragraph text wraps to the box's width).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TextFrame {
	/// The origin of the first line; nothing wraps.
	Point,
	/// The text box in document pixels: the layout wraps to `w` and the
	/// vertical alignment fits the lines into `h`.
	Box { w: f64, h: f64 },
}

impl TextFrame {
	/// The width the layout wraps at, or `None` for point text.
	pub fn wrap_width(&self) -> Option<f64> {
		match self {
			TextFrame::Point => None,
			TextFrame::Box { w, .. } => Some(w.abs().max(1.0)),
		}
	}

	/// The box's size, `(x, y)` extents; a point frame is as large as the text.
	pub fn size(&self) -> (f64, f64) {
		match self {
			TextFrame::Point => (0.0, 0.0),
			TextFrame::Box { w, h } => (w.abs(), h.abs()),
		}
	}
}

/// Paragraph alignment inside the frame (Photoshop's four buttons).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
	Left,
	Center,
	Right,
	Justify,
}

impl TextAlign {
	/// The label the option bar shows.
	pub fn label(self) -> &'static str {
		match self {
			TextAlign::Left => "Left align text",
			TextAlign::Center => "Center text",
			TextAlign::Right => "Right align text",
			TextAlign::Justify => "Justify last left",
		}
	}
}

/// Photoshop's anti-aliasing menu. Fotox draws text with tiny-skia's analytic
/// coverage (D-050), the same path shapes take, so the five settings differ in
/// how the outlines are prepared rather than in the rasteriser: `None` turns
/// coverage off (an aliased edge), the four smooth settings all anti-alias, and
/// `Sharp`/`Crisp`/`Strong` change how thin stems are held.
///
/// (The card asks to verify this mapping: parley/skrifa offer no
/// Photoshop-compatible contrast control, so Fotox keeps one analytic edge for
/// every smoothing level, and `None` is exact. See the T07 report.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAntialias {
	None,
	Sharp,
	Crisp,
	Strong,
	Smooth,
}

impl TextAntialias {
	/// The label the option bar's menu shows.
	pub fn label(self) -> &'static str {
		match self {
			TextAntialias::None => "None",
			TextAntialias::Sharp => "Sharp",
			TextAntialias::Crisp => "Crisp",
			TextAntialias::Strong => "Strong",
			TextAntialias::Smooth => "Smooth",
		}
	}

	/// Whether the renderer anti-aliases the glyph edges.
	pub fn anti_aliased(self) -> bool {
		self != TextAntialias::None
	}

	/// Every setting, in the menu's order.
	pub fn all() -> [TextAntialias; 5] {
		[
			TextAntialias::None,
			TextAntialias::Sharp,
			TextAntialias::Crisp,
			TextAntialias::Strong,
			TextAntialias::Smooth,
		]
	}
}

/// The content of a text layer: everything but its tiles.
///
/// The engine's `LayerKind::Text` flattens this next to its cache, and the
/// commands patch it one field at a time, exactly like a shape's geometry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
	pub text: String,
	/// Formatting runs, in byte order; an empty list formats everything with
	/// the layer's own defaults (`TextContent::default_run`).
	pub runs: Vec<TextRun>,
	pub frame: TextFrame,
	pub align: TextAlign,
	pub antialias: TextAntialias,
	/// Local → document, `[a, b, c, d, e, f]` (see `crate::vector`). The
	/// frame's origin sits at the transform's translation.
	pub transform: [f64; 6],
	/// Warp Text (M10-T08): applied to the glyph outlines, losslessly.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub warp: Option<Warp>,
}

/// Warp Text's styles (M10-T08).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarpStyle {
	Arc,
	Arch,
	Bulge,
	Flag,
	Wave,
	Fish,
	Rise,
	Squeeze,
}

/// A text warp: the style and its Bend (`-1..=1`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Warp {
	pub style: WarpStyle,
	pub bend: f64,
}

impl Warp {
	/// Where a frame-space point moves, the text's box being `[x0, y0, x1, y1]`
	/// (VERIFY: Photoshop's curves; these are the familiar shapes).
	pub fn apply(&self, p: (f64, f64), b: [f64; 4]) -> (f64, f64) {
		let (w, h) = ((b[2] - b[0]).max(1e-9), (b[3] - b[1]).max(1e-9));
		let u = (p.0 - b[0]) / w * 2.0 - 1.0; // -1..1 across
		let v = (p.1 - b[1]) / h * 2.0 - 1.0; // -1 top .. 1 bottom
		let k = self.bend.clamp(-1.0, 1.0);
		let pi = std::f64::consts::PI;
		let dy = match self.style {
			WarpStyle::Arc => -k * (1.0 - u * u) * h * 0.5 * (1.0 - v) / 2.0 * 2.0,
			WarpStyle::Arch => -k * (1.0 - u * u) * h * 0.5,
			WarpStyle::Bulge => -k * (1.0 - u * u) * h * 0.4 * -v,
			WarpStyle::Squeeze => k * (1.0 - u * u) * h * 0.4 * -v,
			WarpStyle::Flag => k * (pi * u).sin() * h * 0.25,
			WarpStyle::Wave => k * (pi * u).sin() * h * 0.25 * v,
			WarpStyle::Fish => -k * (pi * u * 0.5 + pi * 0.5).sin() * h * 0.3 * -v,
			WarpStyle::Rise => -k * u * h * 0.5,
		};
		(p.0, p.1 + dy)
	}
}

impl Default for TextContent {
	fn default() -> Self {
		Self {
			text: String::new(),
			runs: Vec::new(),
			frame: TextFrame::Point,
			align: TextAlign::Left,
			antialias: TextAntialias::Smooth,
			transform: crate::vector::IDENTITY,
			warp: None,
		}
	}
}

impl TextContent {
	/// A point text at `(x, y)` with a single run, the way the Type tool starts
	/// one: Fotox writes in the option bar's family and size.
	pub fn point(x: f64, y: f64, run: TextRun) -> Self {
		Self {
			runs: vec![TextRun { range: (0, 0), ..run }],
			transform: [1.0, 0.0, 0.0, 1.0, x, y],
			..Default::default()
		}
	}

	/// The run formatting `index`, or the first one: every character has
	/// settings, even the ones a run does not reach.
	pub fn run_at(&self, index: usize) -> Option<&TextRun> {
		self.runs.iter().find(|run| run.covers(index)).or_else(|| self.runs.first())
	}

	/// The settings a text layer starts with at `ppi`, in Photoshop's units.
	pub fn default_run(text: &str) -> TextRun {
		TextRun {
			range: (0, text.len()),
			family: "Arial".to_owned(),
			style: FontStyle::Regular,
			size_pt: 12.0,
			color: [0, 0, 0, 65_535],
			tracking: 0.0,
			leading: None,
		}
	}

	/// Make the runs cover the whole text: a gap at either end is closed by
	/// stretching the run next to it (interior splits are the option bar's own
	/// job; it always rewrites runs whole). Without this, a run set on a
	/// selection would leave the text before and after it unformatted.
	pub fn covering(mut self) -> Self {
		if self.runs.is_empty() {
			let mut run = Self::default_run(&self.text);
			run.range = (0, self.text.len());
			self.runs.push(run);
			return self;
		}
		let length = self.text.len();
		for run in &mut self.runs {
			run.range = (run.range.0.min(length), run.range.1.min(length));
			run.range.1 = run.range.1.max(run.range.0);
		}
		if let Some(first) = self.runs.first_mut() {
			first.range.0 = 0;
		}
		if let Some(last) = self.runs.last_mut() {
			last.range.1 = length;
		}
		self
	}

	/// The run covering `index`, with the settings the option bar should show:
	/// the layer's defaults when nothing is formatted yet.
	pub fn run_or_default(&self, index: usize) -> TextRun {
		match self.run_at(index) {
			Some(run) => run.clone(),
			None => Self::default_run(&self.text),
		}
	}

	/// The layer's default name, which Photoshop takes from the text itself
	/// ("Hello"); an empty layer is a "Type" layer.
	pub fn layer_name(&self) -> String {
		Self::name_for_text(&self.text)
	}

	/// The name a text layer with this text has: its first line, at most 24
	/// characters. An empty text is a "Type" layer (which the counter numbers).
	pub fn name_for_text(text: &str) -> String {
		let first_line = text.lines().next().unwrap_or("").trim();
		if first_line.is_empty() {
			return "Type".to_owned();
		}
		let mut name: String = first_line.chars().take(24).collect();
		if first_line.chars().count() > 24 {
			name.push('…');
		}
		name
	}

	/// The pixel size of the run covering `index`, or of the first run.
	pub fn size_px_at(&self, index: usize, ppi: f32) -> f64 {
		self.run_at(index).map_or(12.0, |run| run.size_px(ppi))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A point text of one run, the way the Type tool starts a layer.
	fn point_content(text: &str) -> TextContent {
		TextContent {
			text: text.into(),
			runs: vec![TextContent::default_run(text)],
			..Default::default()
		}
	}

	#[test]
	fn sizes_are_points_at_the_document_resolution() {
		let mut run = TextContent::default_run("Hello");
		run.size_pt = 12.0;
		assert_eq!(run.size_px(72.0), 12.0, "72 ppi: 1 pt is 1 px");
		assert_eq!(run.size_px(300.0), 50.0, "300 ppi: 12 pt is 50 px");
		assert_eq!(point_content("Hello").size_px_at(0, 72.0), 12.0, "the default run is 12 pt");
	}

	#[test]
	fn runs_are_found_by_byte_offset_and_fall_back_to_the_first() {
		let mut content = point_content("Hello world");
		content.runs = vec![
			TextRun {
				range: (0, 5),
				..TextContent::default_run("")
			},
			TextRun {
				range: (5, 11),
				family: "Times New Roman".into(),
				..TextContent::default_run("")
			},
		];
		assert_eq!(content.run_at(0).map(|r| r.family.as_str()), Some("Arial"));
		assert_eq!(content.run_at(4).map(|r| r.family.as_str()), Some("Arial"));
		assert_eq!(content.run_at(5).map(|r| r.family.as_str()), Some("Times New Roman"));
		assert_eq!(content.run_at(99).map(|r| r.family.as_str()), Some("Arial"), "past the end: the first run");
	}

	#[test]
	fn covering_stretches_the_ends_and_never_leaves_a_gap() {
		let mut content = point_content("Hello world");
		content.runs = vec![TextRun {
			range: (2, 5),
			..TextContent::default_run("")
		}];
		let content = content.covering();
		assert_eq!(content.runs[0].range, (0, 11), "the only run covers everything");
		let mut split = point_content("Hello world");
		split.runs = vec![
			TextRun {
				range: (0, 5),
				..TextContent::default_run("")
			},
			TextRun {
				range: (5, 8),
				..TextContent::default_run("")
			},
		];
		let split = split.covering();
		assert_eq!(split.runs.last().map(|run| run.range), Some((5, 11)), "the tail joins the last run");
	}

	#[test]
	fn the_layer_is_named_after_its_first_line() {
		assert_eq!(point_content("Hello\nworld").layer_name(), "Hello");
		assert_eq!(point_content("   \nworld").layer_name(), "Type", "a blank first line is a Type layer");
		assert_eq!(TextContent::name_for_text(&"x".repeat(40)).chars().count(), 25, "24 characters and an ellipsis");
	}

	#[test]
	fn point_text_is_anchored_where_it_was_clicked() {
		let content = TextContent::point(120.0, 80.0, TextContent::default_run("Hi"));
		assert_eq!(content.transform, [1.0, 0.0, 0.0, 1.0, 120.0, 80.0]);
		assert_eq!(content.frame, TextFrame::Point);
		assert_eq!(content.runs.len(), 1);
		assert_eq!(content.runs[0].range, (0, 0), "an empty text formats nothing yet");
	}

	#[test]
	fn a_point_frame_never_wraps_and_a_box_frame_does() {
		assert_eq!(TextFrame::Point.wrap_width(), None);
		assert_eq!(TextFrame::Box { w: 200.0, h: 80.0 }.wrap_width(), Some(200.0));
		assert_eq!(TextFrame::Box { w: -200.0, h: 0.0 }.wrap_width(), Some(200.0), "a negative width is a width");
		assert_eq!(TextFrame::Point.size(), (0.0, 0.0));
		assert_eq!(TextFrame::Box { w: 200.0, h: 80.0 }.size(), (200.0, 80.0));
	}

	#[test]
	fn styles_and_antialias_settings_name_themselves() {
		assert_eq!(FontStyle::of(true, false), FontStyle::Italic);
		assert_eq!(FontStyle::of(false, true), FontStyle::Bold);
		assert_eq!(FontStyle::BoldItalic.label(), "Bold Italic");
		assert!(FontStyle::BoldItalic.bold() && FontStyle::BoldItalic.italic());
		assert_eq!(FontStyle::all().len(), 4);
		assert!(!TextAntialias::None.anti_aliased());
		assert!(TextAntialias::Crisp.anti_aliased());
		assert_eq!(TextAntialias::all().len(), 5, "Photoshop's five settings");
		assert_eq!(TextAlign::Center.label(), "Center text");
	}

	#[test]
	fn text_content_survives_a_json_round_trip() {
		let content = TextContent {
			text: "Héllo wörld".into(),
			runs: vec![TextRun {
				range: (0, 13),
				family: "Times New Roman".into(),
				style: FontStyle::BoldItalic,
				size_pt: 24.0,
				color: [65_535, 0, 0, 65_535],
				tracking: 1.5,
				leading: Some(30.0),
			}],
			frame: TextFrame::Box { w: 400.0, h: 200.0 },
			align: TextAlign::Center,
			antialias: TextAntialias::Strong,
			transform: [1.0, 0.0, 0.0, 1.0, 20.0, 30.0],
			warp: None,
		};
		let json = serde_json::to_string(&content).expect("serialises");
		let back: TextContent = serde_json::from_str(&json).expect("parses");
		assert_eq!(back, content);
	}
}
