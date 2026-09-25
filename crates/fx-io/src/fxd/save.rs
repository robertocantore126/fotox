//! Saving a document to a `.fxd` (M3-T04).
//!
//! A save walks every `TiledImage` of the document (layer pixels and masks) and
//! appends the tiles that are not already in the file, then the manifest, then
//! the footer. Tiles already backed by *this* file keep their chunk, so a save
//! after a small edit writes only what changed (S7).
//!
//! The flattened composite preview is rendered by the **caller** (the engine's
//! CPU reference compositor) and passed in as [`SaveRequest::preview`]:
//! `fx-io` cannot depend on `fx-engine`, and `fx-engine` depends on `fx-io`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fx_core::{Document, Layer, LayerKind};
use fx_tiles::{Backed, PixelFormat, TileError, TileHandle, TileSlot, TileStore, TiledImage};
use rayon::prelude::*;

use super::container::{ChunkRef, Codec, FOOTER_LEN, FxdFile, FxdWriter, HEADER_LEN};
use super::manifest;
use crate::{IoError, Progress};

/// zstd level for an incremental Save: speed matters on the interactive path.
pub const SAVE_LEVEL: i32 = 1;
/// zstd level for Save As / compaction: one-off writes favour size (D-024).
pub const FRESH_LEVEL: i32 = 3;
/// Tiles compressed in parallel before they are written in order: bounds the
/// memory of a save (64 × 512 KiB of 16-bit pixels + their compressed copies).
const IN_FLIGHT: usize = 64;

/// Where a save writes.
pub enum SaveTarget {
	/// Append to the file the document is opened from (D-027).
	Incremental(Arc<FxdFile>),
	/// Write a fresh file (Save As or compaction) and replace `PathBuf`.
	Fresh(PathBuf),
}

/// What one save did.
#[derive(Clone, Copy, Debug, Default)]
pub struct SaveReport {
	/// Tiles whose pixels were compressed and appended.
	pub tiles_written: u64,
	/// Tiles whose existing chunk in this file was reused.
	pub tiles_reused: u64,
	/// Chunk bytes appended (tile payloads; the manifest adds a little more).
	pub bytes_written: u64,
	pub seconds: f64,
}

/// The result of a save: the (re)opened file and the report.
pub struct SavedFxd {
	pub file: Arc<FxdFile>,
	pub report: SaveReport,
}

/// Everything a save needs besides the target.
pub struct SaveRequest<'a> {
	pub doc: &'a Document,
	pub store: &'a TileStore,
	/// The flattened composite at levels ≥ 3, if the caller renders one
	/// (D-026). `None` leaves the manifest's preview empty.
	pub preview: Option<&'a TiledImage>,
}

/// Save `request.doc` to `target`. Blocks; call on a worker thread.
///
/// Order: chunks → `sync_data` → footer → `sync_data` (inside
/// [`FxdWriter::commit`]). A crash at any point leaves the previous version
/// readable.
pub fn save(request: SaveRequest<'_>, target: SaveTarget, progress: Progress<'_>) -> Result<SavedFxd, IoError> {
	let started = Instant::now();
	let tiles = collect_tiles(request.doc, request.preview);
	let mut refs: HashMap<u64, ChunkRef> = HashMap::new();
	let mut written: Vec<(TileHandle, ChunkRef)> = Vec::new();
	let mut report = SaveReport::default();

	let incremental = matches!(target, SaveTarget::Incremental(_));
	let level = if incremental { SAVE_LEVEL } else { FRESH_LEVEL };
	let part = match &target {
		SaveTarget::Incremental(_) => None,
		SaveTarget::Fresh(path) => Some(part_path(path)),
	};
	let (mut writer, reuse_id) = match &target {
		SaveTarget::Incremental(file) => (FxdWriter::append_to((**file).clone())?, Some(file.id())),
		SaveTarget::Fresh(path) => (FxdWriter::create(&part_path(path))?, None),
	};

	let total = tiles.len().max(1);
	// Tiles already in this file keep their chunk; the others are compressed
	// on rayon, IN_FLIGHT at a time, and appended in order.
	let mut pending: Vec<&CollectedTile> = Vec::new();
	for tile in &tiles {
		if let (Some(reuse_id), Some((src, offset, len))) = (reuse_id, tile.handle.backing())
			&& src == reuse_id
		{
			refs.insert(tile.handle.id().get(), ChunkRef { offset, len });
			report.tiles_reused += 1;
		} else {
			pending.push(tile);
		}
	}
	let mut done = report.tiles_reused as usize;
	for batch in pending.chunks(IN_FLIGHT) {
		if !progress(done as f32 / total as f32) {
			return Err(IoError::Cancelled);
		}
		let compressed: Vec<Result<Option<Vec<u8>>, IoError>> = batch
			.par_iter()
			.map(|tile| match request.store.get(&tile.handle) {
				Ok(pixels) => zstd::bulk::compress(pixels.bytes(), level)
					.map(Some)
					.map_err(|e| IoError::Decode(format!("zstd tile: {e}"))),
				// A mip dropped under memory pressure is simply not stored:
				// it is rebuilt after opening, like any missing mip.
				Err(TileError::Evicted) if tile.derived => Ok(None),
				Err(error) => Err(error.into()),
			})
			.collect();
		for (tile, result) in batch.iter().zip(compressed) {
			let Some(bytes) = result? else { continue };
			let chunk = writer.tile(tile.format, Codec::Zstd, &bytes)?;
			refs.insert(tile.handle.id().get(), chunk);
			written.push((tile.handle.clone(), chunk));
			report.tiles_written += 1;
			report.bytes_written += chunk.len;
		}
		done += batch.len();
	}
	progress(1.0);

	let mut manifest = manifest::to_manifest(request.doc, |handle| refs.get(&handle.id().get()).copied())?;
	if let Some(preview) = request.preview {
		manifest.preview = Some(manifest::image_entry(preview, |handle| refs.get(&handle.id().get()).copied())?);
	}
	let payload = manifest::encode_manifest(&manifest, FRESH_LEVEL)?;
	let manifest_chunk = writer.manifest(&payload)?;
	let live = live_bytes(&refs, manifest_chunk.len);
	let committed = writer.commit(manifest_chunk, live)?;

	// Fresh: close the `.part` handle, replace the target, reopen it.
	let file = if let Some(part) = part {
		let target = match target {
			SaveTarget::Fresh(path) => path,
			SaveTarget::Incremental(_) => unreachable!("part is only set for Fresh"),
		};
		drop(committed);
		std::fs::rename(&part, &target)?;
		FxdFile::open(&target)?.0
	} else {
		Arc::new(committed)
	};

	// From now on every tile written is backed by the new file.
	for (handle, chunk) in &written {
		request.store.attach_backing(
			handle,
			Backed {
				source: file.clone(),
				offset: chunk.offset,
				len: chunk.len,
			},
		);
	}

	report.seconds = started.elapsed().as_secs_f64();
	Ok(SavedFxd { file, report })
}

/// True when dead chunks exceed half the file: the engine may compact in the
/// background (M3-T04 step 6).
pub fn needs_compaction(live_bytes: u64, file_len: u64) -> bool {
	live_bytes.saturating_mul(2) < file_len
}

/// `<target>.part` in the same folder, so the replace is a rename.
fn part_path(target: &Path) -> PathBuf {
	let mut name = target.as_os_str().to_os_string();
	name.push(".part");
	PathBuf::from(name)
}

/// Total bytes of live data a save would leave: header + every referenced
/// chunk + footer.
fn live_bytes(refs: &HashMap<u64, ChunkRef>, manifest_len: u64) -> u64 {
	let mut seen = HashSet::new();
	let mut live = HEADER_LEN + FOOTER_LEN + manifest_len;
	for chunk in refs.values() {
		if seen.insert(chunk.offset) {
			live += chunk.len;
		}
	}
	live
}

/// One tile a save stores.
struct CollectedTile {
	format: PixelFormat,
	handle: TileHandle,
	/// A mip tile (level ≥ 3): may be skipped if it was evicted.
	derived: bool,
}

/// Every real tile of the document and the preview, deduplicated and in a
/// deterministic (id) order. Dirty mip tiles are stale and left out (the
/// manifest skips them too, see [`manifest::image_entry`]).
fn collect_tiles(doc: &Document, preview: Option<&TiledImage>) -> Vec<CollectedTile> {
	let mut seen = HashSet::new();
	let mut out = Vec::new();
	let mut visit = |image: &TiledImage| {
		for level in stored_levels(image) {
			for (tx, ty, slot) in image.grid(level).non_empty() {
				if level != 0 && image.is_dirty(level, tx, ty) {
					continue;
				}
				if let TileSlot::Data(handle) = slot
					&& seen.insert(handle.id().get())
				{
					out.push(CollectedTile {
						format: image.format(),
						handle: handle.clone(),
						derived: level != 0,
					});
				}
			}
		}
	};
	for image in images_of(doc) {
		visit(image);
	}
	if let Some(preview) = preview {
		visit(preview);
	}
	out.sort_by_key(|tile| tile.handle.id().get());
	out
}

/// Levels a save stores: level 0 and the derived levels ≥ 3 (D-026).
pub(crate) fn stored_levels(image: &TiledImage) -> impl Iterator<Item = usize> + '_ {
	(0..image.level_count()).filter(|&level| level == 0 || level >= 3)
}

/// Every `TiledImage` of a document (layer pixels and masks).
pub(crate) fn images_of(doc: &Document) -> Vec<&TiledImage> {
	fn go<'a>(layer: &'a Layer, out: &mut Vec<&'a TiledImage>) {
		if let Some(mask) = &layer.mask {
			out.push(&mask.image);
		}
		match &layer.kind {
			LayerKind::Pixel { image, .. } => out.push(image),
			LayerKind::Group { children, .. } => {
				for child in children {
					go(child, out);
				}
			}
			_ => {}
		}
	}
	let mut out = Vec::new();
	for layer in &doc.layers {
		go(layer, &mut out);
	}
	out
}

#[cfg(test)]
mod tests {
	use fx_core::{BitDepth, ColorProfile, DocumentColor, Layer, LayerId, LayerKind};
	use fx_tiles::{PixelValue, TileBuffer, TileStoreConfig};

	use super::super::manifest;
	use super::*;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-io-fxd-save-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let mut config = TileStoreConfig::for_tests(dir.join("scratch"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).unwrap()
	}

	fn path(name: &str) -> PathBuf {
		let dir = std::env::temp_dir().join("fx-io-fxd-save-tests");
		std::fs::create_dir_all(&dir).unwrap();
		dir.join(name)
	}

	/// A 512×512 document with one pixel layer holding one real tile and one
	/// solid layer. Ids are deterministic (1 = pixels, 2 = solid).
	fn document(store: &TileStore, value: u16) -> Document {
		let mut doc = Document::new(
			512,
			512,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			300.0,
		);
		let pixel_id = doc.allocate_layer_id();
		let solid_id = doc.allocate_layer_id();

		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba16);
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		buffer.as_u16_mut()[0] = value;
		image.put_buffer(store, 0, 0, buffer);
		image.set_slot(1, 0, TileSlot::Solid(PixelValue::rgba16(1, 2, 3, 4)));

		doc.layers = vec![
			Arc::new(Layer::new(pixel_id, "Pixels", LayerKind::Pixel { image, offset: (0, 0) })),
			Arc::new(Layer::new(solid_id, "Solid", LayerKind::SolidFill { rgba: [9, 9, 9, 9] })),
		];
		doc.selected = vec![pixel_id];
		doc
	}

	fn store_get(doc: &Document, store: &TileStore) -> Vec<u8> {
		let image = super::images_of(doc)[0];
		let TileSlot::Data(handle) = image.slot(0, 0, 0) else {
			panic!("no data tile")
		};
		store.get(handle).unwrap().bytes().to_vec()
	}

	#[test]
	fn clean_mips_are_stored_and_dirty_ones_are_not() {
		let store = store();
		// 4096² → 16×16 tiles at level 0, a single tile at level 4.
		let mut image = TiledImage::new(4096, 4096, PixelFormat::Rgba16);
		image.put_buffer(&store, 0, 0, TileBuffer::filled(PixelFormat::Rgba16, PixelValue::rgba16(5, 6, 7, 65535)));
		let mut mip = TileBuffer::zeroed(PixelFormat::Rgba16);
		mip.as_u16_mut()[3] = 777;
		let mip = store.insert(mip, fx_tiles::TileClass::Derived);
		image.set_derived_slot(3, 0, 0, TileSlot::Data(mip));
		image.set_derived_slot(4, 0, 0, TileSlot::Solid(PixelValue::rgba16(1, 1, 1, 1)));
		// A later level-0 edit at the far corner makes (3: 1,1) and (4: 0,0)
		// stale; (3: 0,0) stays valid.
		let mut edit = TileBuffer::zeroed(PixelFormat::Rgba16);
		edit.as_u16_mut()[0] = 9;
		image.put_buffer(&store, 15, 15, edit);
		assert!(image.is_dirty(4, 0, 0) && !image.is_dirty(3, 0, 0));

		let mut doc = document(&store, 1);
		let id = doc.allocate_layer_id();
		doc.layers.push(Arc::new(Layer::new(id, "Big", LayerKind::Pixel { image, offset: (0, 0) })));
		let path = path("mips.fxd");
		let saved = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.unwrap();
		let footer = saved.file.footer();
		let (_, payload) = saved
			.file
			.read_chunk(ChunkRef {
				offset: footer.manifest_offset,
				len: footer.manifest_len,
			})
			.unwrap();
		let manifest = manifest::decode_manifest(&payload).unwrap();
		let restored = manifest::from_manifest(&manifest, &saved.file, &store).unwrap();
		let LayerKind::Pixel { image, .. } = &restored.layer(id).unwrap().kind else {
			panic!("pixel layer")
		};
		let TileSlot::Data(handle) = image.slot(3, 0, 0) else {
			panic!("the clean mip was stored")
		};
		assert_eq!(store.get(handle).unwrap().as_u16()[3], 777);
		assert!(image.slot(4, 0, 0).is_empty(), "the stale mip was not stored");
		assert!(image.is_dirty(4, 0, 0), "and is rebuilt after opening");
	}

	#[test]
	fn fresh_then_incremental_reuses_every_tile() {
		let store = store();
		let mut doc = document(&store, 1234);
		let path = path("incremental.fxd");

		let first = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.unwrap();
		assert_eq!(first.report.tiles_written, 1, "one real tile");
		assert_eq!(first.report.tiles_reused, 0);

		// A property change does not touch any tile.
		doc.layer_mut(LayerId(1)).unwrap().opacity = 0.25;
		let second = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Incremental(first.file.clone()),
			&mut |_| true,
		)
		.unwrap();
		assert_eq!(second.report.tiles_written, 0, "no tile changed");
		assert_eq!(second.report.tiles_reused, 1);

		// The new manifest is found and carries the new opacity.
		let (file, _) = FxdFile::open(&path).unwrap();
		let footer = file.footer();
		let (_, payload) = file
			.read_chunk(ChunkRef {
				offset: footer.manifest_offset,
				len: footer.manifest_len,
			})
			.unwrap();
		let manifest = manifest::decode_manifest(&payload).unwrap();
		let restored = manifest::from_manifest(&manifest, &file, &store).unwrap();
		assert_eq!(restored.layer(LayerId(1)).unwrap().opacity, 0.25);
		assert_eq!(store_get(&restored, &store), store_get(&doc, &store));
		assert!(second.file.footer().end_offset > first.file.footer().end_offset);
	}

	#[test]
	fn painting_one_tile_writes_exactly_that_tile() {
		let store = store();
		let mut doc = document(&store, 10);
		let path = path("painted.fxd");

		let first = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(path),
			&mut |_| true,
		)
		.unwrap();

		// Paint the real tile (a new, unbacked handle).
		let image = doc.layer_mut(LayerId(1)).unwrap();
		let LayerKind::Pixel { image, .. } = &mut image.kind else {
			panic!("not a pixel layer")
		};
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		buffer.as_u16_mut()[0] = 999;
		image.put_buffer(&store, 0, 0, buffer);

		let second = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Incremental(first.file.clone()),
			&mut |_| true,
		)
		.unwrap();
		assert_eq!(second.report.tiles_written, 1, "only the painted level-0 tile");
		assert_eq!(second.report.tiles_reused, 0);
	}

	#[test]
	fn compaction_shrinks_the_file_and_keeps_every_pixel() {
		let store = store();
		let mut doc = document(&store, 77);
		let grown_path = path("compact.fxd");

		let first = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(grown_path.clone()),
			&mut |_| true,
		)
		.unwrap();
		let mut file = first.file;

		// Many manifest-only saves append dead weight.
		for i in 0..20u16 {
			doc.layer_mut(LayerId(1)).unwrap().opacity = 1.0 - i as f32 / 100.0;
			file = save(
				SaveRequest {
					doc: &doc,
					store: &store,
					preview: None,
				},
				SaveTarget::Incremental(file),
				&mut |_| true,
			)
			.unwrap()
			.file;
		}
		let grown = std::fs::metadata(&grown_path).unwrap().len();

		// Compaction: a fresh save to a new path drops the dead chunks.
		let compact_path = path("compact-new.fxd");
		let compacted = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(compact_path.clone()),
			&mut |_| true,
		)
		.unwrap();
		let small = std::fs::metadata(&compact_path).unwrap().len();
		assert!(small < grown, "compaction shrank the file ({small} < {grown})");

		let (reopened, _) = FxdFile::open(&compact_path).unwrap();
		let footer = reopened.footer();
		let (_, payload) = reopened
			.read_chunk(ChunkRef {
				offset: footer.manifest_offset,
				len: footer.manifest_len,
			})
			.unwrap();
		let manifest = manifest::decode_manifest(&payload).unwrap();
		let restored = manifest::from_manifest(&manifest, &reopened, &store).unwrap();
		assert_eq!(store_get(&restored, &store), store_get(&doc, &store));
		assert!(!compacted.report.seconds.is_nan());
	}

	#[test]
	fn a_truncated_last_save_opens_the_previous_version() {
		let store = store();
		let mut doc = document(&store, 5);
		let path = path("interrupted.fxd");

		let first = save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(path.clone()),
			&mut |_| true,
		)
		.unwrap();
		let len1 = std::fs::metadata(&path).unwrap().len();

		doc.layer_mut(LayerId(1)).unwrap().opacity = 0.5;
		save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Incremental(first.file.clone()),
			&mut |_| true,
		)
		.unwrap();
		let len2 = std::fs::metadata(&path).unwrap().len();
		assert!(len2 > len1);

		// Simulate a crash mid-save: the bytes of the second save are gone.
		let full = std::fs::read(&path).unwrap();
		std::fs::write(&path, &full[..len1 as usize]).unwrap();
		let (reopened, footer) = FxdFile::open(&path).unwrap();
		assert_eq!(footer.manifest_offset, first.file.footer().manifest_offset, "the previous footer is used");

		let (_, payload) = reopened
			.read_chunk(ChunkRef {
				offset: footer.manifest_offset,
				len: footer.manifest_len,
			})
			.unwrap();
		let manifest = manifest::decode_manifest(&payload).unwrap();
		let restored = manifest::from_manifest(&manifest, &reopened, &store).unwrap();
		assert_eq!(restored.layer(LayerId(1)).unwrap().opacity, 1.0);
		assert_eq!(store_get(&restored, &store), store_get(&doc, &store));
	}
}
