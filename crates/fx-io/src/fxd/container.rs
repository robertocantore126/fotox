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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use fx_tiles::{PixelFormat, ReadPool, TileBuffer, TileError, TileSource};

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
static WRITER_PATHS: OnceLock<(Mutex<std::collections::HashSet<PathBuf>>, Condvar)> = OnceLock::new();

/// Process-wide exclusive lease for writing a file path.
pub(crate) struct PathWriteLock(PathBuf);

impl PathWriteLock {
	pub(crate) fn acquire(path: &Path) -> Self {
		let absolute = if path.is_absolute() {
			path.to_path_buf()
		} else {
			std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
		};
		let key = absolute.canonicalize().unwrap_or_else(|_| {
			let parent = absolute.parent().and_then(|parent| parent.canonicalize().ok());
			match (parent, absolute.file_name()) {
				(Some(parent), Some(name)) => parent.join(name),
				_ => absolute,
			}
		});
		#[cfg(windows)]
		let key = PathBuf::from(key.to_string_lossy().to_lowercase());

		let (paths, available) = WRITER_PATHS.get_or_init(|| (Mutex::new(std::collections::HashSet::new()), Condvar::new()));
		let mut held = paths.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		while held.contains(&key) {
			held = available.wait(held).unwrap_or_else(std::sync::PoisonError::into_inner);
		}
		held.insert(key.clone());
		Self(key)
	}
}

impl Drop for PathWriteLock {
	fn drop(&mut self) {
		let (paths, available) = WRITER_PATHS.get().expect("writer path lock initialized");
		paths.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
		available.notify_all();
	}
}

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
		self.offset.saturating_add(self.len)
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
	// AUDIT-FIX(D5): zero means unknown for legacy files; reserved bytes remain backwards compatible.
	pub save_counter: u64,
	pub saved_at: u64,
}

impl Footer {
	fn to_bytes(self) -> [u8; FOOTER_LEN as usize] {
		let mut bytes = [0u8; FOOTER_LEN as usize];
		bytes[0..8].copy_from_slice(FOOTER_MAGIC);
		bytes[8..16].copy_from_slice(&self.manifest_offset.to_le_bytes());
		bytes[16..24].copy_from_slice(&self.manifest_len.to_le_bytes());
		bytes[24..32].copy_from_slice(&self.end_offset.to_le_bytes());
		bytes[32..40].copy_from_slice(&self.live_bytes.to_le_bytes());
		// AUDIT-FIX(D5): persist save identity in formerly reserved footer bytes.
		bytes[40..48].copy_from_slice(&self.save_counter.to_le_bytes());
		bytes[48..56].copy_from_slice(&self.saved_at.to_le_bytes());
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
			save_counter: u64::from_le_bytes(bytes[40..48].try_into().ok()?),
			saved_at: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
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

/// A chunk reference must lie inside the saved part of a file of `len` bytes.
fn check_bounds(at: ChunkRef, len: u64) -> Result<(), IoError> {
	if at.offset < HEADER_LEN || at.len < CHUNK_HEADER_LEN || at.offset.checked_add(at.len).is_none_or(|end| end > len) {
		return Err(IoError::Decode(format!(
			"chunk at {} with length {} exceeds saved file bounds ({len} bytes)",
			at.offset, at.len
		)));
	}
	Ok(())
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

/// An open `.fxd` file. Cheap to clone (it shares its OS handles) and shared by
/// every backed tile of the document. Save appends to it (D-027). Tiles are
/// read through a [`ReadPool`], one handle per thread: through one shared
/// handle, Windows queues the reads of every thread one after the other.
#[derive(Clone)]
pub struct FxdFile {
	file: Arc<ReadPool>,
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
		// AUDIT-FIX(D10): opening documents requires only read access; upgrade at append time.
		let file = OpenOptions::new().read(true).open(path)?;
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
			file: Arc::new(ReadPool::new(Arc::new(file))),
			id: NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed),
			path: path.to_path_buf(),
			footer,
		});
		Ok((fxd, footer))
	}

	// AUDIT-FIX(D5): search strictly before this footer after a manifest/structure failure.
	pub fn previous(&self) -> Result<Option<Arc<Self>>, IoError> {
		let end = self.footer.end_offset.saturating_sub(FOOTER_LEN);
		let previous = find_footer(self.file.main(), end)?;
		Ok(previous.map(|(_, footer)| Arc::new(Self { footer, ..self.clone() })))
	}

	// AUDIT-FIX(D5): uncommitted/damaged tails are visible instead of silently rolling back.
	pub fn has_newer_tail(&self) -> Result<bool, IoError> {
		Ok(self.file.main().metadata()?.len() > self.footer.end_offset)
	}

	// AUDIT-FIX(D5): lazy open checks tile framing without reading/decompressing tile pixels.
	// PERF(open): all of a document's tiles at once. One file-length query, and
	// the headers read in file order: one by one, with two metadata calls each,
	// they were most of a 6,000-layer reopen.
	pub fn validate_tiles(&self, chunks: &mut Vec<ChunkRef>) -> Result<(), IoError> {
		use rayon::prelude::*;
		let len = self.file.main().metadata()?.len().min(self.footer.end_offset);
		chunks.sort_unstable_by_key(|at| at.offset);
		chunks.dedup();
		// Runs of neighbouring chunks, read by several threads at once.
		chunks.par_chunks(256).try_for_each(|run| {
			let file = self.file.reader();
			let mut header = [0u8; CHUNK_HEADER_LEN as usize];
			for &at in run {
				check_bounds(at, len)?;
				read_exact_at(file, &mut header, at.offset)?;
				let kind = ChunkKind::from_byte(header[0])?;
				let payload_len = u64::from_le_bytes(header[4..12].try_into().expect("8-byte slice"));
				if !matches!(kind, ChunkKind::Tile | ChunkKind::PreviewTile)
					|| header[1] != 0
					|| payload_len.checked_add(CHUNK_HEADER_LEN) != Some(at.len)
					|| payload_len < 4
				{
					return Err(IoError::Decode("invalid backed tile chunk structure".into()));
				}
			}
			Ok(())
		})
	}

	// AUDIT-FIX(D8): keep the compacted backing id while rebinding its published path.
	pub fn rebind_path(&self, path: &Path) -> Result<Arc<Self>, IoError> {
		let rebound = Self {
			path: path.to_path_buf(),
			..self.clone()
		};
		if !rebound.matches_path()? {
			return Err(IoError::Decode("Compacted save path changed before rebind".into()));
		}
		Ok(Arc::new(rebound))
	}

	// AUDIT-FIX(D3): compare OS identity rather than the pathname held by an old handle.
	pub fn matches_path(&self) -> Result<bool, IoError> {
		let current = match File::open(&self.path) {
			Ok(file) => file,
			Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
			Err(error) => return Err(error.into()),
		};
		Ok(file_identity(self.file.main())? == file_identity(&current)?)
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

	// AUDIT-FIX(I1): validate references before allocating payloads or registering lazy backed tiles.
	pub fn validate_chunk(&self, at: ChunkRef) -> Result<(), IoError> {
		check_bounds(at, self.file.main().metadata()?.len().min(self.footer.end_offset))
	}

	/// Read a chunk at `at`, verifying the payload checksum. Returns its kind
	/// and raw payload.
	pub fn read_chunk(&self, at: ChunkRef) -> Result<(ChunkKind, Vec<u8>), IoError> {
		// AUDIT-FIX(I1): footer lengths are untrusted, including apparently consistent headers.
		self.validate_chunk(at)?;
		let mut header = [0u8; CHUNK_HEADER_LEN as usize];
		let file = self.file.reader();
		read_exact_at(file, &mut header, at.offset)?;
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
		// AUDIT-FIX(I1): allocation failures at the container boundary become errors.
		let count = usize::try_from(payload_len).map_err(|_| IoError::Decode("chunk length exceeds address space".into()))?;
		let mut payload = Vec::new();
		payload
			.try_reserve_exact(count)
			.map_err(|e| IoError::Decode(format!("cannot allocate chunk payload: {e}")))?;
		payload.resize(count, 0);
		read_exact_at(file, &mut payload, at.offset + CHUNK_HEADER_LEN)?;
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

// AUDIT-FIX(D3): Windows volume serial + file index identify the handle across pathname replacement.
#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
	use std::os::windows::io::AsRawHandle;
	#[repr(C)]
	#[derive(Default)]
	struct Info {
		attributes: u32,
		creation: [u32; 2],
		access: [u32; 2],
		write: [u32; 2],
		volume: u32,
		size_high: u32,
		size_low: u32,
		links: u32,
		index_high: u32,
		index_low: u32,
	}
	#[link(name = "kernel32")]
	unsafe extern "system" {
		fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
	}
	let mut info = Info::default();
	// SAFETY: the handle is borrowed from a live File and Info matches the Windows ABI.
	if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
		return Err(io::Error::last_os_error());
	}
	Ok((u64::from(info.volume), (u64::from(info.index_high) << 32) | u64::from(info.index_low)))
}

// AUDIT-FIX(D3): equivalent identity for supported Unix filesystems.
#[cfg(unix)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
	use std::os::unix::fs::MetadataExt;
	let metadata = file.metadata()?;
	Ok((metadata.dev(), metadata.ino()))
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
	_lease: PathWriteLock,
	/// Offset of the next chunk / the footer.
	pos: u64,
	// AUDIT-FIX(D5): inherited save sequence, incremented only by commit.
	save_counter: u64,
}

impl FxdWriter {
	/// Create a new file (truncating any existing one) and write its header.
	pub fn create(path: &Path) -> Result<Self, IoError> {
		let lease = PathWriteLock::acquire(path);
		let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
		let arc = Arc::new(file);
		write_all_at(&arc, &build_header(), 0)?;
		Ok(FxdWriter {
			file: arc,
			id: NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed),
			path: path.to_path_buf(),
			_lease: lease,
			pos: HEADER_LEN,
			save_counter: 0,
		})
	}

	/// Continue an open file after its current footer. Any torn bytes past
	/// that footer are overwritten by the next save.
	pub fn append_to(file: FxdFile) -> Result<Self, IoError> {
		let lease = PathWriteLock::acquire(&file.path);
		// AUDIT-FIX(D3): never append into an orphaned handle, including a replace racing the save decision.
		if !file.matches_path()? {
			return Err(IoError::Decode("The save path was replaced; save again to rebind the document".into()));
		}
		// AUDIT-FIX(D10): acquire a writable handle lazily and verify it still names the same file.
		let writable = OpenOptions::new().read(true).write(true).open(&file.path)?;
		if file_identity(&writable)? != file_identity(file.file.main())? {
			return Err(IoError::Decode("The target changed while preparing Save; try again".into()));
		}
		let len = file.file.main().metadata()?.len();
		let (_, footer) = find_footer(file.file.main(), len)?.ok_or_else(|| IoError::Decode("not a complete .fxd: no valid footer".into()))?;
		Ok(FxdWriter {
			file: Arc::new(writable),
			id: file.id,
			path: file.path,
			_lease: lease,
			pos: footer.end_offset,
			save_counter: footer.save_counter,
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
			save_counter: self.save_counter.saturating_add(1),
			saved_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
		};
		write_all_at(&self.file, &footer.to_bytes(), self.pos)?;
		self.file.sync_data()?;
		// Bytes past the new footer are the torn tail of an interrupted save:
		// drop them so no stale data outlives this save.
		if self.file.metadata()?.len() > footer.end_offset {
			self.file.set_len(footer.end_offset)?;
		}
		Ok(FxdFile {
			file: Arc::new(ReadPool::new(self.file)),
			id: self.id,
			path: self.path,
			footer,
		})
	}
}
