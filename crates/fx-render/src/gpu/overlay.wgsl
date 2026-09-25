// Fotox — viewport overlay pass (M5-T02).
//
// Vertices arrive already tessellated in screen pixels (fx_render::overlay::
// tessellate). This shader only projects them to clip space and, for a
// marching-ants line, picks black or white from the vertex's arc length and
// the time uniform (so the ants crawl without any per-frame CPU work).

struct OverlayGlobals {
	size: vec2<f32>,
	time: f32,
	_pad: f32,
};

struct Vertex {
	pos: vec2<f32>,
	_pad0: vec2<f32>,
	color: vec4<f32>,
	arc: f32,
	ants: f32,
	_pad1: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: OverlayGlobals;
@group(0) @binding(1) var<storage, read> vertices: array<Vertex>;

struct VsOut {
	@builtin(position) clip: vec4<f32>,
	@location(0) color: vec4<f32>,
	@location(1) arc: f32,
	@location(2) ants: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
	let v = vertices[index];
	var out: VsOut;
	// Screen pixels (origin top-left) → clip space.
	out.clip = vec4<f32>(v.pos.x / globals.size.x * 2.0 - 1.0, 1.0 - v.pos.y / globals.size.y * 2.0, 0.0, 1.0);
	out.color = v.color;
	out.arc = v.arc;
	out.ants = v.ants;
	return out;
}

// The ants crawl towards the end of the path, like Photoshop's.
const ANTS_SPEED: f32 = 30.0;
const ANTS_PERIOD: f32 = 8.0;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
	var rgb = in.color.rgb;
	if (in.ants > 0.5) {
		let phase = fract((in.arc - globals.time * ANTS_SPEED) / ANTS_PERIOD);
		rgb = select(vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(1.0, 1.0, 1.0), phase < 0.5);
	}
	let alpha = in.color.a;
	// Premultiplied, for PREMULTIPLIED_ALPHA_BLENDING.
	return vec4<f32>(rgb * alpha, alpha);
}
