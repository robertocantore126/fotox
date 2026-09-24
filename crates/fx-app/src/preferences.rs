//! User preferences, stored as JSON next to the instance lock.
//!
//! Reading never fails: a missing or unreadable file means defaults. Writing
//! logs and continues — losing a preference is not worth taking the app down
//! for.

use serde::{Deserialize, Serialize};

use crate::dirs;

/// Preferences that survive a restart.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
	/// Start the UI with CEF's software paint path. Set by
	/// `--disable-ui-acceleration`, and by the app itself when the accelerated
	/// path never presents a frame (see `app.rs`).
	pub(crate) disable_ui_acceleration: bool,
}

/// Read the preferences, or defaults when there are none to read.
pub(crate) fn read() -> Preferences {
	let Ok(data) = std::fs::read_to_string(dirs::preferences_file_path()) else {
		return Preferences::default();
	};
	match serde_json::from_str(&data) {
		Ok(preferences) => preferences,
		Err(error) => {
			tracing::error!("Ignoring unreadable preferences: {error}");
			Preferences::default()
		}
	}
}

/// Write the preferences back to disk.
pub(crate) fn write(preferences: &Preferences) {
	let data = match serde_json::to_string_pretty(preferences) {
		Ok(data) => data,
		Err(error) => {
			tracing::error!("Failed to serialize preferences: {error}");
			return;
		}
	};
	if let Err(error) = std::fs::write(dirs::preferences_file_path(), data) {
		tracing::error!("Failed to write preferences: {error}");
	}
}

/// Read the preferences, change them, and write them back.
pub(crate) fn modify(f: impl FnOnce(&mut Preferences)) {
	let mut preferences = read();
	f(&mut preferences);
	write(&preferences);
}
