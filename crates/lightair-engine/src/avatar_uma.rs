//! Umamo puppets (`.uma`) drawn with libobs's graphics ([`crate::avatar`]), the way Live2D models
//! are ([`crate::avatar2d`]): each drawable a dynamic vertex buffer, in the puppet's draw order,
//! with its opacity, multiply and screen colours and blend mode (normal, additive, multiply); a
//! clipped drawable's masks drawn first into a texture of their own (the frame's size) that it
//! samples by position. Umamo's textures are premultiplied, so the colour maths is done on
//! premultiplied texels. Output premultiplied, see-through around the character.

use std::ffi::{CStr, CString, c_char, c_void};

use lightair_avatar::Framing;
use lightair_avatar::uma::{Blend, Puppet};

use crate::sys;

/// An Umamo puppet read in the background, with its textures' smaller copies made there too.
pub struct LoadedUma {
    pub puppet: Puppet,
    pub mips: Vec<Vec<Vec<u8>>>,
}

impl LoadedUma {
    pub fn load(path: &std::path::Path) -> Result<LoadedUma, String> {
        let puppet = Puppet::load(path).map_err(|e| e.to_string())?;
        let mips =
            puppet.textures().iter().map(|t| lightair_avatar::vrm::mip_chain(t.width, t.height, &t.rgba)).collect();
        Ok(LoadedUma { puppet, mips })
    }
}

const EFFECT: &str = r#"
uniform float4x4 ViewProj;
uniform texture2d image;
uniform texture2d mask_image;
uniform float4 multiply;
uniform float4 screen;
uniform float4 frame;

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

// t premultiplied: its straight colour tinted, then screened, times its alpha
float4 PSMain(VertOut v) : TARGET
{
    float4 t = image.Sample(tex_sampler, v.uv);
    float k = multiply.a;
    if (screen.a > 0.5) {
        float m = mask_image.Sample(tex_sampler, v.muv).a;
        k = k * (screen.a > 1.5 ? 1.0 - m : m);
    }
    float3 c = t.rgb * multiply.rgb;
    c = c + screen.rgb * t.a - c * screen.rgb;
    return float4(c, t.a) * k;
}

float4 PSMask(VertOut v) : TARGET
{
    float a = image.Sample(tex_sampler, v.uv).a;
    return float4(a, a, a, a);
}

technique Draw
{
    pass
    {
        vertex_shader = VSMain(v);
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

pub struct GpuUma {
    pub path: String,
    meshes: Vec<GpuMesh>,
    textures: Vec<*mut sys::gs_texture_t>,
    effect: *mut sys::gs_effect_t,
    mask: *mut sys::gs_texrender_t,
}

impl GpuUma {
    /// (Graphics context held.)
    pub unsafe fn new(path: &str, l: &LoadedUma) -> GpuUma {
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
        let mut meshes = Vec::new();
        for d in l.puppet.drawables() {
            let n = d.uvs.len().max(1);
            unsafe {
                let vbd = bzalloc(std::mem::size_of::<sys::gs_vb_data>()) as *mut sys::gs_vb_data;
                (*vbd).num = n;
                (*vbd).points = bzalloc(16 * n) as *mut sys::vec3;
                (*vbd).num_tex = 1;
                let tv = bzalloc(std::mem::size_of::<sys::gs_tvertarray>()) as *mut sys::gs_tvertarray;
                (*tv).width = 2;
                let uvs = bzalloc(8 * n) as *mut [f32; 2];
                for (k, uv) in d.uvs.iter().enumerate() {
                    *uvs.add(k) = *uv;
                }
                (*tv).array = uvs as *mut c_void;
                (*vbd).tvarray = tv;
                let vb = sys::gs_vertexbuffer_create(vbd, sys::GS_DYNAMIC);
                let ni = d.indices.len();
                let ib_data = sys::bmalloc(4 * ni.max(1)) as *mut u32;
                std::ptr::copy_nonoverlapping(d.indices.as_ptr(), ib_data, ni);
                let ib = sys::gs_indexbuffer_create(sys::GS_UNSIGNED_LONG, ib_data as *mut c_void, ni, 0);
                meshes.push(GpuMesh { vb, ib, indices: ni as u32, vertices: d.uvs.len() });
            }
        }
        let code = CString::new(EFFECT).unwrap_or_default();
        let mut err: *mut c_char = std::ptr::null_mut();
        let effect = unsafe { sys::gs_effect_create(code.as_ptr(), c"lightair-avatar-uma.effect".as_ptr(), &mut err) };
        if !err.is_null() {
            tracing::warn!(error = %unsafe { CStr::from_ptr(err) }.to_string_lossy(), "Umamo shader");
            unsafe { sys::bfree(err as *mut c_void) };
        }
        let mask = unsafe { sys::gs_texrender_create(sys::GS_RGBA, sys::GS_ZS_NONE) };
        GpuUma { path: path.to_string(), meshes, textures, effect, mask }
    }

    /// The posed drawables' vertices. (Graphics context held.)
    pub unsafe fn upload(&self, p: &Puppet) {
        for (g, d) in self.meshes.iter().zip(p.drawables()) {
            if g.vb.is_null() || !d.posed || d.positions.len() != g.vertices {
                continue;
            }
            unsafe {
                let data = sys::gs_vertexbuffer_get_data(g.vb);
                if data.is_null() {
                    continue;
                }
                let pts = (*data).points as *mut [f32; 4];
                for (k, q) in d.positions.iter().enumerate() {
                    *pts.add(k) = [q[0], q[1], 0.0, 1.0];
                }
                sys::gs_vertexbuffer_flush(g.vb);
            }
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
        if self.effect.is_null() || target.is_null() {
            return;
        }
        let (left, right, top, bottom) = p.frame(framing, w as f32 / h as f32, zoom);
        let e = self.effect;
        let param = |n: &CStr| unsafe { sys::gs_effect_get_param_by_name(e, n.as_ptr()) };
        let drawables = p.drawables();
        unsafe {
            sys::gs_texrender_reset(target);
            if !sys::gs_texrender_begin(target, w, h) {
                return;
            }
            let clear = v4([0.0; 4]);
            sys::gs_clear(sys::GS_CLEAR_COLOR, &clear, 1.0, 0);
            sys::gs_ortho(left, right, top, bottom, -100.0, 100.0);
            let cull = sys::gs_get_cull_mode();
            sys::gs_set_cull_mode(sys::GS_NEITHER);
            sys::gs_blend_state_push();
            sys::gs_enable_blending(true);
            sys::gs_effect_set_vec4(param(c"frame"), &v4([left, top, 1.0 / (right - left), 1.0 / (top - bottom)]));
            let draw_mesh = |i: usize, technique: &CStr| {
                let (Some(g), Some(d)) = (self.meshes.get(i), drawables.get(i)) else { return };
                if g.vb.is_null() || g.ib.is_null() || g.indices == 0 || !d.posed {
                    return;
                }
                let tex = self.textures.get(d.texture).copied().unwrap_or(std::ptr::null_mut());
                if tex.is_null() {
                    return;
                }
                sys::gs_effect_set_texture(param(c"image"), tex);
                sys::gs_load_vertexbuffer(g.vb);
                sys::gs_load_indexbuffer(g.ib);
                while sys::gs_effect_loop(e, technique.as_ptr()) {
                    sys::gs_draw(sys::GS_TRIS, 0, g.indices);
                }
            };
            let mut masked: Option<&[usize]> = None;
            for i in p.draw_order() {
                let Some(d) = drawables.get(i) else { continue };
                if !d.posed || d.opacity <= 0.001 {
                    continue;
                }
                // its clip: the masks drawn into a texture of their own (again only when they change)
                let mode = if d.masks.is_empty() {
                    0.0
                } else {
                    if masked != Some(d.masks.as_slice()) {
                        sys::gs_texrender_reset(self.mask);
                        if sys::gs_texrender_begin(self.mask, w, h) {
                            sys::gs_clear(sys::GS_CLEAR_COLOR, &clear, 1.0, 0);
                            sys::gs_ortho(left, right, top, bottom, -100.0, 100.0);
                            sys::gs_blend_function_separate(
                                sys::GS_BLEND_ONE,
                                sys::GS_BLEND_INVSRCALPHA,
                                sys::GS_BLEND_ONE,
                                sys::GS_BLEND_INVSRCALPHA,
                            );
                            for &m in &d.masks {
                                draw_mesh(m, c"Mask");
                            }
                            sys::gs_texrender_end(self.mask);
                        }
                        masked = Some(d.masks.as_slice());
                    }
                    sys::gs_effect_set_texture(param(c"mask_image"), sys::gs_texrender_get_texture(self.mask));
                    if d.inverted_mask { 2.0 } else { 1.0 }
                };
                let (mul, scr) = (d.multiply, d.screen);
                sys::gs_effect_set_vec4(param(c"multiply"), &v4([mul[0], mul[1], mul[2], d.opacity]));
                sys::gs_effect_set_vec4(param(c"screen"), &v4([scr[0], scr[1], scr[2], mode]));
                match d.blend {
                    Blend::Normal => sys::gs_blend_function_separate(
                        sys::GS_BLEND_ONE,
                        sys::GS_BLEND_INVSRCALPHA,
                        sys::GS_BLEND_ONE,
                        sys::GS_BLEND_INVSRCALPHA,
                    ),
                    Blend::Additive => sys::gs_blend_function_separate(
                        sys::GS_BLEND_ONE,
                        sys::GS_BLEND_ONE,
                        sys::GS_BLEND_ZERO,
                        sys::GS_BLEND_ONE,
                    ),
                    Blend::Multiply => sys::gs_blend_function_separate(
                        sys::GS_BLEND_DSTCOLOR,
                        sys::GS_BLEND_INVSRCALPHA,
                        sys::GS_BLEND_ZERO,
                        sys::GS_BLEND_ONE,
                    ),
                }
                draw_mesh(i, c"Draw");
            }
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
            for g in self.meshes.drain(..) {
                sys::gs_vertexbuffer_destroy(g.vb);
                sys::gs_indexbuffer_destroy(g.ib);
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
            if !self.mask.is_null() {
                sys::gs_texrender_destroy(self.mask);
                self.mask = std::ptr::null_mut();
            }
        }
    }
}
