//! A small base64 codec (M8): tip and pattern pixels in JSON (presets, the
//! UI's file uploads). FAST: hand-rolled, no crate.

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
	let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
	for chunk in data.chunks(3) {
		let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
		let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
		out.push(TABLE[(n >> 18) as usize & 63] as char);
		out.push(TABLE[(n >> 12) as usize & 63] as char);
		out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
		out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
	}
	out
}

/// Decode, ignoring anything that is not a base64 digit (a `data:` prefix
/// must be stripped first).
pub fn decode(text: &str) -> Vec<u8> {
	let mut out = Vec::with_capacity(text.len() / 4 * 3);
	let mut acc = 0u32;
	let mut bits = 0;
	for c in text.bytes() {
		let v = match c {
			b'A'..=b'Z' => c - b'A',
			b'a'..=b'z' => c - b'a' + 26,
			b'0'..=b'9' => c - b'0' + 52,
			b'+' | b'-' => 62,
			b'/' | b'_' => 63,
			_ => continue,
		};
		acc = (acc << 6) | u32::from(v);
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((acc >> bits) as u8);
		}
	}
	out
}
