// The clip effects: a port of modules/lightair-effect-stack/data/fx/*.effect. fx_prelude.wgsl is
// prepended (uniforms, textures, samplers, `img`/`img_point`/`src`, and common.inc's helpers).
//
// Each effect `name` is `fs_name_0` ("Draw") or `fs_name_0`..`fs_name_N` ("Pass0".."PassN").
// Uniforms follow the order of the .effect file's own `uniform` declarations: u.p[0], u.p[1], …

// HLSL fmod: the result keeps the sign of a.
fn fx_fmod(a: f32, b: f32) -> f32 {
    return a - b * trunc(a / b);
}

// ---------------------------------------------------------------------------------------------
// blur: Gaussian blur, two separable passes (twice). p0 amount

@fragment
fn fs_blur_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    return gauss9(v.uv, vec2<f32>(0.5 + amount * 9.0, 0.0));
}

@fragment
fn fs_blur_1(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    return gauss9(v.uv, vec2<f32>(0.0, 0.5 + amount * 9.0));
}

@fragment
fn fs_blur_2(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    return gauss9(v.uv, vec2<f32>(0.3 + amount * 5.0, 0.0));
}

@fragment
fn fs_blur_3(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    return gauss9(v.uv, vec2<f32>(0.0, 0.3 + amount * 5.0));
}

// ---------------------------------------------------------------------------------------------
// bulge: bulge (positive) or pinch (negative) at the centre. p0 amount, p1 radius

@fragment
fn fs_bulge_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let radius = u.p[1].x;
    var p = to_square(v.uv);
    let r = length(p) / max(radius, 0.01);
    if (r < 1.0) {
        let k = amount * (1.0 - r * r);
        p = p * (1.0 - k * 0.5);
    }
    return img(from_square(p));
}

// ---------------------------------------------------------------------------------------------
// chroma: colour fringes, stronger towards the edges. p0 amount, p1 angle

@fragment
fn fs_chroma_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let angle = u.p[1].x;
    let d = vec2<f32>(cos(angle), sin(angle)) * u.texel * amount * 14.0;
    let k = 0.4 + length(v.uv - 0.5) * 1.6;
    let c = img(v.uv);
    let r = img(v.uv + d * k).r;
    let b = img(v.uv - d * k).b;
    return vec4<f32>(r, c.g, b, c.a);
}

// ---------------------------------------------------------------------------------------------
// duotone: two colours, shadows to highlights. p0 shadow_color, p1 highlight_color, p2 amount

@fragment
fn fs_duotone_0(v: VOut) -> @location(0) vec4<f32> {
    let shadow_color = u.p[0];
    let highlight_color = u.p[1];
    let amount = u.p[2].x;
    let c = img(v.uv);
    let s = unpremul(c);
    let l = smoothstep(0.0, 1.0, luma(s));
    return vec4<f32>(mix(s, mix(shadow_color.rgb, highlight_color.rgb, l), amount) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// edges: glowing outlines. p0 amount, p1 glow_color, p2 keep

fn edges_l_at(uv: vec2<f32>) -> f32 {
    return luma(img(uv).rgb);
}

@fragment
fn fs_edges_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let glow_color = u.p[1];
    let keep = u.p[2].x;
    let t = u.texel * 1.5;
    let uv = v.uv;
    let gx = -edges_l_at(uv + vec2<f32>(-t.x, -t.y)) - 2.0 * edges_l_at(uv + vec2<f32>(-t.x, 0.0)) - edges_l_at(uv + vec2<f32>(-t.x, t.y))
        + edges_l_at(uv + vec2<f32>(t.x, -t.y)) + 2.0 * edges_l_at(uv + vec2<f32>(t.x, 0.0)) + edges_l_at(uv + vec2<f32>(t.x, t.y));
    let gy = -edges_l_at(uv + vec2<f32>(-t.x, -t.y)) - 2.0 * edges_l_at(uv + vec2<f32>(0.0, -t.y)) - edges_l_at(uv + vec2<f32>(t.x, -t.y))
        + edges_l_at(uv + vec2<f32>(-t.x, t.y)) + 2.0 * edges_l_at(uv + vec2<f32>(0.0, t.y)) + edges_l_at(uv + vec2<f32>(t.x, t.y));
    let e = sat(length(vec2<f32>(gx, gy)) * 2.0);
    let c = img(uv);
    let s = c.rgb * keep + glow_color.rgb * e * amount * 1.6;
    return vec4<f32>(sat3(s), c.a);
}

// ---------------------------------------------------------------------------------------------
// emboss. p0 amount, p1 angle

@fragment
fn fs_emboss_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let angle = u.p[1].x;
    let d = vec2<f32>(cos(angle), sin(angle)) * u.texel * 1.5;
    let c = img(v.uv);
    let e = luma(img(v.uv + d).rgb) - luma(img(v.uv - d).rgb);
    let g = sat(0.5 + e * 3.0);
    return vec4<f32>(mix(unpremul(c), vec3<f32>(g, g, g), amount) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// glow: bloom, bright parts glow. p0 amount, p1 threshold, p2 glow_size

@fragment
fn fs_glow_0(v: VOut) -> @location(0) vec4<f32> {
    let threshold = u.p[1].x;
    let glow_size = u.p[2].x;
    let o = vec2<f32>(glow_size * 4.0, 0.0);
    let s = gauss9(v.uv, o);
    let l = luma(s.rgb);
    let k = smoothstep(threshold, min(threshold + 0.25, 1.0), l);
    return vec4<f32>(s.rgb * k, 1.0);
}

@fragment
fn fs_glow_1(v: VOut) -> @location(0) vec4<f32> {
    let glow_size = u.p[2].x;
    return gauss9(v.uv, vec2<f32>(0.0, glow_size * 4.0));
}

@fragment
fn fs_glow_2(v: VOut) -> @location(0) vec4<f32> {
    let glow_size = u.p[2].x;
    return gauss9(v.uv, vec2<f32>(glow_size * 8.0, 0.0));
}

@fragment
fn fs_glow_3(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let glow_size = u.p[2].x;
    let g = gauss9(v.uv, vec2<f32>(0.0, glow_size * 8.0));
    let c = src(v.uv);
    // screen
    let s = 1.0 - (1.0 - c.rgb) * (1.0 - sat3(g.rgb * amount * 1.6) * c.a);
    return vec4<f32>(s, c.a);
}

// ---------------------------------------------------------------------------------------------
// grain: film grain, strongest in the midtones, new every frame. p0 amount, p1 grain_size, p2 colored

@fragment
fn fs_grain_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let grain_size = u.p[1].x;
    let colored = u.p[2].x;
    let c = img(v.uv);
    let s = unpremul(c);
    let cell = floor(v.uv * u.size / max(grain_size, 0.5));
    let seed = floor(time() * 24.0);
    let n = hash12(cell + seed * 17.13) - 0.5;
    let nc = vec3<f32>(n, hash12(cell + seed * 9.71 + 3.1) - 0.5, hash12(cell + seed * 5.37 + 7.7) - 0.5);
    let noise = mix(vec3<f32>(n, n, n), nc, colored);
    let l = luma(s);
    let w = 0.35 + 0.65 * (1.0 - abs(l * 2.0 - 1.0));
    return vec4<f32>(sat3(s + noise * amount * 0.35 * w) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// halftone: print dots. p0 dot_size, p1 angle, p2 colored

fn halftone_dot_at(px: vec2<f32>, a: f32, val: f32, dot_size: f32) -> f32 {
    let cs = cos(a);
    let sn = sin(a);
    let r = vec2<f32>(cs * px.x - sn * px.y, sn * px.x + cs * px.y);
    let cell = fract(r / dot_size) - 0.5;
    let rad = sqrt(sat(val)) * 0.72;
    let d = length(cell);
    let aa = 1.0 / dot_size;
    return 1.0 - smoothstep(rad - aa, rad + aa, d);
}

@fragment
fn fs_halftone_0(v: VOut) -> @location(0) vec4<f32> {
    let dot_size = u.p[0].x;
    let angle = u.p[1].x;
    let colored = u.p[2].x;
    let c = img(v.uv);
    let s = unpremul(c);
    let px = v.uv * u.size;
    let ink = halftone_dot_at(px, angle, 1.0 - luma(s), dot_size);
    let mono = vec3<f32>(1.0 - ink, 1.0 - ink, 1.0 - ink);
    let cmy = vec3<f32>(
        1.0 - halftone_dot_at(px, angle + 0.26, 1.0 - s.r, dot_size),
        1.0 - halftone_dot_at(px, angle + 1.31, 1.0 - s.g, dot_size),
        1.0 - halftone_dot_at(px, angle, 1.0 - s.b, dot_size));
    return vec4<f32>(mix(mono, cmy, colored) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// invert: negative. p0 amount

@fragment
fn fs_invert_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let c = img(v.uv);
    let s = unpremul(c);
    return vec4<f32>(mix(s, 1.0 - s, amount) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// kaleido: kaleidoscope. p0 segments, p1 rotation

@fragment
fn fs_kaleido_0(v: VOut) -> @location(0) vec4<f32> {
    let segments = u.p[0].x;
    let rotation = u.p[1].x;
    let p = to_square(v.uv);
    let r = length(p);
    var a = atan2(p.y, p.x) + rotation;
    let seg = 6.2831853 / max(floor(segments), 2.0);
    a = abs(fx_fmod(a + 62.831853, seg) - seg * 0.5);
    let q = vec2<f32>(cos(a), sin(a)) * r;
    return img(from_square(q));
}

// ---------------------------------------------------------------------------------------------
// mirror: mirror half the picture onto the other. p0 axis (int: 0 left onto right, 1 right onto
// left, 2 top onto bottom, 3 bottom onto top)

@fragment
fn fs_mirror_0(v: VOut) -> @location(0) vec4<f32> {
    let axis = i32(round(u.p[0].x));
    var uv = v.uv;
    if (axis == 0) {
        uv.x = select(uv.x, 1.0 - uv.x, uv.x > 0.5);
    } else if (axis == 1) {
        uv.x = select(uv.x, 1.0 - uv.x, uv.x < 0.5);
    } else if (axis == 2) {
        uv.y = select(uv.y, 1.0 - uv.y, uv.y > 0.5);
    } else {
        uv.y = select(uv.y, 1.0 - uv.y, uv.y < 0.5);
    }
    return img(uv);
}

// ---------------------------------------------------------------------------------------------
// mono: black & white, optionally tinted. p0 amount, p1 tint_color, p2 tint

@fragment
fn fs_mono_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let tint_color = u.p[1];
    let tint = u.p[2].x;
    let c = img(v.uv);
    let s = unpremul(c);
    let l = luma(s);
    let m = mix(vec3<f32>(l, l, l), sat3(l * tint_color.rgb * 1.25), tint);
    return vec4<f32>(mix(s, m, amount) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// pixelate: blocks. p0 block_size

@fragment
fn fs_pixelate_0(v: VOut) -> @location(0) vec4<f32> {
    let block_size = u.p[0].x;
    let b = max(block_size, 1.0) * u.texel;
    let uv = (floor(v.uv / b) + 0.5) * b;
    return img(uv);
}

// ---------------------------------------------------------------------------------------------
// posterize: fewer colour levels. p0 levels

@fragment
fn fs_posterize_0(v: VOut) -> @location(0) vec4<f32> {
    let levels = u.p[0].x;
    let c = img(v.uv);
    let n = max(levels - 1.0, 1.0);
    return vec4<f32>(floor(unpremul(c) * n + 0.5) / n * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// scanlines: CRT, scanlines and a curved screen. p0 amount, p1 line_size, p2 curvature

@fragment
fn fs_scanlines_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let line_size = u.p[1].x;
    let curvature = u.p[2].x;
    var p = v.uv * 2.0 - 1.0;
    p = p * (1.0 + curvature * 0.25 * dot(p.yx, p.yx));
    let uv = p * 0.5 + 0.5;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let c = img(uv);
    let line = 0.5 + 0.5 * sin(uv.y * u.size.y * 3.14159265 / max(line_size, 1.0));
    let edge = smoothstep(0.0, 0.04 * curvature + 0.001, min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y)));
    let s = c.rgb * mix(1.0, 0.55 + 0.6 * line, amount) * edge;
    return vec4<f32>(s, c.a);
}

// ---------------------------------------------------------------------------------------------
// sketch: pencil on paper. p0 amount, p1 weight

fn sketch_l_at(uv: vec2<f32>) -> f32 {
    return luma(img(uv).rgb);
}

@fragment
fn fs_sketch_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let weight = u.p[1].x;
    let t = u.texel * max(weight, 0.5);
    let uv = v.uv;
    let gx = sketch_l_at(uv + vec2<f32>(t.x, 0.0)) - sketch_l_at(uv - vec2<f32>(t.x, 0.0));
    let gy = sketch_l_at(uv + vec2<f32>(0.0, t.y)) - sketch_l_at(uv - vec2<f32>(0.0, t.y));
    let e = length(vec2<f32>(gx, gy));
    let pencil = 1.0 - smoothstep(0.03, 0.3, e);
    let l = sketch_l_at(uv);
    let shade = mix(0.82, 1.0, smoothstep(0.15, 0.6, l));
    let paper = 0.96 + (hash12(floor(uv * u.size * 0.5)) - 0.5) * 0.04;
    let sketch = vec3<f32>(1.0, 1.0, 1.0) * pencil * shade * paper;
    let c = img(uv);
    return vec4<f32>(mix(unpremul(c), sketch, amount) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// swirl: twist around the centre. p0 amount, p1 radius

@fragment
fn fs_swirl_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let radius = u.p[1].x;
    var p = to_square(v.uv);
    let r = length(p) / max(radius, 0.01);
    if (r < 1.0) {
        let a = amount * 4.0 * (1.0 - r) * (1.0 - r);
        let cs = cos(a);
        let sn = sin(a);
        p = vec2<f32>(cs * p.x - sn * p.y, sn * p.x + cs * p.y);
    }
    return img(from_square(p));
}

// ---------------------------------------------------------------------------------------------
// tiltshift: miniature look, sharp band, blurred above and below. p0 amount, p1 focus,
// p2 focus_size

fn tiltshift_spread(y: f32) -> f32 {
    let amount = u.p[0].x;
    let focus = u.p[1].x;
    let focus_size = u.p[2].x;
    let d = abs(y - focus) - focus_size * 0.5;
    return smoothstep(0.0, 0.3, d) * amount * 8.0;
}

@fragment
fn fs_tiltshift_0(v: VOut) -> @location(0) vec4<f32> {
    return gauss9(v.uv, vec2<f32>(tiltshift_spread(v.uv.y), 0.0));
}

@fragment
fn fs_tiltshift_1(v: VOut) -> @location(0) vec4<f32> {
    return gauss9(v.uv, vec2<f32>(0.0, tiltshift_spread(v.uv.y)));
}

@fragment
fn fs_tiltshift_2(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let c = gauss9(v.uv, vec2<f32>(tiltshift_spread(v.uv.y) * 0.6, tiltshift_spread(v.uv.y) * 0.6));
    let s = unpremul(c);
    let l = luma(s);
    // a touch more colour, like a toy
    return vec4<f32>(sat3(mix(vec3<f32>(l, l, l), s, 1.0 + amount * 0.25)) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// vhs: worn videotape, line jitter, a rolling tracking band, colour bleed and noise.
// p0 amount, p1 tracking, p2 bleed

@fragment
fn fs_vhs_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let tracking = u.p[1].x;
    let bleed = u.p[2].x;
    var uv = v.uv;
    let frame = floor(time() * 30.0);
    let row = floor(uv.y * 240.0);
    var jitter = (hash12(vec2<f32>(row, frame)) - 0.5) * 0.004 * amount;
    let band_y = fract(time() * 0.12);
    // smoothstep(0.06, 0.0, x) == 1 - smoothstep(0.0, 0.06, x) (WGSL wants low < high)
    let band = (1.0 - smoothstep(0.0, 0.06, abs(uv.y - band_y))) * tracking;
    jitter = jitter + band * (hash12(vec2<f32>(row, frame + 3.0)) - 0.5) * 0.06;
    uv.x = uv.x + jitter;
    let b = bleed * 6.0 * u.texel.x * (1.0 + amount);
    let c = img(uv);
    let r = unpremul(img(uv + vec2<f32>(b, 0.0))).r;
    let bl = unpremul(img(uv - vec2<f32>(b, 0.0))).b;
    var s = vec3<f32>(r, unpremul(c).g, bl);
    let l = luma(s);
    s = mix(vec3<f32>(l, l, l), s, 1.0 - 0.25 * amount);
    let noise = (hash12(uv * u.size + frame) - 0.5) * 0.18 * amount
        + band * (hash12(uv * u.size * 0.5 + frame) - 0.3) * 0.6;
    let lines = 1.0 - 0.08 * amount * step(0.5, fract(v.uv.y * u.size.y * 0.5));
    return vec4<f32>(sat3(s * lines + noise) * c.a, c.a);
}

// ---------------------------------------------------------------------------------------------
// vignette: darkened edges. p0 amount, p1 vignette_size, p2 softness, p3 roundness

@fragment
fn fs_vignette_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let vignette_size = u.p[1].x;
    let softness = u.p[2].x;
    let roundness = u.p[3].x;
    let c = img(v.uv);
    var p = (v.uv - 0.5) * 2.0;
    p.x = p.x * mix(1.0, u.size.x / u.size.y, roundness);
    let r = length(p) / mix(1.414, 1.0 + u.size.x / u.size.y * 0.414, roundness);
    let vv = smoothstep(vignette_size * 0.9, vignette_size * 0.9 + softness * 0.9 + 0.05, r);
    return vec4<f32>(c.rgb * (1.0 - vv * amount), c.a);
}

// ---------------------------------------------------------------------------------------------
// wave: moving waves. p0 amount, p1 frequency, p2 speed

@fragment
fn fs_wave_0(v: VOut) -> @location(0) vec4<f32> {
    let amount = u.p[0].x;
    let frequency = u.p[1].x;
    let speed = u.p[2].x;
    var uv = v.uv;
    let f = 4.0 + frequency * 40.0;
    let t = time() * (0.5 + speed * 6.0);
    uv.x = uv.x + sin(uv.y * f + t) * amount * 0.02;
    uv.y = uv.y + sin(uv.x * f * 0.7 + t * 0.8) * amount * 0.012;
    return img(uv);
}
