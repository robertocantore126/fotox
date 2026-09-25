//! The Windows bundle: `target/<profile>/Fotox/`.
//!
//! CEF on Windows loads `libcef.dll` and its resources from the directory of
//! the running executable, so `fotox.exe` cannot be run from `target/debug`
//! directly — it has to be copied next to the runtime. This module performs
//! that copy, leaving out the parts of the distribution the app never loads.

use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::common;

/// Folder inside the profile directory holding the runnable app.
const BUNDLE_DIR: &str = "Fotox";

/// The app binary; matches `[[bin]] name` in `crates/fx-app/Cargo.toml`.
const APP_EXE: &str = "fotox.exe";

/// Root directories of the CEF distribution that are build-time only.
const EXCLUDED_ROOT_DIRS: [&str; 3] = ["cmake", "include", "libcef_dll"];

/// Root files of the CEF distribution that are build-time only.
const EXCLUDED_ROOT_FILES: [&str; 6] = [
	"archive.json",
	"CMakeLists.txt",
	"bootstrapc.exe",
	"bootstrap.exe",
	"libcef.lib",
	"CREDITS.html",
];

/// CEF ships a locale pack per language; only this one is kept.
const KEPT_LOCALE: &str = "locales/en-US.pak";

/// Paths the bundler puts into the bundle directory itself rather than taking
/// from CEF. The prune pass must leave these alone.
const BUNDLER_OWNED: [&str; 1] = [APP_EXE];

/// Build `fx-app` and assemble `target/<profile>/Fotox/`, returning the path of
/// the bundled executable.
pub(crate) fn bundle(profile: &str) -> Result<PathBuf> {
	// Dev builds read the UI from ./ui (GRAPHITE_RESOURCES, set by
	// .cargo/config.toml, reaches only processes started through cargo). A
	// release exe is started by double-click too, so it carries the UI inside.
	let features: &[&str] = if profile == "dev" { &[] } else { &["embedded_resources"] };
	common::cargo_build("fx-app", profile, features)?;

	let profile_dir = common::target_dir().join(common::profile_dir_name(profile));
	let exe_src = profile_dir.join(APP_EXE);
	if !exe_src.is_file() {
		anyhow::bail!("cargo build did not produce {}", exe_src.display());
	}

	let out_dir = profile_dir.join(BUNDLE_DIR);
	let cef_src = common::cef_runtime_path()?;
	tracing::info!("bundling {} into {}", cef_src.display(), out_dir.display());
	fs::create_dir_all(&out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;

	let mut stats = sync_cef(&cef_src, &out_dir)?;

	// Copied after the mirror and exempt from pruning, so a rebuild that does
	// not change the executable is still a no-op.
	let exe_dst = out_dir.join(APP_EXE);
	copy_file_if_changed(&exe_src, &exe_dst, &mut stats)?;

	tracing::info!(
		"{} files copied, {} unchanged, {} stale removed; app at {}",
		stats.copied,
		stats.unchanged,
		stats.removed,
		exe_dst.display()
	);
	if stats.unaligned_timestamps > 0 {
		tracing::warn!(
			"{} files could not keep their timestamp and will be copied again next time",
			stats.unaligned_timestamps
		);
	}

	Ok(exe_dst)
}

/// Counters for one bundling pass, reported so a slow or surprising run can be
/// explained without guessing.
#[derive(Default)]
struct Stats {
	/// Files whose contents or timestamp differed and were rewritten.
	copied: usize,
	/// Files already up to date, left alone.
	unchanged: usize,
	/// Files in the bundle directory that the CEF distribution no longer
	/// provides, or that the filter excludes.
	removed: usize,
	/// Copies whose timestamp could not be aligned with the source, so the next
	/// run has to copy them again. Not an error, only a lost optimisation.
	unaligned_timestamps: usize,
}

/// Bring `dst` in sync with the CEF distribution at `src`.
///
/// Copying is incremental — the reference bundler wiped the directory first,
/// which makes every iteration pay for a ~600 MB copy — and stale files are
/// discarded, so a CEF upgrade cannot leave an old `libcef.dll` behind.
fn sync_cef(src: &Path, dst: &Path) -> Result<Stats> {
	let mut stats = Stats::default();
	copy_needed(src, Path::new(""), dst, &mut stats)?;
	prune_stale(src, dst, &mut stats)?;
	Ok(stats)
}

/// Copy every needed file under `src_dir` that is missing or out of date.
fn copy_needed(src_dir: &Path, rel_dir: &Path, dst_root: &Path, stats: &mut Stats) -> Result<()> {
	let entries = fs::read_dir(src_dir).with_context(|| format!("cannot read {}", src_dir.display()))?;
	for entry in entries {
		let entry = entry.with_context(|| format!("cannot read an entry of {}", src_dir.display()))?;
		let rel = rel_dir.join(entry.file_name());
		if !is_needed(&relative_key(&rel)) {
			continue;
		}

		let src = entry.path();
		let dst = dst_root.join(&rel);
		if src.is_dir() {
			fs::create_dir_all(&dst).with_context(|| format!("cannot create {}", dst.display()))?;
			copy_needed(&src, &rel, dst_root, stats)?;
		} else {
			copy_file_if_changed(&src, &dst, stats)?;
		}
	}
	Ok(())
}

/// Delete everything in the bundle directory that the CEF distribution does not
/// provide, then the directories left empty.
fn prune_stale(cef_src: &Path, dst_root: &Path, stats: &mut Stats) -> Result<()> {
	let mut entries = Vec::new();
	collect_entries(dst_root, Path::new(""), &mut entries)?;

	for (rel, is_dir) in &entries {
		if *is_dir {
			continue;
		}
		let key = relative_key(rel);
		if BUNDLER_OWNED.contains(&key.as_str()) || (is_needed(&key) && cef_src.join(rel).is_file()) {
			continue;
		}

		let path = dst_root.join(rel);
		fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
		stats.removed += 1;
	}

	// Deepest first, so children are gone before their parent is considered.
	// Removal only succeeds while a directory is empty: one that still holds
	// needed files stays behind, which is the correct outcome, so a failure is
	// deliberately not an error.
	let mut dirs: Vec<&PathBuf> = entries.iter().filter(|(_, is_dir)| *is_dir).map(|(rel, _)| rel).collect();
	dirs.sort_by_key(|rel| Reverse(rel.components().count()));
	for rel in dirs {
		let _ = fs::remove_dir(dst_root.join(rel));
	}

	Ok(())
}

/// Collect every entry under `dir` as `(path relative to the bundle root, is a directory)`.
fn collect_entries(dir: &Path, rel_dir: &Path, out: &mut Vec<(PathBuf, bool)>) -> Result<()> {
	let entries = fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
	for entry in entries {
		let entry = entry.with_context(|| format!("cannot read an entry of {}", dir.display()))?;
		let src = entry.path();
		let rel = rel_dir.join(entry.file_name());
		let is_dir = src.is_dir();
		out.push((rel.clone(), is_dir));
		if is_dir {
			collect_entries(&src, &rel, out)?;
		}
	}
	Ok(())
}

/// Copy `src` over `dst` unless `dst` already has the same size and timestamp.
fn copy_file_if_changed(src: &Path, dst: &Path, stats: &mut Stats) -> Result<()> {
	let src_meta = fs::metadata(src).with_context(|| format!("cannot read {}", src.display()))?;

	if let Ok(dst_meta) = fs::metadata(dst) {
		let same_size = dst_meta.len() == src_meta.len();
		let same_time = dst_meta.modified().ok() == src_meta.modified().ok();
		if same_size && same_time {
			stats.unchanged += 1;
			return Ok(());
		}
	}

	fs::copy(src, dst).with_context(|| format!("cannot copy {} to {}", src.display(), dst.display()))?;

	// Give the copy the source's timestamp so the next bundle compares instead
	// of copying again. Windows preserves it through `fs::copy`; setting it
	// explicitly makes the skip check depend on our own behaviour rather than
	// on the platform's.
	if !align_modified_time(&src_meta, dst) {
		stats.unaligned_timestamps += 1;
	}

	stats.copied += 1;
	Ok(())
}

/// Set `dst`'s modification time to `src_meta`'s, reporting whether it worked.
fn align_modified_time(src_meta: &fs::Metadata, dst: &Path) -> bool {
	let Ok(modified) = src_meta.modified() else {
		return false;
	};
	let Ok(file) = fs::File::options().write(true).open(dst) else {
		return false;
	};
	file.set_modified(modified).is_ok()
}

/// Whether a file in the CEF distribution is needed at runtime. This is the
/// `remove_unnecessary_cef_files` list of the ported Graphite bundler, applied
/// as a filter so the excluded files are never copied in the first place.
fn is_needed(rel: &str) -> bool {
	if EXCLUDED_ROOT_DIRS.iter().any(|dir| rel == *dir || rel.starts_with(&format!("{dir}/"))) {
		return false;
	}
	if EXCLUDED_ROOT_FILES.contains(&rel) {
		return false;
	}
	if rel.starts_with("locales/") {
		return rel == KEPT_LOCALE;
	}
	true
}

/// A path relative to the CEF root or the bundle root, always separated by
/// `/`, so it can be compared against the lists above on any platform.
fn relative_key(rel: &Path) -> String {
	rel.to_string_lossy().replace('\\', "/")
}
