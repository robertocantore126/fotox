//! Puppet Warp's mesh and solver (M11-T07, D-079).
//!
//! The mesh is a grid over the layer's content box: a cell is kept when the
//! alpha (dilated by Expansion) covers any of its samples, and split into two
//! triangles. The pins deform it by moving least squares (Schaefer et al.
//! 2006): Rigid, Normal (similarity) or Distort (affine).
//!
//! FAST: MLS on a grid instead of ARAP on a constrained triangulation; no
//! pin rotation or depth.

use fx_core::warp_map::TriMesh;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
	Rigid,
	Normal,
	Distort,
}

/// A grid mesh over `rect` with cells of `step` pixels, keeping the cells
/// where `covered(x, y)` holds for one of nine samples. `src == dst`.
pub fn build_mesh(rect: [f64; 4], step: f64, covered: &mut dyn FnMut(f64, f64) -> bool) -> TriMesh {
	let cols = (((rect[2] - rect[0]) / step).ceil() as usize).max(1);
	let rows = (((rect[3] - rect[1]) / step).ceil() as usize).max(1);
	let at = |i: usize, j: usize| [rect[0] + i as f64 * step, rect[1] + j as f64 * step];
	let mut index = vec![u32::MAX; (cols + 1) * (rows + 1)];
	let mut mesh = TriMesh::default();
	let mut vertex = |mesh: &mut TriMesh, i: usize, j: usize| -> u32 {
		let k = j * (cols + 1) + i;
		if index[k] == u32::MAX {
			index[k] = mesh.src.len() as u32;
			mesh.src.push(at(i, j));
			mesh.dst.push(at(i, j));
		}
		index[k]
	};
	for j in 0..rows {
		for i in 0..cols {
			let (x0, y0) = (rect[0] + i as f64 * step, rect[1] + j as f64 * step);
			let keep = (0..3).any(|a| (0..3).any(|b| covered(x0 + step * f64::from(a) / 2.0, y0 + step * f64::from(b) / 2.0)));
			if !keep {
				continue;
			}
			let (v00, v10, v11, v01) = (
				vertex(&mut mesh, i, j),
				vertex(&mut mesh, i + 1, j),
				vertex(&mut mesh, i + 1, j + 1),
				vertex(&mut mesh, i, j + 1),
			);
			mesh.tris.push([v00, v10, v11]);
			mesh.tris.push([v00, v11, v01]);
		}
	}
	mesh
}

/// Where `v` goes when the pins `from[i]` move to `to[i]`.
pub fn mls(v: [f64; 2], from: &[[f64; 2]], to: &[[f64; 2]], mode: Mode) -> [f64; 2] {
	match from.len() {
		0 => return v,
		1 => return [v[0] + to[0][0] - from[0][0], v[1] + to[0][1] - from[0][1]],
		_ => {}
	}
	let mut w = Vec::with_capacity(from.len());
	for p in from {
		let d2 = (p[0] - v[0]).powi(2) + (p[1] - v[1]).powi(2);
		if d2 < 1e-9 {
			// On a pin: exactly its target.
			let i = w.len();
			return to[i];
		}
		w.push(1.0 / d2);
	}
	let sw: f64 = w.iter().sum();
	let mut ps = [0.0; 2];
	let mut qs = [0.0; 2];
	for i in 0..from.len() {
		for k in 0..2 {
			ps[k] += w[i] * from[i][k] / sw;
			qs[k] += w[i] * to[i][k] / sw;
		}
	}
	let d = [v[0] - ps[0], v[1] - ps[1]];
	match mode {
		Mode::Distort => {
			// M = (Σ w p̂ᵀp̂)⁻¹ Σ w p̂ᵀq̂
			let (mut a, mut b, mut c) = (0.0, 0.0, 0.0);
			let mut n = [[0.0; 2]; 2];
			for i in 0..from.len() {
				let p = [from[i][0] - ps[0], from[i][1] - ps[1]];
				let q = [to[i][0] - qs[0], to[i][1] - qs[1]];
				a += w[i] * p[0] * p[0];
				b += w[i] * p[0] * p[1];
				c += w[i] * p[1] * p[1];
				for r in 0..2 {
					for s in 0..2 {
						n[r][s] += w[i] * p[r] * q[s];
					}
				}
			}
			let det = a * c - b * b;
			if det.abs() < 1e-12 {
				return [v[0] + qs[0] - ps[0], v[1] + qs[1] - ps[1]];
			}
			let inv = [[c / det, -b / det], [-b / det, a / det]];
			let m = [
				[inv[0][0] * n[0][0] + inv[0][1] * n[1][0], inv[0][0] * n[0][1] + inv[0][1] * n[1][1]],
				[inv[1][0] * n[0][0] + inv[1][1] * n[1][0], inv[1][0] * n[0][1] + inv[1][1] * n[1][1]],
			];
			[d[0] * m[0][0] + d[1] * m[1][0] + qs[0], d[0] * m[0][1] + d[1] * m[1][1] + qs[1]]
		}
		Mode::Normal | Mode::Rigid => {
			let mut f = [0.0; 2];
			let mut mu = 0.0;
			for i in 0..from.len() {
				let p = [from[i][0] - ps[0], from[i][1] - ps[1]];
				let q = [to[i][0] - qs[0], to[i][1] - qs[1]];
				let a = [
					[p[0] * d[0] + p[1] * d[1], p[0] * d[1] - p[1] * d[0]],
					[p[1] * d[0] - p[0] * d[1], p[1] * d[1] + p[0] * d[0]],
				];
				f[0] += w[i] * (q[0] * a[0][0] + q[1] * a[1][0]);
				f[1] += w[i] * (q[0] * a[0][1] + q[1] * a[1][1]);
				mu += w[i] * (p[0] * p[0] + p[1] * p[1]);
			}
			if mode == Mode::Rigid {
				let len = (f[0] * f[0] + f[1] * f[1]).sqrt();
				let dl = (d[0] * d[0] + d[1] * d[1]).sqrt();
				if len < 1e-12 {
					return [qs[0] + d[0], qs[1] + d[1]];
				}
				[dl * f[0] / len + qs[0], dl * f[1] / len + qs[1]]
			} else if mu < 1e-12 {
				[qs[0] + d[0], qs[1] + d[1]]
			} else {
				[f[0] / mu + qs[0], f[1] / mu + qs[1]]
			}
		}
	}
}
