//! The Windows clipboard (M5-T05, D-048): a small copy goes there as
//! `CF_DIBV5` (32-bit, straight alpha, bottom-up rows), and an image another
//! program copied pastes into Fotox from `CF_DIBV5` or `CF_DIB`.
//!
//! The DIB headers are written and parsed by hand (they are plain
//! little-endian structs), so no GDI types are involved.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
	CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};

/// Size of a `BITMAPV5HEADER`.
const V5_HEADER: usize = 124;
/// `BI_RGB` and `BI_BITFIELDS`.
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// `LCS_sRGB` ('sRGB') and `LCS_GM_IMAGES`.
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;

/// The clipboard's change counter: a paste compares it with the value after
/// Fotox's own last copy to know whether another program copied since.
pub(crate) fn sequence() -> u32 {
	// SAFETY: no arguments, no side effects.
	unsafe { GetClipboardSequenceNumber() }
}

/// Keeps the clipboard open for a scope.
struct Open;

impl Open {
	fn new(hwnd: Option<isize>) -> Option<Self> {
		// SAFETY: the handle is the live main window (or none, for reading).
		unsafe { OpenClipboard(hwnd.map(|h| HWND(h as *mut _))) }.ok()?;
		Some(Open)
	}
}

impl Drop for Open {
	fn drop(&mut self) {
		// SAFETY: the clipboard was opened by `Open::new` on this thread.
		unsafe {
			let _ = CloseClipboard();
		}
	}
}

/// Put straight RGBA8 pixels (`width × height`, rows top to bottom) on the
/// clipboard as `CF_DIBV5`. Returns the clipboard sequence number after the
/// write, or `None` when Windows refused.
pub(crate) fn write_image(hwnd: isize, width: u32, height: u32, rgba8: &[u8]) -> Option<u32> {
	let pixels = (width as usize) * (height as usize) * 4;
	if rgba8.len() != pixels || width == 0 || height == 0 {
		return None;
	}
	let mut dib = Vec::with_capacity(V5_HEADER + pixels);
	let push32 = |v: u32, dib: &mut Vec<u8>| dib.extend_from_slice(&v.to_le_bytes());
	push32(V5_HEADER as u32, &mut dib);
	push32(width, &mut dib);
	push32(height, &mut dib); // positive: bottom-up rows
	dib.extend_from_slice(&1u16.to_le_bytes());
	dib.extend_from_slice(&32u16.to_le_bytes());
	push32(BI_BITFIELDS, &mut dib);
	push32(pixels as u32, &mut dib);
	push32(2835, &mut dib); // 72 dpi
	push32(2835, &mut dib);
	push32(0, &mut dib);
	push32(0, &mut dib);
	for mask in [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000u32] {
		push32(mask, &mut dib);
	}
	push32(LCS_SRGB, &mut dib);
	dib.extend_from_slice(&[0u8; 36 + 12]); // endpoints, gamma
	push32(LCS_GM_IMAGES, &mut dib);
	dib.extend_from_slice(&[0u8; 12]); // profile data/size, reserved
	debug_assert_eq!(dib.len(), V5_HEADER);
	for y in (0..height as usize).rev() {
		let row = &rgba8[y * width as usize * 4..(y + 1) * width as usize * 4];
		for p in row.chunks_exact(4) {
			dib.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
		}
	}
	let _open = Open::new(Some(hwnd))?;
	// SAFETY: the clipboard is open and owned by our window. The global block
	// is allocated with the DIB's size, filled while locked, and handed to the
	// clipboard (which then owns it); on failure it is freed here.
	unsafe {
		EmptyClipboard().ok()?;
		let memory: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, dib.len()).ok()?;
		let target = GlobalLock(memory).cast::<u8>();
		if target.is_null() {
			let _ = GlobalFree(Some(memory));
			return None;
		}
		std::ptr::copy_nonoverlapping(dib.as_ptr(), target, dib.len());
		let _ = GlobalUnlock(memory);
		if SetClipboardData(u32::from(CF_DIBV5.0), Some(HANDLE(memory.0))).is_err() {
			let _ = GlobalFree(Some(memory));
			return None;
		}
	}
	Some(sequence())
}

/// The image on the clipboard as straight RGBA8 (`width`, `height`, rows top
/// to bottom), from `CF_DIBV5` or `CF_DIB`. `None` when there is none or its
/// layout is not 24/32-bit uncompressed.
pub(crate) fn read_image() -> Option<(u32, u32, Vec<u8>)> {
	let _open = Open::new(None)?;
	// SAFETY: the clipboard is open; the handle it returns stays valid until
	// it closes, and it is only read while locked, within `GlobalSize`.
	let bytes = unsafe {
		let format = [CF_DIBV5, CF_DIB].into_iter().find(|f| IsClipboardFormatAvailable(u32::from(f.0)).is_ok())?;
		let handle = GetClipboardData(u32::from(format.0)).ok()?;
		let memory = HGLOBAL(handle.0);
		let size = GlobalSize(memory);
		let source = GlobalLock(memory).cast::<u8>();
		if source.is_null() || size == 0 {
			return None;
		}
		let bytes = std::slice::from_raw_parts(source, size).to_vec();
		let _ = GlobalUnlock(memory);
		bytes
	};
	parse_dib(&bytes)
}

/// A packed DIB (header, optional masks, pixels) as straight RGBA8.
fn parse_dib(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
	let u32_at = |at: usize| dib.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
	let header = u32_at(0)? as usize;
	let width = u32_at(4)? as i32;
	let height = u32_at(8)? as i32;
	let bits = u16::from_le_bytes([*dib.get(14)?, *dib.get(15)?]);
	let compression = u32_at(16)?;
	if width <= 0 || height == 0 || !(bits == 24 || bits == 32) || !(compression == BI_RGB || compression == BI_BITFIELDS) {
		return None;
	}
	// Masks: inside a V4/V5 header, or three DWORDs after a 40-byte one.
	let (masks, data) = match (compression, header) {
		(BI_BITFIELDS, 40) => ([u32_at(40)?, u32_at(44)?, u32_at(48)?, 0], header + 12),
		(BI_BITFIELDS, _) => ([u32_at(40)?, u32_at(44)?, u32_at(48)?, u32_at(52)?], header),
		_ => ([0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0], header),
	};
	let (w, h) = (width as usize, height.unsigned_abs() as usize);
	let stride = (w * bits as usize).div_ceil(32) * 4;
	if dib.len() < data + stride * h {
		return None;
	}
	let channel = |value: u32, mask: u32| -> u8 {
		if mask == 0 {
			return 255;
		}
		((value & mask) >> mask.trailing_zeros()) as u8
	};
	let mut out = vec![0u8; w * h * 4];
	let mut any_alpha = false;
	for row in 0..h {
		// Positive height = bottom-up rows.
		let src_row = if height > 0 { h - 1 - row } else { row };
		let line = &dib[data + src_row * stride..];
		for x in 0..w {
			let o = (row * w + x) * 4;
			if bits == 24 {
				let p = &line[x * 3..x * 3 + 3];
				out[o..o + 4].copy_from_slice(&[p[2], p[1], p[0], 255]);
			} else {
				let p = &line[x * 4..x * 4 + 4];
				let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
				let alpha_mask = if masks[3] == 0 && compression == BI_RGB { 0xFF00_0000 } else { masks[3] };
				let a = if alpha_mask == 0 { 255 } else { channel(v, alpha_mask) };
				any_alpha |= a != 0;
				out[o..o + 4].copy_from_slice(&[channel(v, masks[0]), channel(v, masks[1]), channel(v, masks[2]), a]);
			}
		}
	}
	// A 32-bit DIB whose alpha is all zero means "no alpha" (most programs).
	if bits == 32 && !any_alpha {
		for p in out.chunks_exact_mut(4) {
			p[3] = 255;
		}
	}
	Some((w as u32, h as u32, out))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_32_bit_bottom_up_dib_parses_top_down() {
		// 2 × 2, BI_RGB, BGRA rows bottom-up, alpha all zero → opaque.
		let mut dib = vec![0u8; 40];
		dib[0..4].copy_from_slice(&40u32.to_le_bytes());
		dib[4..8].copy_from_slice(&2i32.to_le_bytes());
		dib[8..12].copy_from_slice(&2i32.to_le_bytes());
		dib[14..16].copy_from_slice(&32u16.to_le_bytes());
		// Bottom row (y = 1): blue, green. Top row (y = 0): red, white.
		dib.extend_from_slice(&[255, 0, 0, 0, 0, 255, 0, 0]);
		dib.extend_from_slice(&[0, 0, 255, 0, 255, 255, 255, 0]);
		let (w, h, rgba) = parse_dib(&dib).unwrap();
		assert_eq!((w, h), (2, 2));
		assert_eq!(&rgba[0..8], &[255, 0, 0, 255, 255, 255, 255, 255]);
		assert_eq!(&rgba[8..16], &[0, 0, 255, 255, 0, 255, 0, 255]);
	}
}
