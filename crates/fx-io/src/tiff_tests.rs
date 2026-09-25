//! TIFF import tests: small generated files covering every supported variant,
//! compared pixel-exactly, including the right/bottom edge tiles.

use std::fs::File;
use std::path::{Path, PathBuf};

use fx_core::{BitDepth, ColorProfile};
use fx_tiles::{PixelFormat, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};
use tiff::encoder::{Compression, DeflateLevel, Rational, TiffEncoder, colortype};
use tiff::tags::{ResolutionUnit, Tag};

use crate::{ImportedImage, IoError, import_file};

// 300 × 270: 2 × 2 tiles, with partial edge tiles on the right and bottom.
const W: u32 = 300;
const H: u32 = 270;

fn dir() -> PathBuf {
	let dir = std::env::temp_dir().join("fx-io-tests");
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

fn store() -> TileStore {
	let mut config = TileStoreConfig::for_tests(dir().join("scratch"));
	config.hot_budget = 1 << 30;
	TileStore::new(config).unwrap()
}

/// Deterministic sample value for `(x, y, channel)` on a 16-bit scale.
fn value16(x: u32, y: u32, c: u32) -> u16 {
	((x * 211 + y * 97 + c * 5003) % 65536) as u16
}

fn value8(x: u32, y: u32, c: u32) -> u8 {
	((x * 7 + y * 13 + c * 61) % 256) as u8
}

fn samples16(channels: u32) -> Vec<u16> {
	(0..H)
		.flat_map(|y| (0..W).flat_map(move |x| (0..channels).map(move |c| value16(x, y, c))))
		.collect()
}

fn samples8(channels: u32) -> Vec<u8> {
	(0..H)
		.flat_map(|y| (0..W).flat_map(move |x| (0..channels).map(move |c| value8(x, y, c))))
		.collect()
}

fn import(path: &Path) -> (ImportedImage, TileStore) {
	let store = store();
	let imported = import_file(path, &store, &mut |_| true).unwrap();
	(imported, store)
}

/// RGBA of pixel `(x, y)`, on the image's own scale.
fn pixel(image: &TiledImage, store: &TileStore, x: u32, y: u32) -> [u16; 4] {
	let format = image.format();
	let tile = match image.slot(0, x / 256, y / 256) {
		TileSlot::Empty => TileBuffer::zeroed(format),
		TileSlot::Solid(v) => TileBuffer::filled(format, *v),
		TileSlot::Data(handle) => (*store.get(handle).unwrap()).clone(),
	};
	let i = ((y % 256) * 256 + x % 256) as usize * 4;
	match format {
		PixelFormat::Rgba16 => {
			let s = tile.as_u16();
			[s[i], s[i + 1], s[i + 2], s[i + 3]]
		}
		_ => {
			let b = tile.bytes();
			[b[i].into(), b[i + 1].into(), b[i + 2].into(), b[i + 3].into()]
		}
	}
}

/// Every pixel of a few rows/columns, including the edges, matches `expected`.
fn check(image: &TiledImage, store: &TileStore, expected: impl Fn(u32, u32) -> [u16; 4]) {
	for &(x, y) in &[(0, 0), (255, 0), (256, 0), (299, 0), (0, 255), (0, 256), (299, 269), (123, 200), (257, 263)] {
		assert_eq!(pixel(image, store, x, y), expected(x, y), "pixel ({x}, {y})");
	}
	// Outside the image, inside the edge tiles: transparent.
	for &(x, y) in &[(300, 10), (511, 511), (10, 270), (299, 511)] {
		assert_eq!(pixel(image, store, x, y), [0, 0, 0, 0], "outside pixel ({x}, {y})");
	}
}

#[test]
fn rgb8_uncompressed_strips() {
	let path = dir().join("rgb8.tif");
	let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
	enc.write_image::<colortype::RGB8>(W, H, &samples8(3)).unwrap();
	let (img, store) = import(&path);
	assert_eq!((img.width, img.height, img.depth, img.ppi), (W, H, BitDepth::U8, 72.0));
	assert_eq!(img.profile, ColorProfile::Srgb);
	check(&img.image, &store, |x, y| {
		[value8(x, y, 0).into(), value8(x, y, 1).into(), value8(x, y, 2).into(), 255]
	});
}

#[test]
fn rgb16_lzw_with_odd_rows_per_strip() {
	let path = dir().join("rgb16-lzw.tif");
	let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap().with_compression(Compression::Lzw);
	let mut image = enc.new_image::<colortype::RGB16>(W, H).unwrap();
	image.rows_per_strip(7).unwrap();
	image.write_data(&samples16(3)).unwrap();
	let (img, store) = import(&path);
	assert_eq!(img.depth, BitDepth::U16);
	check(&img.image, &store, |x, y| [value16(x, y, 0), value16(x, y, 1), value16(x, y, 2), 65535]);
}

#[test]
fn rgba8_deflate_straight_alpha() {
	let path = dir().join("rgba8-deflate.tif");
	let mut enc = TiffEncoder::new(File::create(&path).unwrap())
		.unwrap()
		.with_compression(Compression::Deflate(DeflateLevel::Fast));
	enc.write_image::<colortype::RGBA8>(W, H, &samples8(4)).unwrap();
	let (img, store) = import(&path);
	check(&img.image, &store, |x, y| std::array::from_fn(|c| value8(x, y, c as u32).into()));
}

#[test]
fn gray16_packbits_expands_to_rgba() {
	let path = dir().join("gray16-packbits.tif");
	let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap().with_compression(Compression::Packbits);
	enc.write_image::<colortype::Gray16>(W, H, &samples16(1)).unwrap();
	let (img, store) = import(&path);
	check(&img.image, &store, |x, y| {
		let v = value16(x, y, 0);
		[v, v, v, 65535]
	});
}

#[test]
fn bigtiff_with_icc_and_resolution_in_centimetres() {
	let path = dir().join("big-gray8.tif");
	let icc = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9];
	let mut enc = TiffEncoder::new_big(File::create(&path).unwrap()).unwrap();
	let mut image = enc.new_image::<colortype::Gray8>(W, H).unwrap();
	image.resolution(ResolutionUnit::Centimeter, Rational { n: 118, d: 1 });
	image.encoder().write_tag(Tag::IccProfile, &icc[..]).unwrap();
	image.write_data(&samples8(1)).unwrap();
	let (img, store) = import(&path);
	assert_eq!(img.profile, ColorProfile::Icc(icc.into()));
	assert!((img.ppi - 118.0 * 2.54).abs() < 0.01, "ppi {}", img.ppi);
	check(&img.image, &store, |x, y| {
		let v = value8(x, y, 0).into();
		[v, v, v, 255]
	});
}

#[test]
fn gray_alpha_associated_is_unpremultiplied() {
	// gray = alpha / 2 premultiplied → straight gray ≈ 32768 wherever alpha > 0
	let path = dir().join("graya16-assoc-tiled.tif");
	let alpha = |x: u32, y: u32| ((x + y) % 7) as u16 * 9000;
	let samples: Vec<u16> = (0..H).flat_map(|y| (0..W).flat_map(move |x| [alpha(x, y) / 2, alpha(x, y)])).collect();
	write_manual(&path, 2, 16, 1, Some(1), Some((64, 48)), &bytes16(&samples));
	let (img, store) = import(&path);
	check(&img.image, &store, |x, y| {
		let a = alpha(x, y);
		let g = if a == 0 {
			0
		} else {
			((2 * u64::from(a / 2) * 65535 + u64::from(a)) / (2 * u64::from(a))) as u16
		};
		[g, g, g, a]
	});
}

#[test]
fn tiled_rgb8_with_tiles_smaller_than_a_band() {
	let path = dir().join("rgb8-tiled.tif");
	write_manual(&path, 3, 8, 2, None, Some((80, 32)), &samples8(3));
	let (img, store) = import(&path);
	check(&img.image, &store, |x, y| {
		[value8(x, y, 0).into(), value8(x, y, 1).into(), value8(x, y, 2).into(), 255]
	});
}

#[test]
fn unsupported_variants_are_refused_with_a_reason() {
	let path = dir().join("cmyk8.tif");
	let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
	enc.write_image::<colortype::CMYK8>(8, 8, &[0u8; 8 * 8 * 4]).unwrap();
	match import_file(&path, &store(), &mut |_| true) {
		Err(IoError::Unsupported(reason)) => assert!(reason.contains("colour type"), "{reason}"),
		other => panic!("expected Unsupported, got {:?}", other.map(|_| ())),
	}
	let float = dir().join("float.tif");
	let mut enc = TiffEncoder::new(File::create(&float).unwrap()).unwrap();
	enc.write_image::<colortype::Gray32Float>(4, 4, &[0.5f32; 16]).unwrap();
	assert!(matches!(import_file(&float, &store(), &mut |_| true), Err(IoError::Unsupported(_))));
	let junk = dir().join("junk.bin");
	std::fs::write(&junk, b"definitely not an image").unwrap();
	assert!(matches!(import_file(&junk, &store(), &mut |_| true), Err(IoError::UnsupportedFormat)));
}

#[test]
fn progress_can_cancel() {
	let path = dir().join("cancel.tif");
	let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
	let mut image = enc.new_image::<colortype::RGB8>(W, H).unwrap();
	image.rows_per_strip(16).unwrap();
	image.write_data(&samples8(3)).unwrap();
	let mut calls = 0;
	let result = import_file(&path, &store(), &mut |f| {
		calls += 1;
		assert!((0.0..=1.0).contains(&f));
		calls < 3
	});
	assert!(matches!(result, Err(IoError::Cancelled)));
}

fn bytes16(samples: &[u16]) -> Vec<u8> {
	samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// A classic little-endian uncompressed TIFF, written by hand to cover what
/// the `tiff` encoder cannot: gray+alpha, associated alpha, tiles.
/// `data` is the whole image, chunky, row-major, `samples` per pixel.
fn write_manual(path: &Path, samples: u16, bits: u16, photometric: u16, extra: Option<u16>, tile: Option<(u32, u32)>, data: &[u8]) {
	let bpp = usize::from(samples) * usize::from(bits / 8);
	let row = W as usize * bpp;
	let mut chunks: Vec<Vec<u8>> = Vec::new();
	match tile {
		Some((tw, th)) => {
			for ty in 0..H.div_ceil(th) {
				for tx in 0..W.div_ceil(tw) {
					// Full-size tiles, padded with zeros past the image.
					let mut c = vec![0u8; tw as usize * th as usize * bpp];
					for r in 0..th {
						let y = ty * th + r;
						if y >= H {
							break;
						}
						let x0 = (tx * tw) as usize;
						let n = (tw as usize).min(W as usize - x0) * bpp;
						c[r as usize * tw as usize * bpp..][..n].copy_from_slice(&data[y as usize * row + x0 * bpp..][..n]);
					}
					chunks.push(c);
				}
			}
		}
		None => chunks.push(data.to_vec()),
	}
	let mut out = vec![b'I', b'I', 42, 0, 0, 0, 0, 0];
	let mut offsets = Vec::new();
	for c in &chunks {
		offsets.push(out.len() as u32);
		out.extend_from_slice(c);
	}
	if out.len() % 2 == 1 {
		out.push(0);
	}
	let ifd = out.len() as u32;
	out[4..8].copy_from_slice(&ifd.to_le_bytes());

	let long_array = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
	let counts: Vec<u32> = chunks.iter().map(|c| c.len() as u32).collect();
	let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = vec![
		(256, 4, 1, W.to_le_bytes().to_vec()),
		(257, 4, 1, H.to_le_bytes().to_vec()),
		(258, 3, u32::from(samples), (0..samples).flat_map(|_| bits.to_le_bytes()).collect()),
		(259, 3, 1, 1u16.to_le_bytes().to_vec()),
		(262, 3, 1, photometric.to_le_bytes().to_vec()),
		(277, 3, 1, samples.to_le_bytes().to_vec()),
		(284, 3, 1, 1u16.to_le_bytes().to_vec()),
	];
	match tile {
		Some((tw, th)) => {
			tags.push((322, 4, 1, tw.to_le_bytes().to_vec()));
			tags.push((323, 4, 1, th.to_le_bytes().to_vec()));
			tags.push((324, 4, offsets.len() as u32, long_array(&offsets)));
			tags.push((325, 4, counts.len() as u32, long_array(&counts)));
		}
		None => {
			tags.push((273, 4, 1, long_array(&offsets)));
			tags.push((278, 4, 1, H.to_le_bytes().to_vec()));
			tags.push((279, 4, 1, long_array(&counts)));
		}
	}
	if let Some(e) = extra {
		tags.push((338, 3, 1, e.to_le_bytes().to_vec()));
	}
	tags.sort_by_key(|t| t.0);
	let mut extra_at = ifd + 2 + tags.len() as u32 * 12 + 4;
	let mut dir = (tags.len() as u16).to_le_bytes().to_vec();
	let mut tail = Vec::new();
	for (tag, kind, count, bytes) in tags {
		dir.extend_from_slice(&tag.to_le_bytes());
		dir.extend_from_slice(&kind.to_le_bytes());
		dir.extend_from_slice(&count.to_le_bytes());
		if bytes.len() <= 4 {
			let mut v = bytes.clone();
			v.resize(4, 0);
			dir.extend_from_slice(&v);
		} else {
			dir.extend_from_slice(&extra_at.to_le_bytes());
			extra_at += bytes.len() as u32;
			tail.extend_from_slice(&bytes);
		}
	}
	dir.extend_from_slice(&0u32.to_le_bytes());
	out.extend_from_slice(&dir);
	out.extend_from_slice(&tail);
	std::fs::write(path, out).unwrap();
}
