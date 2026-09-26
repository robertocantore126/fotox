//! Preferences (M7-T09, D-062): `%APPDATA%\Fotox\preferences.json`.
//!
//! A JSON object, versioned, and unknown keys are kept, so an older Fotox
//! does not drop what a newer one wrote. Keys used now:
//!
//! * `grid_every` (px), `subdivisions` — the grid (M7-T06);
//! * `memory_budget_mb`, `scratch_dir` — read at start, applied at the next;
//! * `recent` — up to 10 paths, newest first (File ▸ Open Recent).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub const VERSION: u64 = 1;
const RECENT_MAX: usize = 10;

/// Where the file lives (`None` without `%APPDATA%`).
pub fn path() -> Option<PathBuf> {
	std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Fotox").join("preferences.json"))
}

#[derive(Clone, Debug, Default)]
pub struct Prefs(pub Map<String, Value>);

impl Prefs {
	/// Read the file; a missing or broken one gives the defaults.
	pub fn load() -> Self {
		let Some(path) = path() else { return Self::default() };
		match std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
			Some(Value::Object(map)) => Self(map),
			_ => Self::default(),
		}
	}

	/// Write the file (FAST: errors are logged, not reported).
	pub fn save(&self) {
		let Some(path) = path() else { return };
		let mut map = self.0.clone();
		map.insert("version".into(), Value::from(VERSION));
		if let Some(dir) = path.parent()
			&& let Err(error) = std::fs::create_dir_all(dir)
		{
			tracing::warn!("cannot create {}: {error}", dir.display());
			return;
		}
		match serde_json::to_string_pretty(&Value::Object(map)) {
			Ok(text) => {
				if let Err(error) = std::fs::write(&path, text) {
					tracing::warn!("cannot write {}: {error}", path.display());
				}
			}
			Err(error) => tracing::warn!("cannot serialise the preferences: {error}"),
		}
	}

	/// Merge `patch` (an object) into the preferences.
	pub fn merge(&mut self, patch: &Value) {
		if let Value::Object(patch) = patch {
			for (key, value) in patch {
				self.0.insert(key.clone(), value.clone());
			}
		}
	}

	pub fn number(&self, key: &str) -> Option<f64> {
		self.0.get(key).and_then(Value::as_f64)
	}

	pub fn string(&self, key: &str) -> Option<String> {
		self.0.get(key).and_then(Value::as_str).map(str::to_owned)
	}

	/// The recent files, newest first.
	pub fn recent(&self) -> Vec<PathBuf> {
		self.0
			.get("recent")
			.and_then(Value::as_array)
			.map(|list| list.iter().filter_map(Value::as_str).map(PathBuf::from).collect())
			.unwrap_or_default()
	}

	/// Put `path` first in the recent list.
	pub fn add_recent(&mut self, path: &Path) {
		let mut list = self.recent();
		list.retain(|p| p != path);
		list.insert(0, path.to_path_buf());
		list.truncate(RECENT_MAX);
		let values = list.iter().map(|p| Value::from(p.display().to_string())).collect();
		self.0.insert("recent".into(), Value::Array(values));
	}

	/// The grid options as the `_prefs` tool options (M7-T06's reader).
	pub fn grid_options(&self) -> Value {
		serde_json::json!({
			"Gridline Every": self.number("grid_every").unwrap_or(100.0),
			"Subdivisions": self.number("subdivisions").unwrap_or(4.0),
		})
	}
}
