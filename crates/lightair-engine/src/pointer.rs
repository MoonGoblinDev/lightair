//! The drawn pointer (docs/plans/screen-studio.md, "Cursor"): a filter on a screen capture that
//! draws the pointer itself, so it can be bigger, smoother and styled. The capture's own cursor
//! is turned off while it's on (`reconcile::source_settings`).
//!
//! - The pointer comes from `lightair_platform::source` each frame and is followed by a critically
//!   damped spring (smoothing), tilted by its speed, and faded out when it rests.
//! - Shapes are Lightair's own, drawn here at a high resolution (an arrow, an I-beam, a
//!   crosshair; a dot and a ring as styles), so they stay sharp at any size. Any other pointer
//!   (a hand, a resize arrow, an app's own) is the system's own picture (macOS,
//!   `lightair_platform::system_pointer`).
//! - Clicks (when the OS lets Lightair see them) draw a circle or a ripple.
//!
//! - The follow-zoom (docs/plans/screen-studio.md, "Auto zoom", live): in to where a click was
//!   and out again after a while without one, or always in, following the pointer. Scale and
//!   centre move on critically damped springs; the view keeps the pointer inside it and never
//!   leaves the capture. The pointer and clicks are drawn zoomed too.
//!
//! A scene's [`lightair_model::ScreenZoomPointer`] puts one of these on each of its screen captures
//! (`reconcile`); the effect catalog's "Drawn pointer" is the same filter without the zoom.
//!
//! Positions map from display points to the capture's pixels through the frame of what it
//! captures: its display, or its window where the window is now (read a few times a second);
//! the main display when that can't be told (`capture_frame`).

use std::collections::VecDeque;
use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lightair_platform::input::{self, Event, Recorder, Shape};
use lightair_platform::system_pointer;

use crate::sys;

pub const KIND: &str = lightair_model::catalog::POINTER_KIND;

use lightair_render::shapes::{Bitmap, RES, arrow, crosshair, disc, dot, ibeam, ring};
/// The pointer's size in display points at size 1.
const POINTS: f32 = 22.0;
/// How long a click's effect lasts.
const CLICK: Duration = Duration::from_millis(450);

// ------------------------------------------------------------------ clicks

/// Clicks seen lately, for every pointer filter (one listener for all of them).
struct Clicks {
    recorder: Option<Recorder>,
    started: Instant,
    recent: VecDeque<(Instant, f32, f32)>,
    /// The last key typed (not a shortcut: ⌘ or Ctrl held), and the last click (where typing
    /// most likely goes).
    typed: Option<Instant>,
    last_click: Option<(f32, f32)>,
    users: usize,
}

static CLICKS: Mutex<Option<Clicks>> = Mutex::new(None);

fn clicks_join() {
    let mut c = CLICKS.lock().unwrap();
    let c = c.get_or_insert_with(|| Clicks { recorder: None, started: Instant::now(), recent: VecDeque::new(), typed: None, last_click: None, users: 0 });
    c.users += 1;
    if c.recorder.is_none() && input::can_listen() {
        c.started = Instant::now();
        c.recorder = Some(Recorder::start(30));
    }
}

fn clicks_leave() {
    let mut guard = CLICKS.lock().unwrap();
    if let Some(c) = guard.as_mut() {
        c.users = c.users.saturating_sub(1);
        if c.users == 0 {
            *guard = None;
        }
    }
}

/// Clicks in the last `CLICK`, oldest first.
fn recent_clicks() -> Vec<(Instant, f32, f32)> {
    let mut guard = CLICKS.lock().unwrap();
    let Some(c) = guard.as_mut() else { return Vec::new() };
    if let Some(r) = &c.recorder {
        for s in r.take() {
            match s.event {
                Event::Down { x, y, .. } => {
                    c.recent.push_back((c.started + s.at, x, y));
                    c.last_click = Some((x, y));
                }
                Event::KeyDown { modifiers, .. } if modifiers & (input::modifiers::COMMAND | input::modifiers::CONTROL) == 0 => {
                    c.typed = Some(c.started + s.at);
                }
                _ => {}
            }
        }
    }
    let now = Instant::now();
    while c.recent.front().is_some_and(|(t, ..)| now.duration_since(*t) > CLICK) {
        c.recent.pop_front();
    }
    c.recent.iter().copied().collect()
}

/// When a key was last typed, and where the last click was (display points).
fn typing() -> (Option<Instant>, Option<(f32, f32)>) {
    recent_clicks();
    let guard = CLICKS.lock().unwrap();
    guard.as_ref().map_or((None, None), |c| (c.typed, c.last_click))
}

// ------------------------------------------------------------------ following, on a key

/// Zoomed in, following the pointer, on looks whose zoom is "toggle" (a shortcut turns it on
/// and off again).
static FOLLOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns following on or off: whether it's on now.
pub fn toggle_follow() -> bool {
    !FOLLOW.fetch_xor(true, std::sync::atomic::Ordering::Relaxed)
}

pub fn following() -> bool {
    FOLLOW.load(std::sync::atomic::Ordering::Relaxed)
}

// ------------------------------------------------------------------ the filter

const EFFECT: &str = r#"
uniform float4x4 ViewProj;
uniform texture2d image;
uniform float opacity;
sampler_state def_sampler { Filter = Linear; AddressU = Clamp; AddressV = Clamp; };
struct VertInOut { float4 pos : POSITION; float2 uv : TEXCOORD0; };
VertInOut VSDefault(VertInOut vert_in)
{
    VertInOut vert_out;
    vert_out.pos = mul(float4(vert_in.pos.xyz, 1.0), ViewProj);
    vert_out.uv = vert_in.uv;
    return vert_out;
}
float4 PSDraw(VertInOut vert_in) : TARGET
{
    return image.Sample(def_sampler, vert_in.uv) * opacity;
}
technique Draw { pass { vertex_shader = VSDefault(vert_in); pixel_shader = PSDraw(vert_in); } }
"#;

/// The capture drawn zoomed: `view` is the part shown (x, y, w, h as fractions of it).
const ZOOM_EFFECT: &str = r#"
uniform float4x4 ViewProj;
uniform texture2d image;
uniform float4 view;
sampler_state def_sampler { Filter = Linear; AddressU = Clamp; AddressV = Clamp; };
struct VertInOut { float4 pos : POSITION; float2 uv : TEXCOORD0; };
VertInOut VSDefault(VertInOut vert_in)
{
    VertInOut vert_out;
    vert_out.pos = mul(float4(vert_in.pos.xyz, 1.0), ViewProj);
    vert_out.uv = vert_in.uv;
    return vert_out;
}
float4 PSDraw(VertInOut vert_in) : TARGET
{
    return image.Sample(def_sampler, view.xy + vert_in.uv * view.zw);
}
technique Draw { pass { vertex_shader = VSDefault(vert_in); pixel_shader = PSDraw(vert_in); } }
"#;

#[derive(Clone, Copy, PartialEq)]
struct Style {
    style: i64,
    size: f32,
    smoothing: f32,
    tilt: bool,
    idle: f32,
    clicks: i64,
    arrow_only: bool,
    /// 0xAABBGGRR
    color: u32,
    /// Draws the pointer (off: only zooms, the capture keeps its own).
    draw: bool,
    /// 0 off, 1 on clicks, 2 always (`ScreenZoomPointer`'s).
    zoom: i64,
    zoom_scale: f32,
    zoom_hold: f32,
    zoom_motion: f32,
    /// On clicks: typing zooms in too, looking where the last click was.
    zoom_typing: bool,
    /// Tells the badge by the real pointer when it's live and zoomed in.
    zoom_badge: bool,
}

/// The follow-zoom's camera (capture pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cam {
    scale: f32,
    scale_v: f32,
    center: (f32, f32),
    center_v: (f32, f32),
    /// Where it's heading (the pointer kept inside the view).
    aim: (f32, f32),
}

impl Cam {
    fn new(size: (f32, f32)) -> Cam {
        let c = (size.0 / 2.0, size.1 / 2.0);
        Cam { scale: 1.0, scale_v: 0.0, center: c, center_v: (0.0, 0.0), aim: c }
    }
}

/// Where the view heads: as it was while the pointer stays within its middle, else moved just
/// enough to bring it back in; kept inside the capture.
fn aim_at(aim: (f32, f32), pointer: (f32, f32), scale: f32, size: (f32, f32)) -> (f32, f32) {
    let scale = scale.max(1.0);
    let half = (size.0 / scale / 2.0, size.1 / scale / 2.0);
    // the middle 60% of the view
    let slack = (half.0 * 0.6, half.1 * 0.6);
    let follow = |a: f32, p: f32, s: f32| if p > a + s { p - s } else if p < a - s { p + s } else { a };
    let a = (follow(aim.0, pointer.0, slack.0), follow(aim.1, pointer.1, slack.1));
    (a.0.clamp(half.0, (size.0 - half.0).max(half.0)), a.1.clamp(half.1, (size.1 - half.1).max(half.1)))
}

/// The part of the capture shown (x, y, w, h in its pixels) by a camera at `center`, `scale`.
fn view_rect(center: (f32, f32), scale: f32, size: (f32, f32)) -> (f32, f32, f32, f32) {
    let scale = scale.max(1.0);
    let (w, h) = (size.0 / scale, size.1 / scale);
    let x = (center.0 - w / 2.0).clamp(0.0, size.0 - w);
    let y = (center.1 - h / 2.0).clamp(0.0, size.1 - h);
    (x, y, w, h)
}

/// One step of the camera towards `scale` and the pointer (capture pixels).
fn cam_step(cam: &mut Cam, want_scale: f32, pointer: (f32, f32), size: (f32, f32), motion: f32, dt: f32, snap_aim: bool) {
    cam.aim = if snap_aim { aim_at(pointer, pointer, want_scale, size) } else { aim_at(cam.aim, pointer, want_scale.max(cam.scale), size) };
    // when out, it heads back to the middle
    let aim = if want_scale <= 1.001 { (size.0 / 2.0, size.1 / 2.0) } else { cam.aim };
    let omega = 14.0 - 10.0 * motion.clamp(0.0, 1.0);
    (cam.scale, cam.scale_v) = spring(cam.scale, cam.scale_v, want_scale, omega, dt);
    let (x, vx) = spring(cam.center.0, cam.center_v.0, aim.0, omega, dt);
    let (y, vy) = spring(cam.center.1, cam.center_v.1, aim.1, omega, dt);
    cam.center = (x, y);
    cam.center_v = (vx, vy);
}

/// The textures, made on the graphics thread when first drawn (and again when the colour changes).
struct Textures {
    effect: *mut sys::gs_effect_t,
    image: *mut sys::gs_eparam_t,
    opacity: *mut sys::gs_eparam_t,
    zoom: *mut sys::gs_effect_t,
    view: *mut sys::gs_eparam_t,
    arrow: (*mut sys::gs_texture_t, (f32, f32)),
    ibeam: (*mut sys::gs_texture_t, (f32, f32)),
    cross: (*mut sys::gs_texture_t, (f32, f32)),
    dot: *mut sys::gs_texture_t,
    ring: *mut sys::gs_texture_t,
    click_ring: *mut sys::gs_texture_t,
    click_disc: *mut sys::gs_texture_t,
    color: u32,
}

struct Filter {
    context: *mut sys::obs_source_t,
    look: Style,
    /// Followed position (display points), its velocity, the tilt.
    pos: Option<(f32, f32)>,
    vel: (f32, f32),
    tilt: f32,
    /// When the pointer last moved, and the opacity now.
    moved: Instant,
    last_target: (f32, f32),
    opacity: f32,
    shape: Shape,
    textures: Option<Textures>,
    /// The follow-zoom: its camera, until when it stays in, the newest click seen.
    cam: Option<Cam>,
    zoom_until: Option<Instant>,
    last_click: Option<Instant>,
    /// The newest key seen (typing zooms in too).
    last_typed: Option<Instant>,
    /// What the capture shows, in global display points (its display, or its window where it
    /// is now), and when that was last read.
    frame: Option<(f32, f32, f32, f32)>,
    frame_read: Option<Instant>,
    /// The system pointer's own picture, when it's a shape Lightair has no art for (a hand, a
    /// resize arrow, an app's own pointer), and its texture (made from the picture `serial`).
    picture: Option<system_pointer::Picture>,
    picture_tex: Option<(*mut sys::gs_texture_t, u64)>,
}

/// What a screen capture shows, in global display points: its display (by the display it's
/// set to), or its window (where it is now). `None`: the main display.
fn capture_frame(context: *mut sys::obs_source_t) -> Option<(f32, f32, f32, f32)> {
    let parent = unsafe { sys::obs_filter_get_parent(context) };
    if parent.is_null() {
        return None;
    }
    let id = unsafe { std::ffi::CStr::from_ptr(sys::obs_source_get_id(parent)) }.to_string_lossy().into_owned();
    let settings = unsafe { sys::obs_source_get_settings(parent) };
    if settings.is_null() {
        return None;
    }
    let text = |k: &std::ffi::CStr| {
        let p = unsafe { sys::obs_data_get_string(settings, k.as_ptr()) };
        if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    };
    let int = |k: &std::ffi::CStr| unsafe { sys::obs_data_get_int(settings, k.as_ptr()) };
    let frame = match id.as_str() {
        // macOS: a display (0), a window (1) or an app's windows on its display (2)
        "screen_capture" | "lightair_screen_capture" => match int(c"type") {
            1 => input::window_frame(&int(c"window").to_string()),
            _ => input::display_frame(&text(c"display_uuid")),
        },
        // Windows
        "window_capture" | "game_capture" => input::window_frame(&text(c"window")),
        _ => None,
    };
    unsafe { sys::obs_data_release(settings) };
    frame
}

// ------------------------------------------------------------------ the badge

/// Each zoomed-in filter on a live capture (with its badge on): its zoom, and when it said so
/// (by filter).
static LIVE_ZOOMS: std::sync::Mutex<Vec<(usize, f32, Instant)>> = std::sync::Mutex::new(Vec::new());

fn report_zoom(filter: usize, zoom: Option<f32>) {
    let mut all = LIVE_ZOOMS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|z| z.0 != filter);
    if let Some(scale) = zoom {
        all.push((filter, scale, Instant::now()));
    }
}

/// What viewers see now, for the badge by the real pointer (`lightair_platform::pointer_badge`):
/// the most a live capture is zoomed in; `None` when none is.
pub fn live_zoom() -> Option<f32> {
    let all = LIVE_ZOOMS.lock().unwrap_or_else(|e| e.into_inner());
    all.iter().filter(|z| z.2.elapsed() < Duration::from_millis(500) && z.1 > 1.02).map(|z| z.1).reduce(f32::max)
}

fn rgb_of(c: u32) -> [f32; 3] {
    [(c & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, ((c >> 16) & 0xFF) as f32 / 255.0]
}

unsafe fn texture(b: &Bitmap) -> *mut sys::gs_texture_t {
    let mut data = b.rgba.as_ptr();
    unsafe { sys::gs_texture_create(RES as u32, RES as u32, sys::GS_RGBA, 1, &mut data, 0) }
}

unsafe fn make_textures(color: u32) -> Option<Textures> {
    let code = CString::new(EFFECT).ok()?;
    let name = c"lightair-cursor.effect";
    let mut err: *mut c_char = std::ptr::null_mut();
    unsafe {
        let effect = sys::gs_effect_create(code.as_ptr(), name.as_ptr(), &mut err);
        if !err.is_null() {
            tracing::warn!(error = %std::ffi::CStr::from_ptr(err).to_string_lossy(), "pointer effect");
            sys::bfree(err as *mut c_void);
        }
        if effect.is_null() {
            return None;
        }
        let zcode = CString::new(ZOOM_EFFECT).ok()?;
        let mut zerr: *mut c_char = std::ptr::null_mut();
        let zoom = sys::gs_effect_create(zcode.as_ptr(), c"lightair-zoom.effect".as_ptr(), &mut zerr);
        if !zerr.is_null() {
            tracing::warn!(error = %std::ffi::CStr::from_ptr(zerr).to_string_lossy(), "zoom effect");
            sys::bfree(zerr as *mut c_void);
        }
        let (a, i, x) = (arrow(), ibeam(), crosshair());
        let rgb = rgb_of(color);
        Some(Textures {
            effect,
            image: sys::gs_effect_get_param_by_name(effect, c"image".as_ptr()),
            opacity: sys::gs_effect_get_param_by_name(effect, c"opacity".as_ptr()),
            view: if zoom.is_null() { std::ptr::null_mut() } else { sys::gs_effect_get_param_by_name(zoom, c"view".as_ptr()) },
            zoom,
            arrow: (texture(&a), a.hot),
            ibeam: (texture(&i), i.hot),
            cross: (texture(&x), x.hot),
            dot: texture(&dot(rgb)),
            ring: texture(&ring(rgb, 0.36, 0.08)),
            click_ring: texture(&ring([1.0, 1.0, 1.0], 0.44, 0.05)),
            click_disc: texture(&disc(rgb)),
            color,
        })
    }
}

unsafe fn free_textures(t: Textures) {
    unsafe {
        for tex in [t.arrow.0, t.ibeam.0, t.cross.0, t.dot, t.ring, t.click_ring, t.click_disc] {
            if !tex.is_null() {
                sys::gs_texture_destroy(tex);
            }
        }
        sys::gs_effect_destroy(t.effect);
        if !t.zoom.is_null() {
            sys::gs_effect_destroy(t.zoom);
        }
    }
}

unsafe fn read_look(settings: *mut sys::obs_data_t) -> Style {
    let int = |k: &std::ffi::CStr| unsafe { sys::obs_data_get_int(settings, k.as_ptr()) };
    let float = |k: &std::ffi::CStr| unsafe { sys::obs_data_get_double(settings, k.as_ptr()) } as f32;
    let flag = |k: &std::ffi::CStr| unsafe { sys::obs_data_get_bool(settings, k.as_ptr()) };
    Style {
        style: int(c"style"),
        size: float(c"size").clamp(0.25, 6.0),
        smoothing: float(c"smoothing").clamp(0.0, 1.0),
        tilt: flag(c"tilt"),
        idle: float(c"idle_s").max(0.0),
        clicks: int(c"clicks"),
        arrow_only: flag(c"arrow_only"),
        color: int(c"color") as u32,
        draw: flag(c"draw"),
        zoom: int(c"zoom"),
        zoom_scale: float(c"zoom_scale").clamp(1.0, 6.0),
        zoom_hold: float(c"zoom_hold_s").max(0.2),
        zoom_motion: float(c"zoom_motion").clamp(0.0, 1.0),
        zoom_typing: flag(c"zoom_typing"),
        zoom_badge: flag(c"zoom_badge"),
    }
}

unsafe extern "C" fn get_name(_: *mut c_void) -> *const c_char {
    c"Drawn pointer".as_ptr()
}

unsafe extern "C" fn get_defaults(settings: *mut sys::obs_data_t) {
    unsafe {
        sys::obs_data_set_default_double(settings, c"size".as_ptr(), 1.5);
        sys::obs_data_set_default_double(settings, c"smoothing".as_ptr(), 0.5);
        sys::obs_data_set_default_bool(settings, c"tilt".as_ptr(), true);
        sys::obs_data_set_default_double(settings, c"idle_s".as_ptr(), 3.0);
        sys::obs_data_set_default_int(settings, c"clicks".as_ptr(), 2);
        sys::obs_data_set_default_int(settings, c"color".as_ptr(), 0xFFFF_9000);
        sys::obs_data_set_default_bool(settings, c"draw".as_ptr(), true);
        sys::obs_data_set_default_double(settings, c"zoom_scale".as_ptr(), 2.0);
        sys::obs_data_set_default_double(settings, c"zoom_hold_s".as_ptr(), 2.0);
        sys::obs_data_set_default_double(settings, c"zoom_motion".as_ptr(), 0.5);
        sys::obs_data_set_default_bool(settings, c"zoom_typing".as_ptr(), true);
        sys::obs_data_set_default_bool(settings, c"zoom_badge".as_ptr(), true);
    }
}

unsafe extern "C" fn create(settings: *mut sys::obs_data_t, context: *mut sys::obs_source_t) -> *mut c_void {
    clicks_join();
    let f = Filter {
        context,
        look: unsafe { read_look(settings) },
        pos: None,
        vel: (0.0, 0.0),
        tilt: 0.0,
        moved: Instant::now(),
        last_target: (f32::NAN, f32::NAN),
        opacity: 1.0,
        shape: Shape::Arrow,
        textures: None,
        cam: None,
        zoom_until: None,
        last_click: None,
        last_typed: None,
        frame: None,
        frame_read: None,
        picture: None,
        picture_tex: None,
    };
    Box::into_raw(Box::new(f)) as *mut c_void
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    let f = unsafe { Box::from_raw(data as *mut Filter) };
    report_zoom(f.context as usize, None);
    if f.textures.is_some() || f.picture_tex.is_some() {
        unsafe {
            sys::obs_enter_graphics();
            if let Some(t) = f.textures {
                free_textures(t);
            }
            if let Some((tex, _)) = f.picture_tex {
                sys::gs_texture_destroy(tex);
            }
            sys::obs_leave_graphics();
        }
    }
    clicks_leave();
}

unsafe extern "C" fn update(data: *mut c_void, settings: *mut sys::obs_data_t) {
    let f = unsafe { &mut *(data as *mut Filter) };
    f.look = unsafe { read_look(settings) };
}

/// One step of a critically damped spring towards `target` (implicit, stable at any `dt`).
fn spring(x: f32, v: f32, target: f32, omega: f32, dt: f32) -> (f32, f32) {
    let f = 1.0 + 2.0 * dt * omega;
    let oo = omega * omega;
    let (hoo, hhoo) = (dt * oo, dt * dt * oo);
    let inv = 1.0 / (f + hhoo);
    ((f * x + dt * v + hhoo * target) * inv, (v + hoo * (target - x)) * inv)
}

unsafe extern "C" fn tick(data: *mut c_void, seconds: f32) {
    let f = unsafe { &mut *(data as *mut Filter) };
    let dt = seconds.clamp(1e-4, 0.1);
    let target = input::pointer();
    if target != f.last_target {
        f.last_target = target;
        f.moved = Instant::now();
    }
    // smoothing 0: stiff (follows at once); 1: a slow glide
    let omega = 60.0 - 52.0 * f.look.smoothing;
    let (x, y) = f.pos.unwrap_or(target);
    let (nx, vx) = spring(x, f.vel.0, target.0, omega, dt);
    let (ny, vy) = spring(y, f.vel.1, target.1, omega, dt);
    f.pos = Some((nx, ny));
    f.vel = (vx, vy);
    let want_tilt = if f.look.tilt { (vx * 0.012).clamp(-14.0, 14.0) } else { 0.0 };
    f.tilt += (want_tilt - f.tilt) * (dt * 12.0).min(1.0);
    let idle = f.look.idle > 0.0 && f.moved.elapsed().as_secs_f32() > f.look.idle;
    let want = if idle { 0.0 } else { 1.0 };
    f.opacity += (want - f.opacity) * (dt * 8.0).min(1.0);
    f.shape = if f.look.arrow_only { Shape::Arrow } else { input::shape() };
    f.picture = if f.look.arrow_only {
        None
    } else {
        system_pointer::now().filter(|p| !matches!(p.shape, Some(Shape::Arrow | Shape::IBeam | Shape::Crosshair)))
    };
    // where the capture is (a window moves): read a few times a second
    if f.frame_read.is_none_or(|t| t.elapsed() > Duration::from_millis(250)) {
        f.frame = capture_frame(f.context);
        f.frame_read = Some(Instant::now());
    }
    zoom_tick(f, dt);
    // what viewers see, for the badge: zoomed in, on a capture that's live
    let parent = unsafe { sys::obs_filter_get_parent(f.context) };
    let live = !parent.is_null() && unsafe { sys::obs_source_active(parent) };
    match f.cam.filter(|_| live && f.look.zoom_badge) {
        Some(cam) => report_zoom(f.context as usize, Some(cam.scale)),
        None => report_zoom(f.context as usize, None),
    }
}

/// The follow-zoom, a step on.
fn zoom_tick(f: &mut Filter, dt: f32) {
    let target = unsafe { sys::obs_filter_get_target(f.context) };
    let size = if target.is_null() { (0, 0) } else { unsafe { (sys::obs_source_get_base_width(target), sys::obs_source_get_base_height(target)) } };
    // toggle: in, following, while the shortcut has it on; else out
    let mode = match f.look.zoom {
        3 if following() => 2,
        3 => 1,
        m => m,
    };
    if mode == 0 || size.0 == 0 || size.1 == 0 {
        f.cam = None;
        f.zoom_until = None;
        return;
    }
    let size = (size.0 as f32, size.1 as f32);
    let (dx, dy, dw, _) = f.frame.unwrap_or_else(input::main_display);
    let k = if dw > 0.0 { size.0 / dw } else { 1.0 };
    let Some((px, py)) = f.pos else { return };
    let pointer = ((px - dx) * k, (py - dy) * k);
    // a new click: in (again) for a while, looking at it
    let newest = recent_clicks().last().map(|c| c.0);
    let clicked = newest.is_some_and(|n| f.last_click.is_none_or(|l| n > l));
    if clicked {
        f.last_click = newest;
    }
    let now = Instant::now();
    let was_in = f.zoom_until.is_some_and(|u| u > now);
    if clicked {
        f.zoom_until = Some(now + Duration::from_secs_f32(f.look.zoom_hold));
    }
    // typing (on clicks): in as well, looking where the last click went (the text field), and
    // staying there while the keys keep coming
    let (typed, click_at) = if mode == 1 && f.look.zoom_typing { typing() } else { (None, None) };
    let new_key = typed.is_some_and(|t| f.last_typed.is_none_or(|l| t > l));
    if new_key {
        f.last_typed = typed;
        f.zoom_until = Some(now + Duration::from_secs_f32(f.look.zoom_hold));
    }
    let typing_now = typed.is_some_and(|t| now.duration_since(t) < Duration::from_secs_f32(f.look.zoom_hold));
    let focus = match click_at.filter(|_| typing_now && !clicked) {
        Some((cx_, cy_)) => ((cx_ - dx) * k, (cy_ - dy) * k),
        None => pointer,
    };
    let zoomed = match (mode, f.look.zoom) {
        (2, _) => true,
        // toggled out: clicks don't zoom
        (_, 3) => false,
        _ => f.zoom_until.is_some_and(|u| u > now),
    };
    let want = if zoomed { f.look.zoom_scale } else { 1.0 };
    let cam = f.cam.get_or_insert_with(|| Cam::new(size));
    cam_step(cam, want, focus, size, f.look.zoom_motion, dt, (clicked || new_key) && !was_in && mode == 1);
}


/// Draws `tex` with its hot spot at `at`, `size` pixels square, turned by `angle` degrees.
unsafe fn draw(t: &Textures, tex: *mut sys::gs_texture_t, at: (f32, f32), size: f32, hot: (f32, f32), angle: f32, opacity: f32) {
    if tex.is_null() || opacity <= 0.001 {
        return;
    }
    unsafe {
        sys::gs_effect_set_texture(t.image, tex);
        sys::gs_effect_set_float(t.opacity, opacity);
        sys::gs_matrix_push();
        sys::gs_matrix_translate3f(at.0, at.1, 0.0);
        sys::gs_matrix_rotaa4f(0.0, 0.0, 1.0, angle.to_radians());
        sys::gs_matrix_translate3f(-hot.0 * size, -hot.1 * size, 0.0);
        sys::gs_draw_sprite(tex, 0, size as u32, size as u32);
        sys::gs_matrix_pop();
    }
}

unsafe extern "C" fn render(data: *mut c_void, _: *mut sys::gs_effect_t) {
    let f = unsafe { &mut *(data as *mut Filter) };
    let target = unsafe { sys::obs_filter_get_target(f.context) };
    let (tw, th) = if target.is_null() { (0, 0) } else { unsafe { (sys::obs_source_get_base_width(target), sys::obs_source_get_base_height(target)) } };
    if f.textures.as_ref().is_some_and(|t| t.color != f.look.color)
        && let Some(old) = f.textures.take()
    {
        unsafe { free_textures(old) };
    }
    if f.textures.is_none() {
        f.textures = unsafe { make_textures(f.look.color) };
    }
    // the capture: zoomed, or as it is
    let size_px = (tw as f32, th as f32);
    let zoomable = f.textures.as_ref().is_some_and(|t| !t.zoom.is_null());
    let view = f.cam.filter(|c| zoomable && c.scale > 1.001 && tw > 0 && th > 0).map(|c| view_rect(c.center, c.scale, size_px));
    match (view, f.textures.as_ref()) {
        (Some((vx, vy, vw, vh)), Some(t)) => unsafe {
            if sys::obs_source_process_filter_begin(f.context, sys::GS_RGBA, sys::OBS_ALLOW_DIRECT_RENDERING) {
                let v = sys::vec4 { __bindgen_anon_1: sys::vec4__bindgen_ty_1 { ptr: [vx / size_px.0, vy / size_px.1, vw / size_px.0, vh / size_px.1] } };
                sys::gs_effect_set_vec4(t.view, &v);
                sys::obs_source_process_filter_end(f.context, t.zoom, tw, th);
            }
        },
        _ => unsafe { sys::obs_source_skip_video_filter(f.context) },
    }
    let Some((px, py)) = f.pos else { return };
    if tw == 0 || th == 0 || !f.look.draw {
        return;
    }
    // the system pointer's picture, as a texture when it changes
    if let Some(p) = &f.picture
        && f.picture_tex.is_none_or(|(_, serial)| serial != p.serial)
    {
        unsafe {
            if let Some((old, _)) = f.picture_tex.take() {
                sys::gs_texture_destroy(old);
            }
            let side = system_pointer::SIDE as u32;
            let mut data = p.rgba.as_ptr();
            let tex = sys::gs_texture_create(side, side, sys::GS_RGBA, 1, &mut data, 0);
            if !tex.is_null() {
                f.picture_tex = Some((tex, p.serial));
            }
        }
    }
    let Some(t) = &f.textures else { return };
    // display points → the capture's pixels → the view's
    let (dx, dy, dw, _) = f.frame.unwrap_or_else(input::main_display);
    let scale = if dw > 0.0 { tw as f32 / dw } else { 1.0 };
    let (vx, vy, vk) = match view {
        Some((vx, vy, vw, _)) => (vx, vy, size_px.0 / vw),
        None => (0.0, 0.0, 1.0),
    };
    let place = |x: f32, y: f32| (((x - dx) * scale - vx) * vk, ((y - dy) * scale - vy) * vk);
    let (x, y) = place(px, py);
    let size = POINTS * f.look.size * scale * vk;
    unsafe {
        sys::gs_blend_state_push();
        sys::gs_enable_blending(true);
        sys::gs_blend_function(sys::GS_BLEND_ONE, sys::GS_BLEND_INVSRCALPHA);
        let tech = sys::gs_effect_get_technique(t.effect, c"Draw".as_ptr());
        sys::gs_technique_begin(tech);
        sys::gs_technique_begin_pass(tech, 0);
        // clicks under the pointer
        if f.look.clicks > 0 {
            let now = Instant::now();
            for (at, cx, cy) in recent_clicks() {
                let k = (now.duration_since(at).as_secs_f32() / CLICK.as_secs_f32()).clamp(0.0, 1.0);
                let (cx, cy) = place(cx, cy);
                let (tex, grow, fade) = if f.look.clicks == 1 { (t.click_disc, 0.9 + 0.3 * k, 0.45) } else { (t.click_ring, 0.6 + 1.4 * k, 0.9) };
                draw(t, tex, (cx, cy), size * 1.6 * grow, (0.5, 0.5), 0.0, fade * (1.0 - k) * f.opacity);
            }
        }
        let system = f.picture.as_ref().zip(f.picture_tex).filter(|(p, (_, serial))| p.serial == *serial);
        let (tex, hot, angle) = match (f.look.style, f.shape) {
            (1, _) => (t.dot, (0.5, 0.5), 0.0),
            (2, _) => (t.ring, (0.5, 0.5), 0.0),
            (_, _) if let Some((p, (tex, _))) = system => (tex, p.hot, 0.0),
            (_, Shape::IBeam) => (t.ibeam.0, t.ibeam.1, 0.0),
            (_, Shape::Crosshair) => (t.cross.0, t.cross.1, 0.0),
            _ => (t.arrow.0, t.arrow.1, f.tilt),
        };
        draw(t, tex, (x, y), size, hot, angle, f.opacity);
        sys::gs_technique_end_pass(tech);
        sys::gs_technique_end(tech);
        sys::gs_blend_state_pop();
    }
}

/// Registers the filter with libobs (once; after startup).
pub fn register() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        let mut info: sys::obs_source_info = std::mem::zeroed();
        info.id = c"lightair_pointer".as_ptr();
        info.type_ = sys::OBS_SOURCE_TYPE_FILTER;
        info.output_flags = sys::OBS_SOURCE_VIDEO;
        info.get_name = Some(get_name);
        info.create = Some(create);
        info.destroy = Some(destroy);
        info.update = Some(update);
        info.get_defaults = Some(get_defaults);
        info.video_tick = Some(tick);
        info.video_render = Some(render);
        sys::obs_register_source_s(&info, std::mem::size_of::<sys::obs_source_info>());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spring_settles_without_overshoot() {
        let (mut x, mut v) = (0.0f32, 0.0f32);
        let mut max = 0.0f32;
        for _ in 0..240 {
            (x, v) = spring(x, v, 100.0, 20.0, 1.0 / 60.0);
            max = max.max(x);
        }
        assert!((x - 100.0).abs() < 0.5, "{x}");
        assert!(max <= 100.01, "critically damped: no overshoot ({max})");
    }

    #[test]
    fn the_zoom_keeps_the_pointer_in_view_and_the_view_on_the_capture() {
        let size = (1920.0, 1080.0);
        // in the middle, a small move doesn't move it; a big one brings the pointer back in
        let aim = (960.0, 540.0);
        assert_eq!(aim_at(aim, (1000.0, 560.0), 2.0, size), aim);
        let moved = aim_at(aim, (1500.0, 540.0), 2.0, size);
        let (x, _, w, _) = view_rect(moved, 2.0, size);
        assert!(1500.0 > x && 1500.0 < x + w, "the pointer is in the view: {moved:?}");
        // at the corner it stops at the edge
        let (x, y, w, h) = view_rect(aim_at(aim, (0.0, 0.0), 2.0, size), 2.0, size);
        assert_eq!((x, y, w, h), (0.0, 0.0, 960.0, 540.0));
        let (x, y, ..) = view_rect((1900.0, 1070.0), 3.0, size);
        assert_eq!((x, y), (1920.0 - 640.0, 1080.0 - 360.0));
    }

    #[test]
    fn the_zoom_goes_in_and_comes_back_out() {
        let size = (1920.0, 1080.0);
        let mut cam = Cam::new(size);
        let step = 1.0 / 60.0;
        cam_step(&mut cam, 2.0, (1600.0, 300.0), size, 0.5, step, true);
        for _ in 0..120 {
            cam_step(&mut cam, 2.0, (1600.0, 300.0), size, 0.5, step, false);
        }
        assert!((cam.scale - 2.0).abs() < 0.02, "in: {cam:?}");
        let (x, y, w, h) = view_rect(cam.center, cam.scale, size);
        assert!(1600.0 > x && 1600.0 < x + w && 300.0 > y && 300.0 < y + h, "looking at it: {cam:?}");
        for _ in 0..180 {
            cam_step(&mut cam, 1.0, (1600.0, 300.0), size, 0.5, step, false);
        }
        assert!((cam.scale - 1.0).abs() < 0.02, "out again: {cam:?}");
    }

    #[test]
    fn shapes_are_drawn_with_their_tip_at_the_hot_spot() {
        let a = arrow();
        // the tip pixel is dark (edge), the body white, the far corner empty
        let px = |x: f32, y: f32| {
            let i = (((y * RES as f32) as usize) * RES + (x * RES as f32) as usize) * 4;
            [a.rgba[i], a.rgba[i + 1], a.rgba[i + 2], a.rgba[i + 3]]
        };
        assert!(px(a.hot.0 + 0.01, a.hot.1 + 0.02)[3] > 0, "something at the tip");
        assert_eq!(px(0.2, 0.4), [255, 255, 255, 255], "white body");
        assert_eq!(px(0.95, 0.1)[3], 0, "nothing far from it");
    }
}
