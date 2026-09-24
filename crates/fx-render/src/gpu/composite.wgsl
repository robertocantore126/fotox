// Fotox tile compositor. One invocation = one pixel of one output tile.
// Executes the tile's op list (see program.rs / encode.rs) on a small stack of
// premultiplied accumulators. Must match reference.rs + blend.rs exactly
// (up to f32 vs f64 rounding): tests in gpu/tests.rs compare them.

const TILE: u32 = 256u;

// source/mask slot codes (anything smaller is an atlas slot index)
const EMPTY: u32 = 0xFFFFFFFFu;
const SOLID: u32 = 0xFFFFFFFEu;
const OUTSIDE: u32 = 0xFFFFFFFDu;

// op kinds
const K_LAYER: u32 = 0u;
const K_ADJUST_LUT: u32 = 1u;
const K_BEGIN_ISOLATED: u32 = 3u;
const K_BEGIN_PASS: u32 = 4u;
const K_END_ISOLATED: u32 = 5u;
const K_END_PASS: u32 = 6u;
const K_LOAD_PREFIX: u32 = 7u;

// op flags
const F_CLIP: u32 = 1u;
const F_MASK: u32 = 2u;
const F_DISSOLVE: u32 = 4u;

const STACK: u32 = 12u;

struct Op {
	kind: u32,
	blend: u32,
	flags: u32,
	alpha: f32,
	src: vec4<u32>,
	mask: vec4<u32>,
	src_shift: vec2<u32>,
	mask_shift: vec2<u32>,
	lut_row: u32,
	seed: u32,
	mask_outside: f32,
	_pad: f32,
	src_solid: array<vec4<f32>, 4>,
	mask_solid: vec4<f32>,
	params: vec4<f32>,
}

struct Job {
	op_start: u32,
	op_count: u32,
	out_slot: u32,
	_pad0: u32,
	origin: vec2<u32>,
	_pad1: vec2<u32>,
}

struct Globals {
	layers_per_page: u32,
	lut_size: u32,
	_pad: vec2<u32>,
}

@group(0) @binding(0) var page0: texture_2d_array<f32>;
@group(0) @binding(1) var page1: texture_2d_array<f32>;
@group(0) @binding(2) var page2: texture_2d_array<f32>;
@group(0) @binding(3) var page3: texture_2d_array<f32>;
@group(0) @binding(4) var page4: texture_2d_array<f32>;
@group(0) @binding(5) var page5: texture_2d_array<f32>;
@group(0) @binding(6) var page6: texture_2d_array<f32>;
@group(0) @binding(7) var page7: texture_2d_array<f32>;
@group(0) @binding(8) var luts: texture_2d<f32>;
@group(0) @binding(9) var<storage, read> ops: array<Op>;
@group(0) @binding(10) var<storage, read> jobs: array<Job>;
@group(0) @binding(11) var out_tiles: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(12) var<uniform> globals: Globals;

fn fetch(code: u32, p: vec2<u32>) -> vec4<f32> {
	let page = code / globals.layers_per_page;
	let layer = code % globals.layers_per_page;
	let q = vec2<i32>(p);
	switch page {
		case 0u: { return textureLoad(page0, q, layer, 0); }
		case 1u: { return textureLoad(page1, q, layer, 0); }
		case 2u: { return textureLoad(page2, q, layer, 0); }
		case 3u: { return textureLoad(page3, q, layer, 0); }
		case 4u: { return textureLoad(page4, q, layer, 0); }
		case 5u: { return textureLoad(page5, q, layer, 0); }
		case 6u: { return textureLoad(page6, q, layer, 0); }
		default: { return textureLoad(page7, q, layer, 0); }
	}
}

// Straight RGBA of the op's source at output pixel p.
fn sample_src(op_index: u32, p: vec2<u32>) -> vec4<f32> {
	let local = p + ops[op_index].src_shift;
	let idx = (local.y / TILE) * 2u + local.x / TILE;
	let code = ops[op_index].src[idx];
	if code == EMPTY || code == OUTSIDE {
		return vec4<f32>(0.0);
	}
	if code == SOLID {
		return ops[op_index].src_solid[idx];
	}
	return fetch(code, local % vec2<u32>(TILE));
}

fn sample_mask(op_index: u32, p: vec2<u32>) -> f32 {
	if (ops[op_index].flags & F_MASK) == 0u {
		return 1.0;
	}
	let local = p + ops[op_index].mask_shift;
	let idx = (local.y / TILE) * 2u + local.x / TILE;
	let code = ops[op_index].mask[idx];
	if code == OUTSIDE {
		return ops[op_index].mask_outside;
	}
	if code == SOLID || code == EMPTY {
		return ops[op_index].mask_solid[idx];
	}
	return fetch(code, local % vec2<u32>(TILE)).r;
}

// ---------------------------------------------------------------- blending

fn screen(b: f32, s: f32) -> f32 { return b + s - b * s; }

fn hard_light(b: f32, s: f32) -> f32 {
	if s <= 0.5 { return b * 2.0 * s; }
	return screen(b, 2.0 * s - 1.0);
}

fn color_burn(b: f32, s: f32) -> f32 {
	if b >= 1.0 { return 1.0; }
	if s <= 0.0 { return 0.0; }
	return 1.0 - min(1.0, (1.0 - b) / s);
}

fn color_dodge(b: f32, s: f32) -> f32 {
	if b <= 0.0 { return 0.0; }
	if s >= 1.0 { return 1.0; }
	return min(1.0, b / (1.0 - s));
}

// Ids = fx_core::BlendMode::shader_id (enum order). Never reorder.
fn blend_channel(mode: u32, b: f32, s: f32) -> f32 {
	switch mode {
		case 3u: { return min(b, s); }                          // Darken
		case 4u: { return b * s; }                              // Multiply
		case 5u: { return color_burn(b, s); }                   // ColorBurn
		case 6u: { return max(b + s - 1.0, 0.0); }              // LinearBurn
		case 8u: { return max(b, s); }                          // Lighten
		case 9u: { return screen(b, s); }                       // Screen
		case 10u: { return color_dodge(b, s); }                 // ColorDodge
		case 11u: { return min(b + s, 1.0); }                   // LinearDodge
		case 13u: { return hard_light(s, b); }                  // Overlay
		case 14u: {                                             // SoftLight (Photoshop)
			if s <= 0.5 { return 2.0 * b * s + b * b * (1.0 - 2.0 * s); }
			return 2.0 * b * (1.0 - s) + sqrt(b) * (2.0 * s - 1.0);
		}
		case 15u: { return hard_light(b, s); }                  // HardLight
		case 16u: {                                             // VividLight
			if s <= 0.5 { return color_burn(b, 2.0 * s); }
			return color_dodge(b, 2.0 * (s - 0.5));
		}
		case 17u: { return clamp(b + 2.0 * s - 1.0, 0.0, 1.0); } // LinearLight
		case 18u: {                                             // PinLight
			if s <= 0.5 { return min(b, 2.0 * s); }
			return max(b, 2.0 * s - 1.0);
		}
		case 19u: { return select(0.0, 1.0, b + s >= 1.0); }    // HardMix
		case 20u: { return abs(b - s); }                        // Difference
		case 21u: { return b + s - 2.0 * b * s; }               // Exclusion
		case 22u: { return max(b - s, 0.0); }                   // Subtract
		case 23u: {                                             // Divide
			if s == 0.0 { return select(1.0, 0.0, b == 0.0); }
			return min(b / s, 1.0);
		}
		default: { return s; }                                  // PassThrough, Normal, Dissolve
	}
}

fn lum(c: vec3<f32>) -> f32 { return 0.3 * c.r + 0.59 * c.g + 0.11 * c.b; }

fn clip_color(c: vec3<f32>) -> vec3<f32> {
	let l = lum(c);
	let n = min(c.r, min(c.g, c.b));
	let x = max(c.r, max(c.g, c.b));
	var out = c;
	if n < 0.0 { out = l + (out - l) * l / (l - n); }
	if x > 1.0 { out = l + (out - l) * (1.0 - l) / (x - l); }
	return out;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> { return clip_color(c + (l - lum(c))); }

fn sat(c: vec3<f32>) -> f32 { return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)); }

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
	// indices of min/mid/max, ties broken like a stable sort of (r, g, b)
	var lo = 0u;
	var mi = 1u;
	var hi = 2u;
	if c[mi] < c[lo] { let t = lo; lo = mi; mi = t; }
	if c[hi] < c[mi] { let t = mi; mi = hi; hi = t; }
	if c[mi] < c[lo] { let t = lo; lo = mi; mi = t; }
	var out = vec3<f32>(0.0);
	if c[hi] > c[lo] {
		out[mi] = (c[mi] - c[lo]) * s / (c[hi] - c[lo]);
		out[hi] = s;
	}
	return out;
}

fn blend(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
	switch mode {
		case 7u: { return select(cb, cs, cs.r + cs.g + cs.b <= cb.r + cb.g + cb.b); }  // DarkerColor
		case 12u: { return select(cb, cs, cs.r + cs.g + cs.b >= cb.r + cb.g + cb.b); } // LighterColor
		case 24u: { return set_lum(set_sat(cs, sat(cb)), lum(cb)); }                    // Hue
		case 25u: { return set_lum(set_sat(cb, sat(cs)), lum(cb)); }                    // Saturation
		case 26u: { return set_lum(cs, lum(cb)); }                                      // Color
		case 27u: { return set_lum(cb, lum(cs)); }                                      // Luminosity
		default: {
			return vec3<f32>(blend_channel(mode, cb.r, cs.r), blend_channel(mode, cb.g, cs.g), blend_channel(mode, cb.b, cs.b));
		}
	}
}

fn unpremultiply(p: vec4<f32>) -> vec3<f32> {
	if p.a <= 0.0 { return vec3<f32>(0.0); }
	return p.rgb / p.a;
}

fn composite(mode: u32, backdrop: vec4<f32>, cs: vec3<f32>, alpha_s: f32, atop: bool) -> vec4<f32> {
	let ab = backdrop.a;
	let cb = unpremultiply(backdrop);
	let mixed = blend(mode, cb, cs);
	let cs2 = (1.0 - ab) * cs + ab * mixed;
	if atop {
		return vec4<f32>(alpha_s * ab * cs2 + (1.0 - alpha_s) * backdrop.rgb, ab);
	}
	return vec4<f32>(alpha_s * cs2 + (1.0 - alpha_s) * backdrop.rgb, alpha_s + ab * (1.0 - alpha_s));
}

fn dissolve_hash(x: u32, y: u32, seed: u32) -> f32 {
	var h = (x * 0x9E3779B1u) ^ (y * 0x85EBCA77u) ^ (seed * 0xC2B2AE3Du);
	h ^= h >> 16u;
	h *= 0x7FEB352Du;
	h ^= h >> 15u;
	h *= 0x846CA68Bu;
	h ^= h >> 16u;
	return f32(h) / 4294967296.0;
}

fn lut_channel(row: u32, c: u32, v: f32) -> f32 {
	let x = clamp(v, 0.0, 1.0) * f32(globals.lut_size - 1u);
	let i = min(u32(floor(x)), globals.lut_size - 2u);
	let t = x - f32(i);
	let a = textureLoad(luts, vec2<i32>(i32(i), i32(row)), 0)[c];
	let b = textureLoad(luts, vec2<i32>(i32(i + 1u), i32(row)), 0)[c];
	return a + (b - a) * t;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
	let job = jobs[gid.z];
	let p = gid.xy;
	var stack: array<vec4<f32>, STACK>;
	var sp = 0u;
	stack[0] = vec4<f32>(0.0);

	for (var n = 0u; n < job.op_count; n++) {
		let i = job.op_start + n;
		let kind = ops[i].kind;
		switch kind {
			case K_LAYER: {
				let src = sample_src(i, p);
				var alpha_s = src.a * ops[i].alpha * sample_mask(i, p);
				var mode = ops[i].blend;
				if (ops[i].flags & F_DISSOLVE) != 0u {
					let h = dissolve_hash(job.origin.x + p.x, job.origin.y + p.y, ops[i].seed);
					alpha_s = select(0.0, 1.0, h < alpha_s);
					mode = 1u; // Normal
				}
				stack[sp] = composite(mode, stack[sp], src.rgb, alpha_s, (ops[i].flags & F_CLIP) != 0u);
			}
			case K_ADJUST_LUT: {
				let cb = unpremultiply(stack[sp]);
				let row = ops[i].lut_row;
				let f = vec3<f32>(lut_channel(row, 0u, cb.r), lut_channel(row, 1u, cb.g), lut_channel(row, 2u, cb.b));
				stack[sp] = composite(ops[i].blend, stack[sp], f, ops[i].alpha * sample_mask(i, p), true);
			}
			case K_BEGIN_ISOLATED: {
				sp += 1u;
				stack[sp] = vec4<f32>(0.0);
			}
			case K_BEGIN_PASS: {
				sp += 1u;
				stack[sp] = stack[sp - 1u];
			}
			case K_END_ISOLATED: {
				let g = stack[sp];
				sp -= 1u;
				let alpha_s = g.a * ops[i].alpha * sample_mask(i, p);
				stack[sp] = composite(ops[i].blend, stack[sp], unpremultiply(g), alpha_s, (ops[i].flags & F_CLIP) != 0u);
			}
			case K_END_PASS: {
				let r = stack[sp];
				sp -= 1u;
				let t = ops[i].alpha * sample_mask(i, p);
				stack[sp] = stack[sp] + (r - stack[sp]) * t;
			}
			case K_LOAD_PREFIX: {
				// premultiplied composite stored in the atlas by an earlier frame
				stack[sp] = fetch(ops[i].src.x, p);
			}
			default: {}
		}
	}
	textureStore(out_tiles, vec2<i32>(p), i32(job.out_slot), stack[0]);
}
