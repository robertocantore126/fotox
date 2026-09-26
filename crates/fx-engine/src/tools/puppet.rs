//! Edit ▸ Puppet Warp (M11-T07) as a warp session: a mesh over the layer's
//! content (`fx_ops::puppet`), pins added by a click, dragged to deform it.
//! Every change registers the deformed mesh as a `Mapping::Custom`.
//!
//! FAST: Alt+click deletes a pin (no Alt-rotate, no Pin Depth); Density
//! and Expansion rebuild the mesh only when the session starts.

use fx_core::Mapping;
use fx_core::warp_map::{TriMesh, WarpData, register};
use fx_ops::puppet::{Mode, mls};
use fx_render::{OverlayItem, OverlayStyle};

use crate::tools::DocPointer;
use crate::tools::transform::{CustomWarp, Update};
use crate::view::BUTTON_LEFT;
use crate::{CursorShape, PointerKind};

/// A press within this many screen pixels of a pin takes it.
const PICK_PX: f64 = 8.0;

#[derive(Clone, Debug)]
pub struct Puppet {
	mesh: TriMesh,
	pins: Vec<([f64; 2], [f64; 2])>,
	mode: Mode,
	show_mesh: bool,
	drag: Option<usize>,
	id: Option<u64>,
}

impl Puppet {
	pub fn new(mesh: TriMesh) -> Self {
		Self {
			mesh,
			pins: Vec::new(),
			mode: Mode::Normal,
			show_mesh: true,
			drag: None,
			id: None,
		}
	}

	fn solve(&mut self) {
		let from: Vec<[f64; 2]> = self.pins.iter().map(|p| p.0).collect();
		let to: Vec<[f64; 2]> = self.pins.iter().map(|p| p.1).collect();
		let moved = self.pins.iter().any(|(a, b)| a != b);
		for (d, s) in self.mesh.dst.iter_mut().zip(&self.mesh.src) {
			*d = mls(*s, &from, &to, self.mode);
		}
		self.id = moved.then(|| register(WarpData::Mesh(self.mesh.clone())));
	}
}

impl CustomWarp for Puppet {
	fn clone_box(&self) -> Box<dyn CustomWarp> {
		Box::new(self.clone())
	}

	fn bar(&self) -> &'static str {
		"_warp-puppet"
	}

	fn mapping(&self) -> Option<Mapping> {
		self.id.map(|id| Mapping::Custom {
			id,
			src: [0.0; 2],
			dst: [0.0; 2],
		})
	}

	fn pointer(&mut self, event: &DocPointer, zoom: f64) -> Update {
		let at = [event.x, event.y];
		match event.kind {
			PointerKind::Down if event.buttons & BUTTON_LEFT != 0 => {
				let reach = PICK_PX / zoom;
				let hit = self.pins.iter().position(|(_, q)| (q[0] - at[0]).hypot(q[1] - at[1]) <= reach);
				match hit {
					Some(i) if event.modifiers.alt => {
						self.pins.remove(i);
						self.solve();
						Update::Changed { dragging: false }
					}
					Some(i) => {
						self.drag = Some(i);
						Update::Redraw
					}
					None => {
						// A new pin where the mesh is now: its source is found by
						// inverting the current deformation (FAST: nearest vertex).
						let Some(k) = (0..self.mesh.dst.len()).min_by(|a, b| {
							let da = (self.mesh.dst[*a][0] - at[0]).hypot(self.mesh.dst[*a][1] - at[1]);
							let db = (self.mesh.dst[*b][0] - at[0]).hypot(self.mesh.dst[*b][1] - at[1]);
							da.total_cmp(&db)
						}) else {
							return Update::None;
						};
						let off = [at[0] - self.mesh.dst[k][0], at[1] - self.mesh.dst[k][1]];
						let src = [self.mesh.src[k][0] + off[0], self.mesh.src[k][1] + off[1]];
						self.pins.push((src, at));
						self.drag = Some(self.pins.len() - 1);
						Update::Redraw
					}
				}
			}
			PointerKind::Move if event.buttons & BUTTON_LEFT != 0 => match self.drag {
				Some(i) => {
					self.pins[i].1 = at;
					self.solve();
					Update::Changed { dragging: true }
				}
				None => Update::None,
			},
			PointerKind::Up => {
				self.drag = None;
				Update::Changed { dragging: false }
			}
			_ => Update::None,
		}
	}

	fn set_option(&mut self, key: &str, value: &serde_json::Value) -> Update {
		match key {
			"Mode" => {
				self.mode = match value.as_str() {
					Some("Rigid") => Mode::Rigid,
					Some("Distort") => Mode::Distort,
					_ => Mode::Normal,
				};
				self.solve();
				Update::Changed { dragging: false }
			}
			"Show Mesh" => {
				self.show_mesh = value.as_bool().unwrap_or(true);
				Update::Redraw
			}
			_ => Update::None,
		}
	}

	fn overlay(&self) -> Vec<OverlayItem> {
		let mut items = Vec::new();
		if self.show_mesh {
			let grey = OverlayStyle::Solid([0.5, 0.5, 0.5, 0.7]);
			for t in &self.mesh.tris {
				let p = t.map(|i| {
					let v = self.mesh.dst[i as usize];
					(v[0], v[1])
				});
				items.push(OverlayItem::Polyline {
					points: p.to_vec(),
					closed: true,
					style: grey,
				});
			}
		}
		for (_, q) in &self.pins {
			items.push(OverlayItem::Handle {
				at: (q[0], q[1]),
				size_px: 9.0,
			});
		}
		items
	}

	fn status(&self) -> String {
		format!(
			"Puppet Warp: {} pins · click adds, drag moves, Alt+click removes · Enter applies",
			self.pins.len()
		)
	}

	fn cursor(&self) -> CursorShape {
		CursorShape::Crosshair
	}
}
