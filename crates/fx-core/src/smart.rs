//! Smart Objects (M12-T01..T03, D-082, D-083).
//!
//! A Smart Object layer keeps a **source** (a nested document, embedded or
//! read from a linked file) and its flattened **composite**; what the canvas
//! shows is a derived cache resampled from the composite's mips through
//! `transform` at the level being drawn, then run through the Smart Filters.
//! Transforming a Smart Object only changes `transform`: it is lossless.

use std::sync::Arc;

use fx_tiles::TiledImage;
use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::document::Document;
use crate::ops::FilterParams;
use crate::transform::Mapping;

/// Where a Smart Object's content comes from.
#[derive(Clone, Debug)]
pub struct SmartSource {
	/// The nested document (layers, size, colour).
	pub doc: Arc<Document>,
	/// Its flattened pixels at the source's size (level 0 authoritative; the
	/// mips are built when a smaller level is drawn).
	pub composite: TiledImage,
	/// A Linked Smart Object's file (`None` = embedded).
	pub linked: Option<String>,
	/// The linked file's modification time (seconds) when it was read.
	pub linked_mtime: Option<u64>,
	/// A stable id: duplicates made by Duplicate Layer share it (Photoshop's
	/// "instances"); New Smart Object via Copy gets a new one.
	pub uid: u64,
}

/// One Smart Filter (M12-T03).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SmartFilter {
	pub filter: FilterParams,
	#[serde(default = "yes")]
	pub enabled: bool,
	#[serde(default)]
	pub mode: BlendMode,
	#[serde(default = "one")]
	pub opacity: f32,
}

fn yes() -> bool {
	true
}

fn one() -> f32 {
	1.0
}

/// Everything of a Smart Object layer but its derived cache.
#[derive(Clone, Debug)]
pub struct SmartObject {
	pub source: SmartSource,
	/// Source composite pixels → document pixels.
	pub transform: Mapping,
	pub filters: Vec<SmartFilter>,
	/// Whether the Smart Filters apply (the eye of the "Smart Filters" row).
	pub filters_enabled: bool,
}

impl SmartObject {
	/// The source's size in pixels.
	pub fn source_size(&self) -> (u32, u32) {
		(self.source.composite.width(), self.source.composite.height())
	}

	/// The document box the source covers (`None` for a mapping without a
	/// computable box).
	pub fn bounds(&self) -> Option<((i32, i32), (u32, u32))> {
		let (w, h) = self.source_size();
		crate::transform::dest_rect(&self.transform, [0.0, 0.0, f64::from(w), f64::from(h)])
	}
}

/// A fresh id for a new Smart Object source.
pub fn new_uid() -> u64 {
	use std::sync::atomic::{AtomicU64, Ordering};
	static NEXT: AtomicU64 = AtomicU64::new(1);
	let t = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| d.as_nanos() as u64);
	t ^ NEXT.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}
