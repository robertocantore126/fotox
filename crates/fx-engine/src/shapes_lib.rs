//! The custom shapes library (M10-T07, D-076): Fotox JSON, paths in a unit
//! box (`0..=1` on both axes), `%APPDATA%\Fotox\shapes.json` for the user's
//! shapes (Edit ▸ Define Custom Shape) plus built-in ones.
//!
//! The engine puts the whole library into the tool settings as
//! `_custom_shapes` (name → path) so the Custom Shape tool can read it.

use std::path::PathBuf;

use fx_core::path::{Anchor, NamedPath, Path, Subpath, smooth_through};

pub fn path() -> Option<PathBuf> {
	std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Fotox").join("shapes.json"))
}

fn closed(points: &[(f64, f64)]) -> Path {
	Path {
		subpaths: vec![Subpath {
			anchors: points.iter().map(|&p| Anchor::corner(p)).collect(),
			closed: true,
			op: Default::default(),
		}],
	}
}

fn round(points: &[(f64, f64)]) -> Path {
	Path {
		subpaths: vec![smooth_through(points, true, &[])],
	}
}

/// The built-in shapes.
pub fn builtin() -> Vec<NamedPath> {
	let star: Vec<(f64, f64)> = (0..10)
		.map(|i| {
			let a = -std::f64::consts::FRAC_PI_2 + f64::from(i) * std::f64::consts::PI / 5.0;
			let r = if i % 2 == 0 { 0.5 } else { 0.2 };
			(0.5 + r * a.cos(), 0.52 + r * a.sin())
		})
		.collect();
	let named = |name: &str, path: Path| NamedPath { name: name.into(), path };
	vec![
		named("Star", closed(&star)),
		named(
			"Arrow",
			closed(&[(0.0, 0.35), (0.6, 0.35), (0.6, 0.1), (1.0, 0.5), (0.6, 0.9), (0.6, 0.65), (0.0, 0.65)]),
		),
		named(
			"Check",
			closed(&[(0.0, 0.55), (0.15, 0.4), (0.38, 0.62), (0.85, 0.1), (1.0, 0.25), (0.38, 0.9)]),
		),
		named(
			"Lightning",
			closed(&[(0.55, 0.0), (0.15, 0.55), (0.45, 0.55), (0.3, 1.0), (0.85, 0.4), (0.55, 0.4), (0.75, 0.0)]),
		),
		named(
			"Heart",
			round(&[
				(0.5, 0.3),
				(0.75, 0.05),
				(1.0, 0.3),
				(0.85, 0.6),
				(0.5, 0.95),
				(0.15, 0.6),
				(0.0, 0.3),
				(0.25, 0.05),
			]),
		),
		named(
			"Cloud",
			round(&[
				(0.2, 0.75),
				(0.05, 0.55),
				(0.2, 0.35),
				(0.4, 0.2),
				(0.62, 0.25),
				(0.8, 0.3),
				(0.95, 0.52),
				(0.8, 0.75),
			]),
		),
		named("Leaf", round(&[(0.5, 0.0), (0.85, 0.35), (0.8, 0.7), (0.5, 1.0), (0.2, 0.7), (0.15, 0.35)])),
		named(
			"Speech Bubble",
			closed(&[(0.0, 0.0), (1.0, 0.0), (1.0, 0.7), (0.45, 0.7), (0.2, 1.0), (0.25, 0.7), (0.0, 0.7)]),
		),
	]
}

/// The user's shapes.
pub fn load() -> Vec<NamedPath> {
	path()
		.and_then(|p| std::fs::read_to_string(p).ok())
		.and_then(|t| serde_json::from_str(&t).ok())
		.unwrap_or_default()
}

/// FAST: errors are logged.
pub fn save(shapes: &[NamedPath]) {
	let Some(path) = path() else { return };
	if let Some(dir) = path.parent() {
		let _ = std::fs::create_dir_all(dir);
	}
	if let Ok(text) = serde_json::to_string(shapes)
		&& let Err(error) = std::fs::write(&path, text)
	{
		tracing::warn!("cannot write {}: {error}", path.display());
	}
}

/// A path scaled into the unit box (Define Custom Shape).
pub fn normalise(path: &Path) -> Option<Path> {
	let b = path.bounds()?;
	let (w, h) = ((b[2] - b[0]).max(1e-9), (b[3] - b[1]).max(1e-9));
	let s = w.max(h);
	Some(path.map(|(x, y)| ((x - b[0]) / s, (y - b[1]) / s)))
}
