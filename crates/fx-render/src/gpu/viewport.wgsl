// Viewport pass: background, transparency checkerboard, composite tiles.
// Output = document-encoded values (the shell's composite pass treats the
// viewport texture as sRGB-encoded, like Graphite's). Target format must be a
// non-sRGB format (Rgba8Unorm / Bgra8Unorm) so values are stored as-is.

struct Globals {
	size: vec2<f32>,        // viewport size in pixels
	nearest: u32,           // 1 = hard pixels (zoom >= 100 %)
	_pad: u32,
	_unused: vec4<f32>,
	doc_rect: vec4<f32>,    // x0 y0 x1 y1, screen pixels
}

struct Draw {
	src: vec4<f32>,         // u0 v0 u1 v1 inside the tile
	dst: vec4<f32>,         // x0 y0 x1 y1 screen pixels
	slot: u32,
	_pad0: u32,
	_pad1: u32,
	_pad2: u32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var tiles: texture_2d_array<f32>;
@group(0) @binding(2) var linear_sampler: sampler;
@group(0) @binding(3) var nearest_sampler: sampler;
@group(0) @binding(4) var<storage, read> draws: array<Draw>;

struct VsOut {
	@builtin(position) pos: vec4<f32>,
	@location(0) uv: vec2<f32>,
	@location(1) @interpolate(flat) slot: u32,
	@location(2) @interpolate(flat) kind: u32, // 0 checkerboard, 1 tile
}

fn corner(i: u32) -> vec2<f32> {
	// two triangles: (0,0) (1,0) (0,1) / (0,1) (1,0) (1,1)
	var c = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
	return c[i];
}

fn to_clip(p: vec2<f32>) -> vec4<f32> {
	return vec4<f32>(p.x / globals.size.x * 2.0 - 1.0, 1.0 - p.y / globals.size.y * 2.0, 0.0, 1.0);
}

// Instance 0 = checkerboard over the document rect; instance i>0 = draws[i-1].
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
	var out: VsOut;
	let c = corner(vi);
	if ii == 0u {
		let r = globals.doc_rect;
		out.pos = to_clip(mix(r.xy, r.zw, c));
		out.uv = c;
		out.slot = 0u;
		out.kind = 0u;
		return out;
	}
	let d = draws[ii - 1u];
	out.pos = to_clip(mix(d.dst.xy, d.dst.zw, c));
	out.uv = mix(d.src.xy, d.src.zw, c);
	out.slot = d.slot;
	out.kind = 1u;
	return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
	if in.kind == 0u {
		// Photoshop-style 8 px checkerboard in screen space.
		let cell = vec2<u32>(in.pos.xy / 8.0);
		let light = ((cell.x + cell.y) & 1u) == 0u;
		return select(vec4<f32>(0.8, 0.8, 0.8, 1.0), vec4<f32>(1.0, 1.0, 1.0, 1.0), light);
	}
	// Explicit level 0: tiles have no mips, and this avoids derivative
	// (uniform control flow) requirements after the branch above.
	if globals.nearest == 1u {
		return textureSampleLevel(tiles, nearest_sampler, in.uv, in.slot, 0.0);
	}
	return textureSampleLevel(tiles, linear_sampler, in.uv, in.slot, 0.0); // premultiplied
}
