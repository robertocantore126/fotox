//! Pixel transforms for document data (M4-T03/T04): Convert to Profile, and
//! RGB → CMYK for CMYK TIFF export. Built without lcms2's cache, so one
//! transform is shared by every rayon worker of a job (lcms2 documents a
//! no-cache transform as `Sync`).

use std::path::{Path, PathBuf};

use fx_core::ColorProfile;
use fx_core::color::RenderingIntent;
use lcms2::{ColorSpaceSignature, DisallowCache, Flags, GlobalContext, InfoType, Intent, Locale, PixelFormat, Profile, Transform};

use crate::profile::{ColorError, profile};

/// The lcms2 intent of a document rendering intent (D-033).
pub fn lcms_intent(intent: RenderingIntent) -> Intent {
	match intent {
		RenderingIntent::Perceptual => Intent::Perceptual,
		RenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
		RenderingIntent::Saturation => Intent::Saturation,
		RenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
	}
}

fn flags(bpc: bool) -> Flags<DisallowCache> {
	if bpc {
		Flags::NO_CACHE | Flags::BLACKPOINT_COMPENSATION
	} else {
		Flags::NO_CACHE
	}
}

/// RGBA16 → RGBA16 between two RGB spaces; alpha is carried through.
pub struct RgbTransform(Transform<[u16; 4], [u16; 4], GlobalContext, DisallowCache>);

impl RgbTransform {
	pub fn new(from: &ColorProfile, to: &ColorProfile, intent: RenderingIntent, bpc: bool) -> Result<Self, ColorError> {
		let (src, dst) = (profile(from)?, profile(to)?);
		let transform = Transform::new_flags_context(
			GlobalContext::new(),
			&src,
			PixelFormat::RGBA_16,
			&dst,
			PixelFormat::RGBA_16,
			lcms_intent(intent),
			flags(bpc) | Flags::COPY_ALPHA,
		)?;
		Ok(Self(transform))
	}

	/// Transform `pixels` in place (straight RGBA, alpha untouched).
	pub fn apply(&self, pixels: &mut [[u16; 4]]) {
		let alpha: Vec<u16> = pixels.iter().map(|p| p[3]).collect();
		self.0.transform_in_place(pixels);
		// COPY_ALPHA is honoured by lcms2 for these formats; keep it certain.
		for (p, a) in pixels.iter_mut().zip(alpha) {
			p[3] = a;
		}
	}
}

/// RGB16 (of `from`) → CMYK16 (of the CMYK profile `cmyk`). In the output,
/// 0 = no ink, 65535 = full ink, the TIFF "separated" convention.
pub struct CmykTransform(Transform<[u16; 3], [u16; 4], GlobalContext, DisallowCache>);

impl CmykTransform {
	pub fn new(from: &ColorProfile, cmyk: &[u8], intent: RenderingIntent, bpc: bool) -> Result<Self, ColorError> {
		let src = profile(from)?;
		let dst = cmyk_profile(cmyk)?;
		let transform = Transform::new_flags_context(
			GlobalContext::new(),
			&src,
			PixelFormat::RGB_16,
			&dst,
			PixelFormat::CMYK_16,
			lcms_intent(intent),
			flags(bpc),
		)?;
		Ok(Self(transform))
	}

	pub fn apply(&self, rgb: &[[u16; 3]], cmyk: &mut [[u16; 4]]) {
		self.0.transform_pixels(rgb, cmyk);
	}
}

/// Parse CMYK ICC bytes; any other colour space is an error.
pub fn cmyk_profile(bytes: &[u8]) -> Result<Profile, ColorError> {
	let p = Profile::new_icc(bytes).map_err(|error| ColorError::InvalidIcc(error.to_string()))?;
	if p.color_space() != ColorSpaceSignature::CmykData {
		return Err(ColorError::InvalidIcc("not a CMYK profile".into()));
	}
	Ok(p)
}

/// The ICC bytes of a document profile, for embedding in exported files:
/// embedded bytes as they are; the named spaces as lcms2 builds them.
pub fn icc_bytes(space: &ColorProfile) -> Result<Vec<u8>, ColorError> {
	match space {
		ColorProfile::Icc(bytes) => Ok(bytes.to_vec()),
		named => Ok(profile(named)?.icc()?),
	}
}

/// The description stored in an ICC profile (what Photoshop lists).
pub fn icc_description(bytes: &[u8]) -> Option<String> {
	let p = Profile::new_icc(bytes).ok()?;
	p.info(InfoType::Description, Locale::none()).filter(|s| !s.trim().is_empty())
}

/// A CMYK profile the user can proof and export with.
#[derive(Clone, Debug, PartialEq)]
pub struct CmykProfileFile {
	/// The profile's description (or file name).
	pub name: String,
	pub path: PathBuf,
}

/// The CMYK profiles of `dirs` (D-032: Windows' colour folder and
/// `%APPDATA%/Fotox/profiles`), sorted by name. Unreadable files are skipped.
pub fn cmyk_profiles(dirs: &[&Path]) -> Vec<CmykProfileFile> {
	let mut out = Vec::new();
	for dir in dirs {
		let Ok(entries) = std::fs::read_dir(dir) else { continue };
		for entry in entries.flatten() {
			let path = entry.path();
			let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
			if !matches!(ext.as_deref(), Some("icc" | "icm")) {
				continue;
			}
			let Ok(bytes) = std::fs::read(&path) else { continue };
			if cmyk_profile(&bytes).is_err() {
				continue;
			}
			let name = icc_description(&bytes).unwrap_or_else(|| path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned()));
			out.push(CmykProfileFile { name, path });
		}
	}
	out.sort_by(|a, b| a.name.cmp(&b.name));
	out.dedup_by(|a, b| a.name == b.name);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn srgb_to_adobe_and_back_keeps_in_gamut_colours() {
		let there = RgbTransform::new(&ColorProfile::Srgb, &ColorProfile::AdobeRgb1998, RenderingIntent::RelativeColorimetric, true).unwrap();
		let back = RgbTransform::new(&ColorProfile::AdobeRgb1998, &ColorProfile::Srgb, RenderingIntent::RelativeColorimetric, true).unwrap();
		let original = [[20_000u16, 30_000, 40_000, 12_345], [50_000, 20_000, 10_000, 65_535], [32_768; 4]];
		let mut px = original;
		there.apply(&mut px);
		assert_ne!(px[0][..3], original[0][..3], "the numbers change");
		back.apply(&mut px);
		for (a, b) in px.iter().zip(&original) {
			assert_eq!(a[3], b[3], "alpha is carried through");
			for c in 0..3 {
				assert!((i32::from(a[c]) - i32::from(b[c])).abs() <= 40, "{a:?} vs {b:?}");
			}
		}
	}

	#[test]
	fn named_profiles_have_icc_bytes() {
		for space in [
			ColorProfile::Srgb,
			ColorProfile::AdobeRgb1998,
			ColorProfile::DisplayP3,
			ColorProfile::ProPhotoRgb,
		] {
			let bytes = icc_bytes(&space).unwrap();
			assert!(bytes.len() > 128, "{space:?}");
			assert!(profile(&ColorProfile::Icc(bytes.into())).is_ok());
		}
	}

	#[test]
	fn an_rgb_profile_is_not_a_cmyk_profile() {
		let bytes = icc_bytes(&ColorProfile::Srgb).unwrap();
		assert!(cmyk_profile(&bytes).is_err());
	}

	#[test]
	fn windows_rswop_converts_white_to_no_ink() {
		let path = Path::new(r"C:\Windows\System32\spool\drivers\color\RSWOP.icm");
		let Ok(bytes) = std::fs::read(path) else {
			eprintln!("no RSWOP.icm on this machine: test skipped");
			return;
		};
		let t = CmykTransform::new(&ColorProfile::Srgb, &bytes, RenderingIntent::RelativeColorimetric, true).unwrap();
		let mut out = [[0u16; 4]; 2];
		t.apply(&[[65535; 3], [0; 3]], &mut out);
		assert!(out[0].iter().all(|&v| v < 700), "white → (almost) no ink: {:?}", out[0]);
		assert!(out[1][3] > 30_000, "black → plenty of K: {:?}", out[1]);
		let found = cmyk_profiles(&[path.parent().unwrap()]);
		assert!(found.iter().any(|p| p.path == path), "{found:?}");
	}
}
