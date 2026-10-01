//! Audit 2026-10-01 — malformed and oversized inputs. Each crafted file is
//! opened in a child process (this test binary re-run with
//! `FOTOX_AUDIT_OPEN=<path>`), so an abort or an allocation failure is
//! observed instead of killing the runner. The parent bounds the child's
//! time and reports its exit status and peak memory.
//!
//!   cargo test --release -p fx-io --test audit_inputs -- --ignored --nocapture --test-threads 1

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fx_core::{BitDepth, ColorProfile, Document, DocumentColor, Layer, LayerKind};
use fx_io::fxd::{self, FxdWriter, SaveRequest, SaveTarget};
use fx_tiles::{PixelFormat, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

fn root() -> PathBuf {
	let dir = std::env::var_os("FOTOX_AUDIT_DIR")
		.map_or_else(std::env::temp_dir, PathBuf::from)
		.join(format!("fx-audit-inputs-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

fn store(dir: &Path) -> TileStore {
	let mut config = TileStoreConfig::for_tests(dir.join("scratch"));
	config.hot_budget = 1 << 30;
	TileStore::new(config).unwrap()
}

/// Child body: open the file named by `FOTOX_AUDIT_OPEN` the way the engine
/// does (fxd: open, then read every tile, as drawing would; others: import).
#[test]
#[ignore = "child process of the input audits"]
fn child_open() {
	let Some(path) = std::env::var_os("FOTOX_AUDIT_OPEN").map(PathBuf::from) else {
		return;
	};
	let store = store(path.parent().unwrap());
	let started = Instant::now();
	let result: Result<String, String> = if fx_io::sniff(&std::fs::read(&path).unwrap_or_default()[..16.min(std::fs::metadata(&path).map_or(0, |m| m.len() as usize))]) == Some(fx_io::Sniffed::Fxd) {
		match fxd::open(&path, &store) {
			Ok(opened) => {
				let mut tiles = 0;
				let mut errors = Vec::new();
				let mut layers = 0;
				opened.document.walk(|layer, _| {
					layers += 1;
					if let LayerKind::Pixel { image, .. } = &layer.kind {
						for (_, _, slot) in image.grid(0).non_empty() {
							if let TileSlot::Data(h) = slot {
								tiles += 1;
								if let Err(e) = store.get(h) {
									errors.push(e.to_string());
								}
							}
						}
					}
				});
				Ok(format!("opened: {layers} layers, {tiles} tiles read, {} tile errors {:?}", errors.len(), errors.first()))
			}
			Err(e) => Err(e.to_string()),
		}
	} else {
		fx_io::import_file(&path, &store, &mut |_| true)
			.map(|i| format!("imported {}×{}", i.width, i.height))
			.map_err(|e| e.to_string())
	};
	let peak = std::process::Command::new("powershell")
		.args(["-NoProfile", "-Command", &format!("(Get-Process -Id {}).PeakPagedMemorySize64", std::process::id())])
		.output()
		.ok()
		.and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
		.map_or(0, |b| b >> 20);
	println!("CHILD-RESULT {:?} in {:.2} s, child peak private {peak} MiB", result, started.elapsed().as_secs_f64());
}

/// Run the child on `path`; returns (exit status text, child stdout result
/// line, seconds, peak private MiB sampled by the parent).
fn open_in_child(path: &Path, limit: Duration) -> (String, String, f64, u64) {
	let exe = std::env::current_exe().unwrap();
	let started = Instant::now();
	let mut child = std::process::Command::new(exe)
		.args(["child_open", "--exact", "--ignored", "--nocapture", "--test-threads", "1"])
		.env("FOTOX_AUDIT_OPEN", path)
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.spawn()
		.unwrap();
	let pid = child.id();
	let mut peak = 0u64;
	let status = loop {
		if let Some(status) = child.try_wait().unwrap() {
			break format!("exit {:?} (0x{:X})", status.code(), status.code().unwrap_or(0) as u32);
		}
		peak = peak.max(private_mib(pid));
		// Safety valve: never let a child eat the machine.
		if peak > 6 * 1024 || started.elapsed() > limit {
			let _ = child.kill();
			let _ = child.wait();
			break format!("KILLED by the parent (private {peak} MiB, {:.0} s)", started.elapsed().as_secs_f64());
		}
		std::thread::sleep(Duration::from_millis(50));
	};
	let mut out = String::new();
	if let Some(mut s) = child.stdout.take() {
		use std::io::Read;
		let _ = s.read_to_string(&mut out);
	}
	let mut err = String::new();
	if let Some(mut s) = child.stderr.take() {
		use std::io::Read;
		let _ = s.read_to_string(&mut err);
	}
	let line = out
		.lines()
		.find(|l| l.contains("CHILD-RESULT"))
		.map(str::to_owned)
		.unwrap_or_else(|| format!("(no result; stderr: {})", err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("")));
	(status, line, started.elapsed().as_secs_f64(), peak)
}

fn private_mib(pid: u32) -> u64 {
	let out = std::process::Command::new("powershell")
		.args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).PrivateMemorySize64")])
		.output();
	out.ok()
		.and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
		.map_or(0, |b| b >> 20)
}

fn report(name: &str, path: &Path, limit: Duration) {
	let (status, line, secs, peak) = open_in_child(path, limit);
	println!("AUDIT input {name}: {status}; {line}; {secs:.1} s; peak private ≈ {peak} MiB");
}

fn small_fxd(dir: &Path, name: &str) -> PathBuf {
	let store = store(dir);
	let mut doc = Document::new(
		512,
		512,
		DocumentColor {
			depth: BitDepth::U8,
			profile: ColorProfile::Srgb,
		},
		72.0,
	);
	let id = doc.allocate_layer_id();
	let mut image = TiledImage::new(512, 512, PixelFormat::Rgba8);
	let mut b = TileBuffer::zeroed(PixelFormat::Rgba8);
	b.bytes_mut()[0] = 9;
	image.put_buffer(&store, 0, 0, b);
	doc.layers = vec![Arc::new(Layer::new(id, "L", LayerKind::Pixel { image, offset: (0, 0) }))];
	let path = dir.join(name);
	fxd::save(
		SaveRequest {
			doc: &doc,
			store: &store,
			preview: None,
		},
		SaveTarget::Fresh(path.clone()),
		&mut |_| true,
	)
	.unwrap();
	path
}

/// Re-encode `path`'s manifest after `edit` changed its JSON, appended as a
/// new version (a valid footer: CRCs are not a defence against a crafted file).
fn rewrite_manifest(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
	let (file, footer) = fxd::FxdFile::open(path).unwrap();
	let (_, payload) = file
		.read_chunk(fxd::ChunkRef {
			offset: footer.manifest_offset,
			len: footer.manifest_len,
		})
		.unwrap();
	let json = zstd::bulk::decompress(&payload, 1 << 26).unwrap();
	let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
	edit(&mut value);
	let json = serde_json::to_vec(&value).unwrap();
	let payload = zstd::bulk::compress(&json, 3).unwrap();
	let mut writer = FxdWriter::append_to((*file).clone()).unwrap();
	let chunk = writer.manifest(&payload).unwrap();
	writer.commit(chunk, 0).unwrap();
}

#[test]
#[ignore = "audit: child processes"]
fn crafted_fxd_files() {
	let dir = root();

	// 1. A footer whose manifest chunk claims 1 TiB.
	let p = small_fxd(&dir, "huge-manifest.fxd");
	{
		let mut bytes = std::fs::read(&p).unwrap();
		let at = bytes.len() as u64;
		let len: u64 = 1 << 40;
		let mut header = [0u8; 16];
		header[0] = 2; // MANIFEST
		header[4..12].copy_from_slice(&len.to_le_bytes());
		bytes.extend_from_slice(&header);
		let mut footer = [0u8; 64];
		footer[0..8].copy_from_slice(b"FXDEND01");
		footer[8..16].copy_from_slice(&at.to_le_bytes());
		footer[16..24].copy_from_slice(&(len + 16).to_le_bytes());
		let end = bytes.len() as u64 + 64;
		footer[24..32].copy_from_slice(&end.to_le_bytes());
		let crc = crc32fast::hash(&footer[0..60]);
		footer[60..64].copy_from_slice(&crc.to_le_bytes());
		bytes.extend_from_slice(&footer);
		std::fs::write(&p, bytes).unwrap();
	}
	report("fxd: footer → manifest chunk of 1 TiB", &p, Duration::from_secs(60));

	// 2. A tile reference of 1 TiB (opening is lazy: the read happens on draw).
	let p = small_fxd(&dir, "huge-tile.fxd");
	rewrite_manifest(&p, |v| {
		let slot = &mut v["layers"][0]["image"]["levels"][0]["slots"][0];
		if let Some(arr) = slot.as_array_mut() {
			arr[4] = serde_json::json!(1u64 << 40);
		} else {
			println!("  (unexpected slot shape: {slot})");
		}
	});
	report("fxd: tile chunk reference of 1 TiB", &p, Duration::from_secs(60));

	// 3. A tile reference past the end of the file.
	let p = small_fxd(&dir, "past-end.fxd");
	rewrite_manifest(&p, |v| {
		if let Some(arr) = v["layers"][0]["image"]["levels"][0]["slots"][0].as_array_mut() {
			arr[3] = serde_json::json!(1u64 << 33);
		}
	});
	report("fxd: tile chunk past the end of the file", &p, Duration::from_secs(60));

	// 4. A canvas at the 300 000 px limit with many layers: metadata alone.
	for layers in [1usize, 20, 200] {
		let p = small_fxd(&dir, &format!("wide-{layers}.fxd"));
		rewrite_manifest(&p, |v| {
			v["width"] = serde_json::json!(300_000);
			v["height"] = serde_json::json!(300_000);
			let template = v["layers"][0].clone();
			let mut list = Vec::new();
			for i in 0..layers {
				let mut l = template.clone();
				l["id"] = serde_json::json!(i + 1);
				list.push(l);
			}
			v["layers"] = serde_json::Value::Array(list);
			v["next_layer_id"] = serde_json::json!(layers + 1);
		});
		report(&format!("fxd: 300 000² canvas, {layers} layers (each a 512² image)"), &p, Duration::from_secs(120));
	}

	// 5. Groups nested deep (serde_json's recursion limit is 128; the
	//    engine refuses more than 10 levels, an older or foreign file may not).
	for depth in [50usize, 60, 200] {
		let store = store(&dir);
		let mut doc = Document::new(
			256,
			256,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let id = doc.allocate_layer_id();
		let mut node = Arc::new(Layer::new(id, "leaf", LayerKind::SolidFill { rgba: [1, 2, 3, 4] }));
		for _ in 0..depth {
			let gid = doc.allocate_layer_id();
			node = Arc::new(Layer::new(gid, "g", LayerKind::Group { children: vec![node], expanded: true }));
		}
		doc.layers = vec![node];
		let p = dir.join(format!("deep-{depth}.fxd"));
		let saved = fxd::save(
			SaveRequest {
				doc: &doc,
				store: &store,
				preview: None,
			},
			SaveTarget::Fresh(p.clone()),
			&mut |_| true,
		);
		println!("AUDIT input groups nested {depth} deep: save → {:?}", saved.as_ref().map(|_| "ok").map_err(ToString::to_string));
		report(&format!("fxd: groups nested {depth} deep"), &p, Duration::from_secs(60));
	}

	// 6. A manifest that decompresses to ~1 GiB of whitespace.
	let p = small_fxd(&dir, "bomb.fxd");
	{
		let (file, _) = fxd::FxdFile::open(&p).unwrap();
		let mut enc = zstd::stream::Encoder::new(Vec::new(), 19).unwrap();
		enc.set_pledged_src_size(Some(1 << 30)).unwrap();
		enc.include_contentsize(true).unwrap();
		let block = vec![b' '; 1 << 20];
		for _ in 0..1024 {
			enc.write_all(&block).unwrap();
		}
		let payload = enc.finish().unwrap();
		println!("  (bomb payload {} KiB → 1 GiB)", payload.len() >> 10);
		let mut writer = FxdWriter::append_to((*file).clone()).unwrap();
		let chunk = writer.manifest(&payload).unwrap();
		writer.commit(chunk, 0).unwrap();
	}
	report("fxd: manifest bomb (64 KiB → 1 GiB)", &p, Duration::from_secs(120));

	let _ = std::fs::remove_dir_all(&dir);
}

/// TIFF/JPEG/PNG headers that declare huge images with almost no data.
#[test]
#[ignore = "audit: child processes"]
fn crafted_image_files() {
	let dir = root();

	// TIFF: 300 000² RGBA16, one Deflate strip covering every row (a legal
	// layout many writers use for uncompressed files: RowsPerStrip = height).
	for (name, side, rows_per_strip) in [
		("TIFF 300 000² RGBA16, one strip", 300_000u32, 300_000u32),
		("TIFF 12 000² RGBA16, one strip", 12_000, 12_000),
		("TIFF 12 000² RGBA16, 64-row strips", 12_000, 64),
	] {
		let path = dir.join(format!("{}.tif", name.replace([' ', '²', ','], "_")));
		write_tiff_header(&path, side, rows_per_strip);
		report(name, &path, Duration::from_secs(180));
	}

	// JPEG: a valid 16×16 JPEG with its SOF0 size patched.
	for (name, w, h) in [("JPEG header 65 535 × 65 535", 65_535u16, 65_535u16), ("JPEG header 30 000 × 30 000", 30_000, 30_000)] {
		let path = dir.join(format!("{w}x{h}.jpg"));
		let mut bytes = tiny_jpeg();
		let sof = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).expect("SOF0");
		bytes[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
		bytes[sof + 7..sof + 9].copy_from_slice(&w.to_be_bytes());
		std::fs::write(&path, bytes).unwrap();
		report(name, &path, Duration::from_secs(180));
	}
	let _ = std::fs::remove_dir_all(&dir);
}

fn tiny_jpeg() -> Vec<u8> {
	let mut out = Vec::new();
	let enc = jpeg_encoder::Encoder::new(&mut out, 90);
	enc.encode(&[128u8; 16 * 16 * 3], 16, 16, jpeg_encoder::ColorType::Rgb).unwrap();
	out
}

/// A little-endian TIFF: RGBA 16-bit, Deflate, one strip per
/// `rows_per_strip`, every strip the same tiny zlib stream of zeros.
fn write_tiff_header(path: &Path, side: u32, rows_per_strip: u32) {
	let strips = side.div_ceil(rows_per_strip);
	// zlib stream of 64 KiB of zeros.
	let data = {
		let mut z = flate2_free_zlib_zeros();
		z.shrink_to_fit();
		z
	};
	let mut bytes: Vec<u8> = Vec::new();
	bytes.extend_from_slice(b"II");
	bytes.extend_from_slice(&42u16.to_le_bytes());
	bytes.extend_from_slice(&8u32.to_le_bytes());
	let entries: u16 = 11;
	let ifd_len = 2 + entries as u32 * 12 + 4;
	let bps_at = 8 + ifd_len;
	let offsets_at = bps_at + 8;
	let counts_at = offsets_at + strips * 4;
	let data_at = if strips == 1 { bps_at + 8 } else { counts_at + strips * 4 };
	let e = |tag: u16, ty: u16, count: u32, value: u32, out: &mut Vec<u8>| {
		out.extend_from_slice(&tag.to_le_bytes());
		out.extend_from_slice(&ty.to_le_bytes());
		out.extend_from_slice(&count.to_le_bytes());
		out.extend_from_slice(&value.to_le_bytes());
	};
	bytes.extend_from_slice(&entries.to_le_bytes());
	e(256, 4, 1, side, &mut bytes); // width
	e(257, 4, 1, side, &mut bytes); // height
	e(258, 3, 4, bps_at, &mut bytes); // bits per sample
	e(259, 3, 1, 8, &mut bytes); // Deflate
	e(262, 3, 1, 2, &mut bytes); // RGB
	e(273, 4, strips, if strips == 1 { data_at } else { offsets_at }, &mut bytes);
	e(277, 3, 1, 4, &mut bytes); // samples per pixel
	e(278, 4, 1, rows_per_strip, &mut bytes);
	e(279, 4, strips, if strips == 1 { data.len() as u32 } else { counts_at }, &mut bytes);
	e(284, 3, 1, 1, &mut bytes); // chunky
	e(338, 3, 1, 2, &mut bytes); // unassociated alpha
	bytes.extend_from_slice(&0u32.to_le_bytes());
	for _ in 0..4 {
		bytes.extend_from_slice(&16u16.to_le_bytes());
	}
	if strips > 1 {
		for _ in 0..strips {
			bytes.extend_from_slice(&data_at.to_le_bytes());
		}
		for _ in 0..strips {
			bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
		}
	}
	assert_eq!(bytes.len() as u32, data_at);
	bytes.extend_from_slice(&data);
	std::fs::write(path, bytes).unwrap();
}

/// A zlib stream (stored, no compression library needed) of 64 KiB of zeros.
fn flate2_free_zlib_zeros() -> Vec<u8> {
	let raw = vec![0u8; 65_535];
	let mut out = vec![0x78, 0x01];
	out.push(1); // final stored block
	out.extend_from_slice(&(raw.len() as u16).to_le_bytes());
	out.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
	out.extend_from_slice(&raw);
	// Adler-32 of 65 535 zeros: a = 1, b = 65 535 mod 65 521 = 14.
	let (a, b) = (1u32, 65_535u32 % 65_521);
	out.extend_from_slice(&((b << 16) | a).to_be_bytes());
	out
}


/// Real TIFFs of the same image, one strip vs 64-row strips: the importer's
/// peak memory (the streaming claim of fx-io's crate docs).
#[test]
#[ignore = "audit: writes ~1.2 GB"]
fn tiff_strip_layout_peak_memory() {
	let dir = root();
	let (w, h) = (10_000u32, 10_000u32);
	for rows in [64u32, h] {
		let path = dir.join(format!("real-{rows}.tif"));
		let mut writer = fx_io::tiff_write::TiffWriter::create(&path, w, h, 16, rows).unwrap();
		let mut y = 0;
		while y < h {
			let n = rows.min(h - y);
			let strip: Vec<u8> = (0..(w * n) as usize).flat_map(|i| [(i % 65_535) as u16, 7, 9].into_iter().flat_map(u16::to_le_bytes)).collect();
			writer.write_strip(&strip).unwrap();
			y += n;
		}
		writer.finish().unwrap();
		report(&format!("TIFF 10 000² RGB16 uncompressed, {rows}-row strips ({} MiB file)", std::fs::metadata(&path).unwrap().len() >> 20), &path, Duration::from_secs(300));
		let _ = std::fs::remove_file(&path);
	}
	let _ = std::fs::remove_dir_all(&dir);
}

/// Canvas-sized layer images on a 300 000² canvas: the dense slot grids.
#[test]
#[ignore = "audit: child processes"]
fn canvas_sized_layers_metadata() {
	let dir = root();
	for (side, layers) in [(30_000u32, 1000usize), (300_000, 1), (300_000, 10)] {
		let p = small_fxd(&dir, &format!("canvas-{side}-{layers}.fxd"));
		rewrite_manifest(&p, |v| {
			v["width"] = serde_json::json!(side);
			v["height"] = serde_json::json!(side);
			let mut template = v["layers"][0].clone();
			template["image"]["width"] = serde_json::json!(side);
			template["image"]["height"] = serde_json::json!(side);
			let list: Vec<serde_json::Value> = (0..layers)
				.map(|i| {
					let mut l = template.clone();
					l["id"] = serde_json::json!(i + 1);
					l
				})
				.collect();
			v["layers"] = serde_json::Value::Array(list);
			v["next_layer_id"] = serde_json::json!(layers + 1);
		});
		report(&format!("fxd: {side}² canvas, {layers} canvas-sized layers, one real tile each"), &p, Duration::from_secs(120));
	}
	let _ = std::fs::remove_dir_all(&dir);
}

/// An untouched small `.fxd` written by `fxd::save`: the open's peak memory.
#[test]
#[ignore = "audit: child process"]
fn plain_small_fxd_open_peak() {
	let dir = root();
	let p = small_fxd(&dir, "plain.fxd");
	let (file, footer) = fxd::FxdFile::open(&p).unwrap();
	let (_, payload) = file.read_chunk(fxd::ChunkRef { offset: footer.manifest_offset, len: footer.manifest_len }).unwrap();
	println!(
		"AUDIT plain manifest: {} B compressed, frame content size {:?}",
		payload.len(),
		zstd::zstd_safe::get_frame_content_size(&payload)
	);
	drop(file);
	report("fxd: a 512² one-layer file saved by fxd::save", &p, Duration::from_secs(60));
	let _ = std::fs::remove_dir_all(&dir);
}
