//! The Warp surface (M6-T01, D-053): a 4 × 4 bicubic Bézier patch
//! forward-mapped into a triangle mesh.
//!
//! A Bézier patch cannot be inverted in closed form, so the patch is evaluated
//! on a `segments × segments` grid of parameter space; each quad becomes two
//! triangles that carry their `(u, v)` at the corners. A destination point's
//! barycentric coordinates inside its triangle give `(u, v)`, hence the source
//! point. Where the surface folds over itself the **last** triangle in mesh
//! order wins — Photoshop shows fold-overs the same way.

use fx_core::BezierPatch;

/// Grid resolution of the triangle mesh (Photoshop's default warp grid is a
/// 4 × 4 patch; the mesh is only the renderer's approximation of it).
pub const DEFAULT_SEGMENTS: u32 = 64;

/// One triangle of the mesh: its three destination corners and the parameter
/// `(u, v)` each of them came from.
#[derive(Clone, Copy, Debug)]
pub struct Triangle {
	pub dst: [[f64; 2]; 3],
	pub uv: [[f64; 2]; 3],
}

impl Triangle {
	/// The destination bounding box `[x0, y0, x1, y1]`.
	pub fn bbox(&self) -> [f64; 4] {
		let xs = [self.dst[0][0], self.dst[1][0], self.dst[2][0]];
		let ys = [self.dst[0][1], self.dst[1][1], self.dst[2][1]];
		[
			xs.iter().copied().fold(f64::INFINITY, f64::min),
			ys.iter().copied().fold(f64::INFINITY, f64::min),
			xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
			ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
		]
	}

	/// Barycentric weights `[a, b, c]` of `p` (outside → `None`), where `p = a·A
	/// + b·B + c·C`.
	pub fn barycentric(&self, p: (f64, f64)) -> Option<[f64; 3]> {
		let (ax, ay) = (self.dst[0][0], self.dst[0][1]);
		let (bx, by) = (self.dst[1][0], self.dst[1][1]);
		let (cx, cy) = (self.dst[2][0], self.dst[2][1]);
		let (v0x, v0y) = (cx - ax, cy - ay);
		let (v1x, v1y) = (bx - ax, by - ay);
		let (v2x, v2y) = (p.0 - ax, p.1 - ay);
		let d00 = v0x * v0x + v0y * v0y;
		let d01 = v0x * v1x + v0y * v1y;
		let d11 = v1x * v1x + v1y * v1y;
		let d20 = v2x * v0x + v2y * v0y;
		let d21 = v2x * v1x + v2y * v1y;
		let denom = d00 * d11 - d01 * d01;
		if denom.abs() < 1e-18 {
			return None;
		}
		let c = (d11 * d20 - d01 * d21) / denom;
		let b = (d00 * d21 - d01 * d20) / denom;
		let a = 1.0 - b - c;
		// A small tolerance keeps pixels exactly on an edge from falling out.
		const EPS: f64 = -1e-9;
		(a >= EPS && b >= EPS && c >= EPS).then_some([a, b, c])
	}

	/// The bounding box of this triangle's corners in source pixels.
	pub fn source_bbox(&self, src_rect: &[f64; 4]) -> [f64; 4] {
		let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
		for uv in &self.uv {
			let p = source_uv(src_rect, (uv[0], uv[1]));
			bbox[0] = bbox[0].min(p.0);
			bbox[1] = bbox[1].min(p.1);
			bbox[2] = bbox[2].max(p.0);
			bbox[3] = bbox[3].max(p.1);
		}
		bbox
	}
}

/// The cubic Bernstein basis at `t`.
#[inline]
fn bernstein(t: f64) -> [f64; 4] {
	let u = 1.0 - t;
	[u * u * u, 3.0 * t * u * u, 3.0 * t * t * u, t * t * t]
}

/// The surface point `P(u, v)` of `patch` (destination document pixels).
pub fn evaluate(patch: &BezierPatch, u: f64, v: f64) -> (f64, f64) {
	let bu = bernstein(u);
	let bv = bernstein(v);
	let mut p = (0.0, 0.0);
	for (i, weight_v) in bv.iter().enumerate() {
		for (j, weight_u) in bu.iter().enumerate() {
			let w = weight_v * weight_u;
			p.0 += w * patch.points[i * 4 + j][0];
			p.1 += w * patch.points[i * 4 + j][1];
		}
	}
	p
}

/// Parameter `(u, v)` → source pixel point inside `src_rect`.
#[inline]
pub fn source_uv(src_rect: &[f64; 4], uv: (f64, f64)) -> (f64, f64) {
	(
		src_rect[0] + uv.0 * (src_rect[2] - src_rect[0]),
		src_rect[1] + uv.1 * (src_rect[3] - src_rect[1]),
	)
}

/// The patch's triangle mesh (destination space).
#[derive(Clone, Debug)]
pub struct WarpGrid {
	pub triangles: Vec<Triangle>,
}

impl WarpGrid {
	/// Evaluate `patch` on a `segments × segments` grid and split every quad
	/// into two triangles.
	pub fn build(patch: &BezierPatch, segments: u32) -> Self {
		let n = segments.max(1);
		let mut triangles = Vec::with_capacity((n * n * 2) as usize);
		let step = 1.0 / f64::from(n);
		for l in 0..n {
			let v0 = f64::from(l) * step;
			let v1 = f64::from(l + 1) * step;
			for k in 0..n {
				let u0 = f64::from(k) * step;
				let u1 = f64::from(k + 1) * step;
				let p00 = evaluate(patch, u0, v0);
				let p10 = evaluate(patch, u1, v0);
				let p11 = evaluate(patch, u1, v1);
				let p01 = evaluate(patch, u0, v1);
				triangles.push(Triangle {
					dst: [[p00.0, p00.1], [p10.0, p10.1], [p11.0, p11.1]],
					uv: [[u0, v0], [u1, v0], [u1, v1]],
				});
				triangles.push(Triangle {
					dst: [[p00.0, p00.1], [p11.0, p11.1], [p01.0, p01.1]],
					uv: [[u0, v0], [u1, v1], [u0, v1]],
				});
			}
		}
		Self { triangles }
	}

	/// Indices of the triangles whose destination box intersects `rect`.
	pub fn intersecting(&self, rect: [f64; 4]) -> Vec<usize> {
		self.triangles
			.iter()
			.enumerate()
			.filter(|(_, t)| {
				let b = t.bbox();
				b[0] <= rect[2] && b[1] <= rect[3] && b[2] >= rect[0] && b[3] >= rect[1]
			})
			.map(|(i, _)| i)
			.collect()
	}

	/// The parameter `(u, v)` of a destination point, looked up in
	/// `candidates`; the last containing triangle wins (fold-overs).
	pub fn parameter_at(&self, candidates: &[usize], dst: (f64, f64)) -> Option<(f64, f64)> {
		let mut found = None;
		for &index in candidates {
			let t = &self.triangles[index];
			if let Some(b) = t.barycentric(dst) {
				let uv = [
					b[0] * t.uv[0][0] + b[1] * t.uv[1][0] + b[2] * t.uv[2][0],
					b[0] * t.uv[0][1] + b[1] * t.uv[1][1] + b[2] * t.uv[2][1],
				];
				found = Some((uv[0], uv[1]));
			}
		}
		found
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn identity_patch_evaluates_to_the_point_itself() {
		let patch = BezierPatch::identity(200, 100);
		for (u, v) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.25), (0.3, 0.8)] {
			let p = evaluate(&patch, u, v);
			assert!((p.0 - u * 200.0).abs() < 1e-9 && (p.1 - v * 100.0).abs() < 1e-9, "{u},{v}: {p:?}");
		}
	}

	#[test]
	fn identity_mesh_inverts_exactly() {
		let patch = BezierPatch::identity(200, 100);
		let grid = WarpGrid::build(&patch, DEFAULT_SEGMENTS);
		let all: Vec<usize> = (0..grid.triangles.len()).collect();
		for (u, v) in [(0.0, 0.0), (0.5, 0.5), (0.99, 0.01), (0.25, 0.75)] {
			let dst = evaluate(&patch, u, v);
			let (iu, iv) = grid.parameter_at(&all, dst).expect("the point is inside the mesh");
			assert!((iu - u).abs() < 1e-9 && (iv - v).abs() < 1e-9, "({u},{v}) -> ({iu},{iv})");
		}
	}

	#[test]
	fn outside_the_mesh_there_is_no_parameter() {
		let patch = BezierPatch::identity(200, 100);
		let grid = WarpGrid::build(&patch, DEFAULT_SEGMENTS);
		let all: Vec<usize> = (0..grid.triangles.len()).collect();
		assert!(grid.parameter_at(&all, (-10.0, 50.0)).is_none());
		assert!(grid.parameter_at(&all, (250.0, 50.0)).is_none());
	}

	#[test]
	fn a_moved_control_point_moves_the_surface() {
		let mut patch = BezierPatch::identity(100, 100);
		patch.set_point(1, 1, 0.0, 0.0); // pull an interior control point
		let base = evaluate(&patch, 1.0 / 3.0, 1.0 / 3.0);
		let moved = evaluate(&patch, 1.0 / 3.0, 1.0 / 3.0);
		assert_eq!(base, moved, "deterministic");
		// Pushing the control point towards the origin moves the surface inward.
		let flat = evaluate(&BezierPatch::identity(100, 100), 1.0 / 3.0, 1.0 / 3.0);
		assert!(base.0 < flat.0 && base.1 < flat.1, "{base:?} vs {flat:?}");
	}
}
