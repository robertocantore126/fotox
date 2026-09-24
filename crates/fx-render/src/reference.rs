//! CPU reference implementations (M2-T02).
//!
//! Every GPU shader that produces document pixels has a straightforward,
//! slow, obviously-correct f64 CPU twin here. Tests render random tiles with
//! both and require max abs error ≤ 1/1024 (display path, f16) — or exact
//! equality for the 16-bit commit path.
//!
//! Formulas: docs/BLEND_MODES.md. Do not optimise this module; clarity wins.
