//! ICC profiles: the working spaces Fotox knows by name, and the profiles the
//! user's machine provides as files (embedded in an image, or a display
//! profile read from Windows).
//!
//! Profiles are built on demand and dropped again: building one is a few
//! microseconds of lcms2 work, and a display transform is built when the
//! document or the monitor profile *changes*, never per frame (M4-T02). That
//! is also why nothing here is cached or shared between threads: lcms2 objects
//! live on the thread that uses them.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use fx_core::ColorProfile;
use lcms2::{CIExyY, CIExyYTRIPLE, Profile, ToneCurve};

/// Why a profile could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ColorError {
	/// The bytes are not an ICC profile lcms2 accepts.
	#[error("not a readable ICC profile: {0}")]
	InvalidIcc(String),
	/// lcms2 refused to build a virtual profile (wrong primaries, a curve it
	/// cannot represent). A bug in our constants, not user data.
	#[error("cannot build the ICC profile: {0}")]
	Lcms(#[from] lcms2::Error),
}

/// D50, the white point of the ICC reference medium (used by ProPhoto RGB).
const D50: CIExyY = CIExyY {
	x: 0.345_67,
	y: 0.358_50,
	Y: 1.0,
};

/// D65, the white point of sRGB, Adobe RGB (1998) and Display P3.
const D65: CIExyY = CIExyY {
	x: 0.312_7,
	y: 0.329_0,
	Y: 1.0,
};

/// Adobe RGB (1998) primaries (docs/tasks/M4.md, M4-T01).
const ADOBE_RGB: CIExyYTRIPLE = CIExyYTRIPLE {
	Red: CIExyY { x: 0.6400, y: 0.3300, Y: 1.0 },
	Green: CIExyY { x: 0.2100, y: 0.7100, Y: 1.0 },
	Blue: CIExyY { x: 0.1500, y: 0.0600, Y: 1.0 },
};

/// Display P3 primaries (DCI-P3 with the D65 white point).
const DISPLAY_P3: CIExyYTRIPLE = CIExyYTRIPLE {
	Red: CIExyY { x: 0.680, y: 0.320, Y: 1.0 },
	Green: CIExyY { x: 0.265, y: 0.690, Y: 1.0 },
	Blue: CIExyY { x: 0.150, y: 0.060, Y: 1.0 },
};

/// ROMM RGB (ProPhoto RGB) primaries: D50 white, one primary off the locus.
const PROPHOTO: CIExyYTRIPLE = CIExyYTRIPLE {
	Red: CIExyY { x: 0.7347, y: 0.2653, Y: 1.0 },
	Green: CIExyY { x: 0.1596, y: 0.8404, Y: 1.0 },
	Blue: CIExyY { x: 0.0366, y: 0.0001, Y: 1.0 },
};

/// The sRGB transfer curve as lcms2 itself builds it: ICC parametric curve
/// type 4 (`Y = ((X + b) / a)^g` above `d`, `X / c` below) with the parameters
/// from the sRGB specification (IEC 61966-2.1), the same numbers as lcms2's
/// own `cmsCreate_sRGBProfile`.
const SRGB_CURVE: [f64; 5] = [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045];

/// ProPhoto RGB's curve: γ = 1.8 with the ROMM linear segment below 1/512
/// (ISO 22028-2). In the ICC direction (encoded → linear) the segment has
/// slope 1/16 and meets the power function at 1/32.
const PROPHOTO_CURVE: [f64; 5] = [1.8, 1.0, 0.0, 1.0 / 16.0, 1.0 / 32.0];

/// Adobe RGB (1998) gamma: 563/256.
const ADOBE_GAMMA: f64 = 563.0 / 256.0;

/// Build the ICC profile of `space`.
///
/// `Srgb` uses lcms2's built-in sRGB profile; the other named spaces are built
/// from their primaries, white point and transfer curve, so they do not depend
/// on an ICC file being installed (M4-T01). `Icc` parses the embedded bytes.
pub fn profile(space: &ColorProfile) -> Result<Profile, ColorError> {
	match space {
		ColorProfile::Srgb => Ok(Profile::new_srgb()),
		ColorProfile::AdobeRgb1998 => {
			let curve = ToneCurve::new_parametric(1, &[ADOBE_GAMMA])?;
			Ok(Profile::new_rgb(&D65, &ADOBE_RGB, &[&curve, &curve, &curve])?)
		}
		ColorProfile::DisplayP3 => {
			let curve = ToneCurve::new_parametric(4, &SRGB_CURVE)?;
			Ok(Profile::new_rgb(&D65, &DISPLAY_P3, &[&curve, &curve, &curve])?)
		}
		ColorProfile::ProPhotoRgb => {
			let curve = ToneCurve::new_parametric(4, &PROPHOTO_CURVE)?;
			Ok(Profile::new_rgb(&D50, &PROPHOTO, &[&curve, &curve, &curve])?)
		}
		ColorProfile::Icc(bytes) => Profile::new_icc(bytes).map_err(|error| ColorError::InvalidIcc(error.to_string())),
	}
}

/// Whether two profiles are the same space, so a transform between them can be
/// skipped (criterion C1: an sRGB document on an sRGB monitor is displayed
/// bit-exactly, without a 3D LUT in the way).
///
/// True when the named spaces are equal, when the ICC bytes are equal, or when
/// both profiles carry the same non-zero ICC profile ID in their header (the
/// same profile re-encoded byte by byte differently, e.g. re-saved by another
/// application).
pub fn same_profile(a: &ColorProfile, b: &ColorProfile) -> bool {
	match (a, b) {
		(ColorProfile::Icc(a), ColorProfile::Icc(b)) => {
			if a == b {
				return true;
			}
			let (a, b) = (icc_profile_id(a), icc_profile_id(b));
			a != [0; 16] && a == b
		}
		_ => a == b,
	}
}

/// A stable key for caching transforms between profiles: the named space, or
/// the content of the ICC bytes.
pub fn profile_key(space: &ColorProfile) -> u64 {
	let mut hasher = DefaultHasher::new();
	match space {
		ColorProfile::Icc(bytes) => {
			// Hash the whole profile: two different ICC files must not share a
			// display transform, and ICC profiles are small (a few KiB).
			0u8.hash(&mut hasher);
			bytes.hash(&mut hasher);
		}
		named => {
			1u8.hash(&mut hasher);
			std::mem::discriminant(named).hash(&mut hasher);
		}
	}
	hasher.finish()
}

/// The ICC profile ID of the profile header (bytes 84..100), zeros when the
/// profile carries none. lcms2 computes it only on request, so a plain
/// `profile_id()` would return zeros for most profiles: the header field is
/// what the file actually contains.
fn icc_profile_id(bytes: &Arc<[u8]>) -> [u8; 16] {
	const HEADER_END: usize = 100;
	const ID_START: usize = 84;
	match bytes.get(ID_START..HEADER_END) {
		Some(id) => <[u8; 16]>::try_from(id).unwrap_or([0; 16]),
		None => [0; 16],
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn named_spaces_build() {
		for space in [
			ColorProfile::Srgb,
			ColorProfile::AdobeRgb1998,
			ColorProfile::DisplayP3,
			ColorProfile::ProPhotoRgb,
		] {
			assert!(profile(&space).is_ok(), "{space:?}");
		}
	}

	#[test]
	fn a_broken_icc_profile_is_an_error_not_a_panic() {
		let space = ColorProfile::Icc(Arc::from(&b"not a profile"[..]));
		assert!(matches!(profile(&space), Err(ColorError::InvalidIcc(_))));
	}

	#[test]
	fn same_profile_for_named_spaces_and_equal_bytes() {
		assert!(same_profile(&ColorProfile::Srgb, &ColorProfile::Srgb));
		assert!(!same_profile(&ColorProfile::Srgb, &ColorProfile::AdobeRgb1998));
		let bytes: Arc<[u8]> = Arc::from(&b"whatever"[..]);
		assert!(same_profile(&ColorProfile::Icc(bytes.clone()), &ColorProfile::Icc(bytes)));
		assert!(!same_profile(&ColorProfile::Icc(Arc::from(&b"a"[..])), &ColorProfile::Srgb));
	}

	#[test]
	fn profile_keys_follow_the_content() {
		let a: Arc<[u8]> = Arc::from(&b"profile a"[..]);
		let b: Arc<[u8]> = Arc::from(&b"profile b"[..]);
		assert_ne!(profile_key(&ColorProfile::Icc(a.clone())), profile_key(&ColorProfile::Icc(b)));
		assert_eq!(profile_key(&ColorProfile::Icc(a.clone())), profile_key(&ColorProfile::Icc(a)));
		assert_ne!(profile_key(&ColorProfile::Srgb), profile_key(&ColorProfile::AdobeRgb1998));
	}
}
