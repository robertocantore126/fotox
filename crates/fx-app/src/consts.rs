//! Names and file names the app uses.

/// Window title and app name.
pub(crate) const APP_NAME: &str = "Fotox";

/// Windows AppUserModelID: groups the app's taskbar entries and lets a pinned
/// shortcut keep working across restarts.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) const APP_ID: &str = "art.fotox.Fotox";

/// Folder created inside the platform's user data directory.
#[cfg(target_os = "linux")]
pub(crate) const APP_DIRECTORY_NAME: &str = "fotox";
/// Folder created inside the platform's user data directory.
#[cfg(not(target_os = "linux"))]
pub(crate) const APP_DIRECTORY_NAME: &str = "Fotox";

/// Lock file holding the process id of the running instance.
pub(crate) const APP_LOCK_FILE_NAME: &str = "instance.lock";

/// File the user preferences are stored in.
pub(crate) const APP_PREFERENCES_FILE_NAME: &str = "preferences.json";
