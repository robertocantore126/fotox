//! `fotox-cli bench`: engine-only benchmarks (docs/PERFORMANCE.md §4, M1-T10).
//!
//! Every result is appended as a row to `bench/results.csv`:
//! `date,commit,scenario,document,metric,value,unit,notes`.
//!
//! Scenarios:
//! * `import --file F` — S1 engine part: file → tiles (MB/s of file), next to
//!   the raw sequential read speed of the same file and the ratio.
//! * `trim` — hot → warm: LZ4 compression throughput, then decompression.
//! * `scratch` — hot → cold: compression + scratch-file write, then read-back.
//! * `mips [--file F]` — full mip pyramid of F (or of a generated 8192² image).
//!
//! `trim` and `scratch` use `gen`'s photo-like content, so compression ratios
//! are as honest as the benchmark files (not flattering noise-free tiles).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use fx_tiles::{PixelFormat, TILE_SIZE, TileBuffer, TileClass, TileStore, TileStoreConfig, TiledImage};
use rayon::prelude::*;

use crate::r#gen::Content;

/// One measured value.
pub struct Row {
	pub scenario: &'static str,
	pub document: String,
	pub metric: &'static str,
	pub value: f64,
	pub unit: &'static str,
	pub notes: String,
}

/// Run `scenario` and append its rows to `csv`.
pub fn run(scenario: &str, file: Option<&Path>, csv: &Path) -> Result<()> {
	let rows = match scenario {
		"import" => import(file.context("`bench import` needs --file")?)?,
		"trim" => trim()?,
		"scratch" => scratch()?,
		"mips" => mips(file)?,
		other => bail!("unknown scenario `{other}` (import, trim, scratch, mips)"),
	};
	let commit = git_commit();
	let date = today();
	for row in &rows {
		println!("{:<8} {:<28} {:>12.2} {}", row.scenario, row.metric, row.value, row.unit);
	}
	append(csv, &date, &commit, &rows)?;
	println!("appended {} row(s) to {}", rows.len(), csv.display());
	Ok(())
}

fn store(scratch: &str, hot: u64, warm: u64) -> Result<TileStore> {
	let dir = std::env::temp_dir().join("fotox-bench").join(scratch);
	let mut config = TileStoreConfig::reference_machine(dir);
	config.hot_budget = hot;
	config.warm_budget = warm;
	config.background_trim = false;
	TileStore::new(config).context("cannot create the tile store")
}

fn file_name(path: &Path) -> String {
	path.file_name()
		.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

// ------------------------------------------------------------------ import

fn import(path: &Path) -> Result<Vec<Row>> {
	let size = std::fs::metadata(path)?.len() as f64;
	let doc = file_name(path);

	// Raw sequential read first: it also warms nothing the import can use
	// unfairly on a file larger than RAM, and on a smaller one the note says so.
	let start = Instant::now();
	let mut f = File::open(path)?;
	let mut buf = vec![0u8; 8 << 20];
	while f.read(&mut buf)? > 0 {}
	let raw = size / 1e6 / start.elapsed().as_secs_f64();

	let store = store("import", 5 << 30, 3 << 30)?;
	let start = Instant::now();
	let imported = fx_io::import_file(path, &store, &mut |_| true).map_err(|e| anyhow::anyhow!("{e}"))?;
	let seconds = start.elapsed().as_secs_f64();
	let speed = size / 1e6 / seconds;
	let note = format!(
		"{} × {} {:?}; file {:.2} GB; the file may be in the OS cache after the raw read",
		imported.width,
		imported.height,
		imported.depth,
		size / 1e9
	);
	Ok(vec![
		row("import", &doc, "raw_read", raw, "MB/s", &note),
		row("import", &doc, "import", speed, "MB/s", &note),
		row("import", &doc, "import_seconds", seconds, "s", &note),
		row("import", &doc, "import_vs_raw", 100.0 * speed / raw, "%", "target ≥ 80 %"),
	])
}

// ------------------------------------------------------------------ trim / scratch

/// `count` photo-like Rgba16 tiles (from `gen`'s content function).
fn photo_tiles(count: usize) -> Vec<TileBuffer> {
	let content = Content::new(30_000, 30_000, 42);
	(0..count)
		.into_par_iter()
		.map(|i| {
			let (tx, ty) = ((i % 100) as u32, (i / 100) as u32);
			let mut tile = TileBuffer::zeroed(PixelFormat::Rgba16);
			let px = tile.as_u16_mut();
			for y in 0..TILE_SIZE {
				for x in 0..TILE_SIZE {
					let v = content.pixel(tx * TILE_SIZE + x, ty * TILE_SIZE + y);
					let o = ((y * TILE_SIZE + x) * 4) as usize;
					for c in 0..3 {
						px[o + c] = (v[c] * 65535.0).round() as u16;
					}
					px[o + 3] = 65535;
				}
			}
			tile
		})
		.collect()
}

const BENCH_TILES: usize = 512; // 256 MiB of Rgba16

fn trim() -> Result<Vec<Row>> {
	let tiles = photo_tiles(BENCH_TILES);
	let bytes = (BENCH_TILES * PixelFormat::Rgba16.tile_bytes()) as f64;
	// Hot budget far below the data, warm budget far above: trim compresses
	// everything into the warm tier and writes nothing to disk.
	let store = store("trim", 1 << 20, 8 << 30)?;
	let handles: Vec<_> = tiles.into_iter().map(|t| store.insert(t, TileClass::Authoritative)).collect();
	let start = Instant::now();
	store.trim();
	let compress = bytes / 1e6 / start.elapsed().as_secs_f64();
	let warm = store.stats();
	let start = Instant::now();
	handles.par_iter().try_for_each(|h| store.get(h).map(|_| ()))?;
	let decompress = bytes / 1e6 / start.elapsed().as_secs_f64();
	let note = format!("{BENCH_TILES} photo-like Rgba16 tiles; stats after trim: {warm:?}");
	Ok(vec![
		row("trim", "generated", "lz4_compress", compress, "MB/s", &note),
		row("trim", "generated", "lz4_decompress", decompress, "MB/s", &note),
	])
}

fn scratch() -> Result<Vec<Row>> {
	let tiles = photo_tiles(BENCH_TILES);
	let bytes = (BENCH_TILES * PixelFormat::Rgba16.tile_bytes()) as f64;
	// Both RAM tiers tiny: trim compresses and spills everything to disk.
	let store = store("scratch", 1 << 20, 1 << 20)?;
	let handles: Vec<_> = tiles.into_iter().map(|t| store.insert(t, TileClass::Authoritative)).collect();
	let start = Instant::now();
	store.trim();
	let spill = bytes / 1e6 / start.elapsed().as_secs_f64();
	let on_disk = store.scratch_used();
	let start = Instant::now();
	for h in &handles {
		store.get(h)?;
		// Keep RAM bounded like the app would.
		store.trim();
	}
	let read_back = bytes / 1e6 / start.elapsed().as_secs_f64();
	let note = format!("{BENCH_TILES} photo-like Rgba16 tiles; {:.0} MB on disk", on_disk as f64 / 1e6);
	Ok(vec![
		row("scratch", "generated", "spill", spill, "MB/s", &note),
		row("scratch", "generated", "read_back", read_back, "MB/s", &note),
	])
}

// ------------------------------------------------------------------ mips

fn mips(file: Option<&Path>) -> Result<Vec<Row>> {
	let store = store("mips", 5 << 30, 3 << 30)?;
	let (doc, mut image) = match file {
		Some(path) => {
			let imported = fx_io::import_file(path, &store, &mut |_| true).map_err(|e| anyhow::anyhow!("{e}"))?;
			(file_name(path), imported.image)
		}
		None => {
			let side = 8192;
			let mut image = TiledImage::new(side, side, PixelFormat::Rgba16);
			let per_side = (side / TILE_SIZE) as usize;
			let tiles = photo_tiles(per_side * per_side);
			for (i, tile) in tiles.into_iter().enumerate() {
				image.put_buffer(&store, (i % per_side) as u32, (i / per_side) as u32, tile);
			}
			("generated 8192²".to_string(), image)
		}
	};
	let start = Instant::now();
	fx_engine::mips::ensure_all_mips(&mut image, &store)?;
	let seconds = start.elapsed().as_secs_f64();
	let note = format!("{} × {}, {} levels", image.width(), image.height(), image.level_count());
	Ok(vec![row("mips", &doc, "full_pyramid", seconds, "s", &note)])
}

// ------------------------------------------------------------------ output

fn row(scenario: &'static str, document: &str, metric: &'static str, value: f64, unit: &'static str, notes: &str) -> Row {
	Row {
		scenario,
		document: document.to_string(),
		metric,
		value,
		unit,
		notes: notes.to_string(),
	}
}

fn append(csv: &Path, date: &str, commit: &str, rows: &[Row]) -> Result<()> {
	let new = !csv.exists();
	if let Some(dir) = csv.parent() {
		std::fs::create_dir_all(dir)?;
	}
	let mut f = OpenOptions::new()
		.create(true)
		.append(true)
		.open(csv)
		.with_context(|| format!("cannot open {}", csv.display()))?;
	if new {
		writeln!(f, "date,commit,scenario,document,metric,value,unit,notes")?;
	}
	for r in rows {
		writeln!(
			f,
			"{date},{commit},{},{},{},{:.3},{},{}",
			quote(r.scenario),
			quote(&r.document),
			quote(r.metric),
			r.value,
			quote(r.unit),
			quote(&r.notes)
		)?;
	}
	Ok(())
}

/// CSV field: quoted when it contains a comma, quote or newline.
fn quote(field: &str) -> String {
	if field.contains([',', '"', '\n']) {
		format!("\"{}\"", field.replace('"', "\"\""))
	} else {
		field.to_string()
	}
}

fn git_commit() -> String {
	std::process::Command::new("git")
		.args(["rev-parse", "--short", "HEAD"])
		.output()
		.ok()
		.filter(|o| o.status.success())
		.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
		.unwrap_or_else(|| "unknown".into())
}

/// Today's date (UTC) as `YYYY-MM-DD`, without a date crate.
fn today() -> String {
	let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
	let (y, m, d) = civil_from_days(days);
	format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's days → civil date algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
	let z = z + 719_468;
	let era = z.div_euclid(146_097);
	let doe = z.rem_euclid(146_097);
	let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
	let y = yoe + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
	let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
	(if m <= 2 { y + 1 } else { y }, m, d)
}

/// Default results file: `bench/results.csv` at the repository root.
pub fn default_csv() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/results.csv")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dates_and_quoting() {
		assert_eq!(civil_from_days(0), (1970, 1, 1));
		assert_eq!(civil_from_days(20_721), (2026, 9, 25));
		assert_eq!(quote("plain"), "plain");
		assert_eq!(quote("a, b"), "\"a, b\"");
		assert_eq!(quote("say \"x\""), "\"say \"\"x\"\"\"");
	}

	#[test]
	fn rows_append_with_a_header_once() {
		let csv = std::env::temp_dir().join("fx-cli-tests").join("results.csv");
		let _ = std::fs::remove_file(&csv);
		let rows = [row("trim", "generated", "lz4_compress", 1234.5678, "MB/s", "n, with comma")];
		append(&csv, "2026-09-25", "abc1234", &rows).unwrap();
		append(&csv, "2026-09-25", "abc1234", &rows).unwrap();
		let text = std::fs::read_to_string(&csv).unwrap();
		let lines: Vec<&str> = text.lines().collect();
		assert_eq!(lines.len(), 3, "{text}");
		assert_eq!(lines[1], "2026-09-25,abc1234,trim,generated,lz4_compress,1234.568,MB/s,\"n, with comma\"");
	}
}
