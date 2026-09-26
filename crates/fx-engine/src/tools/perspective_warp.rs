//! Edit ▸ Perspective Warp (M11-T08) as a warp session.
//!
//! Layout: drag to draw a quad on a plane of the image, drag its corners
//! onto the plane; a corner dropped near another quad's corner snaps to it
//! (the planes then share it). Warp: drag the corners; each quad maps its
//! layout shape to its warped shape by a homography, subdivided into one
//! triangle mesh registered as a `Mapping::Custom`. Straighten makes the
//! warped edges that are nearly vertical / horizontal exactly so.
//!
//! FAST: shared edges can crack slightly (each quad's homography is its
//! own); no Shift+click on one edge; quads cannot be deleted one by one
//! (Escape cancels the whole session).

use fx_core::Mapping;
use fx_core::warp_map::{TriMesh, WarpData, register};
use fx_render::{OverlayItem, OverlayStyle};

use crate::tools::DocPointer;
use crate::tools::transform::{CustomWarp, Update};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, PointerKind};

/// Screen pixels a press may miss a corner by.
const PICK_PX: f64 = 8.0;
/// Subdivisions per quad side of the mesh.
const SEGMENTS: usize = 16;
/// Edges within this many degrees of the axis are straightened.
const STRAIGHTEN_DEG: f64 = 20.0;

type P = [f64; 2];

#[derive(Clone, Debug, Default)]
pub struct PerspectiveWarp {
	/// Layout corners (TL, TR, BR, BL) and warped corners per quad.
	quads: Vec<([P; 4], [P; 4])>,
	warp: bool,
	/// The corner being dragged: (quad, corner), or a new quad's first corner.
	drag: Option<(usize, usize)>,
	drawing: Option<P>,
	hover: Option<P>,
	zoom: f64,
	id: Option<u64>,
}

fn homography(quad: [P; 4]) -> Option<Mapping> {
	Mapping::from_quad([0.0, 0.0, 1.0, 1.0], quad.map(|p| (p[0], p[1])))
}

impl PerspectiveWarp {
	fn publish(&mut self) {
		let moved = self.quads.iter().any(|(a, b)| a != b);
		if !moved {
			self.id = None;
			return;
		}
		let mut mesh = TriMesh::default();
		for (layout, warped) in &self.quads {
			let (Some(hs), Some(hd)) = (homography(*layout), homography(*warped)) else {
				continue;
			};
			let base = mesh.src.len() as u32;
			for j in 0..=SEGMENTS {
				for i in 0..=SEGMENTS {
					let (u, v) = (i as f64 / SEGMENTS as f64, j as f64 / SEGMENTS as f64);
					let s = hs.forward_point(u, v).unwrap_or((0.0, 0.0));
					let d = hd.forward_point(u, v).unwrap_or((0.0, 0.0));
					mesh.src.push([s.0, s.1]);
					mesh.dst.push([d.0, d.1]);
				}
			}
			let n = (SEGMENTS + 1) as u32;
			for j in 0..SEGMENTS as u32 {
				for i in 0..SEGMENTS as u32 {
					let a = base + j * n + i;
					mesh.tris.push([a, a + 1, a + n + 1]);
					mesh.tris.push([a, a + n + 1, a + n]);
				}
			}
		}
		self.id = (!mesh.tris.is_empty()).then(|| register(WarpData::Mesh(mesh)));
	}

	fn corner_at(&self, at: P) -> Option<(usize, usize)> {
		let reach = PICK_PX / self.zoom.max(1e-6);
		let mut best = None;
		for (q, (layout, warped)) in self.quads.iter().enumerate() {
			let corners = if self.warp { warped } else { layout };
			for (c, p) in corners.iter().enumerate() {
				let d = (p[0] - at[0]).hypot(p[1] - at[1]);
				if d <= reach && best.is_none_or(|(_, _, bd)| d < bd) {
					best = Some((q, c, d));
				}
			}
		}
		best.map(|(q, c, _)| (q, c))
	}

	/// Every (quad, corner) at the same place as `(q, c)` (shared corners).
	fn linked(&self, q: usize, c: usize) -> Vec<(usize, usize)> {
		let p = if self.warp { self.quads[q].1[c] } else { self.quads[q].0[c] };
		let mut out = Vec::new();
		for (qi, (layout, warped)) in self.quads.iter().enumerate() {
			let corners = if self.warp { warped } else { layout };
			for (ci, o) in corners.iter().enumerate() {
				if (o[0] - p[0]).abs() < 1e-6 && (o[1] - p[1]).abs() < 1e-6 {
					out.push((qi, ci));
				}
			}
		}
		out
	}

	fn straighten(&mut self) {
		let tan = STRAIGHTEN_DEG.to_radians().tan();
		for (_, warped) in &mut self.quads {
			for e in 0..4 {
				let (a, b) = (warped[e], warped[(e + 1) % 4]);
				let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
				if dx.abs() <= dy.abs() * tan {
					let x = (a[0] + b[0]) / 2.0;
					warped[e][0] = x;
					warped[(e + 1) % 4][0] = x;
				} else if dy.abs() <= dx.abs() * tan {
					let y = (a[1] + b[1]) / 2.0;
					warped[e][1] = y;
					warped[(e + 1) % 4][1] = y;
				}
			}
		}
		self.publish();
	}
}

impl CustomWarp for PerspectiveWarp {
	fn clone_box(&self) -> Box<dyn CustomWarp> {
		Box::new(self.clone())
	}

	fn bar(&self) -> &'static str {
		"_warp-perspective"
	}

	fn mapping(&self) -> Option<Mapping> {
		self.id.map(|id| Mapping::Custom {
			id,
			src: [0.0; 2],
			dst: [0.0; 2],
		})
	}

	fn pointer(&mut self, event: &DocPointer, zoom: f64) -> Update {
		self.zoom = zoom;
		let at = [event.x, event.y];
		self.hover = Some(at);
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				if let Some(hit) = self.corner_at(at) {
					self.drag = Some(hit);
				} else if !self.warp {
					self.drawing = Some(at);
				}
				Update::Redraw
			}
			PointerKind::Move if event.buttons & BUTTON_LEFT != 0 => {
				if let Some((q, c)) = self.drag {
					let group = self.linked(q, c);
					for (qi, ci) in group {
						if self.warp {
							self.quads[qi].1[ci] = at;
						} else {
							self.quads[qi].0[ci] = at;
							self.quads[qi].1[ci] = at;
						}
					}
					if self.warp {
						self.publish();
						return Update::Changed { dragging: true };
					}
				}
				Update::Redraw
			}
			PointerKind::Up => {
				if let Some(start) = self.drawing.take()
					&& (at[0] - start[0]).abs() > 4.0 / zoom
					&& (at[1] - start[1]).abs() > 4.0 / zoom
				{
					let (x0, y0, x1, y1) = (start[0].min(at[0]), start[1].min(at[1]), start[0].max(at[0]), start[1].max(at[1]));
					let quad = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
					self.quads.push((quad, quad));
				}
				// A layout corner dropped near another quad's corner snaps to it.
				if let Some((q, c)) = self.drag.take()
					&& !self.warp
				{
					let p = self.quads[q].0[c];
					let reach = 2.0 * PICK_PX / zoom.max(1e-6);
					let target = self
						.quads
						.iter()
						.enumerate()
						.filter(|(qi, _)| *qi != q)
						.flat_map(|(_, (l, _))| l.iter().copied())
						.find(|o| (o[0] - p[0]).hypot(o[1] - p[1]) <= reach);
					if let Some(t) = target {
						self.quads[q].0[c] = t;
						self.quads[q].1[c] = t;
					}
				}
				Update::Changed { dragging: false }
			}
			_ => Update::Redraw,
		}
	}

	fn set_option(&mut self, key: &str, value: &serde_json::Value) -> Update {
		match key {
			"Phase" => {
				self.warp = value.as_f64() == Some(1.0);
				Update::Redraw
			}
			"_straighten" => {
				self.straighten();
				Update::Changed { dragging: false }
			}
			_ => Update::None,
		}
	}

	fn overlay(&self) -> Vec<OverlayItem> {
		let mut items = Vec::new();
		let style = OverlayStyle::Solid([0.2, 0.6, 1.0, 1.0]);
		for (layout, warped) in &self.quads {
			let corners = if self.warp { warped } else { layout };
			items.push(OverlayItem::Polyline {
				points: corners.iter().map(|p| (p[0], p[1])).collect(),
				closed: true,
				style,
			});
			for p in corners {
				items.push(OverlayItem::Handle {
					at: (p[0], p[1]),
					size_px: 8.0,
				});
			}
		}
		if let (Some(start), Some(h)) = (self.drawing, self.hover) {
			items.push(OverlayItem::Polyline {
				points: vec![(start[0], start[1]), (h[0], start[1]), (h[0], h[1]), (start[0], h[1])],
				closed: true,
				style: OverlayStyle::Xor,
			});
		}
		items
	}

	fn status(&self) -> String {
		if self.warp {
			"Perspective Warp · Warp: drag the corners · Straighten · Enter applies".into()
		} else {
			format!(
				"Perspective Warp · Layout: drag to draw a plane ({} so far), then switch to Warp",
				self.quads.len()
			)
		}
	}

	fn cursor(&self) -> CursorShape {
		CursorShape::Crosshair
	}
}
