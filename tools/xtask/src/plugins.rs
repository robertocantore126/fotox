//! `cargo xtask plugins [--watch]`: build the brush plugins (D-096) for
//! wasm32 and copy the `.wasm` files where Fotox looks for them
//! (`FOTOX_PLUGINS`, else `%APPDATA%\Fotox\plugins`). With `--watch`, every
//! save under `plugins/` rebuilds and copies again; the running app reloads
//! the plugin by itself — the "refresh" loop of web development.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

use crate::common;

const POLL: Duration = Duration::from_millis(300);

/// Where the app loads plugins from (the same rule as `fx_engine::plugins::dir`).
fn destination() -> Result<PathBuf> {
	if let Some(dir) = std::env::var_os("FOTOX_PLUGINS") {
		return Ok(PathBuf::from(dir));
	}
	let appdata = std::env::var_os("APPDATA").context("neither FOTOX_PLUGINS nor APPDATA is set")?;
	Ok(PathBuf::from(appdata).join("Fotox").join("plugins"))
}

pub(crate) fn run(watch: bool) -> Result<()> {
	let root = common::workspace_path().join("plugins");
	let dest = destination()?;
	std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;
	tracing::info!("plugins: {} → {}", root.display(), dest.display());
	build_and_copy(&root, &dest)?;
	if !watch {
		return Ok(());
	}
	tracing::info!("watching {} — save a plugin source to rebuild (Ctrl+C to stop)", root.display());
	let mut last = newest_source(&root);
	loop {
		std::thread::sleep(POLL);
		let now = newest_source(&root);
		if now != last {
			last = now;
			// A failed build is reported; the app keeps the last good plugin.
			if let Err(error) = build_and_copy(&root, &dest) {
				tracing::error!("{error:#}");
			}
		}
	}
}

fn build_and_copy(root: &Path, dest: &Path) -> Result<()> {
	let started = std::time::Instant::now();
	let target = root.join("target");
	let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
	let status = std::process::Command::new(cargo)
		.current_dir(root)
		.args(["build", "--release", "--target", "wasm32-unknown-unknown", "--target-dir"])
		.arg(&target)
		.status()
		.context("running cargo for the plugins")?;
	if !status.success() {
		bail!("the plugin build failed ({status})");
	}
	let built = target.join("wasm32-unknown-unknown").join("release");
	let mut copied = Vec::new();
	for entry in std::fs::read_dir(&built)?.flatten() {
		let path = entry.path();
		if path.extension().is_none_or(|e| e != "wasm") {
			continue;
		}
		let name = path.file_name().expect("a file");
		let to = dest.join(name);
		let bytes = std::fs::read(&path)?;
		// Unchanged plugins are left alone: no needless reload in the app.
		if std::fs::read(&to).is_ok_and(|old| old == bytes) {
			continue;
		}
		// Write beside, then rename: the app never reads a half-written file.
		let tmp = dest.join(format!("{}.tmp", name.to_string_lossy()));
		std::fs::write(&tmp, &bytes)?;
		std::fs::rename(&tmp, &to)?;
		copied.push(name.to_string_lossy().into_owned());
	}
	if copied.is_empty() {
		tracing::info!("plugins built in {:.1} s, nothing changed", started.elapsed().as_secs_f64());
	} else {
		tracing::info!("plugins built in {:.1} s, copied {}", started.elapsed().as_secs_f64(), copied.join(", "));
	}
	Ok(())
}

/// The newest modification time of a plugin source (`.rs`, `.toml`, `.json`).
fn newest_source(dir: &Path) -> Option<SystemTime> {
	let mut newest = None;
	for entry in std::fs::read_dir(dir).ok()?.flatten() {
		let path = entry.path();
		let name = entry.file_name();
		if path.is_dir() {
			if name != "target" && name != ".cargo" {
				newest = newest.max(newest_source(&path));
			}
		} else if path.extension().is_some_and(|e| e == "rs" || e == "toml" || e == "json") {
			newest = newest.max(entry.metadata().and_then(|m| m.modified()).ok());
		}
	}
	newest
}
