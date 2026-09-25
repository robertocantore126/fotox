//! Open documents (M1-T08): each with its history and its own view.
//!
//! The engine thread owns this list. The render thread only ever sees
//! snapshots (`Arc<Document>`, cheap: layers are shared `Arc`s and images are
//! slot grids of shared tile handles).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fx_core::{ColorProfile, Document, DocumentColor, History, Layer, LayerId, LayerKind};
use fx_io::ImportedImage;
use fx_io::fxd::{FxdFile, OpenedFxd};
use fx_protocol::{DocId, DocumentInfo};

use crate::view::ViewState;

/// One open document.
pub struct OpenDoc {
	pub id: DocId,
	pub name: String,
	pub doc: Document,
	pub history: History,
	pub view: ViewState,
	pub dirty: bool,
	/// Bumped by every content change (command, undo, redo). The render
	/// thread keys its caches on it: `Document::revision` is not unique
	/// because undo winds it back.
	pub generation: u64,
	/// The last edited layer and when, and the layer that is "hot" (M2-T05).
	last_edit: Option<(LayerId, Instant)>,
	hot: Option<(LayerId, Instant)>,
	/// The snapshot last handed to the render thread; rebuilt when `doc`
	/// changes (see [`OpenDoc::snapshot`]).
	snapshot: Option<Arc<Document>>,
	/// Derived data (mips) changed without a new revision.
	snapshot_stale: bool,
	/// The open `.fxd` this document was loaded from or last saved to (M3,
	/// D-027), shared by its backed tiles. `None` for a flat import not yet
	/// saved: Save then needs a path.
	pub file: Option<Arc<FxdFile>>,
	/// The `.fxd`'s path (the Save target while `file` is set).
	pub path: Option<PathBuf>,
}

impl OpenDoc {
	/// A document holding one imported image as its "Background" layer
	/// (Photoshop's name for the single layer of an opened flat file).
	pub fn from_import(id: DocId, path: &Path, imported: ImportedImage) -> Self {
		let name = path
			.file_name()
			.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
		let mut doc = Document::new(
			imported.width,
			imported.height,
			DocumentColor {
				depth: imported.depth,
				profile: imported.profile,
			},
			imported.ppi,
		);
		let layer_id = doc.allocate_layer_id();
		let mut layer = Layer::new(
			layer_id,
			"Background",
			LayerKind::Pixel {
				image: imported.image,
				offset: (0, 0),
			},
		);
		layer.locked_position = true;
		doc.layers.push(Arc::new(layer));
		doc.selected = vec![layer_id];
		let view = ViewState::new((imported.width, imported.height));
		Self {
			id,
			name,
			doc,
			history: History::default(),
			view,
			dirty: false,
			generation: 0,
			last_edit: None,
			hot: None,
			snapshot: None,
			snapshot_stale: false,
			file: None,
			path: None,
		}
	}

	/// A document loaded lazily from a `.fxd` (M3-T05): opened clean, with the
	/// file kept for incremental saves.
	pub fn from_fxd(id: DocId, path: &Path, opened: OpenedFxd) -> Self {
		let name = path
			.file_name()
			.map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
		let view = ViewState::new((opened.document.width, opened.document.height));
		Self {
			id,
			name,
			doc: opened.document,
			history: History::default(),
			view,
			dirty: false,
			generation: 0,
			last_edit: None,
			hot: None,
			snapshot: None,
			snapshot_stale: false,
			file: Some(opened.file),
			path: Some(path.to_path_buf()),
		}
	}

	/// The document as the render thread should see it now.
	pub fn snapshot(&mut self) -> Arc<Document> {
		match &self.snapshot {
			Some(s) if s.revision == self.doc.revision && !self.snapshot_stale => s.clone(),
			_ => {
				let s = Arc::new(self.doc.clone());
				self.snapshot = Some(s.clone());
				self.snapshot_stale = false;
				s
			}
		}
	}

	/// Tell [`snapshot`](Self::snapshot) that derived data (mips) changed
	/// without a new revision.
	pub fn invalidate_snapshot(&mut self) {
		self.snapshot_stale = true;
	}

	/// The document content changed (command, undo, redo).
	pub fn changed(&mut self) {
		self.generation += 1;
		// Undo can bring back an earlier revision number: never trust it alone.
		self.snapshot_stale = true;
	}

	/// Record edits of `layers` at `now`. A layer edited twice within
	/// [`HOT_AFTER`] becomes the hot layer (slider drags, painting), which
	/// lets the compositor cache everything below it (M2-T05).
	pub fn note_edits(&mut self, layers: &[LayerId], now: Instant) {
		for &layer in layers {
			if let Some((last, at)) = self.last_edit
				&& last == layer
				&& now.saturating_duration_since(at) <= HOT_AFTER
			{
				self.hot = Some((layer, now));
			} else if self.hot.is_some_and(|(hot, _)| hot == layer) {
				self.hot = Some((layer, now));
			}
			self.last_edit = Some((layer, now));
		}
	}

	/// The hot layer, if any.
	pub fn hot_layer(&self) -> Option<LayerId> {
		self.hot.map(|(layer, _)| layer)
	}

	/// When the hot layer cools down if nothing edits it.
	pub fn hot_expiry(&self) -> Option<Instant> {
		self.hot.map(|(_, at)| at + HOT_IDLE)
	}

	/// Drop the hot layer after [`HOT_IDLE`] without edits. Returns true if it changed.
	pub fn expire_hot(&mut self, now: Instant) -> bool {
		if self.hot_expiry().is_some_and(|expiry| now >= expiry) {
			self.hot = None;
			return true;
		}
		false
	}

	/// What the UI's document tabs show.
	pub fn info(&self) -> DocumentInfo {
		DocumentInfo {
			doc: self.id,
			name: self.name.clone(),
			width: self.doc.width,
			height: self.doc.height,
			depth: self.doc.color.depth,
			profile_name: profile_name(&self.doc.color.profile).into(),
			ppi: self.doc.ppi,
			dirty: self.dirty,
		}
	}
}

/// Two edits of one layer within this interval make it hot (M2-T05).
pub const HOT_AFTER: Duration = Duration::from_secs(1);
/// The hot layer cools down after this long without edits.
pub const HOT_IDLE: Duration = Duration::from_secs(2);

fn profile_name(profile: &ColorProfile) -> &'static str {
	match profile {
		ColorProfile::Srgb => "sRGB IEC61966-2.1",
		ColorProfile::AdobeRgb1998 => "Adobe RGB (1998)",
		ColorProfile::DisplayP3 => "Display P3",
		ColorProfile::ProPhotoRgb => "ProPhoto RGB",
		ColorProfile::Icc(_) => "Embedded profile",
	}
}

/// All open documents and which one is active.
#[derive(Default)]
pub struct Documents {
	docs: Vec<OpenDoc>,
	active: Option<DocId>,
	next_id: u32,
}

impl Documents {
	/// A fresh document id (never 0: 0 is the virtual M0 test document).
	pub fn allocate_id(&mut self) -> DocId {
		self.next_id += 1;
		DocId(self.next_id)
	}

	pub fn add(&mut self, doc: OpenDoc) {
		self.active = Some(doc.id);
		self.docs.push(doc);
	}

	/// Remove `id`; the tab to its left (or the new first one) becomes active.
	pub fn close(&mut self, id: DocId) -> Option<OpenDoc> {
		let index = self.docs.iter().position(|d| d.id == id)?;
		let closed = self.docs.remove(index);
		if self.active == Some(id) {
			self.active = self.docs.get(index.saturating_sub(1)).or_else(|| self.docs.first()).map(|d| d.id);
		}
		Some(closed)
	}

	pub fn activate(&mut self, id: DocId) -> bool {
		let known = self.docs.iter().any(|d| d.id == id);
		if known {
			self.active = Some(id);
		}
		known
	}

	pub fn active_id(&self) -> Option<DocId> {
		self.active
	}

	pub fn active_mut(&mut self) -> Option<&mut OpenDoc> {
		let id = self.active?;
		self.get_mut(id)
	}

	pub fn get_mut(&mut self, id: DocId) -> Option<&mut OpenDoc> {
		self.docs.iter_mut().find(|d| d.id == id)
	}

	pub fn ids(&self) -> Vec<DocId> {
		self.docs.iter().map(|d| d.id).collect()
	}

	pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut OpenDoc> {
		self.docs.iter_mut()
	}

	pub fn is_empty(&self) -> bool {
		self.docs.is_empty()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use fx_core::BitDepth;
	use fx_tiles::{PixelFormat, TiledImage};

	fn imported() -> ImportedImage {
		ImportedImage {
			width: 100,
			height: 50,
			depth: BitDepth::U8,
			profile: ColorProfile::Srgb,
			ppi: 72.0,
			image: TiledImage::new(100, 50, PixelFormat::Rgba8),
		}
	}

	#[test]
	fn an_import_becomes_a_background_layer() {
		let doc = OpenDoc::from_import(DocId(1), Path::new("C:/pics/sky.tif"), imported());
		assert_eq!(doc.name, "sky.tif");
		assert_eq!((doc.doc.width, doc.doc.height), (100, 50));
		assert_eq!(doc.doc.layers.len(), 1);
		assert_eq!(doc.doc.layers[0].name, "Background");
		assert_eq!(doc.info().profile_name, "sRGB IEC61966-2.1");
	}

	#[test]
	fn closing_activates_the_neighbour() {
		let mut docs = Documents::default();
		let ids: Vec<DocId> = (0..3).map(|_| docs.allocate_id()).collect();
		for &id in &ids {
			docs.add(OpenDoc::from_import(id, Path::new("a.png"), imported()));
		}
		assert_eq!(docs.active_id(), Some(ids[2]));
		docs.activate(ids[1]);
		docs.close(ids[1]);
		assert_eq!(docs.active_id(), Some(ids[0]), "left neighbour");
		docs.close(ids[0]);
		assert_eq!(docs.active_id(), Some(ids[2]));
		docs.close(ids[2]);
		assert_eq!(docs.active_id(), None);
		assert!(docs.is_empty());
		assert!(!docs.activate(ids[0]), "a closed document cannot be activated");
	}

	#[test]
	fn a_layer_edited_twice_within_a_second_becomes_hot_then_cools() {
		let mut doc = OpenDoc::from_import(DocId(1), Path::new("a.png"), imported());
		let (a, b) = (LayerId(1), LayerId(2));
		let t0 = Instant::now();
		doc.note_edits(&[a], t0);
		assert_eq!(doc.hot_layer(), None, "one edit is not a drag");
		doc.note_edits(&[a], t0 + Duration::from_millis(300));
		assert_eq!(doc.hot_layer(), Some(a));
		// Another layer's single edit does not steal it.
		doc.note_edits(&[b], t0 + Duration::from_millis(400));
		assert_eq!(doc.hot_layer(), Some(a));
		assert!(!doc.expire_hot(t0 + Duration::from_millis(2000)));
		assert!(doc.expire_hot(t0 + Duration::from_millis(2400)));
		assert_eq!(doc.hot_layer(), None);
		// Edits further apart than a second never heat up.
		doc.note_edits(&[a], t0 + Duration::from_secs(10));
		doc.note_edits(&[a], t0 + Duration::from_secs(12));
		assert_eq!(doc.hot_layer(), None);
	}

	#[test]
	fn a_change_always_gives_a_new_snapshot() {
		let mut doc = OpenDoc::from_import(DocId(1), Path::new("a.png"), imported());
		let a = doc.snapshot();
		doc.changed();
		assert!(!Arc::ptr_eq(&a, &doc.snapshot()), "same revision after undo, still a new snapshot");
		assert_eq!(doc.generation, 1);
	}

	#[test]
	fn snapshots_follow_revisions_and_invalidation() {
		let mut doc = OpenDoc::from_import(DocId(1), Path::new("a.png"), imported());
		let a = doc.snapshot();
		assert!(Arc::ptr_eq(&a, &doc.snapshot()), "unchanged document → same snapshot");
		doc.invalidate_snapshot();
		let b = doc.snapshot();
		assert!(!Arc::ptr_eq(&a, &b));
		doc.doc.revision += 1;
		assert_eq!(doc.snapshot().revision, 1);
	}
}
