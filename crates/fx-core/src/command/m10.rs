//! M10's path commands (T01): document paths, path ↔ selection, Fill Path.

use std::collections::HashMap;

use fx_tiles::TILE_SIZE;

use super::*;
use crate::path::{NamedPath, Path, PathOp, PathTarget, simplify, smooth_through};

fn path_effect(label: &str) -> CommandEffect {
	// Paths are saved: the step dirties the document, no layer changes.
	CommandEffect {
		label: label.into(),
		..Default::default()
	}
}

fn no_path() -> CommandError {
	CommandError::NotAllowed("there is no such path".into())
}

pub(super) fn set_path(doc: &mut Document, target: PathTarget, path: &Path, name: Option<&str>, label: &str) -> Result<CommandEffect, CommandError> {
	match target {
		PathTarget::Work => doc.work_path = Some(path.clone()),
		PathTarget::Saved(i) if i < doc.paths.len() => doc.paths[i].path = path.clone(),
		PathTarget::Saved(i) if i == doc.paths.len() => doc.paths.push(NamedPath {
			name: name.map(str::to_owned).unwrap_or_else(|| format!("Path {}", i + 1)),
			path: path.clone(),
		}),
		PathTarget::Saved(_) => return Err(no_path()),
	}
	doc.active_path = Some(target);
	Ok(path_effect(label))
}

pub(super) fn delete_path(doc: &mut Document, target: PathTarget) -> Result<CommandEffect, CommandError> {
	match target {
		PathTarget::Work => {
			doc.work_path.take().ok_or_else(no_path)?;
		}
		PathTarget::Saved(i) if i < doc.paths.len() => {
			doc.paths.remove(i);
		}
		PathTarget::Saved(_) => return Err(no_path()),
	}
	doc.active_path = None;
	Ok(path_effect("Delete Path"))
}

pub(super) fn rename_path(doc: &mut Document, index: usize, name: &str) -> Result<CommandEffect, CommandError> {
	let p = doc.paths.get_mut(index).ok_or_else(no_path)?;
	if !name.trim().is_empty() {
		p.name = name.to_owned();
	}
	Ok(path_effect("Rename Path"))
}

pub(super) fn save_work_path(doc: &mut Document, name: &str) -> Result<CommandEffect, CommandError> {
	let path = doc.work_path.take().ok_or_else(no_path)?;
	doc.paths.push(NamedPath {
		name: if name.trim().is_empty() {
			format!("Path {}", doc.paths.len() + 1)
		} else {
			name.to_owned()
		},
		path,
	});
	doc.active_path = Some(PathTarget::Saved(doc.paths.len() - 1));
	Ok(path_effect("Save Path"))
}

/// The polygons of a path's closed (and closed-by-force) subpaths, with how
/// each combines: the first by `mode`, the others by their path operation.
fn polygons(path: &Path, mode: SelectMode) -> Vec<(Vec<(f64, f64)>, SelectMode)> {
	path.flatten(0.25)
		.into_iter()
		.enumerate()
		.filter(|(_, (pts, _, _))| pts.len() >= 3)
		.map(|(i, (pts, _, op))| {
			let m = if i == 0 {
				mode
			} else {
				match op {
					PathOp::Combine | PathOp::Exclude => SelectMode::Add,
					PathOp::Subtract => SelectMode::Subtract,
					PathOp::Intersect => SelectMode::Intersect,
				}
			};
			(pts, m)
		})
		.collect()
}

pub(super) fn path_to_selection(
	doc: &mut Document,
	target: PathTarget,
	feather: f64,
	anti_alias: bool,
	mode: SelectMode,
	ctx: &mut CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let path = doc.path(target).cloned().ok_or_else(no_path)?;
	let parts = polygons(&path, mode);
	if parts.is_empty() {
		return Err(CommandError::NotAllowed("the path encloses no area".into()));
	}
	// All or nothing: work on a copy.
	let mut work = doc.clone();
	for (points, m) in parts {
		select(&mut work, &SelectionShape::Polygon { points }, m, feather, anti_alias, ctx)?;
	}
	doc.selection = work.selection;
	Ok(selection_effect("Make Selection"))
}

/// Selection to path: the selection's outline traced on a grid (≤ 1024 cells
/// per side over its bounds), simplified within `tolerance` and made smooth
/// except at sharp turns.
// FAST: a traced grid, not Schneider's curve fit; holes come out as
// Combine subpaths (the Work Path's fill rule is nonzero).
pub(super) fn selection_to_path(doc: &mut Document, tolerance: f64, ctx: &CommandContext<'_>) -> Result<CommandEffect, CommandError> {
	let Some(selection) = doc.selection.clone() else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let size = (doc.width, doc.height);
	let Some((x0, y0, x1, y1)) = selection.canvas_bounds(size) else {
		return Err(CommandError::NotAllowed("nothing is selected".into()));
	};
	let (bw, bh) = (x1 - x0, y1 - y0);
	let step = f64::from(bw.max(bh)).div_euclid(1024.0).max(0.0) + 1.0;
	let (gw, gh) = ((f64::from(bw) / step).ceil() as i64 + 2, (f64::from(bh) / step).ceil() as i64 + 2);
	let tile = TILE_SIZE;
	let mut cache: HashMap<(u32, u32), crate::selection::TileCoverage> = HashMap::new();
	let mut inside = vec![false; (gw * gh) as usize];
	for gy in 1..gh - 1 {
		for gx in 1..gw - 1 {
			let cx = (f64::from(x0) + (gx as f64 - 0.5) * step) as u32;
			let cy = (f64::from(y0) + (gy as f64 - 0.5) * step) as u32;
			if cx >= size.0 || cy >= size.1 {
				continue;
			}
			let key = (cx / tile, cy / tile);
			if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(key) {
				e.insert(selection.tile_coverage(ctx.tiles, key.0, key.1)?);
			}
			inside[(gy * gw + gx) as usize] = cache[&key].at(cx % tile, cy % tile) >= 0.5;
		}
	}
	// Directed boundary edges, inside on the right; corners are grid points.
	let at = |x: i64, y: i64| x >= 0 && y >= 0 && x < gw && y < gh && inside[(y * gw + x) as usize];
	let mut next: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
	for y in 0..gh {
		for x in 0..gw {
			if !at(x, y) {
				continue;
			}
			if !at(x, y - 1) {
				next.entry((x, y)).or_default().push((x + 1, y));
			}
			if !at(x + 1, y) {
				next.entry((x + 1, y)).or_default().push((x + 1, y + 1));
			}
			if !at(x, y + 1) {
				next.entry((x + 1, y + 1)).or_default().push((x, y + 1));
			}
			if !at(x - 1, y) {
				next.entry((x, y + 1)).or_default().push((x, y));
			}
		}
	}
	let mut path = Path::default();
	while let Some(&start) = next.keys().next() {
		let mut loop_pts = vec![start];
		let mut p = start;
		loop {
			let Some(list) = next.get_mut(&p) else { break };
			let q = list.pop().expect("lists are never left empty");
			if list.is_empty() {
				next.remove(&p);
			}
			if q == start {
				break;
			}
			loop_pts.push(q);
			p = q;
		}
		if loop_pts.len() < 4 {
			continue;
		}
		let doc_pts: Vec<(f64, f64)> = loop_pts
			.iter()
			.map(|&(gx, gy)| (f64::from(x0) + (gx as f64 - 1.0) * step, f64::from(y0) + (gy as f64 - 1.0) * step))
			.collect();
		let mut closed = doc_pts.clone();
		closed.push(doc_pts[0]);
		let mut simple = simplify(&closed, tolerance.max(0.5));
		simple.pop();
		if simple.len() < 3 {
			continue;
		}
		// Sharp turns stay corners.
		let n = simple.len();
		let corners: Vec<bool> = (0..n)
			.map(|i| {
				let (a, b, c) = (simple[(i + n - 1) % n], simple[i], simple[(i + 1) % n]);
				let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1));
				let cos = (u.0 * v.0 + u.1 * v.1) / (u.0.hypot(u.1) * v.0.hypot(v.1)).max(1e-9);
				cos < 0.5
			})
			.collect();
		path.subpaths.push(smooth_through(&simple, true, &corners));
	}
	if path.is_empty() {
		return Err(CommandError::NotAllowed("the selection is too small to trace".into()));
	}
	doc.work_path = Some(path);
	doc.active_path = Some(PathTarget::Work);
	Ok(path_effect("Make Work Path"))
}

pub(super) fn fill_path(
	doc: &mut Document,
	target: PathTarget,
	source: &crate::fill::FillSource,
	mode: BlendMode,
	opacity: f64,
	ctx: &mut CommandContext<'_>,
) -> Result<CommandEffect, CommandError> {
	let path = doc.path(target).cloned().ok_or_else(no_path)?;
	let layer = LayerRef::Active;
	let (id, image, offset) = pixel_target(doc, &layer)?;
	// The path's coverage as a temporary selection.
	let mut work = doc.clone();
	work.selection = None;
	for (points, m) in polygons(&path, SelectMode::Replace) {
		select(&mut work, &SelectionShape::Polygon { points }, m, 0.0, true, ctx)?;
	}
	let Some(region) = work.selection else {
		return Err(CommandError::NotAllowed("the path encloses no area".into()));
	};
	let locked_alpha = doc.layer(id).is_some_and(|l| l.locked_transparency);
	let placed = crate::pixels::Placed { image: &image, offset };
	let canvas = (doc.width, doc.height);
	let (filled, offset) = match source {
		crate::fill::FillSource::Color { rgba } => crate::pixels::fill(
			placed,
			Some(&region),
			canvas,
			&crate::pixels::FillSpec {
				color: *rgba,
				mode,
				opacity: opacity.clamp(0.0, 1.0),
				preserve_transparency: locked_alpha,
			},
			ctx.tiles,
		)?,
		crate::fill::FillSource::Pattern { pattern } => {
			let paint = m8::pattern_paint(doc, *pattern)?;
			crate::pixels::fill_with(placed, Some(&region), canvas, mode, opacity.clamp(0.0, 1.0), locked_alpha, &paint, ctx.tiles)?
		}
	};
	set_pixels(doc, id, filled, offset);
	Ok(CommandEffect {
		label: "Fill Path".into(),
		pixels_changed: vec![id],
		..Default::default()
	})
}
