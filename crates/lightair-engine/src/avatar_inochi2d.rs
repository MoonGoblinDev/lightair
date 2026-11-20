//! Inochi2D puppets drawn with libobs's graphics ([`crate::avatar`]): each part a dynamic vertex
//! buffer, drawn in the order the puppet gives ([`Op`]), with its opacity, tint and screen tint
//! and Inochi2D's blend modes. Masks are drawn into a texture of their own (the frame's size;
//! libobs's shaders can't discard, so a mask's threshold is a step and masks add up by blending);
//! a composite's children into a layer of their own, laid down after. Output premultiplied,
//! see-through around the character, like the other avatars'.

use std::ffi::{CStr, CString, c_char, c_void};

use lightair_avatar::Framing;
use lightair_avatar::inochi2d::{Blend, Look, Op, Puppet, TextureFrame};

use crate::sys;

/// An Inochi2D puppet read in the background, with its textures' smaller copies made there too.
pub struct LoadedInochi {
    pub puppet: Puppet,
    pub mips: Vec<Vec<Vec<u8>>>,
}

impl LoadedInochi {
    pub fn load(path: &std::path::Path) -> Result<LoadedInochi, String> {
        let puppet = Puppet::load(path).map_err(|e| e.to_string())?;
        let mips =
            puppet.textures().iter().map(|t| lightair_avatar::vrm::mip_chain(t.width, t.height, &t.rgba)).collect();
        Ok(LoadedInochi { puppet, mips })
    }
}

const EFFECT: &str = r#"
uniform float4x4 ViewProj;
uniform texture2d image;
uniform texture2d mask_image;
uniform float4 tint;
uniform float4 screen;
uniform float4 frame;
uniform float4 mask_mode;
// a DynamicComposite's texture frame, in its layer's coordinates: origin.xy, x axis.zw; y axis.xy
uniform float4 dyn_a;
uniform float4 dyn_b;

sampler_state tex_sampler {
    Filter = Linear;
    AddressU = Clamp;
    AddressV = Clamp;
};

struct VertIn {
    float4 pos : POSITION;
    float2 uv : TEXCOORD0;
};

struct VertOut {
    float4 pos : POSITION;
    float2 uv : TEXCOORD0;
    float2 muv : TEXCOORD1;
};

VertOut VSMain(VertIn v)
{
    VertOut o;
    o.pos = mul(float4(v.pos.xy, 0.0, 1.0), ViewProj);
    o.uv = v.uv;
    o.muv = float2((v.pos.x - frame.x) * frame.z, (frame.y - v.pos.y) * frame.w);
    return o;
}

// premultiplied colour, screened then tinted, faded; masked where a mask is set
float4 look(float4 t, float2 muv)
{
    float3 screened = 1.0 - (1.0 - t.rgb) * (1.0 - screen.rgb * t.a);
    float4 c = float4(screened * tint.rgb, t.a) * tint.a;
    if (mask_mode.x > 0.5) {
        c = c * mask_image.Sample(tex_sampler, muv).a;
    }
    return c;
}

float4 PSMain(VertOut v) : TARGET
{
    return look(image.Sample(tex_sampler, v.uv), v.muv);
}

// a DynamicComposite's mesh: its UVs say where on the frame (so on its layer) each point shows
VertOut VSDynamic(VertIn v)
{
    VertOut o;
    o.pos = mul(float4(v.pos.xy, 0.0, 1.0), ViewProj);
    o.uv = dyn_a.xy + v.uv.x * dyn_a.zw + v.uv.y * dyn_b.xy;
    o.muv = float2((v.pos.x - frame.x) * frame.z, (frame.y - v.pos.y) * frame.w);
    return o;
}

// a layer the frame's size, sampled where it lies
float4 PSLayer(VertOut v) : TARGET
{
    return look(image.Sample(tex_sampler, v.muv), v.muv);
}

// a mask: set (or, dodging, cleared) where the texture's alpha is over the threshold
float4 PSMask(VertOut v) : TARGET
{
    float a = image.Sample(tex_sampler, v.uv).a;
    float m = a > mask_mode.y ? 1.0 : 0.0;
    return mask_mode.z > 0.5 ? float4(0.0, 0.0, 0.0, m) : float4(m, m, m, m);
}

technique Draw
{
    pass
    {
        vertex_shader = VSMain(v);
        pixel_shader = PSMain(v);
    }
}

technique Layer
{
    pass
    {
        vertex_shader = VSMain(v);
        pixel_shader = PSLayer(v);
    }
}

technique Dynamic
{
    pass
    {
        vertex_shader = VSDynamic(v);
        pixel_shader = PSMain(v);
    }
}

technique Mask
{
    pass
    {
        vertex_shader = VSMain(v);
        pixel_shader = PSMask(v);
    }
}
"#;

fn v4(x: [f32; 4]) -> sys::vec4 {
    sys::vec4 { __bindgen_anon_1: sys::vec4__bindgen_ty_1 { ptr: x } }
}

/// Zeroed memory libobs frees (bzalloc is a header inline).
unsafe fn bzalloc(size: usize) -> *mut c_void {
    let p = unsafe { sys::bmalloc(size) };
    if !p.is_null() {
        unsafe { std::ptr::write_bytes(p as *mut u8, 0, size) };
    }
    p
}

struct GpuMesh {
    vb: *mut sys::gs_vertbuffer_t,
    ib: *mut sys::gs_indexbuffer_t,
    indices: u32,
    vertices: usize,
}

/// A dynamic vertex buffer with these texture coordinates and triangles. (Graphics context held.)
unsafe fn mesh(uvs: &[[f32; 2]], indices: &[u32]) -> GpuMesh {
    let n = uvs.len().max(1);
    unsafe {
        let vbd = bzalloc(std::mem::size_of::<sys::gs_vb_data>()) as *mut sys::gs_vb_data;
        (*vbd).num = n;
        (*vbd).points = bzalloc(16 * n) as *mut sys::vec3;
        (*vbd).num_tex = 1;
        let tv = bzalloc(std::mem::size_of::<sys::gs_tvertarray>()) as *mut sys::gs_tvertarray;
        (*tv).width = 2;
        let out = bzalloc(8 * n) as *mut [f32; 2];
        for (k, uv) in uvs.iter().enumerate() {
            *out.add(k) = *uv;
        }
        (*tv).array = out as *mut c_void;
        (*vbd).tvarray = tv;
        let vb = sys::gs_vertexbuffer_create(vbd, sys::GS_DYNAMIC);
        let ni = indices.len();
        let ib_data = sys::bmalloc(4 * ni.max(1)) as *mut u32;
        std::ptr::copy_nonoverlapping(indices.as_ptr(), ib_data, ni);
        let ib = sys::gs_indexbuffer_create(sys::GS_UNSIGNED_LONG, ib_data as *mut c_void, ni, 0);
        GpuMesh { vb, ib, indices: ni as u32, vertices: uvs.len() }
    }
}

/// Writes a mesh's vertices. (Graphics context held.)
unsafe fn upload(g: &GpuMesh, positions: &[[f32; 2]]) {
    if g.vb.is_null() || positions.len() != g.vertices {
        return;
    }
    unsafe {
        let d = sys::gs_vertexbuffer_get_data(g.vb);
        if d.is_null() {
            return;
        }
        let pts = (*d).points as *mut [f32; 4];
        for (k, q) in positions.iter().enumerate() {
            *pts.add(k) = [q[0], q[1], 0.0, 1.0];
        }
        sys::gs_vertexbuffer_flush(g.vb);
    }
}

/// libobs's blend for an Inochi2D blend mode (premultiplied, as Inochi2D's own renderer).
unsafe fn set_blend(b: Blend) {
    use sys::*;
    unsafe {
        gs_blend_op(GS_BLEND_OP_ADD);
        match b {
            Blend::Normal => gs_blend_function(GS_BLEND_ONE, GS_BLEND_INVSRCALPHA),
            Blend::Multiply => gs_blend_function(GS_BLEND_DSTCOLOR, GS_BLEND_INVSRCALPHA),
            Blend::ColorDodge => gs_blend_function(GS_BLEND_DSTCOLOR, GS_BLEND_ONE),
            Blend::LinearDodge => gs_blend_function(GS_BLEND_ONE, GS_BLEND_ONE),
            Blend::Screen => gs_blend_function(GS_BLEND_ONE, GS_BLEND_INVSRCCOLOR),
            Blend::ClipToLower => gs_blend_function(GS_BLEND_DSTALPHA, GS_BLEND_INVSRCALPHA),
            Blend::SliceFromLower => {
                gs_blend_op(GS_BLEND_OP_SUBTRACT);
                gs_blend_function(GS_BLEND_INVDSTALPHA, GS_BLEND_INVSRCALPHA);
            }
        }
    }
}

pub struct GpuInochi {
    pub path: String,
    parts: Vec<GpuMesh>,
    /// A quad over the frame, for laying down a composite's layer.
    quad: GpuMesh,
    textures: Vec<*mut sys::gs_texture_t>,
    effect: *mut sys::gs_effect_t,
    mask: *mut sys::gs_texrender_t,
    layer: *mut sys::gs_texrender_t,
    /// DynamicComposites' layers, one for each level they nest to.
    dynamic: Vec<*mut sys::gs_texrender_t>,
}

/// How deep DynamicComposites nest that get a layer (deeper ones draw nothing).
const DYNAMIC_LEVELS: usize = 4;

impl GpuInochi {
    /// (Graphics context held.)
    pub unsafe fn new(path: &str, l: &LoadedInochi) -> GpuInochi {
        let mut textures = Vec::new();
        for (t, mips) in l.puppet.textures().iter().zip(&l.mips) {
            // every level given (libobs reads one pointer a level)
            let data: Vec<*const u8> =
                std::iter::once(t.rgba.as_ptr()).chain(mips.iter().map(|m| m.as_ptr())).collect();
            textures.push(unsafe {
                sys::gs_texture_create(
                    t.width,
                    t.height,
                    sys::GS_RGBA,
                    data.len() as u32,
                    data.as_ptr() as *mut *const u8,
                    0,
                )
            });
        }
        let parts = l.puppet.parts().iter().map(|p| unsafe { mesh(&p.uvs, &p.indices) }).collect();
        let quad = unsafe { mesh(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]], &[0, 1, 2, 2, 1, 3]) };
        let code = CString::new(EFFECT).unwrap_or_default();
        let mut err: *mut c_char = std::ptr::null_mut();
        let effect =
            unsafe { sys::gs_effect_create(code.as_ptr(), c"lightair-avatar-inochi2d.effect".as_ptr(), &mut err) };
        if !err.is_null() {
            tracing::warn!(error = %unsafe { CStr::from_ptr(err) }.to_string_lossy(), "Inochi2D shader");
            unsafe { sys::bfree(err as *mut c_void) };
        }
        let mask = unsafe { sys::gs_texrender_create(sys::GS_RGBA, sys::GS_ZS_NONE) };
        let layer = unsafe { sys::gs_texrender_create(sys::GS_RGBA, sys::GS_ZS_NONE) };
        let dynamic =
            (0..DYNAMIC_LEVELS).map(|_| unsafe { sys::gs_texrender_create(sys::GS_RGBA, sys::GS_ZS_NONE) }).collect();
        GpuInochi { path: path.to_string(), parts, quad, textures, effect, mask, layer, dynamic }
    }

    /// The posed parts' vertices. (Graphics context held.)
    pub unsafe fn upload(&self, p: &Puppet) {
        for (g, pos) in self.parts.iter().zip(p.positions()) {
            unsafe { upload(g, pos) };
        }
    }

    /// Draws the puppet into `target` (`w`×`h`, cleared first). (Graphics context held.)
    pub unsafe fn draw(
        &self,
        p: &Puppet,
        target: *mut sys::gs_texrender_t,
        w: u32,
        h: u32,
        framing: Framing,
        zoom: f32,
    ) {
        if self.effect.is_null() || target.is_null() || self.mask.is_null() || self.layer.is_null() {
            return;
        }
        let (left, right, top, bottom) = p.frame(framing, w as f32 / h as f32, zoom);
        let e = self.effect;
        let param = |n: &CStr| unsafe { sys::gs_effect_get_param_by_name(e, n.as_ptr()) };
        let clear = |v: f32| v4([v; 4]);
        unsafe {
            upload(&self.quad, &[[left, top], [right, top], [left, bottom], [right, bottom]]);
            sys::gs_texrender_reset(target);
            if !sys::gs_texrender_begin(target, w, h) {
                return;
            }
            sys::gs_clear(sys::GS_CLEAR_COLOR, &clear(0.0), 1.0, 0);
            sys::gs_ortho(left, right, top, bottom, -100.0, 100.0);
            let cull = sys::gs_get_cull_mode();
            sys::gs_set_cull_mode(sys::GS_NEITHER);
            sys::gs_blend_state_push();
            sys::gs_enable_blending(true);
            // (set before every draw: libobs clears an effect's values when a technique ends)
            let frame = v4([left, top, 1.0 / (right - left), 1.0 / (top - bottom)]);
            let draw = |g: &GpuMesh, tex: *mut sys::gs_texture_t, technique: &CStr| {
                if g.vb.is_null() || g.ib.is_null() || g.indices == 0 || tex.is_null() {
                    return;
                }
                sys::gs_effect_set_texture(param(c"image"), tex);
                sys::gs_effect_set_vec4(param(c"frame"), &frame);
                sys::gs_load_vertexbuffer(g.vb);
                sys::gs_load_indexbuffer(g.ib);
                while sys::gs_effect_loop(e, technique.as_ptr()) {
                    sys::gs_draw(sys::GS_TRIS, 0, g.indices);
                }
            };
            let set_look = |look: &Look, masked: bool, threshold: f32| {
                sys::gs_effect_set_vec4(param(c"tint"), &v4([look.tint[0], look.tint[1], look.tint[2], look.opacity]));
                sys::gs_effect_set_vec4(param(c"screen"), &v4([look.screen[0], look.screen[1], look.screen[2], 0.0]));
                sys::gs_effect_set_vec4(
                    param(c"mask_mode"),
                    &v4([if masked { 1.0 } else { 0.0 }, threshold, 0.0, 0.0]),
                );
                if masked {
                    sys::gs_effect_set_texture(param(c"mask_image"), sys::gs_texrender_get_texture(self.mask));
                }
            };
            // drawing into the mask (between BeginMasks and MaskedContent), drawing with it,
            // drawing into a composite's layer
            let (mut masking, mut masked, mut dodge, mut threshold, mut composite) =
                (false, false, false, 0.0f32, false);
            // DynamicComposites open: each with whether it got a layer
            let mut dynamics: Vec<bool> = Vec::new();
            for op in p.ops() {
                match op {
                    Op::BeginMasks { threshold: th, clear: c } => {
                        threshold = *th;
                        masked = false;
                        sys::gs_texrender_reset(self.mask);
                        masking = sys::gs_texrender_begin(self.mask, w, h);
                        if masking {
                            sys::gs_clear(sys::GS_CLEAR_COLOR, &clear(*c), 1.0, 0);
                            sys::gs_ortho(left, right, top, bottom, -100.0, 100.0);
                        }
                    }
                    Op::Mask { dodge: d } => dodge = *d,
                    Op::MaskedContent => {
                        if masking {
                            sys::gs_texrender_end(self.mask);
                            masking = false;
                            masked = true;
                        }
                    }
                    Op::EndMasks => {
                        if masking {
                            sys::gs_texrender_end(self.mask);
                            masking = false;
                        }
                        masked = false;
                    }
                    Op::BeginComposite { as_mask: false } => {
                        sys::gs_texrender_reset(self.layer);
                        composite = sys::gs_texrender_begin(self.layer, w, h);
                        if composite {
                            sys::gs_clear(sys::GS_CLEAR_COLOR, &clear(0.0), 1.0, 0);
                            sys::gs_ortho(left, right, top, bottom, -100.0, 100.0);
                        }
                    }
                    Op::EndComposite { as_mask: false, look } => {
                        if composite {
                            sys::gs_texrender_end(self.layer);
                            composite = false;
                            set_look(look, masked, threshold);
                            set_blend(look.blend);
                            draw(&self.quad, sys::gs_texrender_get_texture(self.layer), c"Layer");
                        }
                    }
                    // a composite as a mask: its children go straight into the mask
                    Op::BeginComposite { as_mask: true } | Op::EndComposite { as_mask: true, .. } => {}
                    Op::BeginDynamic { as_mask: false } => {
                        let layer = self.dynamic.get(dynamics.len()).copied().unwrap_or(std::ptr::null_mut());
                        let begun = !layer.is_null() && {
                            sys::gs_texrender_reset(layer);
                            sys::gs_texrender_begin(layer, w, h)
                        };
                        if begun {
                            sys::gs_clear(sys::GS_CLEAR_COLOR, &clear(0.0), 1.0, 0);
                            sys::gs_ortho(left, right, top, bottom, -100.0, 100.0);
                        }
                        dynamics.push(begun);
                    }
                    Op::EndDynamic { as_mask: false, part, look, texture } => {
                        let level = dynamics.len().saturating_sub(1);
                        if dynamics.pop() == Some(true) {
                            let layer = self.dynamic[level];
                            sys::gs_texrender_end(layer);
                            // the texture frame on the layer (u right, v down, 0..1 across the frame)
                            let TextureFrame { origin, x_axis, y_axis } = *texture;
                            let (sx, sy) = (1.0 / (right - left), 1.0 / (top - bottom));
                            let o = [(origin[0] - left) * sx, (top - origin[1]) * sy];
                            let (xa, ya) = ([x_axis[0] * sx, -x_axis[1] * sy], [y_axis[0] * sx, -y_axis[1] * sy]);
                            sys::gs_effect_set_vec4(param(c"dyn_a"), &v4([o[0], o[1], xa[0], xa[1]]));
                            sys::gs_effect_set_vec4(param(c"dyn_b"), &v4([ya[0], ya[1], 0.0, 0.0]));
                            if let Some(g) = self.parts.get(*part) {
                                set_look(look, masked && !composite, threshold);
                                set_blend(look.blend);
                                draw(g, sys::gs_texrender_get_texture(layer), c"Dynamic");
                            }
                        }
                    }
                    Op::BeginDynamic { as_mask: true } | Op::EndDynamic { as_mask: true, .. } => {}
                    Op::Part { part, as_mask, look } => {
                        let Some(g) = self.parts.get(*part) else { continue };
                        let tex = p
                            .parts()
                            .get(*part)
                            .and_then(|pt| self.textures.get(pt.texture))
                            .copied()
                            .unwrap_or(std::ptr::null_mut());
                        if *as_mask || masking {
                            if !masking {
                                continue;
                            }
                            sys::gs_effect_set_vec4(
                                param(c"mask_mode"),
                                &v4([0.0, threshold, if dodge { 1.0 } else { 0.0 }, 0.0]),
                            );
                            sys::gs_blend_op(sys::GS_BLEND_OP_ADD);
                            if dodge {
                                sys::gs_blend_function(sys::GS_BLEND_ZERO, sys::GS_BLEND_INVSRCALPHA);
                            } else {
                                sys::gs_blend_function(sys::GS_BLEND_ONE, sys::GS_BLEND_INVSRCALPHA);
                            }
                            draw(g, tex, c"Mask");
                        } else {
                            // (a composite's children aren't masked; the layer is)
                            set_look(look, masked && !composite, threshold);
                            set_blend(look.blend);
                            draw(g, tex, c"Draw");
                        }
                    }
                }
            }
            // (left open by a puppet that ends mid-way)
            if masking {
                sys::gs_texrender_end(self.mask);
            }
            if composite {
                sys::gs_texrender_end(self.layer);
            }
            for (level, begun) in dynamics.into_iter().enumerate().rev() {
                if begun {
                    sys::gs_texrender_end(self.dynamic[level]);
                }
            }
            sys::gs_blend_op(sys::GS_BLEND_OP_ADD);
            sys::gs_load_indexbuffer(std::ptr::null_mut());
            sys::gs_load_vertexbuffer(std::ptr::null_mut());
            sys::gs_set_cull_mode(cull);
            sys::gs_blend_state_pop();
            sys::gs_texrender_end(target);
        }
    }

    /// (Graphics context held.)
    pub unsafe fn destroy(&mut self) {
        unsafe {
            for g in self.parts.drain(..).chain(std::iter::once(std::mem::replace(
                &mut self.quad,
                GpuMesh { vb: std::ptr::null_mut(), ib: std::ptr::null_mut(), indices: 0, vertices: 0 },
            ))) {
                if !g.vb.is_null() {
                    sys::gs_vertexbuffer_destroy(g.vb);
                }
                if !g.ib.is_null() {
                    sys::gs_indexbuffer_destroy(g.ib);
                }
            }
            for t in self.textures.drain(..) {
                if !t.is_null() {
                    sys::gs_texture_destroy(t);
                }
            }
            if !self.effect.is_null() {
                sys::gs_effect_destroy(self.effect);
                self.effect = std::ptr::null_mut();
            }
            for r in [&mut self.mask, &mut self.layer].into_iter().chain(self.dynamic.iter_mut()) {
                if !r.is_null() {
                    sys::gs_texrender_destroy(*r);
                    *r = std::ptr::null_mut();
                }
            }
        }
    }
}
