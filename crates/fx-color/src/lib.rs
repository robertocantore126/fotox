//! # fx-color — colour management (milestone M4)
//!
//! Scope (decided with Rob: RGB editing + soft proof, no native CMYK mode):
//! * Display transform: document profile → monitor profile, applied as a
//!   3D LUT (33³, f16) in the final viewport shader. Until M4 the viewport
//!   assumes sRGB document on an sRGB display.
//! * Soft proof: document → CMYK (FOGRA39/51, SWOP, or a user ICC) → monitor,
//!   baked into the same LUT, with rendering intent and black point compensation.
//! * Gamut warning overlay (from the proof transform).
//! * Export conversion: RGB → CMYK for TIFF/PDF export, tile by tile via lcms2
//!   on worker threads.
//! * Import: honour embedded ICC profiles; offer "convert to working space"
//!   or "keep embedded" like Photoshop.
//!
//! Implemented so far (M4-T02): ICC profiles for the named working spaces and
//! for ICC bytes, and the display LUT with its CPU sampler. The proofing
//! transform, the tile transforms and the CMYK export follow in M4-T03/T04.

mod lut;
mod profile;

pub use lut::{LUT_BYTES, LUT_GRID, Lut3d, display_lut, display_lut_key};
pub use profile::{ColorError, profile, profile_key, same_profile};

/// The rendering intents, exactly lcms2's (D-025).
pub use lcms2::Intent;

/// The intent and black point compensation Fotox displays with by default:
/// relative colorimetric with BPC, like Photoshop's `Convert to Profile`.
pub const DEFAULT_INTENT: Intent = Intent::RelativeColorimetric;
/// Whether black point compensation is on by default (D-025).
pub const DEFAULT_BPC: bool = true;
