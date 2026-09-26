//! M8's painting ops (HOWTO R11): one `DabOp` (or `DabSequence`) per tool.

use fx_core::stroke::StrokeTool;

use super::op::DabSequence;

pub mod background_eraser;
pub mod focus;
pub mod history;
pub mod pattern;
pub mod smudge;
pub mod tone;

/// The sequence of a tool that reads its own stroke, `None` for the others.
/// `color` is the stroke's straight RGB.
pub fn sequence_for(tool: &StrokeTool, color: [f64; 3], seed: u64) -> Option<Box<dyn DabSequence>> {
	let _ = color;
	match tool {
		StrokeTool::Smudge { finger_painting, .. } => Some(Box::new(smudge::Smudge::new(*finger_painting))),
		StrokeTool::ArtHistory { style, area, tolerance, .. } => {
			Some(Box::new(history::ArtHistory::new(*style, f64::from(*area), f64::from(*tolerance), seed)))
		}
		_ => None,
	}
}
