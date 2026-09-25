//! The brush engine (M5-T06, D-041): round tips, dabs along the pointer
//! path, and Photoshop's stroke model on the CPU (f32 maths, rayon over
//! tiles). Live strokes and their replay share [`stroke::Stroke`].

pub mod heal;
pub mod path;
pub mod stroke;
pub mod tip;

pub use path::{Dab, DabPath};
pub use stroke::{LayerSource, SourceTiles, Stroke, StrokeSetup, replay};
pub use tip::Tip;

#[cfg(test)]
mod tests;
