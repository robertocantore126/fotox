//! Paths (M10-T01, D-073): the one path model of document paths, shape
//! outlines, vector masks and type on a path.
//!
//! A [`Path`] is a list of subpaths; a subpath is a list of anchors, each with
//! its incoming and outgoing handle (absolute positions; a handle equal to its
//! anchor means "no handle") and a smooth flag (the Pen tool keeps a smooth
//! anchor's handles collinear). Every segment is a cubic Bézier. Each subpath
//! has Photoshop's path operation.
//!
//! FAST (D-074): the operations are rendered with winding tricks — Subtract
//! reverses the subpath so it cancels the nonzero winding where it overlaps,
//! Intersect / Exclude fall back to Combine / even-odd — until `i_overlay`
//! arrives.

use serde::{Deserialize, Serialize};

use crate::vector::PathEl;

pub type Point = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
	pub pos: Point,
	/// Incoming handle (from the previous anchor).
	#[serde(rename = "in")]
	pub inh: Point,
	/// Outgoing handle (to the next anchor).
	pub out: Point,
	#[serde(default)]
	pub smooth: bool,
}

impl Anchor {
	/// A corner anchor without handles.
	pub fn corner(p: Point) -> Self {
		Self {
			pos: p,
			inh: p,
			out: p,
			smooth: false,
		}
	}

	/// A smooth anchor whose outgoing handle is `out` (the incoming one
	/// mirrored).
	pub fn smooth(p: Point, out: Point) -> Self {
		Self {
			pos: p,
			inh: (2.0 * p.0 - out.0, 2.0 * p.1 - out.1),
			out,
			smooth: true,
		}
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathOp {
	#[default]
	Combine,
	Subtract,
	Intersect,
	Exclude,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Subpath {
	pub anchors: Vec<Anchor>,
	#[serde(default)]
	pub closed: bool,
	#[serde(default)]
	pub op: PathOp,
}

impl Subpath {
	/// The cubic segments `(p0, c1, c2, p1)`, the closing one included.
	pub fn segments(&self) -> Vec<[Point; 4]> {
		let n = self.anchors.len();
		let count = if self.closed { n } else { n.saturating_sub(1) };
		(0..count)
			.map(|i| {
				let (a, b) = (self.anchors[i], self.anchors[(i + 1) % n]);
				[a.pos, a.out, b.inh, b.pos]
			})
			.collect()
	}
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Path {
	pub subpaths: Vec<Subpath>,
}

/// A saved document path (the Paths panel).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedPath {
	pub name: String,
	pub path: Path,
}

/// The point at `t` of a cubic.
pub fn cubic_at(s: &[Point; 4], t: f64) -> Point {
	let u = 1.0 - t;
	let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
	(
		a * s[0].0 + b * s[1].0 + c * s[2].0 + d * s[3].0,
		a * s[0].1 + b * s[1].1 + c * s[2].1 + d * s[3].1,
	)
}

/// De Casteljau split of a cubic at `t`: the two halves.
pub fn split_cubic(s: &[Point; 4], t: f64) -> ([Point; 4], [Point; 4]) {
	let l = |a: Point, b: Point| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
	let (p01, p12, p23) = (l(s[0], s[1]), l(s[1], s[2]), l(s[2], s[3]));
	let (p012, p123) = (l(p01, p12), l(p12, p23));
	let m = l(p012, p123);
	([s[0], p01, p012, m], [m, p123, p23, s[3]])
}

fn dist(a: Point, b: Point) -> f64 {
	(a.0 - b.0).hypot(a.1 - b.1)
}

impl Path {
	pub fn is_empty(&self) -> bool {
		self.subpaths.iter().all(|s| s.anchors.is_empty())
	}

	/// The path as renderer elements. Subtract subpaths are reversed (FAST,
	/// see the module doc).
	pub fn to_elements(&self) -> Vec<PathEl> {
		let mut out = Vec::new();
		for sub in &self.subpaths {
			if sub.anchors.is_empty() {
				continue;
			}
			let mut s = sub.clone();
			if sub.op == PathOp::Subtract {
				s.anchors.reverse();
				for a in &mut s.anchors {
					std::mem::swap(&mut a.inh, &mut a.out);
				}
			}
			let first = s.anchors[0].pos;
			out.push(PathEl::MoveTo([first.0, first.1]));
			for seg in s.segments() {
				if seg[1] == seg[0] && seg[2] == seg[3] {
					out.push(PathEl::LineTo([seg[3].0, seg[3].1]));
				} else {
					out.push(PathEl::CubicTo([seg[1].0, seg[1].1], [seg[2].0, seg[2].1], [seg[3].0, seg[3].1]));
				}
			}
			if s.closed {
				out.push(PathEl::Close);
			}
		}
		out
	}

	/// A path from renderer elements (quadratics are raised to cubics).
	pub fn from_elements(elements: &[PathEl]) -> Path {
		let mut path = Path::default();
		let mut cur: Option<Subpath> = None;
		for el in elements {
			match *el {
				PathEl::MoveTo(p) => {
					if let Some(s) = cur.take() {
						path.subpaths.push(s);
					}
					cur = Some(Subpath {
						anchors: vec![Anchor::corner((p[0], p[1]))],
						closed: false,
						op: PathOp::Combine,
					});
				}
				PathEl::LineTo(p) => {
					if let Some(s) = cur.as_mut() {
						s.anchors.push(Anchor::corner((p[0], p[1])));
					}
				}
				PathEl::QuadTo(c, p) => {
					if let Some(s) = cur.as_mut()
						&& let Some(last) = s.anchors.last_mut()
					{
						let p0 = last.pos;
						last.out = (p0.0 + 2.0 / 3.0 * (c[0] - p0.0), p0.1 + 2.0 / 3.0 * (c[1] - p0.1));
						let inh = (p[0] + 2.0 / 3.0 * (c[0] - p[0]), p[1] + 2.0 / 3.0 * (c[1] - p[1]));
						s.anchors.push(Anchor {
							pos: (p[0], p[1]),
							inh,
							out: (p[0], p[1]),
							smooth: false,
						});
					}
				}
				PathEl::CubicTo(c1, c2, p) => {
					if let Some(s) = cur.as_mut()
						&& let Some(last) = s.anchors.last_mut()
					{
						last.out = (c1[0], c1[1]);
						s.anchors.push(Anchor {
							pos: (p[0], p[1]),
							inh: (c2[0], c2[1]),
							out: (p[0], p[1]),
							smooth: false,
						});
					}
				}
				PathEl::Close => {
					if let Some(mut s) = cur.take() {
						s.closed = true;
						// A closing segment that returns onto the first anchor
						// carries the first anchor's incoming handle.
						if s.anchors.len() > 1 && s.anchors.last().map(|a| a.pos) == s.anchors.first().map(|a| a.pos) {
							let last = s.anchors.pop().expect("len > 1");
							s.anchors[0].inh = last.inh;
						}
						path.subpaths.push(s);
					}
				}
			}
		}
		if let Some(s) = cur {
			path.subpaths.push(s);
		}
		path
	}

	/// Every subpath flattened to a polyline within `tolerance` pixels:
	/// `(points, closed, op)`.
	pub fn flatten(&self, tolerance: f64) -> Vec<(Vec<Point>, bool, PathOp)> {
		self.subpaths
			.iter()
			.filter(|s| !s.anchors.is_empty())
			.map(|s| {
				let mut pts = vec![s.anchors[0].pos];
				for seg in s.segments() {
					let len = dist(seg[0], seg[1]) + dist(seg[1], seg[2]) + dist(seg[2], seg[3]);
					let n = ((len / tolerance.max(0.05)).sqrt().ceil() as usize).clamp(1, 512);
					for i in 1..=n {
						pts.push(cubic_at(&seg, i as f64 / n as f64));
					}
				}
				if s.closed && pts.len() > 1 && dist(pts[0], *pts.last().expect("len > 1")) < 1e-9 {
					pts.pop();
				}
				(pts, s.closed, s.op)
			})
			.collect()
	}

	/// The bounds of the anchors and handles `[x0, y0, x1, y1]`.
	pub fn bounds(&self) -> Option<[f64; 4]> {
		let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
		for s in &self.subpaths {
			for a in &s.anchors {
				for p in [a.pos, a.inh, a.out] {
					b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
				}
			}
		}
		b[0].is_finite().then_some(b)
	}

	/// Every point moved by `f`.
	pub fn map(&self, f: impl Fn(Point) -> Point) -> Path {
		let mut p = self.clone();
		for s in &mut p.subpaths {
			for a in &mut s.anchors {
				a.pos = f(a.pos);
				a.inh = f(a.inh);
				a.out = f(a.out);
			}
		}
		p
	}

	/// The anchor (subpath, index) within `reach` of `p`.
	pub fn anchor_at(&self, p: Point, reach: f64) -> Option<(usize, usize)> {
		let mut best = None;
		let mut d = reach;
		for (si, s) in self.subpaths.iter().enumerate() {
			for (ai, a) in s.anchors.iter().enumerate() {
				let e = dist(a.pos, p);
				if e <= d {
					d = e;
					best = Some((si, ai));
				}
			}
		}
		best
	}

	/// The segment (subpath, segment index, t) nearest `p` within `reach`.
	pub fn segment_at(&self, p: Point, reach: f64) -> Option<(usize, usize, f64)> {
		let mut best = None;
		let mut d = reach;
		for (si, s) in self.subpaths.iter().enumerate() {
			for (gi, seg) in s.segments().iter().enumerate() {
				for k in 0..=64 {
					let t = k as f64 / 64.0;
					let e = dist(cubic_at(seg, t), p);
					if e <= d {
						d = e;
						best = Some((si, gi, t));
					}
				}
			}
		}
		best
	}

	/// Insert an anchor at `t` of segment `seg` of subpath `si`, keeping the
	/// curve (de Casteljau). Returns the new anchor's index.
	pub fn split(&mut self, si: usize, seg: usize, t: f64) -> Option<usize> {
		let s = self.subpaths.get_mut(si)?;
		let segs = s.segments();
		let cubic = *segs.get(seg)?;
		let (l, r) = split_cubic(&cubic, t);
		let n = s.anchors.len();
		let next = (seg + 1) % n;
		s.anchors[seg].out = l[1];
		s.anchors[next].inh = r[2];
		let smooth = !(l[2] == l[3] && r[1] == r[0]);
		s.anchors.insert(
			seg + 1,
			Anchor {
				pos: l[3],
				inh: l[2],
				out: r[1],
				smooth,
			},
		);
		Some(seg + 1)
	}
}

/// Ramer–Douglas–Peucker simplification of a polyline.
pub fn simplify(points: &[Point], tolerance: f64) -> Vec<Point> {
	if points.len() < 3 {
		return points.to_vec();
	}
	let (a, b) = (points[0], *points.last().expect("len >= 3"));
	let (dx, dy) = (b.0 - a.0, b.1 - a.1);
	let len = dx.hypot(dy).max(1e-12);
	let mut far = (0, 0.0);
	for (i, p) in points.iter().enumerate().skip(1).take(points.len() - 2) {
		let d = ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len;
		if d > far.1 {
			far = (i, d);
		}
	}
	if far.1 <= tolerance {
		return vec![a, b];
	}
	let mut left = simplify(&points[..=far.0], tolerance);
	let right = simplify(&points[far.0..], tolerance);
	left.pop();
	left.extend(right);
	left
}

/// Smooth anchors through `points` with Catmull-Rom handles (the Curvature
/// Pen, and the curve fit of the Freeform Pen and Selection to Path).
/// `corners[i]` makes point `i` a corner.
pub fn smooth_through(points: &[Point], closed: bool, corners: &[bool]) -> Subpath {
	let n = points.len();
	let mut anchors: Vec<Anchor> = points.iter().map(|&p| Anchor::corner(p)).collect();
	for i in 0..n {
		if corners.get(i).copied().unwrap_or(false) {
			continue;
		}
		let prev = if i > 0 {
			points[i - 1]
		} else if closed {
			points[n - 1]
		} else {
			continue;
		};
		let next = if i + 1 < n {
			points[i + 1]
		} else if closed {
			points[0]
		} else {
			continue;
		};
		let t = ((next.0 - prev.0) / 6.0, (next.1 - prev.1) / 6.0);
		let p = points[i];
		anchors[i] = Anchor {
			pos: p,
			inh: (p.0 - t.0, p.1 - t.1),
			out: (p.0 + t.0, p.1 + t.1),
			smooth: true,
		};
	}
	Subpath {
		anchors,
		closed,
		op: PathOp::Combine,
	}
}

/// Which document path a command or tool works on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathTarget {
	/// The Work Path (replaced by the next drawing when it is not selected).
	#[default]
	Work,
	/// A saved path by index.
	Saved(usize),
}
