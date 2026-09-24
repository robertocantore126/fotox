//! `fotox-cli` — the engine without a window.
//!
//! Used for: generating the benchmark documents, measuring import/trim/render
//! speed, and (M8) batch processing with recorded actions.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fotox-cli", version, about = "Headless Fotox")]
struct Cli {
	#[command(subcommand)]
	command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
	/// Write a synthetic test image (see docs/PERFORMANCE.md §3). M1-T01
	Gen {
		/// Output .tif path
		out: PathBuf,
		#[arg(long, default_value_t = 30_000)]
		width: u32,
		#[arg(long, default_value_t = 30_000)]
		height: u32,
		/// 8 or 16
		#[arg(long, default_value_t = 16)]
		bits: u8,
	},
	/// Print dimensions, depth, profile and tile statistics of an image. M1-T01
	Info { path: PathBuf },
	/// Run a benchmark scenario from docs/PERFORMANCE.md §4 and append the result to bench/results.csv. M1-T10
	Bench {
		scenario: String,
		#[arg(long)]
		file: Option<PathBuf>,
	},
}

fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.init();
	match Cli::parse().command {
		Cmd::Gen { .. } => todo!("M1-T01"),
		Cmd::Info { .. } => todo!("M1-T01"),
		Cmd::Bench { .. } => todo!("M1-T10"),
	}
}
