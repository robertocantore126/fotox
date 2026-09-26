//! What a fill paints (M8-T02/T06): a colour or a document pattern.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FillSource {
	/// Straight 16-bit RGBA.
	Color { rgba: [u16; 4] },
	/// A pattern of the document's `patterns` by id (M8-T06).
	Pattern { pattern: u64 },
}
