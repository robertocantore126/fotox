//! `xtask` — the Fotox build helper.
//!
//! One command builds `fotox.exe`, puts it next to the CEF runtime it needs,
//! and starts it:
//!
//! ```text
//! cargo xtask bundle [--release]        # assemble target/<profile>/Fotox/
//! cargo xtask run [--release] [-- args] # assemble, then launch fotox.exe
//! ```
//!
//! CEF does not load from a path: `libcef.dll`, its `.pak` resources and the
//! helper executables must sit beside the app binary. The engine crates build
//! without any of this; only `fx-app` and these commands need the runtime.
//!
//! Ported from `reference/graphite-desktop/bundle/src/` (see `docs/GRAPHITE.md`
//! §2 and task `M0-T02`).

mod common;

#[cfg(target_os = "windows")]
mod win;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", version, about = "Fotox build helper")]
struct Cli {
	#[command(subcommand)]
	command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
	/// Build fx-app and assemble target/<profile>/Fotox/ (CEF runtime + fotox.exe)
	Bundle {
		/// Use the release profile instead of dev
		#[arg(long)]
		release: bool,
	},
	/// Assemble the bundle, then launch fotox.exe and wait for it
	Run {
		/// Use the release profile instead of dev
		#[arg(long)]
		release: bool,
		/// Arguments passed through to fotox.exe, after `--`
		#[arg(last = true)]
		args: Vec<String>,
	},
}

fn main() -> anyhow::Result<()> {
	// A build tool should say what it is doing without RUST_LOG set, but still
	// honour it: RUST_LOG=off makes the bundler silent.
	tracing_subscriber::fmt()
		.with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
		.init();

	dispatch(Cli::parse().command)
}

/// Bundle, then either stop or launch, depending on the subcommand.
#[cfg(target_os = "windows")]
fn dispatch(command: Cmd) -> anyhow::Result<()> {
	let release = match &command {
		Cmd::Bundle { release } | Cmd::Run { release, .. } => *release,
	};

	let exe = win::bundle(common::profile_name(release))?;
	tracing::info!("bundle ready: {}", exe.display());

	match command {
		Cmd::Bundle { .. } => Ok(()),
		Cmd::Run { args, .. } => launch(&exe, &args),
	}
}

/// Bundling is Windows-only for now (decision D-014). A clear error beats an
/// unimplemented path.
#[cfg(not(target_os = "windows"))]
fn dispatch(command: Cmd) -> anyhow::Result<()> {
	let _ = command;
	anyhow::bail!("`cargo xtask bundle` is only implemented for Windows (see docs/DECISIONS.md, D-014)")
}

/// Run the bundled app and wait for it, propagating a failure.
#[cfg(target_os = "windows")]
fn launch(exe: &std::path::Path, args: &[String]) -> anyhow::Result<()> {
	let mut line = format!("running {}", exe.display());
	if !args.is_empty() {
		line.push(' ');
		line.push_str(&args.join(" "));
	}
	tracing::info!("{line}");
	let status = std::process::Command::new(exe).args(args).status()?;
	if !status.success() {
		anyhow::bail!("fotox.exe exited with {status}");
	}
	Ok(())
}
