// Kookaburra terminal glyph pipeline.
//
// Two pipelines share this file and the uniform block:
//   * vs_bg / fs_bg — solid colored rectangles (cell bg, cursor, selection).
//   * vs_fg / fs_fg — textured rectangles sampling the R8 glyph atlas.
//
// Instance format is laid out in `glyph_pipeline.rs` as `BgInstance` and
// `FgInstance`. Everything is in physical pixels relative to the surface
// top-left; the vertex shader converts to clip space.

struct Uniforms {
    surface_size: vec2<f32>,
    atlas_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// 6 corners of a quad as triangle-list (two CCW tris sharing diagonal).
const CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 1.0),
);

fn pixel_to_clip(px: vec2<f32>) -> vec4<f32> {
    let x = 2.0 * px.x / u.surface_size.x - 1.0;
    let y = 1.0 - 2.0 * px.y / u.surface_size.y;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn unpack_rgba(c: u32) -> vec4<f32> {
    let r = f32((c      ) & 0xffu) / 255.0;
    let g = f32((c >>  8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    let a = f32((c >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// --- Background pass ---------------------------------------------------

struct BgVsIn {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: u32,
};

struct BgVsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) vid: u32, inst: BgVsIn) -> BgVsOut {
    var corners = CORNERS;
    let corner = corners[vid];
    let px = inst.pos + corner * inst.size;
    var out: BgVsOut;
    out.clip_pos = pixel_to_clip(px);
    out.color = unpack_rgba(inst.color);
    return out;
}

@fragment
fn fs_bg(in: BgVsOut) -> @location(0) vec4<f32> {
    // premultiplied
    return vec4<f32>(in.color.rgb * in.color.a, in.color.a);
}

// --- Glyph pass --------------------------------------------------------

struct FgVsIn {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) color: u32,
};

struct FgVsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_fg(@builtin(vertex_index) vid: u32, inst: FgVsIn) -> FgVsOut {
    var corners = CORNERS;
    let corner = corners[vid];
    let px = inst.pos + corner * inst.size;
    var out: FgVsOut;
    out.clip_pos = pixel_to_clip(px);
    // uv_min/uv_max are already in texel-to-[0,1] space, normalized by
    // atlas size on the CPU side. Mix per-corner.
    out.uv = mix(inst.uv_min, inst.uv_max, corner);
    out.color = unpack_rgba(inst.color);
    return out;
}

@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

@fragment
fn fs_fg(in: FgVsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas_tex, atlas_samp, in.uv).r;
    // premultiplied alpha
    return vec4<f32>(in.color.rgb * in.color.a * coverage, in.color.a * coverage);
}
