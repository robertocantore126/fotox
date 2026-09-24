//! Locations the app reads and writes.
//!
//! Everything Fotox persists lives in one directory so a clean slate is a
//! single delete: `<user data>/Fotox` (`%APPDATA%` on Windows).

use std::path::PathBuf;

use crate::consts::{APP_DIRECTORY_NAME, APP_LOCK_FILE_NAME, APP_PREFERENCES_FILE_NAME};

/// The app's data directory, created if it does not exist yet.
pub(crate) fn app_data_dir() -> PathBuf {
	let dir = user_data_dir().join(APP_DIRECTORY_NAME);
	if !dir.exists()
		&& let Err(error) = std::fs::create_dir_all(&dir)
	{
		tracing::error!("Failed to create {}: {error}", dir.display());
	}
	dir
}

/// Path of the single-instance lock file.
pub(crate) fn lock_file_path() -> PathBuf {
	app_data_dir().join(APP_LOCK_FILE_NAME)
}

/// Path of the preferences file.
pub(crate) fn preferences_file_path() -> PathBuf {
	app_data_dir().join(APP_PREFERENCES_FILE_NAME)
}

/// The platform's per-user data directory.
///
/// Every supported platform defines one (`%APPDATA%`, `~/Library/Application
/// Support`, `$XDG_DATA_HOME`), so a missing one is a broken environment rather
/// than a case to work around.
fn user_data_dir() -> PathBuf {
	dirs::data_dir().expect("every supported platform has a per-user data directory")
}
