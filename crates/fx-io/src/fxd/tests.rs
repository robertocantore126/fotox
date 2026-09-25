//! Container tests (M3-T01): round trips, checksum detection, truncation
//! recovery and header validation.

use std::path::PathBuf;

use fx_tiles::PixelFormat;

use super::container::CHUNK_HEADER_LEN;
use super::{ChunkKind, Codec, FxdFile, FxdWriter, parse_tile_payload};
use crate::IoError;

fn dir() -> PathBuf {
	let dir = std::env::temp_dir().join("fx-io-fxd-tests");
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

fn path(name: &str) -> PathBuf {
	dir().join(name)
}

#[test]
fn round_trips_every_chunk_kind() {
	let path = path("roundtrip.fxd");
	let mut writer = FxdWriter::create(&path).unwrap();
	let tile = writer.tile(PixelFormat::Rgba8, Codec::Raw, &[1, 2, 3, 4, 5]).unwrap();
	let preview = writer.preview_tile(PixelFormat::Gray16, Codec::Zstd, &[9, 9, 9]).unwrap();
	let manifest = writer.manifest(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
	let live = manifest.end();
	let end = writer.position() + 64;
	writer.commit(manifest, live).unwrap();

	let (opened, footer) = FxdFile::open(&path).unwrap();
	assert_eq!(footer.manifest_offset, manifest.offset);
	assert_eq!(footer.manifest_len, manifest.len);
	assert_eq!(footer.end_offset, end);
	assert_eq!(footer.live_bytes, live);

	// A tile payload starts with its own 8-byte header, then the tile bytes.
	let (kind, payload) = opened.read_chunk(tile).unwrap();
	assert_eq!(kind, ChunkKind::Tile);
	assert_eq!(&payload[8..], &[1, 2, 3, 4, 5]);
	let parsed = parse_tile_payload(&payload).unwrap();
	assert_eq!(parsed.format, PixelFormat::Rgba8);
	assert_eq!(parsed.codec, Codec::Raw);
	assert_eq!(parsed.uncompressed_len as usize, PixelFormat::Rgba8.tile_bytes());
	assert_eq!(parsed.data, &[1, 2, 3, 4, 5]);

	let (kind, payload) = opened.read_chunk(preview).unwrap();
	assert_eq!(kind, ChunkKind::PreviewTile);
	assert_eq!(&payload[8..], &[9, 9, 9]);
	assert_eq!(parse_tile_payload(&payload).unwrap().format, PixelFormat::Gray16);

	assert_eq!(opened.read_chunk(manifest).unwrap(), (ChunkKind::Manifest, vec![0xDE, 0xAD, 0xBE, 0xEF]));
}

#[test]
fn flipped_payload_byte_is_an_error_not_garbage() {
	let path = path("flipped.fxd");
	let mut writer = FxdWriter::create(&path).unwrap();
	let tile = writer.tile(PixelFormat::Rgba8, Codec::Raw, &[10, 20, 30, 40]).unwrap();
	let manifest = writer.manifest(&[1, 2, 3, 4]).unwrap();
	writer.commit(manifest, manifest.end()).unwrap();

	let mut bytes = std::fs::read(&path).unwrap();
	let i = (tile.offset + CHUNK_HEADER_LEN) as usize;
	bytes[i] ^= 0xFF;
	std::fs::write(&path, &bytes).unwrap();

	let (opened, _) = FxdFile::open(&path).unwrap();
	let err = opened.read_chunk(tile).unwrap_err();
	assert!(matches!(&err, IoError::Decode(m) if m.contains("corrupt chunk")), "got {err:?}");
	// A different chunk is unaffected.
	assert_eq!(opened.read_chunk(manifest).unwrap().1, vec![1, 2, 3, 4]);
}

#[test]
fn a_flipped_manifest_byte_is_also_an_error() {
	let path = path("flipped-manifest.fxd");
	let mut writer = FxdWriter::create(&path).unwrap();
	let manifest = writer.manifest(&[7; 32]).unwrap();
	writer.commit(manifest, manifest.end()).unwrap();

	let mut bytes = std::fs::read(&path).unwrap();
	let i = (manifest.offset + CHUNK_HEADER_LEN + 4) as usize;
	bytes[i] ^= 0x01;
	std::fs::write(&path, &bytes).unwrap();

	let (opened, _) = FxdFile::open(&path).unwrap();
	assert!(matches!(opened.read_chunk(manifest), Err(IoError::Decode(_))));
}

#[test]
fn truncation_at_every_byte_of_the_last_save_opens_the_previous_version() {
	let path = path("truncate.fxd");

	// Save 1.
	let mut writer = FxdWriter::create(&path).unwrap();
	writer.tile(PixelFormat::Gray8, Codec::Raw, &[7; 8]).unwrap();
	let m1 = writer.manifest(&[1; 10]).unwrap();
	let f1 = writer.commit(m1, m1.end()).unwrap().footer();

	// Save 2, appended after save 1's footer.
	let append = (*FxdFile::open(&path).unwrap().0).clone();
	let mut writer = FxdWriter::append_to(append).unwrap();
	writer.tile(PixelFormat::Gray8, Codec::Raw, &[8; 16]).unwrap();
	writer.preview_tile(PixelFormat::Gray8, Codec::Raw, &[5; 4]).unwrap();
	let m2 = writer.manifest(&[2; 20]).unwrap();
	let f2 = writer.commit(m2, m2.end()).unwrap().footer();

	let full = std::fs::read(&path).unwrap();
	assert_eq!(full.len() as u64, f2.end_offset, "the file ends at its footer");
	assert!(f1.end_offset < f2.end_offset, "save 2 did not extend save 1");

	// Truncating anywhere inside save 2 must leave save 1 readable.
	for off in f1.end_offset..f2.end_offset {
		std::fs::write(&path, &full[..off as usize]).unwrap();
		let (_, footer) = FxdFile::open(&path).unwrap();
		assert_eq!(footer, f1, "truncated at {off}");
	}

	// With the whole file present, save 2 is found.
	std::fs::write(&path, &full).unwrap();
	let (opened, footer) = FxdFile::open(&path).unwrap();
	assert_eq!(footer, f2);
	assert_eq!(opened.read_chunk(m2).unwrap().1, vec![2; 20]);
}

#[test]
fn header_only_file_is_a_clear_error() {
	let path = path("header-only.fxd");
	let writer = FxdWriter::create(&path).unwrap();
	drop(writer); // header written, never committed

	let err = FxdFile::open(&path).unwrap_err();
	assert!(matches!(&err, IoError::Decode(m) if m.contains("not a complete .fxd")), "got {err:?}");
}

#[test]
fn bad_magic_is_unsupported() {
	let path = path("bad-magic.fxd");
	std::fs::write(&path, [0u8; 128]).unwrap();
	let err = FxdFile::open(&path).unwrap_err();
	assert!(matches!(err, IoError::UnsupportedFormat), "got {err:?}");
}

#[test]
fn a_newer_version_is_refused() {
	let path = path("version-2.fxd");
	let mut writer = FxdWriter::create(&path).unwrap();
	let manifest = writer.manifest(&[1, 2, 3]).unwrap();
	writer.commit(manifest, manifest.end()).unwrap();

	let mut bytes = std::fs::read(&path).unwrap();
	bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
	std::fs::write(&path, &bytes).unwrap();

	let err = FxdFile::open(&path).unwrap_err();
	assert!(matches!(&err, IoError::Unsupported(m) if m.contains("fxd version 2")), "got {err:?}");
}

#[test]
fn tile_payload_header_is_validated() {
	let mut payload = vec![0u8, 0, 0, 0]; // Rgba8, raw, reserved
	payload.extend_from_slice(&(PixelFormat::Rgba8.tile_bytes() as u32).to_le_bytes());
	payload.extend_from_slice(&[0u8; 10]);
	let parsed = parse_tile_payload(&payload).unwrap();
	assert_eq!(parsed.format, PixelFormat::Rgba8);
	assert_eq!(parsed.codec, Codec::Raw);
	assert_eq!(parsed.data.len(), 10);

	let mut wrong_len = payload.clone();
	wrong_len[4..8].copy_from_slice(&5u32.to_le_bytes());
	assert!(matches!(parse_tile_payload(&wrong_len), Err(IoError::Decode(_))));

	let mut unknown_format = payload.clone();
	unknown_format[0] = 9;
	assert!(matches!(parse_tile_payload(&unknown_format), Err(IoError::Decode(_))));

	assert!(matches!(parse_tile_payload(&[0, 0, 0]), Err(IoError::Decode(_))));
}
