//! Avatars as sources (`lightair_avatar`, docs/plans/avatars.md): a character model that moves
//! with the face a camera sees. The camera is the avatar's own (private, never shown): each
//! frame it's drawn small off screen, read back and given to the Lightair Effect Stack's face
//! tracker (its `lightair_face_tracker_*` procs); the landmarks that come back pose the model
//! ([`lightair_avatar`]), which is skinned on the CPU and drawn with libobs's graphics: a toon
//! shader close to VRM's MToon, with outlines, into a texture with depth, transparent around
//! the character.
//!
//! Flat (2D) avatars, Live2D models (`.model3.json`) and Inochi2D puppets (`.inp`, `.inx`), are
//! drawn by [`crate::avatar2d`] and [`crate::avatar_inochi2d`], posed by the same face.
//!
//! Settings: `file` (a .vrm, a Live2D .model3.json or an Inochi2D .inp/.inx), `device` (the camera's unique id; none: no tracking), `mirror`
//! (bool, on by default), `framing` ("head", "bust", "half", "full"), `zoom`, `width`,
//! `height`, `calibration` (JSON from the `calibrate` proc), `expression` (an expression's
//! name, e.g. "happy") and `expression_weight`; `body` ("off", "upper", "full"): how much of the
//! body follows (the Effect Stack's body tracker, macOS).

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Quat, Vec3};
use lightair_avatar::body::{BodyFrame, BodyMode, BodyState, HAND_POINTS, JOINTS};
use lightair_avatar::{Avatar, Calibration, Controls, Face, Framing, Landmarks, Measures, Model, Smoother};
use parking_lot::Mutex;
use serde_json::json;

use crate::obs::{Data, ObsSource};
use crate::sys;

pub const KIND: &str = "lightair_avatar";

/// How long the camera keeps running once nothing shows the avatar.
const LINGER: Duration = Duration::from_secs(2);
/// The tracker's picture: at most this wide.
const TRACK_WIDTH: u32 = 640;

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    file: String,
    device: String,
    mirror: bool,
    framing: Framing,
    zoom: f32,
    width: u32,
    height: u32,
    calibration: Calibration,
    expression: String,
    expression_weight: f32,
    body: BodyMode,
    /// The camera's picture in a corner, with what the trackers see marked on it.
    show_tracking: bool,
}

fn read(settings: *mut sys::obs_data_t) -> Settings {
    let st = |k: &CStr| {
        let p = unsafe { sys::obs_data_get_string(settings, k.as_ptr()) };
        if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    };
    let int = |k: &CStr| unsafe { sys::obs_data_get_int(settings, k.as_ptr()) };
    let float = |k: &CStr| unsafe { sys::obs_data_get_double(settings, k.as_ptr()) } as f32;
    Settings {
        file: st(c"file"),
        device: st(c"device"),
        mirror: unsafe { sys::obs_data_get_bool(settings, c"mirror".as_ptr()) },
        framing: Framing::from_name(&st(c"framing")),
        zoom: float(c"zoom").clamp(0.25, 4.0),
        width: int(c"width").clamp(16, 4096) as u32,
        height: int(c"height").clamp(16, 4096) as u32,
        calibration: calibration_from_json(&st(c"calibration")).unwrap_or_default(),
        expression: st(c"expression"),
        expression_weight: float(c"expression_weight"),
        body: BodyMode::from_name(&st(c"body")),
        show_tracking: unsafe { sys::obs_data_get_bool(settings, c"show_tracking".as_ptr()) },
    }
}

fn calibration_from_json(text: &str) -> Option<Calibration> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let f = |v: &serde_json::Value| v.as_f64().map(|x| x as f32);
    let h = v["head"].as_array()?;
    Some(Calibration {
        head: Quat::from_xyzw(f(&h[0])?, f(&h[1])?, f(&h[2])?, f(&h[3])?).normalize(),
        eye: [f(&v["eye"][0])?, f(&v["eye"][1])?],
        brow: [f(&v["brow"][0])?, f(&v["brow"][1])?],
        mouth_width: f(&v["mouth_width"])?,
    })
}

fn calibration_json(c: &Calibration) -> String {
    json!({
        "head": [c.head.x, c.head.y, c.head.z, c.head.w],
        "eye": c.eye,
        "brow": c.brow,
        "mouth_width": c.mouth_width,
    })
    .to_string()
}

/// The face tracker's frame (face-tracker.h's `ft_face`).
#[repr(C)]
struct FtFace {
    valid: bool,
    present: bool,
    frame_id: u64,
    timestamp_ns: u64,
    region_start: [i32; 13],
    region_count: [i32; 13],
    num_points: i32,
    pts: [[f32; 2]; 720],
    skin_valid: bool,
    skin_cbcr: [f32; 2],
}

/// The body tracker's frame (body-tracker.h's `bt_body`).
#[repr(C)]
struct BtBody {
    present: bool,
    has3d: bool,
    frame_id: u64,
    j3: [[f32; 4]; JOINTS],
    j2: [[f32; 3]; JOINTS],
    hand_present: [bool; 2],
    hand: [[[f32; 3]; HAND_POINTS]; 2],
    hand3d: [[[f32; 3]; HAND_POINTS]; 2],
}

const FT_LEFT_PUPIL: usize = 10;
const FT_RIGHT_PUPIL: usize = 11;
const FT_MESH: usize = 12;

/// Calls one of the face tracker's global procs.
fn tracker_call(name: &CStr, fill: impl FnOnce(*mut sys::calldata_t)) -> Option<sys::calldata_t> {
    unsafe {
        let ph = sys::obs_get_proc_handler();
        if ph.is_null() {
            return None;
        }
        let mut cd = sys::calldata_t::default();
        fill(&mut cd);
        if sys::proc_handler_call(ph, name.as_ptr(), &mut cd) {
            Some(cd)
        } else {
            free_cd(&mut cd);
            None
        }
    }
}

/// (calldata_free is a header inline.)
fn free_cd(cd: &mut sys::calldata_t) {
    if !cd.fixed && !cd.stack.is_null() {
        unsafe { sys::bfree(cd.stack as *mut c_void) };
        cd.stack = std::ptr::null_mut();
    }
}

fn set_bool(cd: *mut sys::calldata_t, name: &CStr, v: bool) {
    unsafe { sys::calldata_set_data(cd, name.as_ptr(), &v as *const _ as *const c_void, std::mem::size_of::<bool>()) };
}

fn set_string(cd: *mut sys::calldata_t, name: &CStr, v: &CStr) {
    let b = v.to_bytes_with_nul();
    unsafe { sys::calldata_set_data(cd, name.as_ptr(), b.as_ptr() as *const c_void, b.len()) };
}

fn set_ptr(cd: *mut sys::calldata_t, name: &CStr, p: *mut c_void) {
    unsafe {
        sys::calldata_set_data(cd, name.as_ptr(), &p as *const _ as *const c_void, std::mem::size_of::<*mut c_void>())
    };
}

fn set_int(cd: *mut sys::calldata_t, name: &CStr, v: i64) {
    unsafe { sys::calldata_set_data(cd, name.as_ptr(), &v as *const _ as *const c_void, std::mem::size_of::<i64>()) };
}

fn get_ptr(cd: &mut sys::calldata_t, name: &CStr) -> *mut c_void {
    let mut p: *mut c_void = std::ptr::null_mut();
    unsafe {
        sys::calldata_get_data(cd, name.as_ptr(), &mut p as *mut _ as *mut c_void, std::mem::size_of::<*mut c_void>())
    };
    p
}

fn get_bool(cd: &mut sys::calldata_t, name: &CStr) -> bool {
    let mut b = false;
    unsafe { sys::calldata_get_data(cd, name.as_ptr(), &mut b as *mut _ as *mut c_void, std::mem::size_of::<bool>()) };
    b
}

/// A model being read, or read.
enum Load {
    None,
    Loading(String),
    Ready(String, Loaded),
    Failed(String, String),
}

/// A model read: VRM (shared), or a flat one, Live2D or Inochi2D (moved to the graphics side
/// once, when first drawn), with its expressions.
#[derive(Clone)]
enum Loaded {
    Vrm(Arc<Model>),
    Flat(Arc<Mutex<Option<Box<crate::avatar2d::Flat>>>>, Vec<String>),
}

use crate::avatar2d::is_flat;

/// libobs's 4×4 matrix (rows: x, y, z axes and translation).
#[repr(C, align(16))]
struct M4([f32; 16]);

impl M4 {
    fn from(m: Mat4) -> M4 {
        M4(m.to_cols_array())
    }
    fn ptr(&self) -> *const sys::matrix4 {
        self as *const M4 as *const sys::matrix4
    }
}

/// Zeroed memory libobs frees (bzalloc is a header inline).
unsafe fn bzalloc(size: usize) -> *mut c_void {
    let p = unsafe { sys::bmalloc(size) };
    if !p.is_null() {
        unsafe { std::ptr::write_bytes(p as *mut u8, 0, size) };
    }
    p
}

fn v4(x: [f32; 4]) -> sys::vec4 {
    sys::vec4 { __bindgen_anon_1: sys::vec4__bindgen_ty_1 { ptr: x } }
}

struct GpuPrim {
    vb: *mut sys::gs_vertbuffer_t,
    ib: *mut sys::gs_indexbuffer_t,
    indices: u32,
}

/// The model on the GPU: a dynamic vertex buffer a primitive, its textures.
struct Gpu {
    path: String,
    prims: Vec<GpuPrim>,
    textures: Vec<*mut sys::gs_texture_t>,
}

impl Gpu {
    /// (Graphics context held.)
    unsafe fn new(path: &str, model: &Model) -> Gpu {
        let mut textures = Vec::new();
        for img in &model.images {
            // every level given (libobs reads one pointer a level)
            let data: Vec<*const u8> =
                std::iter::once(img.rgba.as_ptr()).chain(img.mips.iter().map(|m| m.as_ptr())).collect();
            let t = unsafe {
                sys::gs_texture_create(
                    img.width,
                    img.height,
                    sys::GS_RGBA,
                    data.len() as u32,
                    data.as_ptr() as *mut *const u8,
                    0,
                )
            };
            textures.push(t);
        }
        let mut prims = Vec::new();
        for p in &model.primitives {
            let n = p.positions.len();
            unsafe {
                let vbd = bzalloc(std::mem::size_of::<sys::gs_vb_data>()) as *mut sys::gs_vb_data;
                (*vbd).num = n;
                (*vbd).points = bzalloc(16 * n) as *mut sys::vec3;
                (*vbd).normals = bzalloc(16 * n) as *mut sys::vec3;
                (*vbd).num_tex = 1;
                let tv = bzalloc(std::mem::size_of::<sys::gs_tvertarray>()) as *mut sys::gs_tvertarray;
                (*tv).width = 2;
                let uvs = sys::bmalloc(8 * n) as *mut [f32; 2];
                std::ptr::copy_nonoverlapping(p.uvs.as_ptr(), uvs, n);
                (*tv).array = uvs as *mut c_void;
                (*vbd).tvarray = tv;
                let vb = sys::gs_vertexbuffer_create(vbd, sys::GS_DYNAMIC);
                let ni = p.indices.len();
                let ib_data = sys::bmalloc(4 * ni.max(1)) as *mut u32;
                std::ptr::copy_nonoverlapping(p.indices.as_ptr(), ib_data, ni);
                let ib = sys::gs_indexbuffer_create(sys::GS_UNSIGNED_LONG, ib_data as *mut c_void, ni, 0);
                prims.push(GpuPrim { vb, ib, indices: ni as u32 });
            }
        }
        Gpu { path: path.to_string(), prims, textures }
    }

    /// (Graphics context held.)
    unsafe fn upload(&self, a: &Avatar) {
        for (i, g) in self.prims.iter().enumerate() {
            if g.vb.is_null() {
                continue;
            }
            unsafe {
                let d = sys::gs_vertexbuffer_get_data(g.vb);
                if d.is_null() {
                    continue;
                }
                let pts = (*d).points as *mut [f32; 4];
                let nrm = (*d).normals as *mut [f32; 4];
                for (k, (p, n)) in a.positions[i].iter().zip(&a.normals[i]).enumerate() {
                    *pts.add(k) = [p.x, p.y, p.z, 1.0];
                    *nrm.add(k) = [n.x, n.y, n.z, 0.0];
                }
                sys::gs_vertexbuffer_flush(g.vb);
            }
        }
    }

    /// (Graphics context held.)
    unsafe fn destroy(&mut self) {
        unsafe {
            for g in self.prims.drain(..) {
                sys::gs_vertexbuffer_destroy(g.vb);
                sys::gs_indexbuffer_destroy(g.ib);
            }
            for t in self.textures.drain(..) {
                sys::gs_texture_destroy(t);
            }
        }
    }
}

/// The camera's picture on its way to the tracker.
struct Grab {
    texrender: *mut sys::gs_texrender_t,
    stage: [*mut sys::gs_stagesurf_t; 2],
    size: (u32, u32),
    /// Which stage the next copy goes to; whether the other has a copy waiting.
    next: usize,
    waiting: bool,
    frame_id: u64,
}

/// What only the graphics thread touches.
struct Render {
    avatar: Option<(String, Avatar)>,
    /// A flat model instead (Live2D, Inochi2D), and its GPU side.
    puppet: Option<(String, Box<crate::avatar2d::Flat>)>,
    gpu2d: Option<crate::avatar2d::FlatGpu>,
    gpu: Option<Gpu>,
    effect: *mut sys::gs_effect_t,
    /// The shader without cutouts (renderers without `discard`).
    effect_plain: bool,
    effect_failed: bool,
    white: *mut sys::gs_texture_t,
    target: *mut sys::gs_texrender_t,
    grab: Grab,
    smoother: Smoother,
    last_tick: Option<Instant>,
    /// A new pose waits to be uploaded and drawn; a frame of the camera to be grabbed.
    dirty: bool,
    grab_due: bool,
    measures: Option<Measures>,
    body: BodyState,
    /// The tracking view: the last picture given to the trackers (BGRA), what they found in
    /// it, and its texture.
    view_pixels: Vec<u8>,
    view_face: Vec<[f32; 2]>,
    view_body: Option<BodyFrame>,
    view_tex: *mut sys::gs_texture_t,
    view_size: (u32, u32),
}

struct Shared {
    source: usize,
    settings: Mutex<Settings>,
    load: Mutex<Load>,
    camera: Mutex<Option<(String, ObsSource)>>,
    hidden_since: Mutex<Option<Instant>>,
    tracker: Mutex<usize>,
    body_tracker: Mutex<usize>,
    render: Mutex<Render>,
}

// SAFETY: libobs objects are reference counted and thread-safe; the graphics objects in
// `Render` are only touched with the graphics context held.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

fn shared<'a>(data: *mut c_void) -> &'a Arc<Shared> {
    unsafe { &*(data as *const Arc<Shared>) }
}

impl Shared {
    /// Reads the model in the background when the file changed.
    fn load_if_changed(self: &Arc<Self>) {
        let file = self.settings.lock().file.clone();
        let mut load = self.load.lock();
        let current = match &*load {
            Load::None => None,
            Load::Loading(p) | Load::Ready(p, _) | Load::Failed(p, _) => Some(p.clone()),
        };
        if current.as_deref() == Some(file.as_str()) || file.is_empty() && current.is_none() {
            return;
        }
        if file.is_empty() {
            *load = Load::None;
            return;
        }
        *load = Load::Loading(file.clone());
        drop(load);
        let me = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("lightair-avatar-load".into())
            .spawn(move || {
                let result = if is_flat(&file) {
                    crate::avatar2d::Flat::load(std::path::Path::new(&file)).map(|l| {
                        let names = l.expressions();
                        Loaded::Flat(Arc::new(Mutex::new(Some(Box::new(l)))), names)
                    })
                } else {
                    std::fs::read(&file)
                        .map_err(|e| e.to_string())
                        .and_then(|b| Model::load(&b).map(|m| Loaded::Vrm(Arc::new(m))).map_err(|e| e.to_string()))
                };
                let Some(me) = me.upgrade() else { return };
                let mut load = me.load.lock();
                // (a newer file may have been chosen meanwhile)
                if matches!(&*load, Load::Loading(p) if *p == file) {
                    *load = match result {
                        Ok(m) => {
                            tracing::info!(file, flat = matches!(m, Loaded::Flat(..)), "avatar loaded");
                            Load::Ready(file, m)
                        }
                        Err(e) => {
                            tracing::warn!(file, error = %e, "avatar didn't load");
                            Load::Failed(file, e)
                        }
                    };
                }
            })
            .ok();
    }

    /// The camera, made when something shows the avatar and let go a while after nothing does.
    fn keep_camera(&self, shown: bool) {
        let device = self.settings.lock().device.clone();
        let mut cam = self.camera.lock();
        if cam.as_ref().is_some_and(|(d, _)| *d != device)
            && let Some((_, c)) = cam.take()
        {
            unsafe { sys::obs_source_dec_showing(c.as_ptr()) };
        }
        if shown {
            *self.hidden_since.lock() = None;
            if cam.is_none() && !device.is_empty() {
                let Some((kind, settings)) =
                    crate::kinds::resolve(lightair_model::kinds::CAMERA, &json!({"device": device, "height": 720}))
                else {
                    return;
                };
                if let Some(media) = crate::access::needed_by(kind)
                    && crate::access::hold_back(media)
                {
                    return;
                }
                if let Some(c) = ObsSource::create_private(kind, "Avatar camera", Some(&Data::from_json(&settings))) {
                    unsafe { sys::obs_source_inc_showing(c.as_ptr()) };
                    *cam = Some((device, c));
                }
            }
        } else if cam.is_some() {
            let mut since = self.hidden_since.lock();
            if since.get_or_insert_with(Instant::now).elapsed() >= LINGER {
                *since = None;
                if let Some((_, c)) = cam.take() {
                    unsafe { sys::obs_source_dec_showing(c.as_ptr()) };
                }
            }
        }
    }

    fn tracker(&self) -> *mut c_void {
        let mut t = self.tracker.lock();
        if *t == 0
            && let Some(mut cd) = tracker_call(c"lightair_face_tracker_new", |_| {})
        {
            *t = get_ptr(&mut cd, c"tracker") as usize;
            free_cd(&mut cd);
        }
        *t as *mut c_void
    }

    fn body_tracker(&self) -> *mut c_void {
        let mut t = self.body_tracker.lock();
        if *t == 0
            && let Some(mut cd) = tracker_call(c"lightair_body_tracker_new", |_| {})
        {
            *t = get_ptr(&mut cd, c"tracker") as usize;
            free_cd(&mut cd);
        }
        *t as *mut c_void
    }

    /// The body the body tracker sees, if any.
    fn body(&self) -> Option<BodyFrame> {
        let tracker = *self.body_tracker.lock() as *mut c_void;
        if tracker.is_null() {
            return None;
        }
        let mut b: Box<BtBody> = unsafe { Box::new(std::mem::zeroed()) };
        let bp = &mut *b as *mut BtBody as *mut c_void;
        let mut cd = tracker_call(c"lightair_body_tracker_get", |cd| {
            set_ptr(cd, c"tracker", tracker);
            set_ptr(cd, c"body", bp);
        })?;
        let ok = get_bool(&mut cd, c"ok");
        free_cd(&mut cd);
        if !ok || !b.present {
            return None;
        }
        let (w, h) = self.render.lock().grab.size;
        let joints3d = b.has3d.then(|| {
            std::array::from_fn(|j| (b.j3[j][3] > 0.5).then(|| Vec3::new(b.j3[j][0], b.j3[j][1], b.j3[j][2])))
        });
        let joints2d =
            std::array::from_fn(|j| (b.j2[j][2] > 0.0).then(|| (glam::Vec2::new(b.j2[j][0], b.j2[j][1]), b.j2[j][2])));
        let hands = std::array::from_fn(|side| {
            b.hand_present[side].then(|| {
                std::array::from_fn(|k| (glam::Vec2::new(b.hand[side][k][0], b.hand[side][k][1]), b.hand[side][k][2]))
            })
        });
        let hands3d = std::array::from_fn(|side| {
            b.hand_present[side].then(|| std::array::from_fn(|k| Vec3::from(b.hand3d[side][k])))
        });
        Some(BodyFrame {
            joints3d,
            joints2d,
            hands,
            hands3d,
            aspect: w.max(1) as f32 / h.max(1) as f32,
            frame_id: b.frame_id,
        })
    }

    /// The face the tracker sees, if any, and its points (for the tracking view).
    fn landmarks(&self, frame_id: u64) -> Option<(Measures, Vec<[f32; 2]>)> {
        let tracker = *self.tracker.lock() as *mut c_void;
        if tracker.is_null() || frame_id == 0 {
            return None;
        }
        let mut face: Box<FtFace> = unsafe { Box::new(std::mem::zeroed()) };
        let fp = &mut *face as *mut FtFace as *mut c_void;
        let mut cd = tracker_call(c"lightair_face_tracker_get", |cd| {
            set_ptr(cd, c"tracker", tracker);
            set_int(cd, c"frame_id", frame_id as i64);
            set_ptr(cd, c"face", fp);
        })?;
        let ok = get_bool(&mut cd, c"ok");
        free_cd(&mut cd);
        if !ok || !face.valid || !face.present {
            return None;
        }
        let region = |r: usize| -> &[[f32; 2]] {
            let (s, n) = (face.region_start[r].max(0) as usize, face.region_count[r].max(0) as usize);
            face.pts.get(s..s + n).unwrap_or(&[])
        };
        let mesh = region(FT_MESH);
        let irises: Vec<[f32; 2]> =
            [FT_LEFT_PUPIL, FT_RIGHT_PUPIL].iter().flat_map(|&r| region(r).first().copied()).collect();
        let grab = self.render.lock().grab.size;
        let m = lightair_avatar::rig::measure(&Landmarks {
            mesh,
            irises: &irises,
            width: grab.0.max(1) as f32,
            height: grab.1.max(1) as f32,
        })?;
        Some((m, mesh.iter().chain(&irises).copied().collect()))
    }
}

const EFFECT: &str = r#"
uniform float4x4 ViewProj;
uniform float4x4 view_mat;
uniform texture2d base_tex;
uniform texture2d shade_tex;
uniform texture2d emissive_tex;
uniform texture2d matcap_tex;
uniform float4 base_color;
uniform float4 shade_color;
uniform float4 emissive;
uniform float4 rim;
uniform float4 matcap;
uniform float4 shading;
uniform float4 extra;
uniform float4 light_dir;
uniform float4 eye_pos;
uniform float4 outline;

sampler_state tex_sampler {
    Filter = Linear;
    AddressU = Wrap;
    AddressV = Wrap;
};

struct VertIn {
    float4 pos : POSITION;
    float4 norm : NORMAL;
    float2 uv : TEXCOORD0;
};

struct VertOut {
    float4 pos : POSITION;
    float2 uv : TEXCOORD0;
    float3 norm : TEXCOORD1;
    float3 wpos : TEXCOORD2;
};

VertOut VSMain(VertIn v)
{
    VertOut o;
    o.pos = mul(float4(v.pos.xyz, 1.0), ViewProj);
    o.uv = v.uv;
    o.norm = v.norm.xyz;
    o.wpos = v.pos.xyz;
    return o;
}

VertOut VSOutline(VertIn v)
{
    VertOut o;
    float3 p = v.pos.xyz + normalize(v.norm.xyz) * outline.w;
    o.pos = mul(float4(p, 1.0), ViewProj);
    o.uv = v.uv;
    o.norm = v.norm.xyz;
    o.wpos = p;
    return o;
}

float4 base_of(float2 uv)
{
    float4 b = base_tex.Sample(tex_sampler, uv) * base_color;
    CUTOUT
    if (extra.z > 0.5)
        b.a = 1.0;
    return b;
}

float4 PSMain(VertOut v) : TARGET
{
    float4 b = base_of(v.uv);
    if (extra.y > 0.5)
        return b;
    float3 n = normalize(v.norm);
    float3 vdir = normalize(eye_pos.xyz - v.wpos);
    // (back faces of double-sided parts face the camera too)
    if (dot(n, vdir) < 0.0)
        n = -n;
    float s = dot(n, normalize(light_dir.xyz)) + shading.y;
    float lo = -1.0 + shading.x;
    float hi = 1.0 - shading.x;
    s = saturate((s - lo) / max(hi - lo, 0.0001));
    float3 shade = shade_tex.Sample(tex_sampler, v.uv).rgb * shade_color.rgb;
    float3 c = lerp(shade, b.rgb, s);
    // a little ambient light into the shade
    c = lerp(c, b.rgb, 0.12);
    float r = pow(saturate(1.0 - dot(n, vdir) + shading.z), max(shading.w, 0.001));
    c += r * rim.rgb;
    float3 vn = mul(float4(n, 0.0), view_mat).xyz;
    float2 muv = float2(vn.x * 0.5 + 0.5, 0.5 - vn.y * 0.5);
    c += matcap_tex.Sample(tex_sampler, muv).rgb * matcap.rgb;
    c += emissive_tex.Sample(tex_sampler, v.uv).rgb * emissive.rgb;
    return float4(saturate(c), b.a);
}

float4 PSOutline(VertOut v) : TARGET
{
    float4 b = base_of(v.uv);
    float3 c = outline.rgb * lerp(float3(1.0, 1.0, 1.0), b.rgb, extra.w);
    return float4(c, b.a);
}

technique Draw
{
    pass
    {
        vertex_shader = VSMain(v);
        pixel_shader = PSMain(v);
    }
}

technique Outline
{
    pass
    {
        vertex_shader = VSOutline(v);
        pixel_shader = PSOutline(v);
    }
}
"#;

/// The shader, with cutouts where the renderer has `discard` (OpenGL, Direct3D), else without.
unsafe fn make_effect(plain: bool) -> *mut sys::gs_effect_t {
    let cutout = if plain { "" } else { "if (b.a < extra.x)\n        discard;" };
    let code = CString::new(EFFECT.replace("CUTOUT", cutout)).unwrap_or_default();
    let mut err: *mut c_char = std::ptr::null_mut();
    let e = unsafe { sys::gs_effect_create(code.as_ptr(), c"lightair-avatar.effect".as_ptr(), &mut err) };
    if !err.is_null() {
        tracing::warn!(error = %unsafe { CStr::from_ptr(err) }.to_string_lossy(), plain, "avatar shader");
        unsafe { sys::bfree(err as *mut c_void) };
    }
    e
}

impl Render {
    /// Draws the posed avatar into its texture. (Graphics context held.)
    unsafe fn draw(&mut self, settings: &Settings) {
        let (Some((_, avatar)), Some(gpu)) = (&self.avatar, &self.gpu) else { return };
        if self.effect.is_null() {
            if self.effect_failed {
                return;
            }
            self.effect = unsafe { make_effect(false) };
            if self.effect.is_null() {
                self.effect = unsafe { make_effect(true) };
                self.effect_plain = true;
            }
            if self.effect.is_null() {
                self.effect_failed = true;
                return;
            }
        }
        unsafe {
            if self.white.is_null() {
                let px = [255u8; 4];
                let data = [px.as_ptr()];
                self.white = sys::gs_texture_create(1, 1, sys::GS_RGBA, 1, data.as_ptr() as *mut *const u8, 0);
            }
            if self.target.is_null() {
                self.target = sys::gs_texrender_create(sys::GS_RGBA, sys::GS_Z24_S8);
            }
            let (w, h) = (settings.width, settings.height);
            sys::gs_texrender_reset(self.target);
            if !sys::gs_texrender_begin(self.target, w, h) {
                return;
            }
            let clear = v4([0.0; 4]);
            sys::gs_clear(sys::GS_CLEAR_COLOR | sys::GS_CLEAR_DEPTH, &clear, 1.0, 0);
            let model = &avatar.model;
            let cam = avatar.camera(settings.framing, w as f32 / h as f32, settings.zoom);
            sys::gs_projection_push();
            sys::gs_perspective(cam.fovy, w as f32 / h as f32, cam.near, cam.far);
            sys::gs_matrix_push();
            // (libobs's perspective has y down: flipped back)
            let view = cam.view();
            let m = M4::from(Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)) * view);
            sys::gs_matrix_set(m.ptr());
            let cull = sys::gs_get_cull_mode();
            sys::gs_enable_depth_test(true);
            sys::gs_depth_function(sys::GS_LESS);
            sys::gs_blend_state_push();
            sys::gs_enable_blending(true);
            sys::gs_blend_function_separate(
                sys::GS_BLEND_SRCALPHA,
                sys::GS_BLEND_INVSRCALPHA,
                sys::GS_BLEND_ONE,
                sys::GS_BLEND_INVSRCALPHA,
            );

            let e = self.effect;
            let param = |n: &CStr| sys::gs_effect_get_param_by_name(e, n.as_ptr());
            let vm = M4::from(view);
            sys::gs_effect_set_matrix4(param(c"view_mat"), vm.ptr());
            let light = Vec3::new(0.35, 0.6, 1.0).normalize();
            sys::gs_effect_set_vec4(param(c"light_dir"), &v4([light.x, light.y, light.z, 0.0]));
            sys::gs_effect_set_vec4(param(c"eye_pos"), &v4([cam.eye.x, cam.eye.y, cam.eye.z, 1.0]));

            // opaque and cut out first, then the blended ones far to near
            let mut order: Vec<usize> = (0..model.primitives.len()).collect();
            let depth = |i: usize| {
                let ps = &avatar.positions[i];
                if ps.is_empty() {
                    return 0.0;
                }
                let c = ps.iter().step_by((ps.len() / 16).max(1)).copied().sum::<Vec3>()
                    / ps.iter().step_by((ps.len() / 16).max(1)).count() as f32;
                c.distance(cam.eye)
            };
            let blended = |i: usize| {
                model.materials[model.primitives[i].material].alpha == lightair_avatar::vrm::AlphaMode::Blend
            };
            let depths: Vec<f32> = order.iter().map(|&i| if blended(i) { depth(i) } else { 0.0 }).collect();
            order.sort_by(|&a, &b| {
                let (ma, mb) =
                    (&model.materials[model.primitives[a].material], &model.materials[model.primitives[b].material]);
                blended(a)
                    .cmp(&blended(b))
                    .then(ma.queue_offset.cmp(&mb.queue_offset))
                    .then(depths[b].total_cmp(&depths[a]))
            });
            let tex = |t: Option<usize>| {
                t.and_then(|t| gpu.textures.get(t).copied()).filter(|t| !t.is_null()).unwrap_or(self.white)
            };
            for pass in [false, true] {
                for &i in &order {
                    let p = &model.primitives[i];
                    let g = &gpu.prims[i];
                    let mat = &model.materials[p.material];
                    if g.vb.is_null() || g.ib.is_null() || g.indices == 0 {
                        continue;
                    }
                    let outline = pass;
                    if outline && (mat.outline_width <= 0.0 || mat.alpha == lightair_avatar::vrm::AlphaMode::Blend) {
                        continue;
                    }
                    let cutoff = match mat.alpha {
                        lightair_avatar::vrm::AlphaMode::Mask(c) => c,
                        _ => -1.0,
                    };
                    sys::gs_effect_set_texture(param(c"base_tex"), tex(mat.base_texture));
                    sys::gs_effect_set_texture(param(c"shade_tex"), tex(mat.shade_texture));
                    sys::gs_effect_set_texture(param(c"emissive_tex"), tex(mat.emissive_texture));
                    sys::gs_effect_set_texture(param(c"matcap_tex"), tex(mat.matcap_texture));
                    let bc = mat.base_color;
                    let opaque = mat.alpha == lightair_avatar::vrm::AlphaMode::Opaque;
                    sys::gs_effect_set_vec4(
                        param(c"base_color"),
                        &v4([bc[0], bc[1], bc[2], if opaque { 1.0 } else { bc[3] }]),
                    );
                    let sc = mat.shade_color;
                    sys::gs_effect_set_vec4(param(c"shade_color"), &v4([sc[0], sc[1], sc[2], 1.0]));
                    let em = if mat.emissive_texture.is_some() || mat.emissive != [0.0; 3] {
                        mat.emissive
                    } else {
                        [0.0; 3]
                    };
                    sys::gs_effect_set_vec4(param(c"emissive"), &v4([em[0], em[1], em[2], 0.0]));
                    let rc = mat.rim_color;
                    sys::gs_effect_set_vec4(param(c"rim"), &v4([rc[0], rc[1], rc[2], 0.0]));
                    let mc = if mat.matcap_texture.is_some() { mat.matcap_factor } else { [0.0; 3] };
                    sys::gs_effect_set_vec4(param(c"matcap"), &v4([mc[0], mc[1], mc[2], 0.0]));
                    // toony, shift, rim lift, rim fresnel power
                    sys::gs_effect_set_vec4(
                        param(c"shading"),
                        &v4([mat.shading_toony, mat.shading_shift, mat.rim_lift, mat.rim_fresnel_power]),
                    );
                    // cutoff (or -1), unlit, opaque, outline lighting mix
                    let solid = cutoff >= 0.0 || opaque;
                    sys::gs_effect_set_vec4(
                        param(c"extra"),
                        &v4([
                            cutoff,
                            if mat.unlit { 1.0 } else { 0.0 },
                            if solid { 1.0 } else { 0.0 },
                            mat.outline_lighting_mix,
                        ]),
                    );
                    let oc = mat.outline_color;
                    sys::gs_effect_set_vec4(param(c"outline"), &v4([oc[0], oc[1], oc[2], mat.outline_width]));
                    // back faces culled; an outline is the inflated shell's back faces only
                    sys::gs_set_cull_mode(if outline {
                        sys::GS_FRONT
                    } else if mat.double_sided {
                        sys::GS_NEITHER
                    } else {
                        sys::GS_BACK
                    });
                    sys::gs_load_vertexbuffer(g.vb);
                    sys::gs_load_indexbuffer(g.ib);
                    let technique = if outline { c"Outline" } else { c"Draw" };
                    while sys::gs_effect_loop(e, technique.as_ptr()) {
                        sys::gs_draw(sys::GS_TRIS, 0, g.indices);
                    }
                }
            }
            sys::gs_load_indexbuffer(std::ptr::null_mut());
            sys::gs_load_vertexbuffer(std::ptr::null_mut());
            sys::gs_set_cull_mode(cull);
            sys::gs_enable_depth_test(false);
            sys::gs_blend_state_pop();
            sys::gs_matrix_pop();
            sys::gs_projection_pop();
            sys::gs_texrender_end(self.target);
        }
    }

    /// Copies the camera's picture (small) for the tracker; hands it the last one copied.
    /// (Graphics context held.)
    unsafe fn grab(&mut self, camera: *mut sys::obs_source_t, tracker: *mut c_void, body: *mut c_void, keep: bool) {
        unsafe {
            let (cw, ch) = (sys::obs_source_get_width(camera), sys::obs_source_get_height(camera));
            if cw == 0 || ch == 0 || tracker.is_null() {
                return;
            }
            let tw = cw.min(TRACK_WIDTH) & !1;
            let th = ((ch as u64 * tw as u64 / cw as u64) as u32).max(2) & !1;
            let g = &mut self.grab;
            if g.texrender.is_null() {
                g.texrender = sys::gs_texrender_create(sys::GS_BGRA, sys::GS_ZS_NONE);
            }
            if g.size != (tw, th) {
                for s in &mut g.stage {
                    if !s.is_null() {
                        sys::gs_stagesurface_destroy(*s);
                    }
                    *s = sys::gs_stagesurface_create(tw, th, sys::GS_BGRA);
                }
                g.size = (tw, th);
                g.waiting = false;
            }
            // the copy made last frame, to the tracker
            if g.waiting {
                let prev = g.stage[g.next ^ 1];
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut linesize = 0u32;
                if sys::gs_stagesurface_map(prev, &mut data, &mut linesize) {
                    g.frame_id += 1;
                    let id = g.frame_id;
                    let now = sys::os_gettime_ns();
                    if let Some(mut cd) = tracker_call(c"lightair_face_tracker_submit", |cd| {
                        set_ptr(cd, c"tracker", tracker);
                        set_ptr(cd, c"data", data as *mut c_void);
                        set_int(cd, c"linesize", linesize as i64);
                        set_int(cd, c"width", tw as i64);
                        set_int(cd, c"height", th as i64);
                        set_int(cd, c"frame_id", id as i64);
                        set_int(cd, c"timestamp_ns", now as i64);
                    }) {
                        free_cd(&mut cd);
                    }
                    if !body.is_null()
                        && let Some(mut cd) = tracker_call(c"lightair_body_tracker_submit", |cd| {
                            set_ptr(cd, c"tracker", body);
                            set_ptr(cd, c"data", data as *mut c_void);
                            set_int(cd, c"linesize", linesize as i64);
                            set_int(cd, c"width", tw as i64);
                            set_int(cd, c"height", th as i64);
                            set_int(cd, c"frame_id", id as i64);
                        })
                    {
                        free_cd(&mut cd);
                    }
                    if keep {
                        let row = tw as usize * 4;
                        self.view_pixels.resize(row * th as usize, 0);
                        for y in 0..th as usize {
                            let from = std::slice::from_raw_parts(data.add(y * linesize as usize), row);
                            self.view_pixels[y * row..(y + 1) * row].copy_from_slice(from);
                        }
                        self.view_size = (tw, th);
                    }
                    sys::gs_stagesurface_unmap(prev);
                }
                g.waiting = false;
            }
            sys::gs_texrender_reset(g.texrender);
            if sys::gs_texrender_begin(g.texrender, tw, th) {
                let clear = v4([0.0, 0.0, 0.0, 1.0]);
                sys::gs_clear(sys::GS_CLEAR_COLOR, &clear, 0.0, 0);
                sys::gs_ortho(0.0, cw as f32, 0.0, ch as f32, -100.0, 100.0);
                sys::gs_blend_state_push();
                sys::gs_enable_blending(false);
                sys::obs_source_video_render(camera);
                sys::gs_blend_state_pop();
                sys::gs_texrender_end(g.texrender);
                sys::gs_stage_texture(g.stage[g.next], sys::gs_texrender_get_texture(g.texrender));
                g.next ^= 1;
                g.waiting = true;
            }
        }
    }

    /// The tracking view: the trackers' picture (mirrored with the avatar), the face's points
    /// green, the body's joints red (3D found) or orange (the picture's only), the hands
    /// yellow, in the avatar's lower left. (Graphics context held.)
    unsafe fn draw_view(&mut self, settings: &Settings) {
        let (w, h) = self.view_size;
        if w == 0 || h == 0 || self.view_pixels.len() < (w * h * 4) as usize {
            return;
        }
        let mut px = self.view_pixels.clone();
        let mirror = settings.mirror;
        // (dots on the picture as it is, then both flipped together)
        let mut dot = |x: f32, y: f32, r: i32, c: [u8; 3]| {
            let (cx, cy) = ((x * w as f32) as i32, (y * h as f32) as i32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = (cx + dx, cy + dy);
                    if xx >= 0 && yy >= 0 && (xx as u32) < w && (yy as u32) < h {
                        let i = (yy as usize * w as usize + xx as usize) * 4;
                        px[i..i + 4].copy_from_slice(&[c[2], c[1], c[0], 255]);
                    }
                }
            }
        };
        for p in &self.view_face {
            dot(p[0], p[1], 0, [80, 255, 120]);
        }
        if let Some(b) = &self.view_body {
            let colour = if b.joints3d.is_some() { [255, 60, 60] } else { [255, 160, 40] };
            for (p, c) in b.joints2d.iter().flatten() {
                if *c >= 0.3 {
                    dot(p.x, p.y, 4, colour);
                } else if *c > 0.0 {
                    dot(p.x, p.y, 2, [120, 120, 120]);
                }
            }
            for hand in b.hands.iter().flatten() {
                for (p, c) in hand {
                    if *c > 0.2 {
                        dot(p.x, p.y, 2, [255, 230, 60]);
                    }
                }
            }
        }
        if mirror {
            let row = w as usize * 4;
            for y in 0..h as usize {
                let line = &mut px[y * row..(y + 1) * row];
                for x in 0..w as usize / 2 {
                    for k in 0..4 {
                        line.swap(x * 4 + k, (w as usize - 1 - x) * 4 + k);
                    }
                }
            }
        }
        unsafe {
            if !self.view_tex.is_null()
                && (sys::gs_texture_get_width(self.view_tex), sys::gs_texture_get_height(self.view_tex)) != (w, h)
            {
                sys::gs_texture_destroy(self.view_tex);
                self.view_tex = std::ptr::null_mut();
            }
            if self.view_tex.is_null() {
                self.view_tex = sys::gs_texture_create(w, h, sys::GS_BGRA, 1, std::ptr::null_mut(), sys::GS_DYNAMIC);
            }
            if self.view_tex.is_null() {
                return;
            }
            sys::gs_texture_set_image(self.view_tex, px.as_ptr(), w * 4, false);
            let vw = (settings.width as f32 * 0.38).round();
            let vh = (vw * h as f32 / w as f32).round();
            let e = sys::obs_get_base_effect(sys::OBS_EFFECT_DEFAULT);
            sys::gs_effect_set_texture(sys::gs_effect_get_param_by_name(e, c"image".as_ptr()), self.view_tex);
            sys::gs_matrix_push();
            sys::gs_matrix_translate3f(12.0, settings.height as f32 - vh - 12.0, 0.0);
            while sys::gs_effect_loop(e, c"Draw".as_ptr()) {
                sys::gs_draw_sprite(self.view_tex, 0, vw as u32, vh as u32);
            }
            sys::gs_matrix_pop();
        }
    }

    unsafe fn destroy(&mut self) {
        unsafe {
            if !self.view_tex.is_null() {
                sys::gs_texture_destroy(self.view_tex);
            }
            if let Some(mut gpu) = self.gpu.take() {
                gpu.destroy();
            }
            if let Some(mut g) = self.gpu2d.take() {
                g.destroy();
            }
            if !self.effect.is_null() {
                sys::gs_effect_destroy(self.effect);
            }
            if !self.white.is_null() {
                sys::gs_texture_destroy(self.white);
            }
            if !self.target.is_null() {
                sys::gs_texrender_destroy(self.target);
            }
            if !self.grab.texrender.is_null() {
                sys::gs_texrender_destroy(self.grab.texrender);
            }
            for s in self.grab.stage {
                if !s.is_null() {
                    sys::gs_stagesurface_destroy(s);
                }
            }
        }
    }
}

unsafe extern "C" fn get_name(_: *mut c_void) -> *const c_char {
    c"Avatar".as_ptr()
}

unsafe extern "C" fn get_defaults(settings: *mut sys::obs_data_t) {
    unsafe {
        sys::obs_data_set_default_int(settings, c"width".as_ptr(), 1080);
        sys::obs_data_set_default_int(settings, c"height".as_ptr(), 1080);
        sys::obs_data_set_default_bool(settings, c"mirror".as_ptr(), true);
        sys::obs_data_set_default_string(settings, c"framing".as_ptr(), c"bust".as_ptr());
        sys::obs_data_set_default_double(settings, c"zoom".as_ptr(), 1.0);
        sys::obs_data_set_default_double(settings, c"expression_weight".as_ptr(), 1.0);
        sys::obs_data_set_default_string(settings, c"body".as_ptr(), c"upper".as_ptr());
    }
}

unsafe extern "C" fn create(settings: *mut sys::obs_data_t, source: *mut sys::obs_source_t) -> *mut c_void {
    let s = Arc::new(Shared {
        source: source as usize,
        settings: Mutex::new(read(settings)),
        load: Mutex::new(Load::None),
        camera: Mutex::new(None),
        hidden_since: Mutex::new(None),
        tracker: Mutex::new(0),
        body_tracker: Mutex::new(0),
        render: Mutex::new(Render {
            avatar: None,
            puppet: None,
            gpu2d: None,
            gpu: None,
            effect: std::ptr::null_mut(),
            effect_plain: false,
            effect_failed: false,
            white: std::ptr::null_mut(),
            target: std::ptr::null_mut(),
            grab: Grab {
                texrender: std::ptr::null_mut(),
                stage: [std::ptr::null_mut(); 2],
                size: (0, 0),
                next: 0,
                waiting: false,
                frame_id: 0,
            },
            smoother: Smoother::default(),
            last_tick: None,
            dirty: false,
            grab_due: false,
            measures: None,
            body: BodyState::default(),
            view_pixels: Vec::new(),
            view_face: Vec::new(),
            view_body: None,
            view_tex: std::ptr::null_mut(),
            view_size: (0, 0),
        }),
    });
    s.load_if_changed();
    unsafe {
        let ph = sys::obs_source_get_proc_handler(source);
        let data = Box::into_raw(Box::new(Arc::downgrade(&s))) as *mut c_void;
        sys::proc_handler_add(ph, c"void calibrate(out string calibration)".as_ptr(), Some(proc_calibrate), data);
        sys::proc_handler_add(ph, c"void status(out bool loaded, out bool tracking, out string error, out string name, out string body, out string expressions)".as_ptr(), Some(proc_status), data);
        // (the weak reference lives as long as the source: freed with it)
        let _ = data;
    }
    Box::into_raw(Box::new(s)) as *mut c_void
}

/// `calibrate`: the face seen now becomes the neutral one. Gives its JSON (for `calibration`).
unsafe extern "C" fn proc_calibrate(data: *mut c_void, cd: *mut sys::calldata_t) {
    let weak = unsafe { &*(data as *const std::sync::Weak<Shared>) };
    let Some(s) = weak.upgrade() else { return };
    let measures = s.render.lock().measures;
    let text = measures.map(|m| calibration_json(&Calibration::from(&m))).unwrap_or_default();
    let c = CString::new(text).unwrap_or_default();
    set_string(cd, c"calibration", &c);
}

/// `status`: whether the model is ready, a face is being tracked, the model's error or name.
unsafe extern "C" fn proc_status(data: *mut c_void, cd: *mut sys::calldata_t) {
    let weak = unsafe { &*(data as *const std::sync::Weak<Shared>) };
    let Some(s) = weak.upgrade() else { return };
    let expressions = match &*s.load.lock() {
        Load::Ready(_, Loaded::Flat(_, names)) => names.join("\n"),
        _ => String::new(),
    };
    let (loaded, error, name) = match &*s.load.lock() {
        Load::Ready(p, Loaded::Vrm(m)) => {
            (true, String::new(), if m.meta.name.is_empty() { model_name(p) } else { m.meta.name.clone() })
        }
        Load::Ready(p, Loaded::Flat(..)) => (true, String::new(), model_name(p)),
        Load::Failed(_, e) => (false, e.clone(), String::new()),
        _ => (false, String::new(), String::new()),
    };
    let (tracking, body) = {
        let r = s.render.lock();
        let body = if s.settings.lock().body == BodyMode::Off || is_flat(&s.settings.lock().file) {
            String::new()
        } else if *s.body_tracker.lock() == 0 {
            "no body tracker".to_string()
        } else {
            let seen = r.body.seen;
            let parts: Vec<&str> = [(seen[1], "left arm"), (seen[2], "right arm"), (seen[3], "legs")]
                .into_iter()
                .filter(|(v, _)| *v > 0.5)
                .map(|(_, n)| n)
                .collect();
            let torso = if seen[0] > 0.5 { "body" } else { "no body" };
            if parts.is_empty() { torso.to_string() } else { format!("{torso}, {}", parts.join(", ")) }
        };
        (r.measures.is_some(), body)
    };
    let (e, n) = (CString::new(error).unwrap_or_default(), CString::new(name).unwrap_or_default());
    set_bool(cd, c"loaded", loaded);
    set_bool(cd, c"tracking", tracking);
    set_string(cd, c"error", &e);
    set_string(cd, c"name", &n);
    let b = CString::new(body).unwrap_or_default();
    set_string(cd, c"body", &b);
    let x = CString::new(expressions).unwrap_or_default();
    set_string(cd, c"expressions", &x);
}

/// A model's name from its file ("mao_pro" for mao_pro.model3.json).
fn model_name(path: &str) -> String {
    let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = file.to_ascii_lowercase();
    for ext in [".model3.json", ".vrm", ".inp", ".inx"] {
        if lower.ends_with(ext) {
            return file[..file.len() - ext.len()].to_string();
        }
    }
    file
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    let s = unsafe { Box::from_raw(data as *mut Arc<Shared>) };
    if let Some((_, c)) = s.camera.lock().take() {
        unsafe { sys::obs_source_dec_showing(c.as_ptr()) };
    }
    unsafe {
        sys::obs_enter_graphics();
        s.render.lock().destroy();
        sys::obs_leave_graphics();
    }
    let body = std::mem::take(&mut *s.body_tracker.lock());
    if body != 0
        && let Some(mut cd) =
            tracker_call(c"lightair_body_tracker_free", |cd| set_ptr(cd, c"tracker", body as *mut c_void))
    {
        free_cd(&mut cd);
    }
    let tracker = std::mem::take(&mut *s.tracker.lock());
    if tracker != 0
        && let Some(mut cd) =
            tracker_call(c"lightair_face_tracker_free", |cd| set_ptr(cd, c"tracker", tracker as *mut c_void))
    {
        free_cd(&mut cd);
    }
}

unsafe extern "C" fn update(data: *mut c_void, settings: *mut sys::obs_data_t) {
    let s = shared(data);
    *s.settings.lock() = read(settings);
    s.load_if_changed();
}

unsafe extern "C" fn width(data: *mut c_void) -> u32 {
    shared(data).settings.lock().width
}

unsafe extern "C" fn height(data: *mut c_void) -> u32 {
    shared(data).settings.lock().height
}

/// Once a frame: the camera kept or let go, the face read, the model posed.
unsafe extern "C" fn video_tick(data: *mut c_void, _seconds: f32) {
    let s = shared(data);
    let shown = unsafe { sys::obs_source_showing(s.source as *mut sys::obs_source_t) };
    s.keep_camera(shown);
    if !shown {
        s.render.lock().last_tick = None;
        return;
    }
    let settings = s.settings.lock().clone();
    let model = match &*s.load.lock() {
        Load::Ready(p, m) => Some((p.clone(), m.clone())),
        _ => None,
    };
    // (a Live2D model follows the face only)
    let body_mode = if is_flat(&settings.file) { BodyMode::Off } else { settings.body };
    let tracking = s.camera.lock().is_some();
    if tracking {
        s.tracker();
        if body_mode != BodyMode::Off {
            s.body_tracker();
        }
    }
    let frame_id = s.render.lock().grab.frame_id;
    let found = if tracking { s.landmarks(frame_id) } else { None };
    let (measures, face_points) = match found {
        Some((m, p)) => (Some(m), p),
        None => (None, Vec::new()),
    };
    let body = if tracking && body_mode != BodyMode::Off { s.body() } else { None };
    if let Some(b) = &body {
        dump(b);
    }
    let mut r = s.render.lock();
    let now = Instant::now();
    let dt = r.last_tick.map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32());
    r.last_tick = Some(now);
    r.measures = measures;
    if settings.show_tracking {
        r.view_face = face_points;
        r.view_body = body.clone();
    }
    match model {
        Some((path, Loaded::Vrm(m))) => {
            r.puppet = None;
            if r.avatar.as_ref().is_none_or(|(p, _)| *p != path) {
                r.avatar = Some((path, Avatar::new(m)));
            }
        }
        Some((path, Loaded::Flat(slot, _))) => {
            r.avatar = None;
            if r.puppet.as_ref().is_none_or(|(p, _)| *p != path)
                && let Some(l) = slot.lock().take()
            {
                r.puppet = Some((path, l));
            }
        }
        None => {
            r.avatar = None;
            r.puppet = None;
        }
    }
    let seen = measures.map(|m| {
        let f = settings.calibration.face(&m);
        if settings.mirror { f.mirrored() } else { f }
    });
    let face: Face = r.smoother.step(seen, dt);
    let expression =
        (!settings.expression.is_empty()).then(|| (settings.expression.clone(), settings.expression_weight));
    r.body.step(body.as_ref(), settings.mirror, dt);
    let r = &mut *r;
    if let Some((_, a)) = r.avatar.as_mut() {
        let (bones, torso) = a.body_bones(&r.body, settings.body);
        a.update(&Controls { face, expression, bones, torso }, dt);
        r.dirty = true;
    }
    if let Some((_, l)) = r.puppet.as_mut() {
        l.update(&face, (!settings.expression.is_empty()).then_some(settings.expression.as_str()), dt);
        r.dirty = true;
    }
    r.grab_due = tracking;
}

unsafe extern "C" fn video_render(data: *mut c_void, _: *mut sys::gs_effect_t) {
    let s = shared(data);
    let settings = s.settings.lock().clone();
    let camera =
        s.camera.lock().as_ref().map(|(_, c)| unsafe { sys::obs_source_get_ref(c.as_ptr()) }).filter(|c| !c.is_null());
    let tracker = *s.tracker.lock() as *mut c_void;
    let body = if settings.body == BodyMode::Off || is_flat(&settings.file) {
        std::ptr::null_mut()
    } else {
        *s.body_tracker.lock() as *mut c_void
    };
    let mut r = s.render.lock();
    unsafe {
        // once a frame (the avatar may be drawn several times: on air, the preview, a thumbnail)
        if r.grab_due {
            r.grab_due = false;
            if let Some(c) = camera {
                r.grab(c, tracker, body, settings.show_tracking);
            }
        }
        if let Some(c) = camera {
            sys::obs_source_release(c);
        }
        if r.dirty {
            r.dirty = false;
            let path = r.avatar.as_ref().map(|(p, _)| p.clone());
            if r.gpu.as_ref().map(|g| &g.path) != path.as_ref() {
                if let Some(mut g) = r.gpu.take() {
                    g.destroy();
                }
                if let Some((p, a)) = &r.avatar {
                    r.gpu = Some(Gpu::new(p, &a.model));
                }
            }
            if let (Some((_, a)), Some(g)) = (&r.avatar, &r.gpu) {
                g.upload(a);
            }
            if r.avatar.is_some() {
                r.draw(&settings);
            }
            // a flat model
            let path2d = r.puppet.as_ref().map(|(p, _)| p.clone());
            if r.gpu2d.as_ref().map(|g| g.path()) != path2d.as_deref() {
                if let Some(mut g) = r.gpu2d.take() {
                    g.destroy();
                }
                if let Some((p, l)) = &r.puppet {
                    r.gpu2d = Some(crate::avatar2d::FlatGpu::new(p, l));
                }
            }
            if r.target.is_null() {
                r.target = sys::gs_texrender_create(sys::GS_RGBA, sys::GS_Z24_S8);
            }
            if let (Some((_, l)), Some(g)) = (&r.puppet, &r.gpu2d) {
                g.draw(l, r.target, settings.width, settings.height, settings.framing, settings.zoom);
            }
        }
        if r.target.is_null() || (r.avatar.is_none() && r.puppet.is_none()) {
            return;
        }
        let tex = sys::gs_texrender_get_texture(r.target);
        if tex.is_null() {
            return;
        }
        // premultiplied by its own drawing
        sys::gs_blend_state_push();
        sys::gs_blend_function(sys::GS_BLEND_ONE, sys::GS_BLEND_INVSRCALPHA);
        let e = sys::obs_get_base_effect(sys::OBS_EFFECT_DEFAULT);
        sys::gs_effect_set_texture(sys::gs_effect_get_param_by_name(e, c"image".as_ptr()), tex);
        while sys::gs_effect_loop(e, c"Draw".as_ptr()) {
            sys::gs_draw_sprite(tex, 0, settings.width, settings.height);
        }
        sys::gs_blend_state_pop();
        if settings.show_tracking {
            r.draw_view(&settings);
        }
    }
}

/// Registers the source with libobs (once; after startup).
pub fn register() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        let mut info: sys::obs_source_info = std::mem::zeroed();
        info.id = c"lightair_avatar".as_ptr();
        info.type_ = sys::OBS_SOURCE_TYPE_INPUT;
        info.output_flags = sys::OBS_SOURCE_VIDEO | sys::OBS_SOURCE_CUSTOM_DRAW | sys::OBS_SOURCE_SRGB;
        info.get_name = Some(get_name);
        info.create = Some(create);
        info.destroy = Some(destroy);
        info.update = Some(update);
        info.get_defaults = Some(get_defaults);
        info.get_width = Some(width);
        info.get_height = Some(height);
        info.video_tick = Some(video_tick);
        info.video_render = Some(video_render);
        info.icon_type = sys::OBS_ICON_TYPE_CAMERA;
        sys::obs_register_source_s(&info, std::mem::size_of::<sys::obs_source_info>());
    });
}

/// What an avatar is doing, for the Inspector.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub loaded: bool,
    /// A face is being followed.
    pub tracking: bool,
    /// Why the model didn't load.
    pub error: String,
    /// The model's own name.
    pub name: String,
    /// What body tracking sees ("body, left arm", "no body"; empty when off).
    pub body: String,
    /// A Live2D model's own expressions (VRM models have the standard ones).
    pub expressions: Vec<String>,
}

fn get_string(cd: &mut sys::calldata_t, name: &CStr) -> String {
    let mut p: *const c_char = std::ptr::null();
    if unsafe { sys::calldata_get_string(cd, name.as_ptr(), &mut p) } && !p.is_null() {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

/// Calls an avatar source's proc.
fn source_call(source: *mut sys::obs_source_t, name: &CStr) -> Option<sys::calldata_t> {
    unsafe {
        let ph = sys::obs_source_get_proc_handler(source);
        if ph.is_null() {
            return None;
        }
        let mut cd = sys::calldata_t::default();
        if sys::proc_handler_call(ph, name.as_ptr(), &mut cd) {
            Some(cd)
        } else {
            free_cd(&mut cd);
            None
        }
    }
}

/// An avatar source's status.
pub fn status(source: *mut sys::obs_source_t) -> Status {
    let Some(mut cd) = source_call(source, c"status") else { return Status::default() };
    let s = Status {
        loaded: get_bool(&mut cd, c"loaded"),
        tracking: get_bool(&mut cd, c"tracking"),
        error: get_string(&mut cd, c"error"),
        name: get_string(&mut cd, c"name"),
        body: get_string(&mut cd, c"body"),
        expressions: get_string(&mut cd, c"expressions").lines().map(str::to_string).collect(),
    };
    free_cd(&mut cd);
    s
}

/// The face the avatar sees now, as its neutral one: the `calibration` setting's JSON (`None`
/// without a face in view).
pub fn calibrate(source: *mut sys::obs_source_t) -> Option<String> {
    let mut cd = source_call(source, c"calibrate")?;
    let text = get_string(&mut cd, c"calibration");
    free_cd(&mut cd);
    (!text.is_empty()).then_some(text)
}

/// Runs the body tracker (Lightair Effect Stack, loaded) on one BGRA picture and waits for it:
/// for tests and diagnostics. `None` without the tracker or without a body.
pub fn track_body_once(bgra: &[u8], width: u32, height: u32) -> Option<BodyFrame> {
    let mut cd = tracker_call(c"lightair_body_tracker_new", |_| {})?;
    let tracker = get_ptr(&mut cd, c"tracker");
    free_cd(&mut cd);
    if tracker.is_null() {
        return None;
    }
    let s = Shared {
        source: 0,
        settings: Mutex::new(read_defaults()),
        load: Mutex::new(Load::None),
        camera: Mutex::new(None),
        hidden_since: Mutex::new(None),
        tracker: Mutex::new(0),
        body_tracker: Mutex::new(tracker as usize),
        render: Mutex::new(Render {
            avatar: None,
            puppet: None,
            gpu2d: None,
            gpu: None,
            effect: std::ptr::null_mut(),
            effect_plain: false,
            effect_failed: false,
            white: std::ptr::null_mut(),
            target: std::ptr::null_mut(),
            grab: Grab {
                texrender: std::ptr::null_mut(),
                stage: [std::ptr::null_mut(); 2],
                size: (width, height),
                next: 0,
                waiting: false,
                frame_id: 0,
            },
            smoother: Smoother::default(),
            last_tick: None,
            dirty: false,
            grab_due: false,
            measures: None,
            body: BodyState::default(),
            view_pixels: Vec::new(),
            view_face: Vec::new(),
            view_body: None,
            view_tex: std::ptr::null_mut(),
            view_size: (0, 0),
        }),
    };
    if let Some(mut cd) = tracker_call(c"lightair_body_tracker_submit", |cd| {
        set_ptr(cd, c"tracker", tracker);
        set_ptr(cd, c"data", bgra.as_ptr() as *mut c_void);
        set_int(cd, c"linesize", width as i64 * 4);
        set_int(cd, c"width", width as i64);
        set_int(cd, c"height", height as i64);
        set_int(cd, c"frame_id", 1);
    }) {
        free_cd(&mut cd);
    }
    let mut found = None;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(b) = s.body() {
            found = Some(b);
            break;
        }
    }
    if let Some(mut cd) = tracker_call(c"lightair_body_tracker_free", |cd| set_ptr(cd, c"tracker", tracker)) {
        free_cd(&mut cd);
    }
    found
}

fn read_defaults() -> Settings {
    Settings {
        file: String::new(),
        device: String::new(),
        mirror: true,
        framing: Framing::Bust,
        zoom: 1.0,
        width: 1080,
        height: 1080,
        calibration: Calibration::default(),
        expression: String::new(),
        expression_weight: 1.0,
        body: BodyMode::Upper,
        show_tracking: false,
    }
}

/// `LIGHTAIR_AVATAR_DUMP=<file>`: each new body frame, a JSON line (for replaying offline).
fn dump(b: &BodyFrame) {
    use std::io::Write;
    static FILE: std::sync::OnceLock<Option<Mutex<(std::fs::File, u64)>>> = std::sync::OnceLock::new();
    let Some(file) = FILE.get_or_init(|| {
        std::env::var_os("LIGHTAIR_AVATAR_DUMP")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(|f| Mutex::new((f, u64::MAX)))
    }) else {
        return;
    };
    let mut f = file.lock();
    if f.1 == b.frame_id {
        return;
    }
    f.1 = b.frame_id;
    let v3 = |p: &Option<Vec3>| p.map_or(serde_json::Value::Null, |p| json!([p.x, p.y, p.z]));
    let line = json!({
        "frame_id": b.frame_id,
        "aspect": b.aspect,
        "j3": b.joints3d.map(|j| j.iter().map(v3).collect::<Vec<_>>()),
        "j2": b.joints2d.iter().map(|p| p.map_or(serde_json::Value::Null, |(p, c)| json!([p.x, p.y, c]))).collect::<Vec<_>>(),
        "hands": b.hands.iter().map(|h| h.map(|h| h.iter().map(|(p, c)| json!([p.x, p.y, c])).collect::<Vec<_>>())).collect::<Vec<_>>(),
        "hands3d": b.hands3d.iter().map(|h| h.map(|h| h.iter().map(|p| json!([p.x, p.y, p.z])).collect::<Vec<_>>())).collect::<Vec<_>>(),
    });
    let _ = writeln!(f.0, "{line}");
}
