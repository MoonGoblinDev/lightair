// Shared by every clip effect in fx.wgsl (prepended by fx.rs). A port of
// modules/lightair-effect-stack/data/fx/common.inc.
//
// An effect `name` (the catalog's "fx.name") is one pass, `fs_name_0`, or several,
// `fs_name_0`, `fs_name_1`, … run in order: each reads `image` (the input, or the previous
// pass's output) and `source` (the input). Pictures are premultiplied RGBA.
//
// Uniforms: the effect file's own uniforms, in the order the .effect file declares them, are
// `u.p[0]`, `u.p[1]`, …: a float in `.x` (ints too, as floats), a float4 colour in `.xyzw`.

struct U {
    // px
    size: vec2<f32>,
    // 1 / size
    texel: vec2<f32>,
    // x: seconds
    clock: vec4<f32>,
    p: array<vec4<f32>, 8>,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var source: texture_2d<f32>;
@group(0) @binding(3) var linear_clamp: sampler;
@group(0) @binding(4) var point_clamp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    // 0..1, top left first
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    // one triangle covering the target
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(xy.x * 2.0 - 1.0, 1.0 - xy.y * 2.0, 0.0, 1.0);
    o.uv = xy;
    return o;
}

fn time() -> f32 {
    return u.clock.x;
}

// Samples (always these: they work anywhere in a shader, branches included).
fn img(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(image, linear_clamp, uv, 0.0);
}

fn img_point(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(image, point_clamp, uv, 0.0);
}

fn src(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(source, linear_clamp, uv, 0.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Straight colour of a premultiplied sample.
fn unpremul(c: vec4<f32>) -> vec3<f32> {
    if (c.a > 0.0001) {
        return c.rgb / c.a;
    }
    return c.rgb;
}

// Uv in a square space centred on the frame (x scaled by the aspect), and back.
fn to_square(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>((uv.x - 0.5) * u.size.x / u.size.y, uv.y - 0.5);
}

fn from_square(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(p.x * u.size.y / u.size.x + 0.5, p.y + 0.5);
}

// 9-tap gaussian of `image` along dir (px per tap step).
fn gauss9(uv: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    let o = dir * u.texel;
    var s = img(uv) * 0.2270270;
    s = s + (img(uv + o * 1.3846154) + img(uv - o * 1.3846154)) * 0.3162162;
    s = s + (img(uv + o * 3.2307692) + img(uv - o * 3.2307692)) * 0.0702703;
    return s;
}

// HLSL's lerp and saturate, for ports.
fn sat(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}

fn sat3(x: vec3<f32>) -> vec3<f32> {
    return clamp(x, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn sat4(x: vec4<f32>) -> vec4<f32> {
    return clamp(x, vec4<f32>(0.0), vec4<f32>(1.0));
}
