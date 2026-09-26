//! The Horizontal Type tool (M6-T07).
//!
//! A click creates a point text layer (a drag, a paragraph box) or enters an
//! existing text layer. The UI owns the keyboard and the IME: it puts a hidden
//! `<textarea>` up and sends the whole text and selection on every input
//! (`UiToEngine::TextEdit`). While the session is open the tool edits the
//! document *directly*, outside the history, so each keystroke is cheap; the
//! commit puts the document back as it was and returns one command, which the
//! engine records as a single "Type Tool" step. Esc puts it back and returns
//! nothing.

use fx_core::command::NewLayer;
use fx_core::text::{FontStyle, TextAlign, TextAntialias, TextContent, TextFrame, TextRun};
use fx_core::{Command, CommandContext, Document, LayerId, LayerKind, LayerRef};
use fx_render::{Overlay, OverlayItem, TextRect};

use super::{DocPointer, Tool, ToolContext, ToolResult};
use crate::{CursorShape, Modifiers, PointerKind};

const TOOL: &str = "type";

/// What the Type tool tells the UI about its session.
#[derive(Clone, Debug, PartialEq)]
pub enum TextSession {
	/// A session is open (or its text or selection changed): the textarea
	/// shows `text` with `selection` (byte offsets).
	Open { text: String, selection: (usize, usize) },
	/// The session ended (committed or cancelled): the textarea goes away.
	Closed,
}

struct Edit {
	layer: LayerId,
	/// The document before the session: restored on commit (then one command
	/// is applied on top of it) and on cancel.
	before: Document,
	created: bool,
	original: Option<TextContent>,
	content: TextContent,
	selection: (usize, usize),
}

#[derive(Default)]
pub struct TypeTool {
	edit: Option<Edit>,
	press: Option<(f64, f64)>,
	drag: Option<(f64, f64)>,
	overlay: Option<Overlay>,
}

impl TypeTool {
	/// Whether a session is open.
	pub fn editing(&self) -> bool {
		self.edit.is_some()
	}

	/// End the session, keeping the text (commit) or not (cancel).
	fn finish(&mut self, ctx: &mut ToolContext<'_>, keep: bool) -> ToolResult {
		let Some(edit) = self.edit.take() else {
			return ToolResult::default();
		};
		self.overlay = None;
		// FAST: restoring the whole document drops anything else that changed
		// during the session (a panel edit while typing).
		*ctx.doc = edit.before;
		let mut result = ToolResult {
			redraw: true,
			doc_changed: true,
			text_session: Some(TextSession::Closed),
			..Default::default()
		};
		if !keep {
			return result;
		}
		let content = edit.content.clone().covering();
		if edit.created {
			if !content.text.trim().is_empty() {
				result.command = Some(Command::AddLayer {
					layer: NewLayer::Text { content },
					name: None,
				});
			}
		} else if edit.original.as_ref() != Some(&content) {
			result.command = Some(Command::SetText {
				layer: LayerRef::Id(edit.layer),
				content,
				dirty: [0.0, 0.0, f64::from(ctx.doc.width), f64::from(ctx.doc.height)], // FAST: whole canvas
			});
		}
		result
	}

	fn begin(&mut self, ctx: &mut ToolContext<'_>, at: (f64, f64), frame: TextFrame) -> ToolResult {
		let before = ctx.doc.clone();
		// An existing text layer under the click is entered, not replaced.
		if frame == TextFrame::Point
			&& let Some((id, content)) = hit_text_layer(ctx.doc, at)
		{
			let layout = crate::text::layout_uncached(&content, ctx.doc.ppi);
			let local = inverse(content.transform, at);
			let index = layout.index_at(local.0, local.1);
			let _ = Command::SelectLayers {
				layers: vec![LayerRef::Id(id)],
			}
			.apply(ctx.doc, &mut CommandContext { tiles: ctx.store, ops: None });
			self.edit = Some(Edit {
				layer: id,
				before,
				created: false,
				original: Some(content.clone()),
				content: content.clone(),
				selection: (index, index),
			});
			self.refresh_overlay(ctx);
			return ToolResult {
				text_session: Some(TextSession::Open {
					text: content.text,
					selection: (index, index),
				}),
				doc_changed: true,
				redraw: true,
				..Default::default()
			};
		}
		let mut content = TextContent::point(at.0, at.1, run_from_options(ctx));
		content.frame = frame;
		content.align = align_from_options(ctx);
		content.antialias = antialias_from_options(ctx);
		let added = Command::AddLayer {
			layer: NewLayer::Text { content: content.clone() },
			name: None,
		}
		.apply(ctx.doc, &mut CommandContext { tiles: ctx.store, ops: None });
		if let Err(error) = added {
			return ToolResult {
				info: Some(error.to_string()),
				..Default::default()
			};
		}
		let Some(layer) = ctx.doc.active_layer() else {
			return ToolResult::default();
		};
		self.edit = Some(Edit {
			layer,
			before,
			created: true,
			original: None,
			content: content.clone(),
			selection: (0, 0),
		});
		self.refresh_overlay(ctx);
		ToolResult {
			text_session: Some(TextSession::Open {
				text: String::new(),
				selection: (0, 0),
			}),
			doc_changed: true,
			redraw: true,
			..Default::default()
		}
	}

	/// Write the session's content into the layer, outside the history.
	fn push_content(&mut self, ctx: &mut ToolContext<'_>) {
		let Some(edit) = &self.edit else { return };
		let dirty = [0.0, 0.0, f64::from(ctx.doc.width), f64::from(ctx.doc.height)]; // FAST: whole canvas every keystroke
		let _ = Command::SetText {
			layer: LayerRef::Id(edit.layer),
			content: edit.content.clone(),
			dirty,
		}
		.apply(ctx.doc, &mut CommandContext { tiles: ctx.store, ops: None });
		self.refresh_overlay(ctx);
	}

	/// The caret, the selection and the frame box, in document coordinates.
	fn refresh_overlay(&mut self, ctx: &ToolContext<'_>) {
		let Some(edit) = &self.edit else {
			self.overlay = None;
			return;
		};
		let layout = crate::text::layout_uncached(&edit.content, ctx.doc.ppi);
		let t = edit.content.transform;
		let mut items = Vec::new();
		let zoom = ctx.view.zoom.max(1e-6);
		for rect in layout.selection(edit.selection) {
			items.push(OverlayItem::Fill {
				quad: crate::text::doc_quad(t, rect),
				color: [0.2, 0.45, 1.0, 0.35],
			});
		}
		if edit.selection.0 == edit.selection.1 {
			let mut caret = layout.caret(edit.selection.1);
			// At least one screen pixel wide.
			caret.w = caret.w.max(1.0 / zoom);
			if caret.h <= 0.0 {
				let size = edit.content.size_px_at(0, ctx.doc.ppi);
				caret = TextRect {
					x: caret.x,
					y: -size,
					w: 1.0 / zoom,
					h: size * 1.2,
				};
			}
			items.push(OverlayItem::Fill {
				quad: crate::text::doc_quad(t, caret),
				color: [0.0, 0.0, 0.0, 1.0],
			});
		}
		// The frame: the box of a paragraph text, or the ink of a point text.
		let frame = match edit.content.frame {
			TextFrame::Box { w, h } => Some(TextRect { x: 0.0, y: 0.0, w, h }),
			TextFrame::Point => layout.ink_box(),
		};
		if let Some(frame) = frame {
			let quad = crate::text::doc_quad(t, frame);
			let mut points = quad.to_vec();
			points.push(quad[0]);
			items.push(OverlayItem::Polyline {
				points,
				closed: true,
				style: fx_render::OverlayStyle::Xor,
			});
		}
		self.overlay = Some(Overlay { items });
	}
}

impl Tool for TypeTool {
	fn pointer(&mut self, ctx: &mut ToolContext<'_>, event: &DocPointer) -> ToolResult {
		match event.kind {
			PointerKind::Down => {
				// A click inside the edited text moves the caret; outside, it
				// commits (Photoshop).
				if let Some(edit) = &self.edit {
					let layout = crate::text::layout_uncached(&edit.content, ctx.doc.ppi);
					let local = inverse(edit.content.transform, (event.x, event.y));
					let inside = match edit.content.frame {
						TextFrame::Box { w, h } => local.0 >= 0.0 && local.1 >= 0.0 && local.0 <= w && local.1 <= h,
						TextFrame::Point => layout.ink_box().is_some_and(|b| contains(b, local, 4.0)),
					};
					if inside {
						let index = layout.index_at(local.0, local.1);
						let edit = self.edit.as_mut().expect("checked above");
						edit.selection = (index, index);
						let text = edit.content.text.clone();
						self.refresh_overlay(ctx);
						return ToolResult {
							text_session: Some(TextSession::Open {
								text,
								selection: (index, index),
							}),
							redraw: true,
							..Default::default()
						};
					}
					return self.finish(ctx, true);
				}
				self.press = Some((event.x, event.y));
				self.drag = None;
				ToolResult::default()
			}
			PointerKind::Move => {
				if self.press.is_some() && event.buttons != 0 {
					self.drag = Some((event.x, event.y));
					if let (Some(a), Some(b)) = (self.press, self.drag) {
						let quad = [(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)];
						let mut points = quad.to_vec();
						points.push(quad[0]);
						self.overlay = Some(Overlay {
							items: vec![OverlayItem::Polyline {
								points,
								closed: true,
								style: fx_render::OverlayStyle::Xor,
							}],
						});
						return ToolResult {
							redraw: true,
							..Default::default()
						};
					}
				}
				ToolResult::default()
			}
			PointerKind::Up => {
				let Some(press) = self.press.take() else {
					return ToolResult::default();
				};
				let end = self.drag.take().unwrap_or((event.x, event.y));
				let slop = 4.0 / ctx.view.zoom.max(1e-6);
				let (w, h) = ((end.0 - press.0).abs(), (end.1 - press.1).abs());
				if w > slop && h > slop {
					let origin = (press.0.min(end.0), press.1.min(end.1));
					self.begin(ctx, origin, TextFrame::Box { w, h })
				} else {
					self.begin(ctx, press, TextFrame::Point)
				}
			}
			_ => ToolResult::default(),
		}
	}

	fn key(&mut self, ctx: &mut ToolContext<'_>, key: &str) -> ToolResult {
		if self.edit.is_none() {
			return ToolResult::default();
		}
		match key {
			"Escape" => self.finish(ctx, false),
			"Enter" => self.finish(ctx, true),
			// Swallow every other viewport key while typing, so Delete does not
			// clear the layer.
			_ => ToolResult {
				redraw: false,
				cursor: Some(CursorShape::Text),
				..Default::default()
			},
		}
	}

	fn text_input(&mut self, ctx: &mut ToolContext<'_>, text: &str, selection: (usize, usize)) -> ToolResult {
		let Some(edit) = &mut self.edit else {
			return ToolResult::default();
		};
		let clamp = |i: usize| {
			let mut i = i.min(text.len());
			while !text.is_char_boundary(i) {
				i -= 1;
			}
			i
		};
		edit.selection = (clamp(selection.0), clamp(selection.1));
		if edit.content.text != text {
			edit.content.text = text.to_owned();
			// FAST: runs are stretched to the new text; a multi-run text loses
			// its interior boundaries' positions.
			if edit.content.runs.len() == 1 {
				edit.content.runs[0].range = (0, text.len());
			}
			edit.content = edit.content.clone().covering();
			self.push_content(ctx);
			return ToolResult {
				doc_changed: true,
				redraw: true,
				..Default::default()
			};
		}
		self.refresh_overlay(ctx);
		ToolResult {
			redraw: true,
			..Default::default()
		}
	}

	fn options_changed(&mut self, ctx: &mut ToolContext<'_>) -> ToolResult {
		let Some(edit) = &mut self.edit else {
			return ToolResult::default();
		};
		// FAST: the option bar formats the whole layer, not the selection.
		let run = run_from_options(ctx);
		let color = edit.content.runs.first().map_or(run.color, |r| r.color);
		edit.content.runs = vec![TextRun {
			range: (0, edit.content.text.len()),
			color,
			..run
		}];
		edit.content.align = align_from_options(ctx);
		edit.content.antialias = antialias_from_options(ctx);
		self.push_content(ctx);
		ToolResult {
			doc_changed: true,
			redraw: true,
			..Default::default()
		}
	}

	fn deactivate(&mut self, ctx: &mut ToolContext<'_>) -> ToolResult {
		self.press = None;
		self.drag = None;
		self.finish(ctx, true)
	}

	fn overlay(&self) -> Option<Overlay> {
		self.overlay.clone()
	}

	fn cursor(&self, _modifiers: Modifiers) -> CursorShape {
		CursorShape::Text
	}
}

/// The run the option bar describes, in the foreground colour.
fn run_from_options(ctx: &ToolContext<'_>) -> TextRun {
	let style = match ctx.settings.string(TOOL, "Style").as_deref() {
		Some("Bold") => FontStyle::Bold,
		Some("Italic") => FontStyle::Italic,
		Some("Bold Italic") => FontStyle::BoldItalic,
		_ => FontStyle::Regular,
	};
	TextRun {
		range: (0, 0),
		family: ctx.settings.string(TOOL, "Font").unwrap_or_else(|| "Arial".into()),
		style,
		size_pt: ctx.settings.number(TOOL, "Size").filter(|s| *s > 0.0).unwrap_or(24.0),
		color: ctx.settings.fg,
		tracking: 0.0,
		leading: None,
	}
}

fn align_from_options(ctx: &ToolContext<'_>) -> TextAlign {
	match ctx.settings.number(TOOL, "Align") {
		Some(1.0) => TextAlign::Center,
		Some(2.0) => TextAlign::Right,
		_ => TextAlign::Left,
	}
}

fn antialias_from_options(ctx: &ToolContext<'_>) -> TextAntialias {
	let label = ctx.settings.string(TOOL, "Anti-alias").unwrap_or_else(|| "Sharp".into());
	TextAntialias::all().into_iter().find(|a| a.label() == label).unwrap_or(TextAntialias::Sharp)
}

/// The topmost visible text layer whose ink contains the document point.
fn hit_text_layer(doc: &Document, at: (f64, f64)) -> Option<(LayerId, TextContent)> {
	let mut hits = Vec::new();
	doc.walk(|layer, _| {
		if layer.visible
			&& matches!(layer.kind, LayerKind::Text { .. })
			&& let Some(content) = layer.kind.text_content()
		{
			hits.push((layer.id, content));
		}
	});
	// FAST: walk order is assumed bottom → top; the last hit wins.
	hits.into_iter().rev().find(|(_, content)| {
		let layout = crate::text::layout_uncached(content, doc.ppi);
		let local = inverse(content.transform, at);
		match content.frame {
			TextFrame::Box { w, h } => local.0 >= 0.0 && local.1 >= 0.0 && local.0 <= w && local.1 <= h,
			TextFrame::Point => layout.ink_box().is_some_and(|b| contains(b, local, 2.0)),
		}
	})
}

fn contains(rect: TextRect, p: (f64, f64), margin: f64) -> bool {
	p.0 >= rect.x - margin && p.1 >= rect.y - margin && p.0 <= rect.x + rect.w + margin && p.1 <= rect.y + rect.h + margin
}

/// Document → frame space through the inverse of the layer matrix.
fn inverse(m: [f64; 6], p: (f64, f64)) -> (f64, f64) {
	let [a, b, c, d, e, f] = m;
	let det = a * d - b * c;
	if det.abs() < 1e-12 {
		return (p.0 - e, p.1 - f);
	}
	let (x, y) = (p.0 - e, p.1 - f);
	((d * x - c * y) / det, (-b * x + a * y) / det)
}
