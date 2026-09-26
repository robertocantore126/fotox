//! Color Lookup files (M12-T05, D-086): `.cube` (Adobe / Resolve, 1D and 3D)
//! and `.3dl` (Autodesk / Lustre). The result is always a 3D table (red
//! fastest) of straight RGB `0..=1`: a 1D table becomes a 17³ one.
//!
//! FAST: `DOMAIN_MIN` / `DOMAIN_MAX` other than 0 / 1 are ignored; a `.3dl`
//! input shaper line is skipped (its outputs are scaled by the largest value).

use crate::IoError;

/// A parsed LUT: `size³` entries, red fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
	pub size: u32,
	pub table: Vec<[f32; 3]>,
}

/// Parse a LUT file's text; `name` picks the format by its extension.
pub fn parse(name: &str, text: &str) -> Result<Lut3d, IoError> {
	if name.to_ascii_lowercase().ends_with(".3dl") {
		parse_3dl(text)
	} else {
		parse_cube(text)
	}
}

fn numbers(line: &str) -> Option<Vec<f32>> {
	line.split_whitespace().map(|t| t.parse::<f32>().ok()).collect()
}

pub fn parse_cube(text: &str) -> Result<Lut3d, IoError> {
	let (mut size3, mut size1) = (0usize, 0usize);
	let mut rows: Vec<[f32; 3]> = Vec::new();
	for line in text.lines() {
		let line = line.trim();
		if line.is_empty() || line.starts_with('#') {
			continue;
		}
		let mut words = line.split_whitespace();
		match words.next() {
			Some("LUT_3D_SIZE") => size3 = words.next().and_then(|v| v.parse().ok()).unwrap_or(0),
			Some("LUT_1D_SIZE") => size1 = words.next().and_then(|v| v.parse().ok()).unwrap_or(0),
			Some("TITLE" | "DOMAIN_MIN" | "DOMAIN_MAX" | "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE") => {}
			_ => {
				if let Some(v) = numbers(line)
					&& v.len() >= 3
				{
					rows.push([v[0], v[1], v[2]]);
				}
			}
		}
	}
	if size3 >= 2 {
		if rows.len() < size3 * size3 * size3 {
			return Err(IoError::Decode(format!("the .cube has {} rows, {} expected", rows.len(), size3.pow(3))));
		}
		rows.truncate(size3.pow(3));
		return Ok(Lut3d {
			size: size3 as u32,
			table: rows,
		});
	}
	if size1 >= 2 && rows.len() >= size1 {
		// A 1D table per channel → a 17³ cube.
		let n = 17usize;
		let at = |c: usize, x: f32| -> f32 {
			let p = x * (size1 - 1) as f32;
			let i = (p.floor() as usize).min(size1 - 2);
			let t = p - i as f32;
			rows[i][c] + (rows[i + 1][c] - rows[i][c]) * t
		};
		let mut table = Vec::with_capacity(n * n * n);
		for b in 0..n {
			for g in 0..n {
				for r in 0..n {
					let f = |v: usize| v as f32 / (n - 1) as f32;
					table.push([at(0, f(r)), at(1, f(g)), at(2, f(b))]);
				}
			}
		}
		return Ok(Lut3d { size: n as u32, table });
	}
	Err(IoError::Decode("not a .cube LUT (no LUT_3D_SIZE / LUT_1D_SIZE)".into()))
}

pub fn parse_3dl(text: &str) -> Result<Lut3d, IoError> {
	let lines: Vec<Vec<f32>> = text
		.lines()
		.map(str::trim)
		.filter(|l| !l.is_empty() && !l.starts_with('#'))
		.filter_map(numbers)
		.collect();
	// The first line may be the input shaper (N values, not triplets).
	let body: Vec<[f32; 3]> = lines.iter().filter(|v| v.len() == 3).map(|v| [v[0], v[1], v[2]]).collect();
	let n = (body.len() as f64).cbrt().round() as usize;
	if n < 2 || n * n * n != body.len() {
		return Err(IoError::Decode(format!("a .3dl with {} entries is not a cube", body.len())));
	}
	let max = body.iter().flatten().fold(1.0f32, |m, v| m.max(*v));
	let scale = if max > 4095.0 {
		65535.0
	} else if max > 1023.0 {
		4095.0
	} else if max > 1.0 {
		1023.0
	} else {
		1.0
	};
	// .3dl lists blue fastest: reorder to red fastest.
	let mut table = vec![[0.0f32; 3]; n * n * n];
	for (i, v) in body.iter().enumerate() {
		let (r, g, b) = (i / (n * n), (i / n) % n, i % n);
		table[r + n * (g + n * b)] = v.map(|c| c / scale);
	}
	Ok(Lut3d { size: n as u32, table })
}
