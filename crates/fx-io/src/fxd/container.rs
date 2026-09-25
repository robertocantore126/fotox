//! The `.fxd` container: header, chunk framing, footer and crash recovery.
//!
//! Layout (all integers little-endian), normative in `docs/FILE_FORMAT.md`:
//!
//! ```text
//! [Header 64 B] [chunk] [chunk] … [chunk] [Footer 64 B]
//! ```
//!
//! The format is an append-only log. A save appends its chunks, `sync_data`s,
//! writes the footer and `sync_data`s again, so the previous footer stays
//! valid until the new one is on disk. [`find_footer`] recovers the newest
//! complete version after a torn write.
//!
//! All reads and writes go through *positional* I/O ([`read_exact_at`],
//! [`write_all_at`]) so one file handle can be shared by the reader (backed
//! tiles) and the writer without a lock and without relying on the cursor —
//! on Windows `seek_read`/`seek_write` move the cursor. See SNIPPETS §12–13.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fx_tiles::{PixelFormat, TileBuffer, TileError, TileSource};

use crate::IoError;

/// Bytes of the file header.
pub(crate) const HEADER_LEN: u64 = 64;
/// Bytes of one chunk header (before its payload).
pub(crate) const CHUNK_HEADER_LEN: u64 = 16;
/// Bytes of the footer.
pub(crate) const FOOTER_LEN: u64 = 64;
/// The only format version this build reads and writes.
pub(crate) const FORMAT_VERSION: u32 = 1;

const MAGIC: &[u8; 8] = b"FOTOXFXD";
const FOOTER_MAGIC: &[u8; 8] = b"FXDEND01";

/// Process-wide source id, one per opened [`FxdFile`]. Used by backed tiles to
/// recognise which file a tile came from (`TileSource::id`, M3-T03).
static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(1);

/// What a chunk holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ChunkKind {
	/// One tile of level-0 or derived (mip ≥ 3) content.
	Tile = 1,
	/// The zstd-compressed JSON manifest.
	Manifest = 2,
	/// One tile of the flattened composite preview.
	PreviewTile = 3,
}

impl ChunkKind {
	fn from_byte(byte: u8) -> Result<Self, IoError> {
		match byte {
			1 => Ok(ChunkKind::Tile),
			2 => Ok(ChunkKind::Manifest),
			3 => Ok(ChunkKind::PreviewTile),
			other => Err(IoError::Decode(format!("unknown chunk kind {other}"))),
		}
	}
}

/// Compression of a tile payload. The byte is part of the file format: never
/// renumber a codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
	Raw = 0,
	Zstd = 1,
	Lz4 = 2,
}

impl Codec {
	/// The byte stored in a tile payload.
	pub fn to_byte(self) -> u8 {
		self as u8
	}

	/// Parse a tile payload's codec byte.
	pub fn from_byte(byte: u8) -> Result<Self, IoError> {
		match byte {
			0 => Ok(Codec::Raw),
			1 => Ok(Codec::Zstd),
			2 => Ok(Codec::Lz4),
			other => Err(IoError::Decode(format!("unknown tile codec {other}"))),
		}
	}
}

/// Location of one chunk in a `.fxd` file.
///
/// `len` is the **total** chunk length (16-byte header + payload), matching the
/// footer's `MANIFEST chunk total length`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRef {
	pub offset: u64,
	pub len: u64,
}

impl ChunkRef {
	/// End offset of the chunk (exclusive).
	pub fn end(self) -> u64 {
		self.offset + self.len
	}
}

/// The last complete save, as recorded by its footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footer {
	/// Offset of the `MANIFEST` chunk header.
	pub manifest_offset: u64,
	/// Total length (header + payload) of the `MANIFEST` chunk.
	pub manifest_len: u64,
	/// End offset of this footer (its own offset + 64).
	pub end_offset: u64,
	/// Bytes of live data after this save.
	pub live_bytes: u64,
}

impl Footer {
	fn to_bytes(self) -> [u8; FOOTER_LEN as usize] {
		let mut bytes = [0u8; FOOTER_LEN as usize];
		bytes[0..8].copy_from_slice(FOOTER_MAGIC);
		bytes[8..16].copy_from_slice(&self.manifest_offset.to_le_bytes());
		bytes[16..24].copy_from_slice(&self.manifest_len.to_le_bytes());
		bytes[24..32].copy_from_slice(&self.end_offset.to_le_bytes());
		bytes[32..40].copy_from_slice(&self.live_bytes.to_le_bytes());
		let checksum = crc32fast::hash(&bytes[0..60]);
		bytes[60..64].copy_from_slice(&checksum.to_le_bytes());
		bytes
	}

	/// Parse and validate one 64-byte footer. `None` if the magic or the
	/// checksum does not match.
	fn parse(bytes: &[u8]) -> Option<Self> {
		if bytes.len() < FOOTER_LEN as usize || &bytes[0..8] != FOOTER_MAGIC {
			return None;
		}
		let stored = u32::from_le_bytes(bytes[60..64].try_into().ok()?);
		if stored != crc32fast::hash(&bytes[0..60]) {
			return None;
		}
		Some(Footer {
			manifest_offset: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
			manifest_len: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
			end_offset: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
			live_bytes: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
		})
	}
}

/// The decoded prefix of a TILE / PREVIEW_TILE payload.
#[derive(Clone, Copy, Debug)]
pub struct TilePayload<'a> {
	pub format: PixelFormat,
	pub codec: Codec,
	/// Uncompressed length; must equal `format.tile_bytes()`.
	pub uncompressed_len: u32,
	/// The (possibly compressed) tile bytes.
	pub data: &'a [u8],
}

/// Parse a TILE / PREVIEW_TILE payload and validate its header. Does not
/// decompress; [`Codec`] says how.
pub fn parse_tile_payload(payload: &[u8]) -> Result<TilePayload<'_>, IoError> {
	if payload.len() < 8 {
		return Err(IoError::Decode(format!("tile payload too short: {} bytes", payload.len())));
	}
	let format = format_from_byte(payload[0])?;
	let codec = Codec::from_byte(payload[1])?;
	let uncompressed_len = u32::from_le_bytes(payload[4..8].try_into().expect("4-byte slice"));
	if uncompressed_len as usize != format.tile_bytes() {
		return Err(IoError::Decode(format!(
			"tile payload declares {uncompressed_len} uncompressed bytes, {format:?} needs {}",
			format.tile_bytes()
		)));
	}
	Ok(TilePayload {
		format,
		codec,
		uncompressed_len,
		data: &payload[8..],
	})
}

pub(crate) fn format_to_byte(format: PixelFormat) -> u8 {
	match format {
		PixelFormat::Rgba8 => 0,
		PixelFormat::Rgba16 => 1,
		PixelFormat::Gray8 => 2,
		PixelFormat::Gray16 => 3,
	}
}

pub(crate) fn format_from_byte(byte: u8) -> Result<PixelFormat, IoError> {
	match byte {
		0 => Ok(PixelFormat::Rgba8),
		1 => Ok(PixelFormat::Rgba16),
		2 => Ok(PixelFormat::Gray8),
		3 => Ok(PixelFormat::Gray16),
		other => Err(IoError::Decode(format!("unknown pixel format {other}"))),
	}
}

/// Build the 64-byte header of a new file.
fn build_header() -> [u8; HEADER_LEN as usize] {
	let mut bytes = [0u8; HEADER_LEN as usize];
	bytes[0..8].copy_from_slice(MAGIC);
	bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
	// bytes[12..16] flags = 0, reserved.
	let writer = format!("fotox {}", env!("CARGO_PKG_VERSION"));
	let w = writer.as_bytes();
	let n = w.len().min(16);
	bytes[16..16 + n].copy_from_slice(&w[..n]);
	// bytes[32..64] reserved = 0.
	bytes
}

// ---------------------------------------------------------------------------
// Positional I/O (SNIPPETS §12)
// ---------------------------------------------------------------------------

/// Read exactly `buf.len()` bytes at `offset`, looping over short reads.
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
	use std::os::windows::fs::FileExt;
	while !buf.is_empty() {
		match file.seek_read(buf, offset) {
			Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
			Ok(n) => {
				buf = &mut buf[n..];
				offset += n as u64;
			}
			Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
			Err(e) => return Err(e),
		}
	}
	Ok(())
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
	use std::os::unix::fs::FileExt;
	file.read_exact_at(buf, offset)
}

/// Write all of `buf` at `offset`, looping over short writes.
#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
	use std::os::windows::fs::FileExt;
	while !buf.is_empty() {
		match file.seek_write(buf, offset) {
			Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
			Ok(n) => {
				buf = &buf[n..];
				offset += n as u64;
			}
			Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
			Err(e) => return Err(e),
		}
	}
	Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
	use std::os::unix::fs::FileExt;
	file.write_all_at(buf, offset)
}

// ---------------------------------------------------------------------------
// Recovery (SNIPPETS §13)
// ---------------------------------------------------------------------------

/// Newest valid footer ending at or before `end`. 1 MiB blocks are read
/// backwards, overlapping by 63 bytes so a footer that crosses a block
/// boundary is not missed. A footer is accepted only if its checksum is valid
/// **and** its `end_offset` equals its own position + 64 (the magic alone can
/// occur inside compressed tile data).
fn find_footer(file: &File, end: u64) -> io::Result<Option<(u64, Footer)>> {
	const BLOCK: u64 = 1 << 20;
	let footer_len = FOOTER_LEN as usize;
	let mut hi = end;
	while hi >= FOOTER_LEN {
		let lo = hi.saturating_sub(BLOCK);
		let mut buf = vec![0u8; (hi - lo) as usize];
		read_exact_at(file, &mut buf, lo)?;
		// Newest footer first.
		for i in (0..=buf.len() - footer_len).rev() {
			if &buf[i..i + 8] == FOOTER_MAGIC
				&& let Some(footer) = Footer::parse(&buf[i..i + footer_len])
				&& footer.end_offset == lo + i as u64 + FOOTER_LEN
			{
				return Ok(Some((lo + i as u64, footer)));
			}
		}
		if lo == 0 {
			break;
		}
		hi = lo + 63; // overlap so a boundary-crossing footer is seen
	}
	Ok(None)
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// An open `.fxd` file. Cheap to clone (it shares one OS handle) and shared by
/// every backed tile of the document. Holds the handle open read+write; Save
/// appends to it (D-027).
#[derive(Clone)]
pub struct FxdFile {
	file: Arc<File>,
	id: u64,
	path: PathBuf,
	footer: Footer,
}

impl std::fmt::Debug for FxdFile {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("FxdFile")
			.field("id", &self.id)
			.field("path", &self.path)
			.field("footer", &self.footer)
			.finish()
	}
}

impl FxdFile {
	/// Open a `.fxd` read+write and return it with the newest valid footer.
	pub fn open(path: &Path) -> Result<(Arc<FxdFile>, Footer), IoError> {
		let file = OpenOptions::new().read(true).write(true).open(path)?;
		let len = file.metadata()?.len();
		if len < HEADER_LEN {
			return Err(IoError::Decode(format!("not a complete .fxd: file is {len} bytes, header needs {HEADER_LEN}")));
		}
		let mut header = [0u8; HEADER_LEN as usize];
		read_exact_at(&file, &mut header, 0)?;
		if &header[0..8] != MAGIC {
			return Err(IoError::UnsupportedFormat);
		}
		let version = u32::from_le_bytes(header[8..12].try_into().expect("4-byte slice"));
		if version != FORMAT_VERSION {
			return Err(IoError::Unsupported(format!("fxd version {version}")));
		}
		let (_, footer) = find_footer(&file, len)?.ok_or_else(|| IoError::Decode("not a complete .fxd: no valid footer".into()))?;
		let fxd = Arc::new(FxdFile {
			file: Arc::new(file),
			id: NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed),
			path: path.to_path_buf(),
			footer,
		});
		Ok((fxd, footer))
	}

	/// The newest valid footer.
	pub fn footer(&self) -> Footer {
		self.footer
	}

	/// Stable id of this opened file, for backed tiles (`TileSource::id`).
	pub fn id(&self) -> u64 {
		self.id
	}

	/// Path this file was opened from.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Read a chunk at `at`, verifying the payload checksum. Returns its kind
	/// and raw payload.
	pub fn read_chunk(&self, at: ChunkRef) -> Result<(ChunkKind, Vec<u8>), IoError> {
		let mut header = [0u8; CHUNK_HEADER_LEN as usize];
		read_exact_at(&self.file, &mut header, at.offset)?;
		let kind = ChunkKind::from_byte(header[0])?;
		if header[1] != 0 {
			return Err(IoError::Decode(format!("chunk at {} has unknown flags {}", at.offset, header[1])));
		}
		let payload_len = u64::from_le_bytes(header[4..12].try_into().expect("8-byte slice"));
		// A corrupt length must not turn into a huge allocation: it has to
		// match the reference (every reader knows the chunk's total length).
		if payload_len.checked_add(CHUNK_HEADER_LEN) != Some(at.len) {
			return Err(IoError::Decode(format!(
				"corrupt chunk at {}: header says {payload_len} payload bytes, expected {}",
				at.offset,
				at.len.saturating_sub(CHUNK_HEADER_LEN)
			)));
		}
		let checksum = u32::from_le_bytes(header[12..16].try_into().expect("4-byte slice"));
		let mut payload = vec![0u8; payload_len as usize];
		read_exact_at(&self.file, &mut payload, at.offset + CHUNK_HEADER_LEN)?;
		if crc32fast::hash(&payload) != checksum {
			return Err(IoError::Decode(format!("corrupt chunk at {}", at.offset)));
		}
		Ok((kind, payload))
	}
}

/// Backed tiles read their pixels through this: the store calls [`read`] with
/// a chunk location recorded in the manifest, and gets a decompressed tile.
///
/// [`read`]: TileSource::read
impl TileSource for FxdFile {
	fn read(&self, offset: u64, len: u64, format: PixelFormat) -> Result<TileBuffer, TileError> {
		let (kind, payload) = self.read_chunk(ChunkRef { offset, len }).map_err(|e| TileError::Corrupt(e.to_string()))?;
		if kind != ChunkKind::Tile && kind != ChunkKind::PreviewTile {
			return Err(TileError::Corrupt(format!("chunk at {offset} is {kind:?}, not a tile")));
		}
		let parsed = parse_tile_payload(&payload).map_err(|e| TileError::Corrupt(e.to_string()))?;
		if parsed.format != format {
			return Err(TileError::Corrupt(format!(
				"tile at {offset} is {:?}, the store expected {:?}",
				parsed.format, format
			)));
		}
		let tile_bytes = format.tile_bytes();
		let bytes = match parsed.codec {
			Codec::Raw => parsed.data.to_vec(),
			Codec::Zstd => zstd::bulk::decompress(parsed.data, tile_bytes).map_err(|e| TileError::Corrupt(format!("zstd tile: {e}")))?,
			Codec::Lz4 => lz4_flex::block::decompress(parsed.data, tile_bytes).map_err(|e| TileError::Corrupt(format!("lz4 tile: {e}")))?,
		};
		TileBuffer::from_bytes(format, bytes.into_boxed_slice())
	}

	fn id(&self) -> u64 {
		self.id()
	}
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Appends chunks to a `.fxd` file. `commit` finishes a save by writing the
/// footer; until then the previous version stays valid.
pub struct FxdWriter {
	file: Arc<File>,
	id: u64,
	path: PathBuf,
	/// Offset of the next chunk / the footer.
	pos: u64,
}

impl FxdWriter {
	/// Create a new file (truncating any existing one) and write its header.
	pub fn create(path: &Path) -> Result<Self, IoError> {
		let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
		let arc = Arc::new(file);
		write_all_at(&arc, &build_header(), 0)?;
		Ok(FxdWriter {
			file: arc,
			id: NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed),
			path: path.to_path_buf(),
			pos: HEADER_LEN,
		})
	}

	/// Continue an open file after its current footer. Any torn bytes past
	/// that footer are overwritten by the next save.
	pub fn append_to(file: FxdFile) -> Result<Self, IoError> {
		Ok(FxdWriter {
			file: file.file,
			id: file.id,
			path: file.path,
			pos: file.footer.end_offset,
		})
	}

	/// Append a TILE chunk holding one compressed tile.
	pub fn tile(&mut self, format: PixelFormat, codec: Codec, compressed: &[u8]) -> Result<ChunkRef, IoError> {
		self.tile_chunk(ChunkKind::Tile, format, codec, compressed)
	}

	/// Append a PREVIEW_TILE chunk holding one compressed preview tile.
	pub fn preview_tile(&mut self, format: PixelFormat, codec: Codec, compressed: &[u8]) -> Result<ChunkRef, IoError> {
		self.tile_chunk(ChunkKind::PreviewTile, format, codec, compressed)
	}

	fn tile_chunk(&mut self, kind: ChunkKind, format: PixelFormat, codec: Codec, compressed: &[u8]) -> Result<ChunkRef, IoError> {
		let mut payload = Vec::with_capacity(8 + compressed.len());
		payload.push(format_to_byte(format));
		payload.push(codec.to_byte());
		payload.extend_from_slice(&[0u8; 2]); // reserved
		payload.extend_from_slice(&(format.tile_bytes() as u32).to_le_bytes());
		payload.extend_from_slice(compressed);
		self.write_chunk(kind, &payload)
	}

	/// Append the MANIFEST chunk (`json_zstd` is a zstd frame of the JSON).
	pub fn manifest(&mut self, json_zstd: &[u8]) -> Result<ChunkRef, IoError> {
		self.write_chunk(ChunkKind::Manifest, json_zstd)
	}

	/// Frame and append one chunk, returning its location.
	fn write_chunk(&mut self, kind: ChunkKind, payload: &[u8]) -> Result<ChunkRef, IoError> {
		let checksum = crc32fast::hash(payload);
		let mut header = [0u8; CHUNK_HEADER_LEN as usize];
		header[0] = kind as u8;
		// header[1] flags = 0, header[2..4] reserved = 0.
		header[4..12].copy_from_slice(&(payload.len() as u64).to_le_bytes());
		header[12..16].copy_from_slice(&checksum.to_le_bytes());
		let at = self.pos;
		write_all_at(&self.file, &header, at)?;
		write_all_at(&self.file, payload, at + CHUNK_HEADER_LEN)?;
		self.pos = at + CHUNK_HEADER_LEN + payload.len() as u64;
		Ok(ChunkRef {
			offset: at,
			len: CHUNK_HEADER_LEN + payload.len() as u64,
		})
	}

	/// Offset the next chunk will be written at.
	pub fn position(&self) -> u64 {
		self.pos
	}

	/// Finish a save: `sync_data`, write the footer, `sync_data` again. The
	/// previous footer stays valid until this returns.
	pub fn commit(self, manifest: ChunkRef, live_bytes: u64) -> Result<FxdFile, IoError> {
		self.file.sync_data()?;
		let footer = Footer {
			manifest_offset: manifest.offset,
			manifest_len: manifest.len,
			end_offset: self.pos + FOOTER_LEN,
			live_bytes,
		};
		write_all_at(&self.file, &footer.to_bytes(), self.pos)?;
		self.file.sync_data()?;
		// Bytes past the new footer are the torn tail of an interrupted save:
		// drop them so no stale data outlives this save.
		if self.file.metadata()?.len() > footer.end_offset {
			self.file.set_len(footer.end_offset)?;
		}
		Ok(FxdFile {
			file: self.file,
			id: self.id,
			path: self.path,
			footer,
		})
	}
}
