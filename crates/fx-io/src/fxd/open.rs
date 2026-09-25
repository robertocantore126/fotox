//! Opening a `.fxd` lazily (M3-T05).
//!
//! Only the footer and the manifest are read. Every level-0 tile becomes a
//! **backed** tile that reads its pixels from the file on demand, so opening a
//! 220-layer document costs one manifest read, not 220 layers of pixels. The
//! stored composite preview (levels ≥ 3) is returned too, so the first frame
//! can draw before any real tile is loaded.

use std::path::Path;
use std::sync::Arc;

use fx_core::Document;
use fx_tiles::{TileStore, TiledImage};

use super::container::{ChunkKind, ChunkRef, FxdFile};
use super::manifest::{self, Manifest};
use crate::IoError;

/// A lazily opened `.fxd`.
pub struct OpenedFxd {
	/// The document, with every tile backed by `file`.
	pub document: Document,
	/// The open file, kept alive by the backed tiles.
	pub file: Arc<FxdFile>,
	/// The stored flattened composite (levels ≥ 3), if the file has one.
	pub preview: Option<TiledImage>,
	/// The manifest, for callers that need the raw model (e.g. diagnostics).
	pub manifest: Manifest,
}

/// Open a `.fxd`: footer → manifest → backed document. No pixel is read.
pub fn open(path: &Path, store: &TileStore) -> Result<OpenedFxd, IoError> {
	let (file, footer) = FxdFile::open(path)?;
	let (kind, payload) = file.read_chunk(ChunkRef {
		offset: footer.manifest_offset,
		len: footer.manifest_len,
	})?;
	if kind != ChunkKind::Manifest {
		return Err(IoError::Decode(format!("footer points at a {kind:?} chunk, not the manifest")));
	}
	let manifest = manifest::decode_manifest(&payload)?;
	let document = manifest::from_manifest(&manifest, &file, store)?;
	let preview = match &manifest.preview {
		Some(entry) => Some(manifest::image_from_entry(entry, &file, store)?),
		None => None,
	};
	Ok(OpenedFxd {
		document,
		file,
		preview,
		manifest,
	})
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;
	use std::sync::Arc;

	use fx_core::{BitDepth, ColorProfile, Document, DocumentColor, Layer, LayerId, LayerKind};
	use fx_tiles::{PixelFormat, PixelValue, TileBuffer, TileSlot, TileStoreConfig, TiledImage};

	use super::super::save::{SaveRequest, SaveTarget, save};
	use super::*;

	fn store() -> (TileStore, PathBuf) {
		let dir = std::env::temp_dir().join("fx-io-fxd-open-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let mut config = TileStoreConfig::for_tests(dir.join("scratch"));
		config.hot_budget = 1 << 30;
		(TileStore::new(config).unwrap(), dir)
	}

	/// A document with `layers` pixel layers, each one real tile.
	fn many_layer_document(store: &TileStore, layers: usize) -> Document {
		let mut doc = Document::new(
			512,
			512,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			300.0,
		);
		for i in 0..layers {
			let id = doc.allocate_layer_id();
			let mut image = TiledImage::new(512, 512, PixelFormat::Rgba16);
			let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
			buffer.as_u16_mut()[0] = i as u16 + 1;
			image.put_buffer(store, 0, 0, buffer);
			doc.layers
				.push(Arc::new(Layer::new(id, format!("Layer {i}"), LayerKind::Pixel { image, offset: (0, 0) })));
		}
		doc
	}

	#[test]
	fn open_reads_only_the_manifest() {
		let (store, dir) = store();
		let path = dir.join("many-layers.fxd");
		let doc = many_layer_document(&store, 220);
		save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.unwrap();

		let before = store.stats();
		let opened = open(&path, &store).unwrap();
		let after = store.stats();
		assert_eq!(opened.document.layers.len(), 220);
		assert_eq!(after.backed_reads, before.backed_reads, "opening reads no tile");
		assert_eq!(after.hot_bytes, before.hot_bytes, "opening warms no tile");
		assert!(opened.preview.is_none());

		// Reading one layer's tile works and goes through the file.
		let LayerKind::Pixel { image, .. } = &opened.document.layers[7].kind else {
			panic!("not a pixel layer")
		};
		let TileSlot::Data(handle) = image.slot(0, 0, 0) else { panic!("no tile") };
		let pixels = store.get(handle).unwrap();
		assert_eq!(pixels.as_u16()[0], 8);
		assert_eq!(store.stats().backed_reads, before.backed_reads + 1);
	}

	#[test]
	fn open_edit_save_open_equals_the_edited_document() {
		let (store, dir) = store();
		let path = dir.join("edit.fxd");
		let doc = many_layer_document(&store, 3);
		save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.unwrap();

		// Open, edit (opacity + paint one tile), save incrementally.
		let opened = open(&path, &store).unwrap();
		let mut doc = opened.document;
		doc.layer_mut(LayerId(2)).unwrap().opacity = 0.4;
		let LayerKind::Pixel { image, .. } = &mut doc.layer_mut(LayerId(2)).unwrap().kind else {
			panic!("not a pixel layer")
		};
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		buffer.as_u16_mut()[0] = 4242;
		image.put_buffer(&store, 0, 0, buffer);

		save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Incremental(opened.file.clone()),
			&mut |_| true,
		)
		.unwrap();

		// Reopen: the edited document comes back.
		let again = open(&path, &store).unwrap();
		assert_eq!(again.document.layer(LayerId(2)).unwrap().opacity, 0.4);
		let LayerKind::Pixel { image, .. } = &again.document.layer(LayerId(2)).unwrap().kind else {
			panic!("not a pixel layer")
		};
		let TileSlot::Data(handle) = image.slot(0, 0, 0) else { panic!("no tile") };
		assert_eq!(store.get(handle).unwrap().as_u16()[0], 4242);
		assert_eq!(again.document.layers.len(), 3);
	}

	#[test]
	fn the_composite_preview_round_trips() {
		let (store, dir) = store();
		let path = dir.join("preview.fxd");
		let doc = many_layer_document(&store, 1);

		// A preview image at level 3 (2048 px → levels 0..3).
		let mut preview = TiledImage::new(2048, 2048, PixelFormat::Rgba8);
		preview.set_derived_slot(3, 0, 0, TileSlot::Solid(PixelValue::rgba8(10, 20, 30, 255)));

		save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: Some(&preview),
			},
			SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.unwrap();

		let opened = open(&path, &store).unwrap();
		let restored = opened.preview.expect("preview stored");
		assert_eq!(restored.level_count(), 4);
		assert!(matches!(
			restored.slot(3, 0, 0),
			TileSlot::Solid(value) if *value == PixelValue::rgba8(10, 20, 30, 255)
		));
	}
}
