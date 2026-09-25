//! The display transform as a 33³ 3D LUT (M4-T02).
//!
//! A 3D LUT is how the document colours reach the screen: the viewport shader
//! samples it per fragment (three fetches + interpolation), which costs the
//! same whatever the profiles are. The table is built on the CPU from an lcms2
//! transform, one node per sample, and stored as f16 so the GPU can hold it in
//! an `Rgba16Float` 3D texture.
//!
//! Coordinates (docs/tasks/SNIPPETS.md §14): node `i` of an axis sits at the
//! centre of texel `i`, so a colour `c` in `0..=1` samples at
//! `c · (N − 1)/N + 0.5/N`. [`Lut3d::sample`] uses the same convention on the
//! CPU, which is what makes the GPU/CPU comparisons in the tests meaningful.
//!
//! The LUT is applied to **straight** colour: the viewport un-premultiplies,
//! samples, and premultiplies again (docs/tasks/SNIPPETS.md §14).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use fx_core::ColorProfile;
use half::f16;
use lcms2::{Flags, Intent, PixelFormat, Transform};

use crate::profile::{ColorError, profile, profile_key};

/// Nodes per LUT axis. 33³ = 35 937 samples: the usual display-transform size
/// (Photoshop's display 3D LUT is the same order), and small enough that
/// building it is a few milliseconds.
pub const LUT_GRID: usize = 33;

/// Bytes of one LUT in VRAM (`Rgba16Float`).
pub const LUT_BYTES: usize = LUT_GRID * LUT_GRID * LUT_GRID * 8;

/// A 3D colour transform sampled on a uniform grid, ready for the GPU.
///
/// Content is immutable plain data: no lcms2 object is kept, so a LUT can
/// travel to the render thread and be uploaded there (that thread never
/// touches a profile).
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
	/// RGBA f16, `r` fastest, then `g`, then `b` — the layout a 3D texture
	/// upload expects. Alpha is 1.0 (M4-T04 puts the gamut flag there).
	entries: Box<[[f16; 4]]>,
	/// What this table was built from, so a cache can tell two transforms
	/// apart without comparing 35 937 samples.
	key: u64,
}

impl Lut3d {
	/// Nodes per axis (always [`LUT_GRID`]).
	pub fn grid(&self) -> usize {
		LUT_GRID
	}

	/// The raw table for upload: `Rgba16Float`, `r` fastest.
	pub fn bytes(&self) -> &[u8] {
		bytemuck::cast_slice(&self.entries)
	}

	/// Identity of the transform that produced this table.
	pub fn key(&self) -> u64 {
		self.key
	}

	/// The colour a straight `rgb` maps to, interpolated exactly like the
	/// shader does (trilinear over the same node positions). Values outside
	/// `0..=1` clamp to the table's edge.
	pub fn sample(&self, rgb: [f64; 3]) -> [f64; 3] {
		let last = LUT_GRID - 1;
		let f = rgb.map(|c| c.clamp(0.0, 1.0) * last as f64);
		let i0 = f.map(|v| (v.floor() as usize).min(last - 1));
		let t = [f[0] - i0[0] as f64, f[1] - i0[1] as f64, f[2] - i0[2] as f64];
		let i1 = i0.map(|i| i + 1);
		let at = |r: usize, g: usize, b: usize| -> [f64; 3] {
			let e = self.entries[(b * LUT_GRID + g) * LUT_GRID + r];
			[e[0].to_f64(), e[1].to_f64(), e[2].to_f64()]
		};
		let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
		let mut out = [0.0; 3];
		for c in 0..3 {
			// The eight corners, named by their offset on each axis (0 = i0, 1 = i1).
			let c000 = at(i0[0], i0[1], i0[2])[c];
			let c100 = at(i1[0], i0[1], i0[2])[c];
			let c010 = at(i0[0], i1[1], i0[2])[c];
			let c110 = at(i1[0], i1[1], i0[2])[c];
			let c001 = at(i0[0], i0[1], i1[2])[c];
			let c101 = at(i1[0], i0[1], i1[2])[c];
			let c011 = at(i0[0], i1[1], i1[2])[c];
			let c111 = at(i1[0], i1[1], i1[2])[c];
			// Interpolate along r, then g, then b.
			let r0 = lerp(c000, c100, t[0]);
			let r1 = lerp(c010, c110, t[0]);
			let r2 = lerp(c001, c101, t[0]);
			let r3 = lerp(c011, c111, t[0]);
			out[c] = lerp(lerp(r0, r1, t[1]), lerp(r2, r3, t[1]), t[2]);
		}
		out
	}
}

/// Build the transform `src` → `dst` as a LUT: what the document's encoded
/// values become on the display, at `intent`, optionally with black point
/// compensation (D-025).
///
/// The transformation happens in 16-bit RGBA so the grid nodes are exact; the
/// alpha channel is written as 1.0 (lcms2 copies it through, `cmsFLAGS_COPY_ALPHA`
/// is implicit in `TYPE_RGBA_16`).
pub fn display_lut(src: &ColorProfile, dst: &ColorProfile, intent: Intent, bpc: bool) -> Result<Lut3d, ColorError> {
	let key = display_lut_key(src, dst, intent, bpc);
	let (src_profile, dst_profile) = (profile(src)?, profile(dst)?);
	let transform: Transform<[u16; 4], [u16; 4]> = if bpc {
		Transform::new_flags(&src_profile, PixelFormat::RGBA_16, &dst_profile, PixelFormat::RGBA_16, intent, Flags::BLACKPOINT_COMPENSATION)?
	} else {
		Transform::new(&src_profile, PixelFormat::RGBA_16, &dst_profile, PixelFormat::RGBA_16, intent)?
	};
	let nodes = LUT_GRID * LUT_GRID * LUT_GRID;
	let step = u16::MAX as f64 / (LUT_GRID - 1) as f64;
	let mut input = vec![[0u16; 4]; nodes];
	for b in 0..LUT_GRID {
		for g in 0..LUT_GRID {
			for r in 0..LUT_GRID {
				input[(b * LUT_GRID + g) * LUT_GRID + r] = [
					(r as f64 * step).round() as u16,
					(g as f64 * step).round() as u16,
					(b as f64 * step).round() as u16,
					u16::MAX,
				];
			}
		}
	}
	let mut output = vec![[0u16; 4]; nodes];
	transform.transform_pixels(&input, &mut output);
	let entries: Box<[[f16; 4]]> = output
		.into_iter()
		.map(|px| {
			[
				f16::from_f64(px[0] as f64 / 65535.0),
				f16::from_f64(px[1] as f64 / 65535.0),
				f16::from_f64(px[2] as f64 / 65535.0),
				f16::ONE,
			]
		})
		.collect();
	Ok(Lut3d { entries, key })
}

/// The key [`display_lut`] gives the table it would build for these inputs: a
/// caller can check its cache before paying for the build.
pub fn display_lut_key(src: &ColorProfile, dst: &ColorProfile, intent: Intent, bpc: bool) -> u64 {
	lut_key(profile_key(src), profile_key(dst), intent, bpc)
}

fn lut_key(src: u64, dst: u64, intent: Intent, bpc: bool) -> u64 {
	let mut hasher = DefaultHasher::new();
	src.hash(&mut hasher);
	dst.hash(&mut hasher);
	(intent as u32).hash(&mut hasher);
	bpc.hash(&mut hasher);
	LUT_GRID.hash(&mut hasher);
	hasher.finish()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The same transform, applied directly to single pixels: the oracle the
	/// LUT is compared against (the trilinear interpolation is the only
	/// difference).
	fn direct(src: &ColorProfile, dst: &ColorProfile, intent: Intent, bpc: bool, pixels: &mut [[u16; 4]]) {
		let (src, dst) = (profile(src).unwrap(), profile(dst).unwrap());
		let transform: Transform<[u16; 4], [u16; 4]> = if bpc {
			Transform::new_flags(&src, PixelFormat::RGBA_16, &dst, PixelFormat::RGBA_16, intent, Flags::BLACKPOINT_COMPENSATION).unwrap()
		} else {
			Transform::new(&src, PixelFormat::RGBA_16, &dst, PixelFormat::RGBA_16, intent).unwrap()
		};
		transform.transform_in_place(pixels);
	}

	fn rgba16(rgb: [f64; 3]) -> [u16; 4] {
		let v = |c: f64| (c.clamp(0.0, 1.0) * 65535.0).round() as u16;
		[v(rgb[0]), v(rgb[1]), v(rgb[2]), u16::MAX]
	}

	fn as_f64(px: [u16; 4]) -> [f64; 3] {
		[px[0] as f64 / 65535.0, px[1] as f64 / 65535.0, px[2] as f64 / 65535.0]
	}

	#[test]
	fn srgb_to_srgb_is_the_identity_at_every_node() {
		let lut = display_lut(&ColorProfile::Srgb, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		let last = (LUT_GRID - 1) as f64;
		let mut worst = 0.0f64;
		for b in 0..LUT_GRID {
			for g in 0..LUT_GRID {
				for r in 0..LUT_GRID {
					let expected = [r as f64 / last, g as f64 / last, b as f64 / last];
					let got = lut.sample(expected);
					for c in 0..3 {
						worst = worst.max((got[c] - expected[c]).abs());
					}
				}
			}
		}
		// 1/1023: the identity is not bit-exact through lcms2 (rounding in the
		// encoded→linear→encoded round trip), but far below one 16-bit step.
		assert!(worst <= 1.0 / 1023.0, "identity LUT is off by {worst}");
	}

	#[test]
	fn the_lut_matches_a_direct_transform() {
		let lut = display_lut(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		let samples: [[f64; 3]; 12] = [
			[0.0, 0.0, 0.0],
			[1.0, 1.0, 1.0],
			[0.5, 0.5, 0.5],
			[0.25, 0.6, 0.9],
			[0.9, 0.25, 0.4],
			[0.13, 0.77, 0.31],
			[1.0, 0.0, 0.0],
			[0.0, 1.0, 0.0],
			[0.0, 0.0, 1.0],
			[0.42, 0.42, 0.42],
			[0.7, 0.7, 0.2],
			[0.03, 0.09, 0.14],
		];
		let mut pixels: Vec<[u16; 4]> = samples.iter().map(|s| rgba16(*s)).collect();
		direct(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, Intent::RelativeColorimetric, true, &mut pixels);
		for (sample, expected) in samples.iter().zip(&pixels) {
			let expected = as_f64(*expected);
			let got = lut.sample(*sample);
			for c in 0..3 {
				assert!(
					(got[c] - expected[c]).abs() <= 2.0 / 255.0,
					"at {sample:?}: LUT {got:?} vs lcms2 {expected:?}"
				);
			}
		}
	}

	#[test]
	fn a_neutral_stays_neutral() {
		let lut = display_lut(&ColorProfile::ProPhotoRgb, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		for v in [0.05, 0.2, 0.5, 0.75, 0.95] {
			let out = lut.sample([v, v, v]);
			let spread = out[0].max(out[1]).max(out[2]) - out[0].min(out[1]).min(out[2]);
			assert!(spread <= 2.0 / 255.0, "grey {v} came out as {out:?}");
		}
	}

	#[test]
	fn every_intent_builds_and_stays_in_range() {
		let intents = [Intent::Perceptual, Intent::RelativeColorimetric, Intent::Saturation, Intent::AbsoluteColorimetric];
		for intent in intents {
			for bpc in [false, true] {
				let lut = display_lut(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, intent, bpc).unwrap();
				for sample in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.3, 0.6, 0.9], [1.0, 0.0, 0.5]] {
					let out = lut.sample(sample);
					for c in 0..3 {
						assert!(out[c].is_finite() && (0.0..=1.0).contains(&out[c]), "{intent:?} {bpc}: {sample:?} → {out:?}");
					}
				}
			}
		}
	}

	#[test]
	fn the_table_has_the_size_the_gpu_allocates() {
		let lut = display_lut(&ColorProfile::Srgb, &ColorProfile::DisplayP3, Intent::RelativeColorimetric, false).unwrap();
		assert_eq!(lut.grid(), LUT_GRID);
		assert_eq!(lut.bytes().len(), LUT_BYTES);
	}

	#[test]
	fn sampling_outside_the_cube_clamps() {
		let lut = display_lut(&ColorProfile::Srgb, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		assert_eq!(lut.sample([-0.5, 1.5, 0.0]), lut.sample([0.0, 1.0, 0.0]));
	}

	#[test]
	fn two_transforms_have_different_keys() {
		let a = display_lut(&ColorProfile::Srgb, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		let b = display_lut(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		let c = display_lut(&ColorProfile::Srgb, &ColorProfile::Srgb, Intent::Perceptual, true).unwrap();
		let d = display_lut(&ColorProfile::Srgb, &ColorProfile::Srgb, Intent::RelativeColorimetric, false).unwrap();
		assert_ne!(a.key(), b.key());
		assert_ne!(a.key(), c.key());
		assert_ne!(a.key(), d.key());
		let again = display_lut(&ColorProfile::Srgb, &ColorProfile::Srgb, Intent::RelativeColorimetric, true).unwrap();
		assert_eq!(a.key(), again.key());
	}
}
