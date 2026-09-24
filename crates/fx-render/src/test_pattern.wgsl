// Procedural test pattern of a virtual document (M0-T06): a 256 px checker in
// document space, a thicker line every 4096 px, the document bounds in cyan
// and #282828 outside the document. Written straight into an Rgba8Unorm
// target as sRGB-encoded values (the shell's composite pass decodes them).

struct Params {
	// viewport size in physical pixels
	viewport: vec2<f32>,
	// document point at the viewport centre
	center: vec2<f32>,
	// document size in pixels
	doc: vec2<f32>,
	// screen pixels per document pixel
	zoom: f32,
	_pad: f32,
};

@group(0) @binding(0) var<uniform> params: Params;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
	let xy = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0))[i];
	return vec4f(xy, 0.0, 1.0);
}

const OUTSIDE = vec3<f32>(0.157, 0.157, 0.157); // #282828
const CHECK_A = vec3<f32>(0.600, 0.600, 0.620);
const CHECK_B = vec3<f32>(0.780, 0.780, 0.800);
const MAJOR   = vec3<f32>(0.180, 0.260, 0.520); // every 4096 px
const BOUNDS  = vec3<f32>(0.000, 0.900, 0.900); // cyan

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
	// frag.xy is the pixel centre in viewport pixels.
	let doc = (frag.xy - params.viewport * 0.5) / params.zoom + params.center;
	// Width of one screen pixel in document pixels: line widths are in screen space.
	let px = 1.0 / params.zoom;

	// Bounds: a 2-screen-pixel frame just outside the document.
	let outside = doc.x < 0.0 || doc.y < 0.0 || doc.x >= params.doc.x || doc.y >= params.doc.y;
	if (outside) {
		let near = doc.x >= -2.0 * px && doc.y >= -2.0 * px && doc.x < params.doc.x + 2.0 * px && doc.y < params.doc.y + 2.0 * px;
		if (near) {
			return vec4f(BOUNDS, 1.0);
		}
		return vec4f(OUTSIDE, 1.0);
	}

	// Major lines: 3 screen px wide (at least), centred on multiples of 4096.
	let m = doc - 4096.0 * round(doc / 4096.0);
	let half_width = max(1.5 * px, 1.0);
	if (abs(m.x) < half_width || abs(m.y) < half_width) {
		return vec4f(MAJOR, 1.0);
	}

	let cell = floor(doc / 256.0);
	let odd = (i32(cell.x) + i32(cell.y)) & 1;
	return vec4f(select(CHECK_A, CHECK_B, odd == 1), 1.0);
}
