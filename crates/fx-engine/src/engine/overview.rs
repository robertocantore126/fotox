//! The document overview the Navigator, Histogram and Properties panels read
//! (`overview:request`): a small RGBA picture of the composite, its
//! histogram, and the active layer's content box. One background job per
//! request; the UI asks again after an edit (debounced) and keeps the newest.

use fx_core::pixels::{Content, Placed, content_bounds};
use fx_core::{Document, LayerId, LayerKind};
use fx_protocol::EngineToUi;
use fx_tiles::TileStore;
use serde_json::Value;

use super::{Engine, Internal};
use crate::ai;

impl Engine {
	pub(super) fn overview_action(&mut self, id: &str, args: &Value) -> bool {
		if id == "text:style" {
			self.text_style(args);
			return true;
		}
		if id != "overview:request" {
			return false;
		}
		let Some(doc_id) = self.docs.active_id() else { return true };
		let Some(open) = self.docs.get(doc_id) else { return true };
		let request = args.get("request").and_then(Value::as_u64).unwrap_or(0);
		let size = args.get("size").and_then(Value::as_u64).unwrap_or(256).clamp(32, 1024) as u32;
		let layer = args.get("layer").and_then(Value::as_u64).map(LayerId).filter(|l| open.doc.layer(*l).is_some());
		let bounds_only = args.get("bounds_only").and_then(Value::as_bool).unwrap_or(false);
		let text = layer.and_then(|l| text_summary(&open.doc, l));
		let mut doc = open.doc.clone();
		let (store, internal) = (self.store.clone(), self.internal.clone());
		rayon::spawn(move || {
			let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| overview(&mut doc, &store, size, layer, bounds_only)));
			match result {
				Ok(Ok((picture, histogram, bounds))) => {
					let header = EngineToUi::Overview {
						doc: doc_id,
						request,
						width: picture.1,
						height: picture.2,
						doc_width: doc.width,
						doc_height: doc.height,
						histogram,
						layer,
						bounds,
						text,
					};
					let _ = internal.send(Internal::Overview(Box::new((header, picture.0))));
				}
				Ok(Err(error)) => tracing::debug!("overview failed: {error}"),
				Err(_) => tracing::warn!("overview panicked"),
			}
		});
		true
	}
}

type Picture = (Vec<u8>, u32, u32);

fn overview(doc: &mut Document, store: &TileStore, size: u32, layer: Option<LayerId>, bounds_only: bool) -> Result<(Picture, Vec<Vec<u32>>, Option<(i32, i32, i32, i32)>), String> {
	let bounds = layer.and_then(|id| layer_bounds(doc, store, id));
	if bounds_only {
		return Ok(((Vec::new(), 0, 0), Vec::new(), bounds));
	}
	let work = ai::working_composite(doc, store, size)?;
	let mut rgba = Vec::with_capacity(work.pixels.len() * 4);
	// R, G, B, luminosity; transparent pixels are not counted.
	let mut histogram = vec![vec![0u32; 256]; 4];
	for p in &work.pixels {
		let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
		let (r, g, b, a) = (c(p[0]), c(p[1]), c(p[2]), c(p[3]));
		rgba.extend_from_slice(&[r, g, b, a]);
		if a > 0 {
			histogram[0][r as usize] += 1;
			histogram[1][g as usize] += 1;
			histogram[2][b as usize] += 1;
			let y = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)).round() as usize;
			histogram[3][y.min(255)] += 1;
		}
	}
	Ok(((rgba, work.width as u32, work.height as u32), histogram, bounds))
}

/// The canvas box of what `id` shows (x0, y0, x1, y1), or `None` when it
/// shows nothing (or is an adjustment).
pub(super) fn layer_bounds(doc: &Document, store: &TileStore, id: LayerId) -> Option<(i32, i32, i32, i32)> {
	let layer = doc.layer(id)?;
	let drawn;
	let placed = match &layer.kind {
		LayerKind::Pixel { image, offset } => Placed { image, offset: *offset },
		LayerKind::Adjustment(_) => return None,
		LayerKind::Shape { .. } | LayerKind::Text { .. } => {
			drawn = crate::derived::layer_content(doc, store, id).ok()?;
			Placed { image: &drawn, offset: (0, 0) }
		}
		_ => {
			drawn = crate::export::composite_layers(doc, &[id], None, store, None).ok()?;
			Placed { image: &drawn, offset: (0, 0) }
		}
	};
	content_bounds(placed, Content::Opaque, store).ok().flatten()
}

/// The Character / Paragraph panels' view of a text layer: its first run and
/// its alignment.
fn text_summary(doc: &Document, id: LayerId) -> Option<Value> {
	let content = doc.layer(id)?.kind.text_content()?;
	let run = content
		.runs
		.first()
		.cloned()
		.unwrap_or_else(|| fx_core::text::TextContent::default_run(&content.text));
	Some(serde_json::json!({
		"family": run.family,
		"style": run.style,
		"size_pt": run.size_pt,
		"color": run.color,
		"tracking": run.tracking,
		"leading": run.leading,
		"align": content.align,
		"runs": content.runs.len(),
	}))
}

impl Engine {
	/// `text:style`: the Character / Paragraph panels change the active text
	/// layer. Every run gets the values given (`family`, `style`, `size_pt`,
	/// `color`, `tracking`, `leading` — null = auto), and `align` the paragraph.
	fn text_style(&mut self, args: &Value) {
		let Some(doc_id) = self.docs.active_id() else { return };
		let Some(open) = self.docs.get(doc_id) else { return };
		let Some(mut content) = open.doc.active_layer().and_then(|l| open.doc.layer(l)).and_then(|l| l.kind.text_content()) else {
			self.to_ui(&EngineToUi::Toast {
				text: "Select a text layer first".into(),
			});
			return;
		};
		let (w, h) = (open.doc.width, open.doc.height);
		if content.runs.is_empty() {
			content.runs.push(fx_core::text::TextContent::default_run(&content.text));
		}
		let family = args.get("family").and_then(Value::as_str).map(str::to_owned);
		let style = args.get("style").cloned().and_then(|v| serde_json::from_value::<fx_core::text::FontStyle>(v).ok());
		let size = args.get("size_pt").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0);
		let color = args.get("color").cloned().and_then(|v| serde_json::from_value::<[u16; 4]>(v).ok());
		let tracking = args.get("tracking").and_then(Value::as_f64).filter(|v| v.is_finite());
		let leading = args.get("leading").map(|v| v.as_f64().filter(|l| l.is_finite() && *l > 0.0));
		for run in &mut content.runs {
			if let Some(f) = &family {
				run.family.clone_from(f);
			}
			if let Some(s) = style {
				run.style = s;
			}
			if let Some(s) = size {
				run.size_pt = s.min(1296.0);
			}
			if let Some(c) = color {
				run.color = c;
			}
			if let Some(t) = tracking {
				run.tracking = t;
			}
			if let Some(l) = leading {
				run.leading = l;
			}
		}
		if let Some(align) = args.get("align").cloned().and_then(|v| serde_json::from_value::<fx_core::text::TextAlign>(v).ok()) {
			content.align = align;
		}
		self.command(
			doc_id,
			fx_core::Command::SetText {
				layer: fx_core::LayerRef::Active,
				content,
				dirty: [0.0, 0.0, f64::from(w), f64::from(h)],
			},
		);
	}
}
