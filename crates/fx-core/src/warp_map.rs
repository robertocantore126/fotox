//! The geometry behind [`Mapping::Custom`] (M11-T06..T08): a triangle mesh
//! (Puppet Warp, Perspective Warp) or a sparse backward displacement field
//! (Liquify), kept in a small process-wide registry so the `Mapping` stays
//! `Copy` (the sampler and the transform preview rely on it).
//!
//! FAST: the registry keeps the last [`KEEP`] entries only, so a command that
//! names an older id cannot be replayed; commands are not stored in
//! documents (history is by snapshots), so this holds within a session.
//!
//! [`Mapping::Custom`]: crate::transform::Mapping::Custom

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// How many registered geometries are kept.
const KEEP: usize = 48;
/// Nodes per chunk side of a displacement field.
pub const CHUNK: i32 = 64;

/// A triangle mesh: vertex `i` sits at `src[i]` in the source and `dst[i]` in
/// the destination (document pixels).
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
	pub src: Vec<[f64; 2]>,
	pub dst: Vec<[f64; 2]>,
	pub tris: Vec<[u32; 3]>,
}

impl TriMesh {
	pub fn dst_bounds(&self) -> Option<[f64; 4]> {
		let first = self.dst.first()?;
		let mut b = [first[0], first[1], first[0], first[1]];
		for p in &self.dst {
			b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
		}
		Some(b)
	}
}

/// A backward displacement field: the destination point `p` shows the source
/// at `p + d(p)`. Nodes every `cell` pixels, stored in chunks of `CHUNK²`
/// nodes that exist only where something moved.
#[derive(Clone, Debug)]
pub struct DispField {
	pub cell: f64,
	pub chunks: HashMap<(i32, i32), Arc<Vec<[f32; 2]>>>,
}

impl DispField {
	pub fn new(cell: f64) -> Self {
		Self { cell, chunks: HashMap::new() }
	}

	/// The displacement at node `(i, j)`.
	pub fn node(&self, i: i32, j: i32) -> [f32; 2] {
		let key = (i.div_euclid(CHUNK), j.div_euclid(CHUNK));
		match self.chunks.get(&key) {
			Some(c) => c[(j.rem_euclid(CHUNK) * CHUNK + i.rem_euclid(CHUNK)) as usize],
			None => [0.0; 2],
		}
	}

	/// A mutable node (its chunk is created or copied on write).
	pub fn node_mut(&mut self, i: i32, j: i32) -> &mut [f32; 2] {
		let key = (i.div_euclid(CHUNK), j.div_euclid(CHUNK));
		let chunk = self.chunks.entry(key).or_insert_with(|| Arc::new(vec![[0.0; 2]; (CHUNK * CHUNK) as usize]));
		&mut Arc::make_mut(chunk)[(j.rem_euclid(CHUNK) * CHUNK + i.rem_euclid(CHUNK)) as usize]
	}

	/// The bilinear displacement at document point `(x, y)`.
	pub fn at(&self, x: f64, y: f64) -> [f64; 2] {
		let (fx, fy) = (x / self.cell, y / self.cell);
		let (i, j) = (fx.floor() as i32, fy.floor() as i32);
		let (ax, ay) = ((fx - f64::from(i)) as f32, (fy - f64::from(j)) as f32);
		let (a, b, c, d) = (self.node(i, j), self.node(i + 1, j), self.node(i, j + 1), self.node(i + 1, j + 1));
		let v = [0, 1].map(|k| (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay);
		[f64::from(v[0]), f64::from(v[1])]
	}

	/// The largest displacement length (bounds a source rectangle).
	pub fn max(&self) -> f64 {
		let mut m = 0.0f32;
		for c in self.chunks.values() {
			for d in c.iter() {
				m = m.max(d[0].abs().max(d[1].abs()));
			}
		}
		f64::from(m)
	}

	/// Whether nothing moved.
	pub fn is_identity(&self) -> bool {
		self.chunks.values().all(|c| c.iter().all(|d| d[0] == 0.0 && d[1] == 0.0))
	}
}

#[derive(Clone, Debug)]
pub enum WarpData {
	Mesh(TriMesh),
	Field(DispField),
}

struct Registry {
	next: u64,
	entries: Vec<(u64, Arc<WarpData>)>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry { next: 1, entries: Vec::new() });

/// Register a geometry; returns the id a [`Mapping::Custom`] names.
///
/// [`Mapping::Custom`]: crate::transform::Mapping::Custom
pub fn register(data: WarpData) -> u64 {
	let mut r = REGISTRY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
	let id = r.next;
	r.next += 1;
	r.entries.push((id, Arc::new(data)));
	if r.entries.len() > KEEP {
		r.entries.remove(0);
	}
	id
}

/// The geometry of `id`, if it is still registered.
pub fn get(id: u64) -> Option<Arc<WarpData>> {
	let r = REGISTRY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
	r.entries.iter().find(|(i, _)| *i == id).map(|(_, d)| d.clone())
}
