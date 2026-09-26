//! Source ↔ destination geometry for a resample (M6-T01).
//!
//! Everything the sampler asks is a **destination → source** question: the
//! source point of a destination point, the source bounding box of a
//! destination tile, and the local scale (how many source pixels a destination
//! pixel covers) that picks the source mip level.

use fx_core::Mapping;

use crate::resample::warp::{self, WarpGrid};

/// A mapping prepared for sampling: the warp's triangle mesh (if any) is built
/// once, not once per tile.
pub struct Transform {
	mapping: Mapping,
	grid: Option<WarpGrid>,
	/// `Mapping::Custom`'s displacement field (M11-T06).
	field: Option<std::sync::Arc<fx_core::warp_map::WarpData>>,
}

/// A registered mesh as warp triangles whose `uv` are source points.
fn mesh_grid(mesh: &fx_core::warp_map::TriMesh) -> WarpGrid {
	let triangles = mesh
		.tris
		.iter()
		.map(|t| {
			let [a, b, c] = t.map(|i| i as usize);
			warp::Triangle {
				dst: [mesh.dst[a], mesh.dst[b], mesh.dst[c]],
				uv: [mesh.src[a], mesh.src[b], mesh.src[c]],
			}
		})
		.collect();
	WarpGrid { triangles }
}

/// `source_uv` with this rectangle is the identity: mesh `uv` are points.
const UNIT: [f64; 4] = [0.0, 0.0, 1.0, 1.0];

impl Transform {
	/// Prepare `mapping`; builds the warp mesh for [`Mapping::Warp`].
	pub fn new(mapping: Mapping) -> Self {
		let mut field = None;
		let grid = match &mapping {
			Mapping::Warp(patch) => Some(WarpGrid::build(patch, warp::DEFAULT_SEGMENTS)),
			// FAST: an id that left the registry maps like an empty mesh.
			Mapping::Custom { id, .. } => match fx_core::warp_map::get(*id) {
				Some(data) => match data.as_ref() {
					fx_core::warp_map::WarpData::Mesh(mesh) => Some(mesh_grid(mesh)),
					fx_core::warp_map::WarpData::Field(_) => {
						field = Some(data.clone());
						None
					}
				},
				None => Some(WarpGrid { triangles: Vec::new() }),
			},
			_ => None,
		};
		Self { mapping, grid, field }
	}

	/// Whether this is a warp (the sampler then uses per-tile triangle lists).
	pub fn is_warp(&self) -> bool {
		self.grid.is_some()
	}

	/// An exact whole-pixel translation, if this mapping is one: the sampler
	/// then copies pixels instead of resampling them.
	pub fn integer_translation(&self) -> Option<(i32, i32)> {
		self.mapping.integer_translation()
	}

	/// The triangles of a warp whose destination box touches `rect`, or `None`
	/// for an affine/projective mapping.
	pub fn candidates(&self, rect: [f64; 4]) -> Option<Vec<usize>> {
		if let Mapping::Custom { dst, .. } = self.mapping {
			let r = [rect[0] - dst[0], rect[1] - dst[1], rect[2] - dst[0], rect[3] - dst[1]];
			return self.grid.as_ref().map(|grid| grid.intersecting(r));
		}
		self.grid.as_ref().map(|grid| grid.intersecting(rect))
	}

	/// The source point of a destination point. `None` when the mapping is
	/// degenerate or the point is outside a warp's surface. `candidates` (from
	/// [`Transform::candidates`]) restricts the warp lookup to one tile; pass
	/// `None` to search the whole mesh.
	pub fn inverse_point(&self, candidates: Option<&[usize]>, dst: (f64, f64)) -> Option<(f64, f64)> {
		match &self.mapping {
			Mapping::Affine(m) => affine_inverse(m, dst),
			Mapping::Projective(m) => projective_inverse(m, dst),
			Mapping::Warp(patch) => {
				let grid = self.grid.as_ref().expect("a warp mapping has a mesh");
				let uv = match candidates {
					Some(list) => grid.parameter_at(list, dst)?,
					None => {
						let all: Vec<usize> = (0..grid.triangles.len()).collect();
						grid.parameter_at(&all, dst)?
					}
				};
				Some(warp::source_uv(&patch.src_rect, uv))
			}
			Mapping::Custom { src, dst: shift, .. } => {
				let d = (dst.0 - shift[0], dst.1 - shift[1]);
				let p = self.custom_inverse(candidates, d)?;
				Some((p.0 - src[0], p.1 - src[1]))
			}
		}
	}

	/// `G⁻¹` of the registered geometry at `d` (destination minus `dst`).
	fn custom_inverse(&self, candidates: Option<&[usize]>, d: (f64, f64)) -> Option<(f64, f64)> {
		if let Some(data) = &self.field
			&& let fx_core::warp_map::WarpData::Field(field) = data.as_ref()
		{
			let v = field.at(d.0, d.1);
			return Some((d.0 + v[0], d.1 + v[1]));
		}
		let grid = self.grid.as_ref()?;
		let uv = match candidates {
			Some(list) => grid.parameter_at(list, d)?,
			None => {
				let all: Vec<usize> = (0..grid.triangles.len()).collect();
				grid.parameter_at(&all, d)?
			}
		};
		Some(warp::source_uv(&UNIT, uv))
	}

	/// The bounding box in level-0 source pixels of a destination rectangle, or
	/// `None` when nothing of the rectangle is covered.
	pub fn source_bounds(&self, dst_rect: [f64; 4]) -> Option<[f64; 4]> {
		match &self.mapping {
			Mapping::Affine(_) | Mapping::Projective(_) => {
				let corners = [
					(dst_rect[0], dst_rect[1]),
					(dst_rect[2], dst_rect[1]),
					(dst_rect[2], dst_rect[3]),
					(dst_rect[0], dst_rect[3]),
				];
				let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
				for corner in corners {
					let p = self.inverse_point(None, corner)?;
					if !p.0.is_finite() || !p.1.is_finite() {
						return None;
					}
					bbox[0] = bbox[0].min(p.0);
					bbox[1] = bbox[1].min(p.1);
					bbox[2] = bbox[2].max(p.0);
					bbox[3] = bbox[3].max(p.1);
				}
				Some(bbox)
			}
			Mapping::Custom { src, dst, .. } => {
				let r = [dst_rect[0] - dst[0], dst_rect[1] - dst[1], dst_rect[2] - dst[0], dst_rect[3] - dst[1]];
				if let Some(data) = &self.field
					&& let fx_core::warp_map::WarpData::Field(field) = data.as_ref()
				{
					// FAST: the field's global maximum bounds every rectangle.
					let m = field.max().ceil() + 1.0;
					return Some([r[0] - m - src[0], r[1] - m - src[1], r[2] + m - src[0], r[3] + m - src[1]]);
				}
				let grid = self.grid.as_ref()?;
				let candidates = grid.intersecting(r);
				if candidates.is_empty() {
					return None;
				}
				let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
				for &i in &candidates {
					let b = grid.triangles[i].source_bbox(&UNIT);
					bbox = [bbox[0].min(b[0]), bbox[1].min(b[1]), bbox[2].max(b[2]), bbox[3].max(b[3])];
				}
				Some([bbox[0] - src[0], bbox[1] - src[1], bbox[2] - src[0], bbox[3] - src[1]])
			}
			Mapping::Warp(patch) => {
				let grid = self.grid.as_ref()?;
				let candidates = grid.intersecting(dst_rect);
				if candidates.is_empty() {
					return None;
				}
				let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
				for &i in &candidates {
					let b = grid.triangles[i].source_bbox(&patch.src_rect);
					bbox[0] = bbox[0].min(b[0]);
					bbox[1] = bbox[1].min(b[1]);
					bbox[2] = bbox[2].max(b[2]);
					bbox[3] = bbox[3].max(b[3]);
				}
				Some(bbox)
			}
		}
	}

	/// The largest singular value of the destination → source Jacobian at
	/// `dst`: source pixels per destination pixel along the worst direction
	/// (SNIPPETS §10). `> 2` means a reduction that needs a mip level.
	pub fn scale_at(&self, dst: (f64, f64)) -> f64 {
		if let Mapping::Affine(m) = &self.mapping
			&& let Some(j) = affine_inverse_jacobian(m)
		{
			return sigma_max(j[0], j[1], j[2], j[3]);
		}
		// Projective and warp: a central difference of the inverse mapping.
		const H: f64 = 0.5;
		let (Some(p), Some(px), Some(py)) = (
			self.inverse_point(None, dst),
			self.inverse_point(None, (dst.0 + H, dst.1)),
			self.inverse_point(None, (dst.0, dst.1 + H)),
		) else {
			return 1.0;
		};
		let j = [(px.0 - p.0) / H, (py.0 - p.0) / H, (px.1 - p.1) / H, (py.1 - p.1) / H];
		let scale = sigma_max(j[0], j[1], j[2], j[3]);
		if scale.is_finite() { scale } else { 1.0 }
	}
}

/// The largest singular value of the 2 × 2 matrix `[[p, q], [r, s]]`.
fn sigma_max(p: f64, q: f64, r: f64, s: f64) -> f64 {
	let energy = p * p + q * q + r * r + s * s;
	let det = p * s - q * r;
	let disc = (energy * energy - 4.0 * det * det).max(0.0);
	((energy + disc.sqrt()) / 2.0).max(0.0).sqrt()
}

/// The closed-form inverse of an affine mapping at a point.
fn affine_inverse(m: &[f64; 6], dst: (f64, f64)) -> Option<(f64, f64)> {
	let [a, b, c, d, e, f] = *m;
	let det = a * d - b * c;
	if !det.is_finite() || det.abs() < 1e-12 {
		return None;
	}
	let (x, y) = (dst.0 - e, dst.1 - f);
	Some(((d * x - c * y) / det, (-b * x + a * y) / det))
}

/// The Jacobian of [`affine_inverse`] as `[p, q, r, s]`.
fn affine_inverse_jacobian(m: &[f64; 6]) -> Option<[f64; 4]> {
	let [a, b, c, d, _e, _f] = *m;
	let det = a * d - b * c;
	if !det.is_finite() || det.abs() < 1e-12 {
		return None;
	}
	Some([d / det, -c / det, -b / det, a / det])
}

/// The adjugate of a row-major 3 × 3 matrix.
fn adjugate3(m: &[f64; 9]) -> [f64; 9] {
	[
		m[4] * m[8] - m[5] * m[7],
		m[2] * m[7] - m[1] * m[8],
		m[1] * m[5] - m[2] * m[4],
		m[5] * m[6] - m[3] * m[8],
		m[0] * m[8] - m[2] * m[6],
		m[2] * m[3] - m[0] * m[5],
		m[3] * m[7] - m[4] * m[6],
		m[1] * m[6] - m[0] * m[7],
		m[0] * m[4] - m[1] * m[3],
	]
}

/// The closed-form inverse of a homography at a point.
fn projective_inverse(m: &[f64; 9], dst: (f64, f64)) -> Option<(f64, f64)> {
	let adj = adjugate3(m);
	let x = adj[0] * dst.0 + adj[1] * dst.1 + adj[2];
	let y = adj[3] * dst.0 + adj[4] * dst.1 + adj[5];
	let w = adj[6] * dst.0 + adj[7] * dst.1 + adj[8];
	if !w.is_finite() || w.abs() < 1e-12 {
		return None;
	}
	Some((x / w, y / w))
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_core::BezierPatch;

	/// Apply an affine mapping forwards (`source → destination`).
	fn affine_forward(m: &Mapping, src: (f64, f64)) -> (f64, f64) {
		let Mapping::Affine([a, b, c, d, e, f]) = m else { panic!("not affine") };
		(a * src.0 + c * src.1 + e, b * src.0 + d * src.1 + f)
	}

	#[test]
	fn affine_inverse_round_trips() {
		let m = Mapping::affine(2.0, 0.3, -0.4, 1.5, 10.0, -20.0);
		let t = Transform::new(m);
		for src in [(0.0, 0.0), (37.5, -12.25), (1000.0, 2000.0)] {
			let back = t.inverse_point(None, affine_forward(&m, src)).unwrap();
			assert!((back.0 - src.0).abs() < 1e-9 && (back.1 - src.1).abs() < 1e-9, "{src:?} -> {back:?}");
		}
	}

	#[test]
	fn affine_scale_is_constant() {
		// 2× reduction: two source pixels per destination pixel.
		let t = Transform::new(Mapping::scale(0.5, 0.5));
		assert!((t.scale_at((0.0, 0.0)) - 2.0).abs() < 1e-9);
		assert!((t.scale_at((500.0, 500.0)) - 2.0).abs() < 1e-9);
		// 4× reduction.
		assert!((Transform::new(Mapping::scale(0.25, 0.25)).scale_at((3.0, 7.0)) - 4.0).abs() < 1e-9);
		// Magnification.
		assert!((Transform::new(Mapping::scale(3.0, 3.0)).scale_at((0.0, 0.0)) - 1.0 / 3.0).abs() < 1e-9);
	}

	#[test]
	fn an_anisotropic_scale_uses_the_worst_direction() {
		// x reduced 4×, y unchanged: the mip choice must follow x.
		let t = Transform::new(Mapping::scale(0.25, 1.0));
		assert!((t.scale_at((0.0, 0.0)) - 4.0).abs() < 1e-9);
	}

	#[test]
	fn projection_inverts() {
		let m = Mapping::Projective([1.0, 0.1, 5.0, 0.05, 1.2, -3.0, 0.0002, 0.0001, 1.0]);
		let t = Transform::new(m);
		let Mapping::Projective(p) = m else { unreachable!() };
		for src in [(0.0, 0.0), (100.0, 50.0), (-20.0, 300.0)] {
			let w = p[6] * src.0 + p[7] * src.1 + p[8];
			let dst = ((p[0] * src.0 + p[1] * src.1 + p[2]) / w, (p[3] * src.0 + p[4] * src.1 + p[5]) / w);
			let back = t.inverse_point(None, dst).unwrap();
			assert!((back.0 - src.0).abs() < 1e-6 && (back.1 - src.1).abs() < 1e-6, "{src:?} -> {back:?}");
		}
	}

	#[test]
	fn source_bounds_of_a_translation_is_the_rect_moved_back() {
		let t = Transform::new(Mapping::translation(10.0, -4.0));
		let b = t.source_bounds([10.0, -4.0, 110.0, 96.0]).unwrap();
		for (got, want) in b.iter().zip([0.0, 0.0, 100.0, 100.0]) {
			assert!((got - want).abs() < 1e-9, "{b:?}");
		}
	}

	#[test]
	fn a_warp_covers_its_patch_and_nothing_outside() {
		let patch = BezierPatch::identity(100, 100);
		let t = Transform::new(Mapping::Warp(patch));
		assert!(t.inverse_point(None, (50.0, 50.0)).is_some());
		assert!(t.inverse_point(None, (-1.0, 50.0)).is_none());
		let inside = t.source_bounds([0.0, 0.0, 100.0, 100.0]).unwrap();
		assert!(inside[0].abs() < 1e-6 && (inside[2] - 100.0).abs() < 1e-6, "{inside:?}");
		assert!(t.source_bounds([200.0, 200.0, 300.0, 300.0]).is_none());
	}
}
