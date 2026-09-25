//! PNG and JPEG import tests: every PNG variant written with the `png`
//! encoder and compared pixel-exactly (edges included); JPEG from a small
//! committed fixture, with EXIF orientation spliced in.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use ::png::{BitDepth as PngDepth, ColorType, PixelDimensions, Unit};
use fx_core::{BitDepth, ColorProfile};
use fx_tiles::{PixelFormat, TileBuffer, TileSlot, TileStore, TileStoreConfig, TiledImage};

use crate::jpeg::{exif_orientation, source_position};
use crate::{IoError, import_file};

const W: u32 = 300;
const H: u32 = 270;

fn dir() -> PathBuf {
	let dir = std::env::temp_dir().join("fx-io-png-jpeg-tests");
	std::fs::create_dir_all(&dir).unwrap();
	dir
}

fn store() -> TileStore {
	let mut config = TileStoreConfig::for_tests(dir().join("scratch"));
	config.hot_budget = 1 << 30;
	TileStore::new(config).unwrap()
}

fn pixel(image: &TiledImage, store: &TileStore, x: u32, y: u32) -> [u16; 4] {
	let format = image.format();
	let tile = match image.slot(0, x / 256, y / 256) {
		TileSlot::Empty => TileBuffer::zeroed(format),
		TileSlot::Solid(v) => TileBuffer::filled(format, *v),
		TileSlot::Data(handle) => (*store.get(handle).unwrap()).clone(),
	};
	let i = ((y % 256) * 256 + x % 256) as usize * 4;
	if format == PixelFormat::Rgba16 {
		let s = tile.as_u16();
		[s[i], s[i + 1], s[i + 2], s[i + 3]]
	} else {
		let b = tile.bytes();
		[b[i].into(), b[i + 1].into(), b[i + 2].into(), b[i + 3].into()]
	}
}

const PROBES: [(u32, u32); 7] = [(0, 0), (255, 0), (256, 0), (299, 269), (0, 256), (123, 200), (257, 263)];

fn check(image: &TiledImage, store: &TileStore, expected: impl Fn(u32, u32) -> [u16; 4]) {
	for &(x, y) in &PROBES {
		assert_eq!(pixel(image, store, x, y), expected(x, y), "pixel ({x}, {y})");
	}
	for &(x, y) in &[(300, 10), (511, 511), (10, 270)] {
		assert_eq!(pixel(image, store, x, y), [0, 0, 0, 0], "outside pixel ({x}, {y})");
	}
}

fn v8(x: u32, y: u32, c: u32) -> u8 {
	((x * 7 + y * 13 + c * 61) % 256) as u8
}

fn v16(x: u32, y: u32, c: u32) -> u16 {
	((x * 211 + y * 97 + c * 5003) % 65536) as u16
}

fn write_png(path: &Path, color: ColorType, depth: PngDepth, data: &[u8], setup: impl FnOnce(&mut ::png::Encoder<'_, BufWriter<File>>)) {
	let mut enc = ::png::Encoder::new(BufWriter::new(File::create(path).unwrap()), W, H);
	enc.set_color(color);
	enc.set_depth(depth);
	setup(&mut enc);
	let mut writer = enc.write_header().unwrap();
	writer.write_image_data(data).unwrap();
}

fn import(path: &Path) -> (crate::ImportedImage, TileStore) {
	let store = store();
	let img = import_file(path, &store, &mut |_| true).unwrap();
	(img, store)
}

#[test]
fn png_rgba8_with_physical_resolution() {
	let path = dir().join("rgba8.png");
	let data: Vec<u8> = (0..H).flat_map(|y| (0..W).flat_map(move |x| (0..4).map(move |c| v8(x, y, c)))).collect();
	write_png(&path, ColorType::Rgba, PngDepth::Eight, &data, |e| {
		e.set_pixel_dims(Some(PixelDimensions {
			xppu: 11811, // 300 ppi
			yppu: 11811,
			unit: Unit::Meter,
		}))
	});
	let (img, store) = import(&path);
	assert_eq!((img.width, img.height, img.depth), (W, H, BitDepth::U8));
	assert!((img.ppi - 300.0).abs() < 0.1, "ppi {}", img.ppi);
	assert_eq!(img.profile, ColorProfile::Srgb);
	check(&img.image, &store, |x, y| std::array::from_fn(|c| v8(x, y, c as u32).into()));
}

#[test]
fn png_rgb16_big_endian_samples() {
	let path = dir().join("rgb16.png");
	let data: Vec<u8> = (0..H)
		.flat_map(|y| (0..W).flat_map(move |x| (0..3).flat_map(move |c| v16(x, y, c).to_be_bytes())))
		.collect();
	write_png(&path, ColorType::Rgb, PngDepth::Sixteen, &data, |_| {});
	let (img, store) = import(&path);
	assert_eq!(img.depth, BitDepth::U16);
	assert_eq!(img.ppi, 72.0);
	check(&img.image, &store, |x, y| [v16(x, y, 0), v16(x, y, 1), v16(x, y, 2), 65535]);
}

#[test]
fn png_gray_alpha_16() {
	let path = dir().join("graya16.png");
	let data: Vec<u8> = (0..H)
		.flat_map(|y| (0..W).flat_map(move |x| [v16(x, y, 0), v16(x, y, 1)].into_iter().flat_map(u16::to_be_bytes)))
		.collect();
	write_png(&path, ColorType::GrayscaleAlpha, PngDepth::Sixteen, &data, |_| {});
	let (img, store) = import(&path);
	check(&img.image, &store, |x, y| {
		let g = v16(x, y, 0);
		[g, g, g, v16(x, y, 1)]
	});
}

#[test]
fn png_gray8_and_palette_with_transparency() {
	let gray = dir().join("gray8.png");
	let data: Vec<u8> = (0..H).flat_map(|y| (0..W).map(move |x| v8(x, y, 0))).collect();
	write_png(&gray, ColorType::Grayscale, PngDepth::Eight, &data, |_| {});
	let (img, store) = import(&gray);
	check(&img.image, &store, |x, y| {
		let g = v8(x, y, 0).into();
		[g, g, g, 255]
	});

	// 4-entry palette; entry 3 is half transparent via tRNS.
	let palette = [10u8, 20, 30, 40, 50, 60, 70, 80, 90, 200, 210, 220];
	let alpha = [255u8, 255, 255, 128];
	let index = |x: u32, y: u32| ((x / 3 + y) % 4) as usize;
	let pal = dir().join("palette.png");
	let data: Vec<u8> = (0..H).flat_map(|y| (0..W).map(move |x| index(x, y) as u8)).collect();
	write_png(&pal, ColorType::Indexed, PngDepth::Eight, &data, |e| {
		e.set_palette(palette.to_vec());
		e.set_trns(alpha.to_vec());
	});
	let (img, store) = import(&pal);
	assert_eq!(img.depth, BitDepth::U8);
	check(&img.image, &store, |x, y| {
		let i = index(x, y);
		[palette[i * 3].into(), palette[i * 3 + 1].into(), palette[i * 3 + 2].into(), alpha[i].into()]
	});
}

fn fixture() -> Vec<u8> {
	std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/red-blue-64x32.jpg")).unwrap()
}

/// The fixture with an APP1 EXIF segment carrying `orientation`, inserted
/// right after SOI.
fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
	let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec(); // big-endian, IFD at 8
	tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
	tiff.extend_from_slice(&0x0112u16.to_be_bytes());
	tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
	tiff.extend_from_slice(&1u32.to_be_bytes());
	tiff.extend_from_slice(&orientation.to_be_bytes());
	tiff.extend_from_slice(&[0, 0]);
	tiff.extend_from_slice(&0u32.to_be_bytes());
	let mut payload = b"Exif\0\0".to_vec();
	payload.extend_from_slice(&tiff);
	let mut out = jpeg[..2].to_vec();
	out.extend_from_slice(&[0xFF, 0xE1]);
	out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
	out.extend_from_slice(&payload);
	out.extend_from_slice(&jpeg[2..]);
	out
}

/// Red-ish (left of the fixture) vs blue-ish (right).
fn is_red(p: [u16; 4]) -> bool {
	p[0] > 150 && p[2] < 90 && p[3] == 255
}
fn is_blue(p: [u16; 4]) -> bool {
	p[2] > 150 && p[0] < 90 && p[3] == 255
}

#[test]
fn jpeg_baseline_decodes_upright() {
	let path = dir().join("upright.jpg");
	std::fs::write(&path, fixture()).unwrap();
	let (img, store) = import(&path);
	assert_eq!((img.width, img.height, img.depth), (64, 32, BitDepth::U8));
	assert!(is_red(pixel(&img.image, &store, 8, 16)));
	assert!(is_blue(pixel(&img.image, &store, 56, 16)));
	assert_eq!(pixel(&img.image, &store, 64, 0), [0, 0, 0, 0], "outside is transparent");
}

#[test]
fn jpeg_exif_orientation_6_rotates_clockwise() {
	let path = dir().join("rot6.jpg");
	std::fs::write(&path, with_orientation(&fixture(), 6)).unwrap();
	let (img, store) = import(&path);
	// 64 × 32 stored, displayed rotated 90° clockwise: 32 × 64, the stored
	// left (red) half ends up at the top.
	assert_eq!((img.width, img.height), (32, 64));
	assert!(is_red(pixel(&img.image, &store, 16, 8)), "top is red");
	assert!(is_blue(pixel(&img.image, &store, 16, 56)), "bottom is blue");
}

#[test]
fn jpeg_exif_orientation_3_rotates_180() {
	let path = dir().join("rot3.jpg");
	std::fs::write(&path, with_orientation(&fixture(), 3)).unwrap();
	let (img, store) = import(&path);
	assert_eq!((img.width, img.height), (64, 32));
	assert!(is_blue(pixel(&img.image, &store, 8, 16)), "left is now blue");
	assert!(is_red(pixel(&img.image, &store, 56, 16)));
}

#[test]
fn exif_parsing_and_orientation_mapping() {
	let tiff_le = [b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0];
	assert_eq!(exif_orientation(&tiff_le), Some(8));
	assert_eq!(exif_orientation(b"Exif\0\0garbage"), None);
	assert_eq!(exif_orientation(&[]), None);
	// Every orientation maps the output corners onto stored corners.
	let (w, h) = (4, 3);
	for o in 1..=8u16 {
		let (ow, oh) = if o >= 5 { (h, w) } else { (w, h) };
		for (x, y) in [(0, 0), (ow - 1, 0), (0, oh - 1), (ow - 1, oh - 1)] {
			let (sx, sy) = source_position(o, x, y, w, h);
			assert!(sx < w && sy < h, "orientation {o}: ({x}, {y}) → ({sx}, {sy}) out of range");
		}
	}
	assert_eq!(source_position(6, 0, 0, w, h), (0, 2), "90° cw: output top-left is stored bottom-left");
	assert_eq!(source_position(8, 0, 0, w, h), (3, 0), "90° ccw: output top-left is stored top-right");
}

#[test]
fn truncated_png_is_a_decode_error() {
	let path = dir().join("truncated.png");
	let data: Vec<u8> = vec![7; (W * H * 3) as usize];
	write_png(&path, ColorType::Rgb, PngDepth::Eight, &data, |_| {});
	let bytes = std::fs::read(&path).unwrap();
	std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
	assert!(matches!(import_file(&path, &store(), &mut |_| true), Err(IoError::Decode(_) | IoError::Io(_))));
}
