//! `fotox-cli gen`: deterministic, photo-like benchmark images
//! (docs/PERFORMANCE.md §3, M1-T01).
//!
//! Every pixel is a pure function of `(x, y, seed)`: large smooth gradients,
//! 4-octave value noise (periods 4096 … 512 px) and per-pixel grain (±1.5 %).
//! That makes the content look photographic and compress about as badly as a
//! real photo — on purpose, so compression numbers are honest — and lets bands
//! be generated in parallel with identical results run to run.
//!
//! Pipeline: bands of 256 rows are generated one at a time with every core
//! (rows in parallel) and handed to a writer thread through a channel of
//! capacity 2, so at most ~4 bands exist at once (one being generated, two
//! queued, one being written).

use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;

use crate::tiffw::TiffWriter;

/// Rows per strip / generated band.
pub const BAND_ROWS: u32 = 256;

/// Generate `out`. Prints progress and the final throughput.
pub fn generate(out: &Path, width: u32, height: u32, bits: u8, seed: u64) -> Result<()> {
	let writer = TiffWriter::create(out, width, height, u16::from(bits), BAND_ROWS)?;
	let strips = writer.strip_count();
	println!(
		"fotox-cli gen: {width} × {height}, {bits}-bit RGB, seed {seed} → {} ({})",
		out.display(),
		if writer.is_big() { "BigTIFF" } else { "TIFF" }
	);

	let start = Instant::now();
	let (tx, rx) = sync_channel::<Vec<u8>>(2);
	let writer_thread = std::thread::Builder::new()
		.name("gen-writer".into())
		.spawn(move || -> Result<u64> {
			let mut writer = writer;
			for band in rx {
				writer.write_strip(&band)?;
			}
			writer.finish()
		})
		.context("cannot start the writer thread")?;

	let content = Content::new(width, height, seed);
	let mut last_report = 0;
	for strip in 0..strips {
		let y0 = strip * BAND_ROWS;
		let rows = BAND_ROWS.min(height - y0);
		let band = content.band(y0, rows, bits);
		if tx.send(band).is_err() {
			// The writer failed; its error is reported below.
			break;
		}
		let percent = (strip + 1) * 100 / strips;
		if percent >= last_report + 10 || strip + 1 == strips {
			last_report = percent;
			println!("  {percent:3} %  ({:.1} s)", start.elapsed().as_secs_f64());
		}
	}
	drop(tx);
	let bytes = writer_thread.join().map_err(|_| anyhow!("the writer thread panicked"))??;

	let seconds = start.elapsed().as_secs_f64();
	println!(
		"done: {:.2} GB in {seconds:.1} s = {:.0} MB/s",
		bytes as f64 / 1e9,
		bytes as f64 / 1e6 / seconds.max(1e-9)
	);
	Ok(())
}

/// The image content, as a function of position.
pub struct Content {
	width: f64,
	height: f64,
	seed: u64,
}

/// Noise octaves: (period in px, amplitude).
const OCTAVES: [(f64, f64); 4] = [(4096.0, 0.16), (2048.0, 0.09), (1024.0, 0.05), (512.0, 0.03)];
/// Grain amplitude, ± fraction of full scale.
const GRAIN: f64 = 0.015;

impl Content {
	pub fn new(width: u32, height: u32, seed: u64) -> Self {
		Self {
			width: f64::from(width),
			height: f64::from(height),
			seed,
		}
	}

	/// One pixel, channels in `0.0..=1.0`.
	pub fn pixel(&self, x: u32, y: u32) -> [f64; 3] {
		let (fx, fy) = (f64::from(x), f64::from(y));
		let (u, v) = (fx / self.width, fy / self.height);
		// Large smooth gradients, a different direction per channel: a sky-like
		// vertical ramp, a warm diagonal, a cool horizontal.
		let base = [
			0.30 + 0.35 * u + 0.10 * v,
			0.25 + 0.30 * (1.0 - v) + 0.10 * u,
			0.20 + 0.25 * (u * 0.5 + v * 0.5),
		];
		let mut out = [0.0; 3];
		for (c, value) in out.iter_mut().enumerate() {
			let channel_seed = self.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(c as u64 * 0x1000_0000_01B3);
			let mut n = 0.0;
			for (octave, (period, amplitude)) in OCTAVES.iter().enumerate() {
				n += amplitude * (value_noise(fx / period, fy / period, channel_seed ^ (octave as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407)) - 0.5) * 2.0;
			}
			let grain = (hash_unit(x, y, channel_seed ^ 0xD6E8_FEB8_6659_FD93) - 0.5) * 2.0 * GRAIN;
			*value = (base[c] + n + grain).clamp(0.0, 1.0);
		}
		out
	}

	/// `rows` rows starting at `y0`, interleaved RGB, 8-bit or little-endian 16-bit.
	pub fn band(&self, y0: u32, rows: u32, bits: u8) -> Vec<u8> {
		let width = self.width as u32;
		let bytes_per_row = width as usize * 3 * usize::from(bits / 8);
		let mut band = vec![0u8; bytes_per_row * rows as usize];
		band.par_chunks_mut(bytes_per_row).enumerate().for_each(|(row, out)| {
			let y = y0 + row as u32;
			for x in 0..width {
				let px = self.pixel(x, y);
				if bits == 16 {
					let i = x as usize * 6;
					for (c, v) in px.iter().enumerate() {
						out[i + 2 * c..i + 2 * c + 2].copy_from_slice(&((v * 65535.0).round() as u16).to_le_bytes());
					}
				} else {
					let i = x as usize * 3;
					for (c, v) in px.iter().enumerate() {
						out[i + c] = (v * 255.0).round() as u8;
					}
				}
			}
		});
		band
	}
}

/// Smooth value noise in `0..1`: random values on the integer lattice,
/// smoothstep-interpolated.
fn value_noise(x: f64, y: f64, seed: u64) -> f64 {
	let (x0, y0) = (x.floor(), y.floor());
	let (tx, ty) = (smooth(x - x0), smooth(y - y0));
	let (ix, iy) = (x0 as i64, y0 as i64);
	let corner = |dx: i64, dy: i64| lattice(ix + dx, iy + dy, seed);
	let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * tx;
	let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * tx;
	top + (bottom - top) * ty
}

fn smooth(t: f64) -> f64 {
	t * t * (3.0 - 2.0 * t)
}

fn lattice(x: i64, y: i64, seed: u64) -> f64 {
	unit(mix(seed
		^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
		^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)))
}

fn hash_unit(x: u32, y: u32, seed: u64) -> f64 {
	unit(mix(seed ^ (u64::from(y) << 32 | u64::from(x))))
}

/// splitmix64 finaliser: a good 64-bit mix, cheap.
fn mix(mut z: u64) -> u64 {
	z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
	z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
	z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
	z ^ (z >> 31)
}

fn unit(z: u64) -> f64 {
	(z >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pixels_depend_only_on_position_and_seed() {
		let a = Content::new(30_000, 30_000, 7);
		let b = Content::new(30_000, 30_000, 7);
		assert_eq!(a.pixel(12_345, 23_456), b.pixel(12_345, 23_456));
		assert_ne!(a.pixel(12_345, 23_456), Content::new(30_000, 30_000, 8).pixel(12_345, 23_456));
		// A band equals the pixels generated one by one.
		let band = a.band(1000, 2, 16);
		let px = a.pixel(3, 1001);
		let i = (30_000 * 6) + 3 * 6;
		assert_eq!(u16::from_le_bytes([band[i], band[i + 1]]), (px[0] * 65535.0).round() as u16);
	}

	#[test]
	fn content_varies_like_a_photo_not_a_flat_fill() {
		let c = Content::new(4096, 4096, 1);
		let values: Vec<f64> = (0..64).map(|i| c.pixel(i * 61, i * 47)[0]).collect();
		let min = values.iter().copied().fold(1.0, f64::min);
		let max = values.iter().copied().fold(0.0, f64::max);
		assert!(max - min > 0.1, "large-scale variation, got {min}..{max}");
		// neighbouring pixels differ by grain, but only a little
		let (p, q) = (c.pixel(100, 100)[1], c.pixel(101, 100)[1]);
		assert!((p - q).abs() < 0.05 && p != q);
	}

	#[test]
	fn writes_a_readable_classic_tiff() {
		let dir = std::env::temp_dir().join("fx-cli-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("gen-small.tif");
		generate(&path, 300, 600, 16, 3).unwrap();
		let info = crate::info::read(&path).unwrap();
		assert_eq!((info.width, info.height, info.bits, info.channels), (300, 600, 16, 3));
		assert!(!info.big);
		assert_eq!(info.compression, 1);
		assert_eq!(info.layout, crate::info::Layout::Strips { rows_per_strip: 256, count: 3 });
		let bytes = std::fs::read(&path).unwrap();
		assert_eq!(bytes.len() as u64, info.file_size);
		// Two runs, same bytes.
		let again = dir.join("gen-small-2.tif");
		generate(&again, 300, 600, 16, 3).unwrap();
		assert_eq!(std::fs::read(&again).unwrap(), bytes);
	}
}
