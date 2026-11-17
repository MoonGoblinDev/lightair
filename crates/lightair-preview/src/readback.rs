//! The portable preview path (Windows today; any platform without a zero-copy one):
//!
//! 1. libobs renders the canvas, scaled to the preview size, into its own BGRA render target
//!    (the same drawing as every backend, `crate::render`).
//! 2. The target is staged into one of two staging surfaces, and the *previous* one is mapped
//!    and copied out, so the graphics thread never waits for the GPU.
//! 3. The UI shows the newest copy as an image.
//!
//! One frame of extra latency and a copy per frame. On Windows the window's canvas preview
//! doesn't go this way: each frame is presented into a composition swap chain under the UI
//! (`Layer`, `Preview::set_layer`), and frames are read back only while something asks for them
//! (`latest_frame`: thumbnails, screenshots, the colour picker).

use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use lightair_engine::sys;
use parking_lot::Mutex;

use crate::render::{draw_scaled, draw_with};

/// A finished preview frame: BGRA pixels, top row first.
pub struct CpuFrame {
    pub width: usize,
    pub height: usize,
    /// `width * 4` bytes per row.
    pub bgra: Vec<u8>,
}

pub type Frame = Arc<CpuFrame>;

const STAGES: usize = 2;

struct Pool {
    width: u32,
    height: u32,
    target: *mut sys::gs_texture_t,
    stages: [*mut sys::gs_stagesurf_t; STAGES],
    /// The stage written last frame (to map now), if any.
    pending: Option<usize>,
    next: usize,
}

// SAFETY: the libobs handles are only used on libobs's graphics thread.
unsafe impl Send for Pool {}

impl Pool {
    /// Must run inside the libobs graphics context.
    unsafe fn new(width: u32, height: u32) -> Option<Self> {
        unsafe {
            let target = sys::gs_texture_create(width, height, sys::GS_BGRA, 1, ptr::null_mut(), sys::GS_RENDER_TARGET);
            if target.is_null() {
                return None;
            }
            let mut stages = [ptr::null_mut(); STAGES];
            for s in &mut stages {
                *s = sys::gs_stagesurface_create(width, height, sys::GS_BGRA);
                if s.is_null() {
                    let mut pool = Self { width, height, target, stages, pending: None, next: 0 };
                    pool.destroy();
                    return None;
                }
            }
            Some(Self { width, height, target, stages, pending: None, next: 0 })
        }
    }

    unsafe fn destroy(&mut self) {
        unsafe {
            for s in &mut self.stages {
                if !s.is_null() {
                    sys::gs_stagesurface_destroy(*s);
                }
                *s = ptr::null_mut();
            }
            if !self.target.is_null() {
                sys::gs_texture_destroy(self.target);
            }
        }
        self.target = ptr::null_mut();
    }

    /// Stages this frame and copies out the last one.
    unsafe fn stage_and_read(&mut self) -> Option<CpuFrame> {
        unsafe {
            let read = self.pending.take().and_then(|i| self.copy_out(self.stages[i]));
            sys::gs_stage_texture(self.stages[self.next], self.target);
            self.pending = Some(self.next);
            self.next = (self.next + 1) % STAGES;
            read
        }
    }

    unsafe fn copy_out(&self, stage: *mut sys::gs_stagesurf_t) -> Option<CpuFrame> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut data: *mut u8 = ptr::null_mut();
        let mut linesize: u32 = 0;
        unsafe {
            if !sys::gs_stagesurface_map(stage, &mut data, &mut linesize) || data.is_null() {
                return None;
            }
            let mut bgra = vec![0u8; w * h * 4];
            for row in 0..h {
                let src = std::slice::from_raw_parts(data.add(row * linesize as usize), w * 4);
                bgra[row * w * 4..(row + 1) * w * 4].copy_from_slice(src);
            }
            sys::gs_stagesurface_unmap(stage);
            Some(CpuFrame { width: w, height: h, bgra })
        }
    }
}

struct Shared {
    pool: Mutex<Option<Pool>>,
    latest: Mutex<Option<Frame>>,
    /// Requested size, packed as (width << 32 | height). 0 = follow the canvas size.
    size: AtomicU64,
    active: AtomicBool,
    frames: AtomicU64,
    canvas: Option<CanvasRef>,
    source: Mutex<Option<SourceRef>>,
    /// Draw the source at its own size (a player), not in the canvas's coordinates.
    own_size: AtomicBool,
    /// Windows: the swap chain each frame is presented to (the window's picture).
    #[cfg(windows)]
    layer: Mutex<Option<Arc<crate::Layer>>>,
    /// When `latest_frame` was last asked (ms since `EPOCH`): with a layer, frames are read back
    /// only while something reads them.
    asked: AtomicU64,
}

/// Frames are read back while asked for within this long.
const ASKED_FOR_MS: u64 = 2000;

fn now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

struct CanvasRef(*mut sys::obs_canvas_t);

/// A strong, showing reference to a source.
struct SourceRef(*mut sys::obs_source_t);

impl Drop for SourceRef {
    fn drop(&mut self) {
        unsafe {
            sys::obs_source_dec_showing(self.0);
            sys::obs_source_release(self.0);
        }
    }
}

// SAFETY: used by libobs's graphics thread and the UI thread; every field is atomic or behind a
// mutex, and the libobs pointers are only dereferenced on the graphics thread.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// Handle to a preview of a canvas. Cheap to clone; rendering stops when the last clone drops.
#[derive(Clone)]
pub struct Preview(Arc<Handle>);

struct Handle {
    shared: Arc<Shared>,
}

impl Preview {
    /// Starts rendering the main canvas. libobs must be running.
    pub fn attach() -> Self {
        Self::attach_to(None)
    }

    /// Previews another canvas (an additional format); takes ownership of the strong reference.
    pub fn attach_canvas(canvas: *mut sys::obs_canvas_t) -> Self {
        Self::attach_to((!canvas.is_null()).then_some(CanvasRef(canvas)))
    }

    fn attach_to(canvas: Option<CanvasRef>) -> Self {
        let shared = Arc::new(Shared {
            pool: Mutex::new(None),
            latest: Mutex::new(None),
            size: AtomicU64::new(0),
            active: AtomicBool::new(true),
            frames: AtomicU64::new(0),
            canvas,
            source: Mutex::new(None),
            own_size: AtomicBool::new(false),
            #[cfg(windows)]
            layer: Mutex::new(None),
            asked: AtomicU64::new(0),
        });
        unsafe { sys::obs_add_main_rendered_callback(Some(on_rendered), Arc::as_ptr(&shared) as *mut c_void) };
        Self(Arc::new(Handle { shared }))
    }

    pub fn set_size(&self, size: Option<(u32, u32)>) {
        let packed = size.map_or(0, |(w, h)| (((w.max(2) & !1) as u64) << 32) | (h.max(2) & !1) as u64);
        self.0.shared.size.store(packed, Ordering::Relaxed);
    }

    pub fn set_source(&self, source: Option<*mut sys::obs_source_t>) {
        let new = source.filter(|p| !p.is_null()).map(|p| {
            unsafe { sys::obs_source_inc_showing(p) };
            SourceRef(p)
        });
        let old = std::mem::replace(&mut *self.0.shared.source.lock(), new);
        drop(old);
    }

    /// Draws the source at its own size (a player: the whole picture, whatever the canvas),
    /// rather than where it sits on the canvas.
    /// Presents each frame to `layer` too (Windows: a `Layer`, as `Arc::as_ptr` gives it; a
    /// reference is taken here), or stops with `None`. Elsewhere the UI draws the frames.
    pub fn set_layer(&self, layer: Option<*mut std::ffi::c_void>) {
        #[cfg(windows)]
        {
            let new = layer.filter(|l| !l.is_null()).map(|l| unsafe {
                let l = l as *const crate::Layer;
                Arc::increment_strong_count(l);
                Arc::from_raw(l)
            });
            let old = std::mem::replace(&mut *self.0.shared.layer.lock(), new);
            drop(old);
        }
        #[cfg(not(windows))]
        let _ = layer;
    }

    pub fn set_own_size(&self, own: bool) {
        self.0.shared.own_size.store(own, Ordering::Relaxed);
    }

    pub fn source(&self) -> Option<*mut sys::obs_source_t> {
        self.0.shared.source.lock().as_ref().map(|s| s.0)
    }

    pub fn set_active(&self, active: bool) {
        self.0.shared.active.store(active, Ordering::Relaxed);
    }

    pub fn latest_frame(&self) -> Option<Frame> {
        let shared = &self.0.shared;
        let now = now_ms();
        let asked = shared.asked.swap(now, Ordering::Relaxed);
        // presented to a layer and not read back lately: what's kept is stale
        if shared.has_layer() && now.saturating_sub(asked) > ASKED_FOR_MS {
            *shared.latest.lock() = None;
            return None;
        }
        shared.latest.lock().clone()
    }

    pub fn frames(&self) -> u64 {
        self.0.shared.frames.load(Ordering::Relaxed)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            sys::obs_remove_main_rendered_callback(Some(on_rendered), Arc::as_ptr(&self.shared) as *mut c_void);
            if let Some(mut pool) = self.shared.pool.lock().take() {
                sys::obs_enter_graphics();
                pool.destroy();
                sys::obs_leave_graphics();
            }
            if let Some(c) = &self.shared.canvas {
                sys::obs_canvas_release(c.0);
            }
            self.shared.source.lock().take();
        }
    }
}

impl Shared {
    fn has_layer(&self) -> bool {
        #[cfg(windows)]
        return self.layer.lock().is_some();
        #[cfg(not(windows))]
        return false;
    }
}

unsafe extern "C" fn on_rendered(param: *mut c_void) {
    let shared = unsafe { &*(param as *const Shared) };
    if !shared.active.load(Ordering::Relaxed) {
        return;
    }
    let main = unsafe { sys::obs_get_main_texture() };
    if main.is_null() {
        return;
    }
    let (base_w, base_h) = match &shared.canvas {
        None => unsafe { (sys::gs_texture_get_width(main), sys::gs_texture_get_height(main)) },
        Some(c) => {
            let mut ovi = sys::obs_video_info::default();
            if !unsafe { sys::obs_canvas_get_video_info(c.0, &mut ovi) } {
                return;
            }
            (ovi.base_width, ovi.base_height)
        }
    };
    #[cfg(windows)]
    let layer = shared.layer.lock().clone();
    let packed = shared.size.load(Ordering::Relaxed);
    let (w, h) = if packed == 0 { (base_w & !1, base_h & !1) } else { ((packed >> 32) as u32, packed as u32) };
    // shown smaller than the canvas: rendered at the size it's shown (zoomed in: the canvas's own)
    #[cfg(windows)]
    let (w, h) = match layer.as_ref().and_then(|l| l.shown_at()) {
        Some((sw, sh)) if packed == 0 && sw < w && sh < h => (sw, sh),
        _ => (w, h),
    };
    let asked = now_ms().saturating_sub(shared.asked.load(Ordering::Relaxed)) <= ASKED_FOR_MS;
    // the program as it is, at its own size, presented straight from libobs's texture
    #[cfg(windows)]
    if let Some(layer) = &layer
        && !asked
        && shared.canvas.is_none()
        && (base_w, base_h) == (w, h)
        && shared.source.lock().is_none()
        && unsafe { layer.can_present(main) }
    {
        if unsafe { layer.present(main, w, h) } {
            shared.frames.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(pool) = shared.pool.lock().as_mut() {
            pool.pending = None;
        }
        return;
    }
    let mut guard = shared.pool.lock();
    if guard.as_ref().is_none_or(|p| p.width != w || p.height != h) {
        if let Some(mut old) = guard.take() {
            unsafe { old.destroy() };
        }
        *guard = unsafe { Pool::new(w, h) };
    }
    let Some(pool) = guard.as_mut() else { return };
    let source = shared.source.lock();
    match (&*source, &shared.canvas) {
        (Some(src), _) => unsafe {
            let (sw, sh) = (sys::obs_source_get_width(src.0), sys::obs_source_get_height(src.0));
            let (cw, ch) =
                if shared.own_size.load(Ordering::Relaxed) && sw > 0 && sh > 0 { (sw, sh) } else { (base_w, base_h) };
            draw_with(cw, ch, pool.target, w, h, || sys::obs_source_video_render(src.0))
        },
        (None, None) => unsafe { draw_scaled(main, base_w, base_h, pool.target, w, h) },
        (None, Some(c)) => unsafe { draw_with(base_w, base_h, pool.target, w, h, || sys::obs_canvas_render(c.0)) },
    }
    drop(source);
    #[cfg(windows)]
    let presented = match &layer {
        Some(layer) => {
            if unsafe { layer.present(pool.target, w, h) } {
                shared.frames.fetch_add(1, Ordering::Relaxed);
            }
            true
        }
        None => false,
    };
    #[cfg(not(windows))]
    let presented = false;
    if presented && !asked {
        pool.pending = None;
        return;
    }
    if let Some(frame) = unsafe { pool.stage_and_read() } {
        *shared.latest.lock() = Some(Arc::new(frame));
        if !presented {
            shared.frames.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A frame's pixel size.
pub fn frame_size(frame: &Frame) -> (usize, usize) {
    (frame.width, frame.height)
}

/// A frame shrunk to at most `max_width` pixels wide, as RGBA (thumbnails).
pub fn frame_rgba(frame: &Frame, max_width: usize) -> Option<(usize, usize, Vec<u8>)> {
    let (w, h) = (frame.width, frame.height);
    if w == 0 || h == 0 {
        return None;
    }
    let step = w.div_ceil(max_width.max(1)).max(1);
    let (ow, oh) = (w / step, h / step);
    let mut rgba = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        for ox in 0..ow {
            let i = (oy * step * w + ox * step) * 4;
            rgba.extend_from_slice(&[frame.bgra[i + 2], frame.bgra[i + 1], frame.bgra[i], 255]);
        }
    }
    Some((ow, oh, rgba))
}

/// The (Y, Cb, Cr) values (full-range BT.601, as the macOS path produces) at a point given as
/// fractions of the frame's size (diagnostics and tests).
pub fn sample_ycbcr(frame: &Frame, fx: f32, fy: f32) -> Option<(u8, u8, u8)> {
    let (w, h) = (frame.width, frame.height);
    if w == 0 || h == 0 {
        return None;
    }
    let (x, y) = (((w as f32 * fx) as usize).min(w - 1), ((h as f32 * fy) as usize).min(h - 1));
    let i = (y * w + x) * 4;
    let (b, g, r) = (frame.bgra[i] as f32, frame.bgra[i + 1] as f32, frame.bgra[i + 2] as f32);
    let yv = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = 128.0 + (b - yv) * 0.564;
    let cr = 128.0 + (r - yv) * 0.713;
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Some((q(yv), q(cb), q(cr)))
}
