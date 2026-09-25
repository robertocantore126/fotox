//! Platform-independent helpers for the bundler.
//!
//! Nothing here touches the filesystem layout of any particular OS: the
//! Windows specifics live in `win.rs`, so a future macOS/Linux bundle only
//! needs a sibling module plus a `dispatch` arm.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Environment variable `cef-dll-sys` reads to decide where to download CEF.
/// `.cargo/config.toml` sets it to a workspace-relative path.
const CEF_PATH_ENV: &str = "CEF_PATH";

/// CEF location when `CEF_PATH` is not set, relative to the workspace root.
/// Mirrors the `[env]` entry in `.cargo/config.toml`.
const DEFAULT_CEF_PATH: &str = "third_party/cef";

/// The workspace root.
///
/// Taken from `CARGO_MANIFEST_DIR` rather than the current directory, so the
/// bundler works the same no matter where cargo was invoked from.
pub(crate) fn workspace_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.and_then(Path::parent)
		.map(Path::to_path_buf)
		.expect("xtask lives at <workspace>/tools/xtask, so it always has two parent directories")
}

/// The Cargo target directory, honouring `CARGO_TARGET_DIR`.
///
/// A relative `CARGO_TARGET_DIR` is resolved against the workspace root, and
/// `cargo_build` passes this resolved path down, so the directory the bundle
/// is written to is always the one the build wrote to.
pub(crate) fn target_dir() -> PathBuf {
	match std::env::var_os("CARGO_TARGET_DIR") {
		Some(dir) if !dir.is_empty() => {
			let dir = PathBuf::from(dir);
			if dir.is_absolute() { dir } else { workspace_path().join(dir) }
		}
		_ => workspace_path().join("target"),
	}
}

/// The Cargo profile name for a `--release` flag.
pub(crate) fn profile_name(release: bool) -> &'static str {
	if release { "release" } else { "dev" }
}

/// The directory Cargo writes a profile's artefacts to.
///
/// The `dev` profile is the only one whose directory name differs from the
/// profile name.
pub(crate) fn profile_dir_name(profile: &str) -> &str {
	if profile == "dev" { "debug" } else { profile }
}

/// Build one workspace package with `cargo build`, with `features` on.
///
/// The build runs in the workspace root so the nested cargo reads the same
/// `.cargo/config.toml`; `CARGO_TARGET_DIR` is pinned to the directory we
/// already resolved, so the artefacts cannot land somewhere unexpected.
pub(crate) fn cargo_build(package: &str, profile: &str, features: &[&str]) -> Result<()> {
	let target_dir = target_dir();
	tracing::info!("cargo build -p {package} --profile {profile} --features {features:?}");

	let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
	let mut args = vec!["build", "--package", package, "--profile", profile];
	let features = features.join(",");
	if !features.is_empty() {
		args.extend(["--features", features.as_str()]);
	}
	let status = Command::new(&cargo)
		.args(&args)
		.current_dir(workspace_path())
		.env("CARGO_TARGET_DIR", &target_dir)
		.status()
		.with_context(|| format!("failed to run {}", cargo.to_string_lossy()))?;

	if !status.success() {
		anyhow::bail!("cargo build --package {package} failed with {status}");
	}
	Ok(())
}

/// The CEF runtime directory that `cef-dll-sys` downloaded into.
///
/// `cef-dll-sys` unpacks the distribution as
/// `$CEF_PATH/<cef version>/<os_arch>/`, so that is the layout searched here.
/// Both levels are expected to hold exactly one directory: more than one means
/// several CEF builds were downloaded and picking one silently would be a
/// guess, which is not the bundler's job.
pub(crate) fn cef_runtime_path() -> Result<PathBuf> {
	let root = match std::env::var_os(CEF_PATH_ENV) {
		Some(dir) if !dir.is_empty() => {
			let dir = PathBuf::from(dir);
			if dir.is_absolute() { dir } else { workspace_path().join(dir) }
		}
		_ => workspace_path().join(DEFAULT_CEF_PATH),
	};

	if !root.is_dir() {
		anyhow::bail!(
			"CEF is not downloaded yet: {} does not exist.\n\
			 Build the app once to fetch it: `cargo build -p fx-app`",
			root.display()
		);
	}

	let version = single_subdir(&root, "CEF version")?;
	single_subdir(&version, "CEF platform")
}

/// The only directory inside `parent`, or an error explaining what was found.
fn single_subdir(parent: &Path, what: &str) -> Result<PathBuf> {
	let mut dirs = Vec::new();
	let entries = std::fs::read_dir(parent).with_context(|| format!("cannot read {}", parent.display()))?;
	for entry in entries {
		let entry = entry.with_context(|| format!("cannot read an entry of {}", parent.display()))?;
		let path = entry.path();
		if path.is_dir() {
			dirs.push(path);
		}
	}
	dirs.sort();

	match dirs.len() {
		1 => Ok(dirs.remove(0)),
		0 => anyhow::bail!("no {what} directory in {}", parent.display()),
		n => anyhow::bail!(
			"{n} {what} directories in {}: {} — remove the ones you do not want to bundle",
			parent.display(),
			dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")
		),
	}
}
