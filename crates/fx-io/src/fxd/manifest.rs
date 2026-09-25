//! The `.fxd` manifest (M3-T02): a versioned serde model of a [`Document`].
//!
//! The model lives in `fx-io` (not `fx-core`) so the document stays free of
//! file concerns. It is a plain data tree: converting a [`Document`] to a
//! [`Manifest`] and back never touches pixels — the image entries reference
//! tile chunks that the save path has already written.
//!
//! A manifest is serialised to JSON and zstd-compressed before it is stored in
//! a `MANIFEST` chunk (D-025). Unknown JSON fields are ignored (forward
//! compatibility); a version other than [`MANIFEST_VERSION`] is refused.

use std::fmt;
use std::sync::Arc;

use fx_core::{Adjustment, BlendMode, Document, DocumentColor, Layer, LayerId, LayerKind, Mask};
use fx_tiles::{Backed, PixelFormat, PixelValue, TileClass, TileHandle, TileSlot, TileStore, TiledImage};
use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::container::{ChunkRef, FxdFile};
use crate::IoError;

/// Manifest format version stored in [`Manifest::version`].
pub const MANIFEST_VERSION: u32 = 1;

/// The whole document structure. Level-0 pixels are referenced by chunk, not
/// stored here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
	pub version: u32,
	pub width: u32,
	pub height: u32,
	pub color: DocumentColor,
	pub ppi: f32,
	pub next_layer_id: u64,
	/// Per-kind default-name counters, in `fx_core`'s `NameKind` order. A
	/// list, not an array: kinds are appended over time, and a file written
	/// with fewer counters reads the missing ones as 0.
	pub name_counters: Vec<u32>,
	/// Selected layers in the panel (the last is active). *Not* the pixel
	/// selection, which is not saved (D-028).
	pub selected: Vec<LayerId>,
	/// Root layers, bottom → top.
	pub layers: Vec<LayerEntry>,
	/// Flattened composite preview at levels ≥ 3, if the save produced one
	/// (M3-T04).
	pub preview: Option<ImageEntry>,
}

/// One layer: every [`Layer`] field plus its kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerEntry {
	pub id: LayerId,
	pub name: String,
	pub visible: bool,
	pub opacity: f32,
	pub fill: f32,
	pub blend: BlendMode,
	pub clipped: bool,
	pub locked_pixels: bool,
	/// M5 (D-049); files saved before read `false`.
	#[serde(default)]
	pub locked_transparency: bool,
	pub locked_position: bool,
	pub mask: Option<MaskEntry>,
	#[serde(flatten)]
	pub kind: LayerKindEntry,
}

/// The kind-specific part of a [`LayerEntry`], tagged by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayerKindEntry {
	Pixel { offset: (i32, i32), image: ImageEntry },
	Group { expanded: bool, children: Vec<LayerEntry> },
	Adjustment { adjustment: Adjustment },
	SolidFill { rgba: [u16; 4] },
}

/// A layer mask.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskEntry {
	pub enabled: bool,
	pub linked: bool,
	pub outside_value: u16,
	pub image: ImageEntry,
}

/// A tiled image's stored levels. Only non-empty slots are listed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageEntry {
	pub width: u32,
	pub height: u32,
	pub format: PixelFormat,
	pub levels: Vec<LevelEntry>,
}

/// One stored mip level. Level 0 is authoritative; levels ≥ 3 are stored as
/// `derived: true` (D-026). Levels 1–2 are never stored (rebuilt lazily).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelEntry {
	pub level: u32,
	pub derived: bool,
	pub slots: Vec<SlotEntry>,
}

/// One non-empty tile slot, encoded as a compact JSON array:
/// `[tx, ty, "s", r, g, b, a]` (solid) or `[tx, ty, "t", offset, len]` (tile
/// chunk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotEntry {
	Solid { tx: u32, ty: u32, value: [u16; 4] },
	Tile { tx: u32, ty: u32, chunk: ChunkRef },
}

impl Serialize for SlotEntry {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			SlotEntry::Solid { tx, ty, value } => {
				let mut tuple = serializer.serialize_tuple(7)?;
				tuple.serialize_element(tx)?;
				tuple.serialize_element(ty)?;
				tuple.serialize_element("s")?;
				for channel in value {
					tuple.serialize_element(channel)?;
				}
				tuple.end()
			}
			SlotEntry::Tile { tx, ty, chunk } => {
				let mut tuple = serializer.serialize_tuple(5)?;
				tuple.serialize_element(tx)?;
				tuple.serialize_element(ty)?;
				tuple.serialize_element("t")?;
				tuple.serialize_element(&chunk.offset)?;
				tuple.serialize_element(&chunk.len)?;
				tuple.end()
			}
		}
	}
}

impl<'de> Deserialize<'de> for SlotEntry {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct SlotVisitor;

		impl<'de> Visitor<'de> for SlotVisitor {
			type Value = SlotEntry;

			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("a [tx, ty, \"s\", r, g, b, a] or [tx, ty, \"t\", offset, len] slot")
			}

			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SlotEntry, A::Error> {
				let tx = seq.next_element::<u32>()?.ok_or_else(|| de::Error::invalid_length(0, &self))?;
				let ty = seq.next_element::<u32>()?.ok_or_else(|| de::Error::invalid_length(1, &self))?;
				let tag = seq.next_element::<String>()?.ok_or_else(|| de::Error::invalid_length(2, &self))?;
				match tag.as_str() {
					"s" => {
						let mut value = [0u16; 4];
						for (i, channel) in value.iter_mut().enumerate() {
							*channel = seq.next_element()?.ok_or_else(|| de::Error::invalid_length(3 + i, &self))?;
						}
						Ok(SlotEntry::Solid { tx, ty, value })
					}
					"t" => {
						let offset = seq.next_element::<u64>()?.ok_or_else(|| de::Error::invalid_length(3, &self))?;
						let len = seq.next_element::<u64>()?.ok_or_else(|| de::Error::invalid_length(4, &self))?;
						Ok(SlotEntry::Tile {
							tx,
							ty,
							chunk: ChunkRef { offset, len },
						})
					}
					other => Err(de::Error::custom(format!("unknown slot tag {other:?}"))),
				}
			}
		}

		deserializer.deserialize_seq(SlotVisitor)
	}
}

// ---------------------------------------------------------------------------
// Document -> Manifest
// ---------------------------------------------------------------------------

/// Build the manifest of `doc`. `tile_ref` maps a stored tile to the chunk the
/// save path wrote for it; `None` for a derived (mip) tile the save skipped
/// because it was evicted — it is then left out and rebuilt after opening.
///
/// Fails when a level-0 tile has no chunk: every authoritative tile must have
/// been written, so a save reports an error instead of a later panic (review
/// 2026-09-25, S3-04).
pub fn to_manifest(doc: &Document, tile_ref: impl Fn(&TileHandle) -> Option<ChunkRef>) -> Result<Manifest, IoError> {
	let tile_ref = &tile_ref;
	let (next_layer_id, name_counters) = doc.id_state();
	Ok(Manifest {
		version: MANIFEST_VERSION,
		width: doc.width,
		height: doc.height,
		color: doc.color.clone(),
		ppi: doc.ppi,
		next_layer_id,
		name_counters: name_counters.to_vec(),
		selected: doc.selected.clone(),
		layers: doc.layers.iter().map(|layer| layer_entry(layer, tile_ref)).collect::<Result<Vec<_>, _>>()?,
		// The flattened composite preview is rendered by the save path (M3-T04).
		preview: None,
	})
}

fn layer_entry(layer: &Layer, tile_ref: &impl Fn(&TileHandle) -> Option<ChunkRef>) -> Result<LayerEntry, IoError> {
	let mask = match &layer.mask {
		Some(mask) => Some(MaskEntry {
			enabled: mask.enabled,
			linked: mask.linked,
			outside_value: mask.outside_value,
			image: image_entry(&mask.image, tile_ref)?,
		}),
		None => None,
	};
	let kind = match &layer.kind {
		LayerKind::Pixel { image, offset } => LayerKindEntry::Pixel {
			offset: *offset,
			image: image_entry(image, tile_ref)?,
		},
		LayerKind::Group { children, expanded } => LayerKindEntry::Group {
			expanded: *expanded,
			children: children.iter().map(|child| layer_entry(child, tile_ref)).collect::<Result<_, _>>()?,
		},
		LayerKind::Adjustment(adjustment) => LayerKindEntry::Adjustment {
			adjustment: adjustment.clone(),
		},
		LayerKind::SolidFill { rgba } => LayerKindEntry::SolidFill { rgba: *rgba },
	};
	Ok(LayerEntry {
		id: layer.id,
		name: layer.name.clone(),
		visible: layer.visible,
		opacity: layer.opacity,
		fill: layer.fill,
		blend: layer.blend,
		clipped: layer.clipped,
		locked_pixels: layer.locked_pixels,
		locked_transparency: layer.locked_transparency,
		locked_position: layer.locked_position,
		mask,
		kind,
	})
}

/// Build the stored-level entry of one image (level 0 and levels ≥ 3, only
/// non-empty slots). Public so the save path can build the preview entry.
///
/// Mip slots that are dirty (older than level 0) or that the save could not
/// store (`tile_ref` → `None`) are left out: after opening they are empty and
/// dirty, so the renderer rebuilds them. Storing a stale mip would show old
/// pixels until the next edit.
///
/// A level-0 slot with no chunk is an error: the save must have written every
/// authoritative tile (review 2026-09-25, S3-04).
pub fn image_entry(image: &TiledImage, tile_ref: impl Fn(&TileHandle) -> Option<ChunkRef>) -> Result<ImageEntry, IoError> {
	let tile_ref = &tile_ref;
	let mut levels = Vec::new();
	for level in 0..image.level_count() {
		// Level 0 is authoritative; levels ≥ 3 are stored derived (D-026);
		// levels 1–2 are rebuilt lazily and never stored.
		if level != 0 && level < 3 {
			continue;
		}
		let grid = image.grid(level);
		let mut slots = Vec::new();
		for (tx, ty, slot) in grid.non_empty() {
			if level != 0 && image.is_dirty(level, tx, ty) {
				continue;
			}
			match slot {
				TileSlot::Empty => {}
				TileSlot::Solid(value) => slots.push(SlotEntry::Solid { tx, ty, value: value.0 }),
				TileSlot::Data(handle) => match tile_ref(handle) {
					Some(chunk) => slots.push(SlotEntry::Tile { tx, ty, chunk }),
					None if level != 0 => {}
					None => {
						return Err(IoError::Decode(format!("level-0 tile {tx},{ty} was not written by the save")));
					}
				},
			}
		}
		if slots.is_empty() {
			continue;
		}
		levels.push(LevelEntry {
			level: level as u32,
			derived: level != 0,
			slots,
		});
	}
	Ok(ImageEntry {
		width: image.width(),
		height: image.height(),
		format: image.format(),
		levels,
	})
}

// ---------------------------------------------------------------------------
// Manifest -> Document
// ---------------------------------------------------------------------------

/// Rebuild a document from a manifest. Every tile slot becomes a **backed**
/// tile that reads its pixels from `file` on demand — nothing is read here, so
/// opening a document costs only the manifest.
///
/// Levels 1–2 are not in the manifest and are rebuilt lazily when the renderer
/// asks for them (their ancestors start dirty after the level-0 writes).
pub fn from_manifest(manifest: &Manifest, file: &Arc<FxdFile>, store: &TileStore) -> Result<Document, IoError> {
	if manifest.version != MANIFEST_VERSION {
		return Err(IoError::Unsupported(format!("fxd manifest version {}", manifest.version)));
	}
	let mut doc = Document::new(manifest.width, manifest.height, manifest.color.clone(), manifest.ppi);
	doc.layers = manifest
		.layers
		.iter()
		.map(|entry| layer_from_entry(entry, file, store))
		.collect::<Result<Vec<_>, _>>()?;
	doc.selected = manifest.selected.clone();
	let mut counters = [0u32; fx_core::NAME_KINDS];
	for (slot, value) in counters.iter_mut().zip(&manifest.name_counters) {
		*slot = *value;
	}
	Ok(doc.with_id_state(manifest.next_layer_id, counters))
}

fn layer_from_entry(entry: &LayerEntry, file: &Arc<FxdFile>, store: &TileStore) -> Result<Arc<Layer>, IoError> {
	let kind = match &entry.kind {
		LayerKindEntry::Pixel { offset, image } => LayerKind::Pixel {
			image: image_from_entry(image, file, store)?,
			offset: *offset,
		},
		LayerKindEntry::Group { expanded, children } => LayerKind::Group {
			expanded: *expanded,
			children: children.iter().map(|child| layer_from_entry(child, file, store)).collect::<Result<_, _>>()?,
		},
		LayerKindEntry::Adjustment { adjustment } => LayerKind::Adjustment(adjustment.clone()),
		LayerKindEntry::SolidFill { rgba } => LayerKind::SolidFill { rgba: *rgba },
	};
	let mut layer = Layer::new(entry.id, entry.name.clone(), kind);
	layer.visible = entry.visible;
	layer.opacity = entry.opacity;
	layer.fill = entry.fill;
	layer.blend = entry.blend;
	layer.clipped = entry.clipped;
	layer.locked_pixels = entry.locked_pixels;
	layer.locked_transparency = entry.locked_transparency;
	layer.locked_position = entry.locked_position;
	layer.mask = match &entry.mask {
		Some(mask) => Some(Mask {
			image: image_from_entry(&mask.image, file, store)?,
			enabled: mask.enabled,
			linked: mask.linked,
			outside_value: mask.outside_value,
		}),
		None => None,
	};
	Ok(Arc::new(layer))
}

/// Rebuild one `TiledImage` from its manifest entry, with backed tiles.
/// Public so the opener can rebuild the composite preview the same way.
pub fn image_from_entry(entry: &ImageEntry, file: &Arc<FxdFile>, store: &TileStore) -> Result<TiledImage, IoError> {
	let mut image = TiledImage::new(entry.width, entry.height, entry.format);
	for level in &entry.levels {
		let derived = level.level != 0;
		for slot in &level.slots {
			let (tx, ty) = match slot {
				SlotEntry::Solid { tx, ty, .. } => (*tx, *ty),
				SlotEntry::Tile { tx, ty, .. } => (*tx, *ty),
			};
			let tile_slot = match slot {
				SlotEntry::Solid { value, .. } => TileSlot::Solid(PixelValue(*value)),
				SlotEntry::Tile { chunk, .. } => {
					let backed = Backed {
						source: file.clone(),
						offset: chunk.offset,
						len: chunk.len,
					};
					let class = if derived { TileClass::Derived } else { TileClass::Authoritative };
					TileSlot::Data(store.insert_backed(entry.format, class, backed))
				}
			};
			if derived {
				image.set_derived_slot(level.level as usize, tx, ty, tile_slot);
			} else {
				image.set_slot(tx, ty, tile_slot);
			}
		}
	}
	Ok(image)
}

// ---------------------------------------------------------------------------
// JSON + zstd
// ---------------------------------------------------------------------------

/// Serialise a manifest to compact JSON.
pub fn manifest_to_json(manifest: &Manifest) -> Result<Vec<u8>, IoError> {
	serde_json::to_vec(manifest).map_err(|e| IoError::Decode(format!("manifest JSON: {e}")))
}

/// Parse a manifest from JSON, refusing a version other than
/// [`MANIFEST_VERSION`] with a clear error. Unknown fields are ignored.
pub fn manifest_from_json(json: &[u8]) -> Result<Manifest, IoError> {
	#[derive(Deserialize)]
	struct VersionProbe {
		version: u32,
	}
	// Read the version first so a future manifest shape still gets a clear
	// "wrong version" error instead of a confusing JSON error.
	let probe: VersionProbe = serde_json::from_slice(json).map_err(|e| IoError::Decode(format!("manifest JSON: {e}")))?;
	if probe.version != MANIFEST_VERSION {
		return Err(IoError::Unsupported(format!("fxd manifest version {}", probe.version)));
	}
	serde_json::from_slice(json).map_err(|e| IoError::Decode(format!("manifest JSON: {e}")))
}

/// Encode a manifest as a zstd frame (the payload of a `MANIFEST` chunk).
pub fn encode_manifest(manifest: &Manifest, level: i32) -> Result<Vec<u8>, IoError> {
	let json = manifest_to_json(manifest)?;
	zstd::bulk::compress(&json, level).map_err(|e| IoError::Decode(format!("manifest zstd: {e}")))
}

/// Decode a `MANIFEST` chunk payload (a zstd frame of UTF-8 JSON).
pub fn decode_manifest(payload: &[u8]) -> Result<Manifest, IoError> {
	// A manifest is small; this is a hard ceiling against a hostile frame.
	const LIMIT: usize = 1 << 30;
	let json = zstd::bulk::decompress(payload, LIMIT).map_err(|e| IoError::Decode(format!("manifest zstd: {e}")))?;
	manifest_from_json(&json)
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;
	use std::sync::Arc;

	use fx_core::{Adjustment, BitDepth, BlendMode, ColorProfile, Document, DocumentColor, Layer, LayerId, LayerKind, Mask};
	use fx_tiles::{PixelFormat, PixelValue, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

	use super::super::container::{ChunkKind, ChunkRef, Codec, FxdFile, FxdWriter};
	use super::{
		LayerKindEntry, MANIFEST_VERSION, Manifest, SlotEntry, decode_manifest, encode_manifest, from_manifest, image_entry, manifest_from_json,
		manifest_to_json, to_manifest,
	};
	use crate::IoError;

	fn store() -> TileStore {
		let dir = std::env::temp_dir().join("fx-io-fxd-manifest-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let mut config = TileStoreConfig::for_tests(dir.join("scratch"));
		config.hot_budget = 1 << 30;
		TileStore::new(config).unwrap()
	}

	fn solid(id: LayerId, name: &str, rgba: [u16; 4]) -> Arc<Layer> {
		Arc::new(Layer::new(id, name, LayerKind::SolidFill { rgba }))
	}

	/// A document exercising every layer kind, nested groups, masks (linked and
	/// not), clipping, negative offsets, locks and every adjustment kind.
	fn sample_document(store: &TileStore) -> Document {
		let mut doc = Document::new(
			2048,
			2048,
			DocumentColor {
				depth: BitDepth::U16,
				profile: ColorProfile::Srgb,
			},
			300.0,
		);
		let ids: Vec<LayerId> = (0..12).map(|_| doc.allocate_layer_id()).collect();

		// A pixel layer: a solid slot, a real (backed-style) slot, a derived
		// level 3 that is stored and a level 1 that must not be.
		let mut pixel_image = TiledImage::new(2048, 2048, PixelFormat::Rgba16);
		pixel_image.set_slot(0, 0, TileSlot::Solid(PixelValue::rgba8(10, 20, 30, 255)));
		// A non-uniform buffer stays a real tile (a filled one collapses to Solid).
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		buffer.as_u16_mut()[0] = 1234;
		pixel_image.put_buffer(store, 1, 0, buffer);
		pixel_image.set_derived_slot(3, 0, 0, TileSlot::Solid(PixelValue::rgba16(7, 7, 7, 7)));
		pixel_image.set_derived_slot(1, 0, 0, TileSlot::Solid(PixelValue::rgba16(9, 9, 9, 9)));

		let mut mask_image = TiledImage::new(2048, 2048, PixelFormat::Gray16);
		mask_image.put_buffer(store, 0, 0, TileBuffer::filled(PixelFormat::Gray16, PixelValue::gray16(40000)));

		let mut pixel = Layer::new(
			ids[1],
			"Pixels",
			LayerKind::Pixel {
				image: pixel_image,
				offset: (-300, 40),
			},
		);
		pixel.blend = BlendMode::Multiply;
		pixel.opacity = 0.5;
		pixel.fill = 0.25;
		pixel.clipped = true;
		pixel.locked_pixels = true;
		pixel.mask = Some(Mask {
			image: mask_image,
			enabled: false,
			linked: true,
			outside_value: 1234,
		});

		let mut bottom = Layer::new(ids[0], "Solid", LayerKind::SolidFill { rgba: [1, 2, 3, 4] });
		bottom.mask = Some(Mask {
			image: TiledImage::new(2048, 2048, PixelFormat::Gray8),
			enabled: true,
			linked: false,
			outside_value: 0,
		});

		// Nested groups: Group -> Nested -> { Inner solid, Invert adjustment }.
		let nested = Arc::new(Layer::new(
			ids[3],
			"Nested",
			LayerKind::Group {
				expanded: true,
				children: vec![
					solid(ids[2], "Inner", [9, 9, 9, 9]),
					Arc::new(Layer::new(ids[4], "Nested Invert", LayerKind::Adjustment(Adjustment::Invert))),
				],
			},
		));
		let group = Arc::new(Layer::new(
			ids[5],
			"Group",
			LayerKind::Group {
				expanded: false,
				children: vec![nested],
			},
		));

		// `LevelsChannel` is not re-exported by fx-core; build the adjustment by
		// inference and set its public channel fields directly.
		let mut levels = Adjustment::Levels {
			channels: [Default::default(); 4],
		};
		if let Adjustment::Levels { channels } = &mut levels {
			channels[0].in_black = 0.1;
			channels[0].in_white = 0.9;
			channels[0].gamma = 1.2;
		}

		let adjustments: Vec<Arc<Layer>> = vec![
			Arc::new(Layer::new(
				ids[6],
				"Brightness/Contrast",
				LayerKind::Adjustment(Adjustment::BrightnessContrast {
					brightness: 0.1,
					contrast: -0.2,
					legacy: true,
				}),
			)),
			Arc::new(Layer::new(ids[7], "Levels", LayerKind::Adjustment(levels))),
			Arc::new(Layer::new(
				ids[8],
				"Curves",
				LayerKind::Adjustment(Adjustment::Curves {
					channels: [vec![(0.0, 0.0), (0.5, 0.6), (1.0, 1.0)], vec![], vec![], vec![]],
				}),
			)),
			Arc::new(Layer::new(
				ids[9],
				"Exposure",
				LayerKind::Adjustment(Adjustment::Exposure {
					exposure: 0.3,
					offset: 0.05,
					gamma: 1.1,
				}),
			)),
			Arc::new(Layer::new(
				ids[10],
				"Hue/Saturation",
				LayerKind::Adjustment(Adjustment::HueSaturation {
					hue: 20.0,
					saturation: -10.0,
					lightness: 5.0,
					colorize: true,
				}),
			)),
		];

		let mut top = Layer::new(ids[11], "Locked", LayerKind::SolidFill { rgba: [0, 0, 0, 65535] });
		top.locked_position = true;
		top.visible = false;

		doc.layers = vec![Arc::new(bottom), Arc::new(pixel), group];
		doc.layers.extend(adjustments);
		doc.layers.push(Arc::new(top));
		doc.selected = vec![ids[1], ids[5]];
		doc
	}

	fn find_layer<'a>(manifest: &'a Manifest, name: &str) -> &'a super::LayerEntry {
		fn go<'a>(layers: &'a [super::LayerEntry], name: &str) -> Option<&'a super::LayerEntry> {
			for layer in layers {
				if layer.name == name {
					return Some(layer);
				}
				if let LayerKindEntry::Group { children, .. } = &layer.kind
					&& let Some(found) = go(children, name)
				{
					return Some(found);
				}
			}
			None
		}
		go(&manifest.layers, name).unwrap()
	}

	#[test]
	fn a_document_with_every_layer_kind_round_trips_through_the_manifest() {
		let store = store();
		let doc = sample_document(&store);
		let manifest = to_manifest(&doc, |handle| {
			Some(ChunkRef {
				offset: handle.id().get() * 100,
				len: 1234,
			})
		})
		.unwrap();

		// Top-level fields come straight from the document.
		let (next_id, counters) = doc.id_state();
		assert_eq!(manifest.version, MANIFEST_VERSION);
		assert_eq!((manifest.width, manifest.height), (doc.width, doc.height));
		assert_eq!(manifest.ppi, doc.ppi);
		assert_eq!(manifest.next_layer_id, next_id);
		assert_eq!(manifest.name_counters, counters);
		assert_eq!(manifest.selected, doc.selected);
		assert!(manifest.preview.is_none());

		// The pixel layer: negative offset, level 0 + derived level 3, no level 1-2.
		let pixel = find_layer(&manifest, "Pixels");
		let LayerKindEntry::Pixel { offset, image } = &pixel.kind else {
			panic!("Pixels is not a pixel layer")
		};
		assert_eq!(*offset, (-300, 40));
		assert_eq!(image.levels.iter().map(|l| l.level).collect::<Vec<_>>(), vec![0, 3]);
		assert!(!image.levels[0].derived && image.levels[1].derived);
		assert!(image.levels[0].slots.iter().any(|s| matches!(s, SlotEntry::Solid { tx: 0, ty: 0, .. })));
		assert!(image.levels[0].slots.iter().any(|s| matches!(s, SlotEntry::Tile { tx: 1, ty: 0, .. })));

		// The nested group tree survives.
		let group = find_layer(&manifest, "Group");
		let LayerKindEntry::Group { expanded, children } = &group.kind else {
			panic!("Group is not a group")
		};
		assert!(!*expanded && children.len() == 1);
		let nested = find_layer(&manifest, "Nested");
		let LayerKindEntry::Group { children, .. } = &nested.kind else {
			panic!("Nested is not a group")
		};
		assert_eq!(children.len(), 2);

		// Linked and unlinked masks.
		assert!(pixel.mask.as_ref().unwrap().linked);
		assert_eq!(pixel.mask.as_ref().unwrap().outside_value, 1234);
		let bottom = find_layer(&manifest, "Solid");
		assert!(!bottom.mask.as_ref().unwrap().linked);

		// JSON round trip, with an unknown field added (forward compatibility).
		let json = manifest_to_json(&manifest).unwrap();
		let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
		value
			.as_object_mut()
			.unwrap()
			.insert("something_new".into(), serde_json::json!({ "future": true }));
		let with_unknown = serde_json::to_vec(&value).unwrap();
		assert_eq!(manifest_from_json(&with_unknown).unwrap(), manifest);

		// zstd frame round trip.
		let compressed = encode_manifest(&manifest, 3).unwrap();
		assert_eq!(decode_manifest(&compressed).unwrap(), manifest);
	}

	#[test]
	fn a_version_2_manifest_is_refused() {
		let store = store();
		let doc = sample_document(&store);
		let mut manifest = to_manifest(&doc, |_| Some(ChunkRef { offset: 0, len: 0 })).unwrap();
		manifest.version = 2;
		let json = serde_json::to_vec(&manifest).unwrap();
		let err = manifest_from_json(&json).unwrap_err();
		assert!(matches!(&err, IoError::Unsupported(m) if m.contains("manifest version 2")), "got {err:?}");
	}

	#[test]
	fn slots_encode_as_compact_arrays() {
		assert_eq!(
			serde_json::to_string(&SlotEntry::Solid {
				tx: 1,
				ty: 2,
				value: [3, 4, 5, 6]
			})
			.unwrap(),
			"[1,2,\"s\",3,4,5,6]"
		);
		assert_eq!(
			serde_json::to_string(&SlotEntry::Tile {
				tx: 1,
				ty: 2,
				chunk: ChunkRef { offset: 7, len: 8 }
			})
			.unwrap(),
			"[1,2,\"t\",7,8]"
		);
		assert_eq!(
			serde_json::from_str::<SlotEntry>("[1,2,\"s\",3,4,5,6]").unwrap(),
			SlotEntry::Solid {
				tx: 1,
				ty: 2,
				value: [3, 4, 5, 6]
			}
		);
		assert!(serde_json::from_str::<SlotEntry>("[1,2,\"x\",3]").is_err());
	}

	#[test]
	fn a_manifest_with_fewer_name_counters_still_opens() {
		// Files written before the M4 adjustment kinds have 9 counters.
		let doc = Document::new(
			8,
			8,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let mut manifest = to_manifest(&doc, |_| None).unwrap();
		manifest.name_counters = vec![7; 9];
		let json = serde_json::to_string(&manifest).unwrap();
		let back: Manifest = serde_json::from_str(&json).unwrap();
		assert_eq!(back.name_counters.len(), 9);
		let store = fx_tiles::TileStore::new(fx_tiles::TileStoreConfig::for_tests(std::env::temp_dir().join("fx-io-counters"))).unwrap();
		let path = std::env::temp_dir().join("fx-io-counters.fxd");
		let writer = crate::fxd::FxdWriter::create(&path).unwrap();
		let file = std::sync::Arc::new(writer.commit(crate::fxd::ChunkRef { offset: 0, len: 0 }, 0).unwrap());
		let restored = from_manifest(&back, &file, &store).unwrap();
		let (_, counters) = restored.id_state();
		assert_eq!(&counters[..9], &[7; 9]);
		assert!(counters[9..].iter().all(|&c| c == 0));
	}

	#[test]
	fn a_level_0_tile_without_a_chunk_is_an_error() {
		let store = store();
		let mut image = TiledImage::new(512, 512, PixelFormat::Rgba16);
		// A non-uniform buffer stays a real tile (a filled one collapses to Solid).
		let mut buffer = TileBuffer::zeroed(PixelFormat::Rgba16);
		buffer.as_u16_mut()[0] = 1234;
		image.put_buffer(&store, 0, 0, buffer);
		assert!(matches!(image.slot(0, 0, 0), TileSlot::Data(_)));

		// The save never wrote this authoritative tile: building the manifest
		// fails with an error instead of panicking on the save thread.
		let error = image_entry(&image, |_| None).unwrap_err();
		assert!(matches!(&error, IoError::Decode(m) if m.contains("level-0 tile 0,0")), "got {error:?}");

		// A derived (mip) tile with no chunk is allowed: it is rebuilt later.
		let mut with_mip = TiledImage::new(4096, 4096, PixelFormat::Rgba16);
		let mip = store.insert(TileBuffer::zeroed(PixelFormat::Rgba16), fx_tiles::TileClass::Derived);
		with_mip.set_derived_slot(3, 0, 0, TileSlot::Data(mip));
		let entry = image_entry(&with_mip, |_| None).unwrap();
		assert!(entry.levels.is_empty(), "the unwritten mip is left out");
	}

	// -----------------------------------------------------------------------
	// Document <-> file round trip
	// -----------------------------------------------------------------------

	/// Every `TiledImage` of a document (layer pixels and masks).
	fn images_of(doc: &Document) -> Vec<&TiledImage> {
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

	/// Write every real tile of `doc` plus its manifest to `path`. This is a
	/// miniature of the save path (M3-T04); the manifest is what `from_manifest`
	/// later consumes.
	fn write_document(path: &std::path::Path, doc: &Document, store: &TileStore) -> Arc<FxdFile> {
		let mut refs: HashMap<u64, ChunkRef> = HashMap::new();
		let mut writer = FxdWriter::create(path).unwrap();
		for image in images_of(doc) {
			for level in 0..image.level_count() {
				for (_, _, slot) in image.grid(level).non_empty() {
					let TileSlot::Data(handle) = slot else { continue };
					if refs.contains_key(&handle.id().get()) {
						continue;
					}
					let pixels = store.get(handle).unwrap();
					let compressed = zstd::bulk::compress(pixels.bytes(), 3).unwrap();
					let chunk = writer.tile(image.format(), Codec::Zstd, &compressed).unwrap();
					refs.insert(handle.id().get(), chunk);
				}
			}
		}
		let manifest = to_manifest(doc, |handle| refs.get(&handle.id().get()).copied()).unwrap();
		let payload = encode_manifest(&manifest, 3).unwrap();
		let chunk = writer.manifest(&payload).unwrap();
		let live = chunk.end();
		Arc::new(writer.commit(chunk, live).unwrap())
	}

	fn assert_documents_equal(a: &Document, b: &Document, store: &TileStore) {
		assert_eq!((a.width, a.height), (b.width, b.height));
		assert_eq!(a.color, b.color);
		assert_eq!(a.ppi, b.ppi);
		assert_eq!(a.id_state(), b.id_state());
		assert_eq!(a.selected, b.selected);
		assert_eq!(a.layers.len(), b.layers.len());
		for (la, lb) in a.layers.iter().zip(&b.layers) {
			assert_layers_equal(la, lb, store);
		}
	}

	fn assert_layers_equal(a: &Layer, b: &Layer, store: &TileStore) {
		assert_eq!(a.id, b.id);
		assert_eq!(a.name, b.name);
		assert_eq!(a.visible, b.visible);
		assert_eq!(a.opacity, b.opacity);
		assert_eq!(a.fill, b.fill);
		assert_eq!(a.blend, b.blend);
		assert_eq!(a.clipped, b.clipped);
		assert_eq!(a.locked_pixels, b.locked_pixels);
		assert_eq!(a.locked_position, b.locked_position);
		match (&a.mask, &b.mask) {
			(None, None) => {}
			(Some(ma), Some(mb)) => {
				assert_eq!((ma.enabled, ma.linked, ma.outside_value), (mb.enabled, mb.linked, mb.outside_value));
				assert_images_equal(&ma.image, &mb.image, store);
			}
			_ => panic!("mask presence differs for {}", a.name),
		}
		match (&a.kind, &b.kind) {
			(LayerKind::Pixel { image: ia, offset: oa }, LayerKind::Pixel { image: ib, offset: ob }) => {
				assert_eq!(oa, ob);
				assert_images_equal(ia, ib, store);
			}
			(LayerKind::Group { children: ca, expanded: ea }, LayerKind::Group { children: cb, expanded: eb }) => {
				assert_eq!(ea, eb);
				assert_eq!(ca.len(), cb.len());
				for (x, y) in ca.iter().zip(cb) {
					assert_layers_equal(x, y, store);
				}
			}
			(LayerKind::Adjustment(aa), LayerKind::Adjustment(ab)) => assert_eq!(aa, ab),
			(LayerKind::SolidFill { rgba: x }, LayerKind::SolidFill { rgba: y }) => assert_eq!(x, y),
			_ => panic!("kind differs for {}", a.name),
		}
	}

	fn assert_images_equal(a: &TiledImage, b: &TiledImage, store: &TileStore) {
		assert_eq!((a.width(), a.height(), a.format()), (b.width(), b.height(), b.format()));
		assert_eq!(a.level_count(), b.level_count());
		for level in 0..a.level_count() {
			// Levels 1-2 are rebuilt lazily and never stored; compare 0 and >= 3.
			if level != 0 && level < 3 {
				continue;
			}
			let grid = a.grid(level);
			assert_eq!((grid.cols(), grid.rows()), (b.grid(level).cols(), b.grid(level).rows()));
			for ty in 0..grid.rows() {
				for tx in 0..grid.cols() {
					match (a.slot(level, tx, ty), b.slot(level, tx, ty)) {
						(TileSlot::Empty, TileSlot::Empty) => {}
						(TileSlot::Solid(x), TileSlot::Solid(y)) => assert_eq!(x, y, "solid at level {level} ({tx},{ty})"),
						(TileSlot::Data(x), TileSlot::Data(y)) => {
							let xb = store.get(x).unwrap();
							let yb = store.get(y).unwrap();
							assert_eq!(xb.bytes(), yb.bytes(), "pixels at level {level} ({tx},{ty})");
						}
						(x, y) => panic!("slot mismatch at level {level} ({tx},{ty}): {x:?} vs {y:?}"),
					}
				}
			}
		}
	}

	#[test]
	fn the_sample_document_round_trips_through_a_file() {
		let store = store();
		let doc = sample_document(&store);
		let dir = std::env::temp_dir().join("fx-io-fxd-manifest-tests");
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("roundtrip.fxd");

		let file = write_document(&path, &doc, &store);

		// Read the manifest back through the real container (checksum verified).
		let footer = file.footer();
		let (kind, payload) = file
			.read_chunk(ChunkRef {
				offset: footer.manifest_offset,
				len: footer.manifest_len,
			})
			.unwrap();
		assert_eq!(kind, ChunkKind::Manifest);
		let manifest = decode_manifest(&payload).unwrap();

		let restored = from_manifest(&manifest, &file, &store).unwrap();
		assert_documents_equal(&doc, &restored, &store);
	}
}
