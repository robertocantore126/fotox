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
