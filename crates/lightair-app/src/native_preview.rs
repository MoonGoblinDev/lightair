//! The canvas preview drawn by Core Animation, under the window's UI (macOS).
//!
//! GPUI redraws its whole window whenever anything in it moves, so a live picture drawn by GPUI
//! cost a full redraw 60 times a second. Instead a view of Lightair's own sits below GPUI's in
//! the window: its layer paints the window's background, the canvas well, and in it one layer per
//! canvas showing that canvas's preview frames (handed over by the preview straight from libobs's
//! graphics thread, `Preview::set_layer`). The window is transparent where nothing is painted,
//! and GPUI leaves the canvas well unpainted, so the picture shows through; everything GPUI draws
//! (selection, guides, menus, any panel over the canvas) is drawn over it as before.

#[cfg(target_os = "macos")]
pub use mac::{MeterPlace, NativePreview};
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::NativePreview;

/// The window's native layer, shared by what draws into it (the canvas, the mixer).
pub type Shared = std::rc::Rc<std::cell::RefCell<Option<NativePreview>>>;

/// Hides the pointer until it moves (the canvas full screen, as a video player does).
pub fn hide_cursor() {
    #[cfg(target_os = "macos")]
    mac::hide_cursor();
}

/// Hides the menu bar and the Dock outright (not just until the pointer reaches them: the
/// canvas full screen). Leaving full screen puts back what was there.
pub fn hide_menu_bar() {
    #[cfg(target_os = "macos")]
    mac::hide_menu_bar();
}

/// Whether the preview is drawn under the UI (macOS: Core Animation; Windows: DirectComposition,
/// `native_preview/win.rs`; `LIGHTAIR_GPUI_PREVIEW=1` draws it with GPUI as before).
pub fn on() -> bool {
    (cfg!(target_os = "macos") || cfg!(windows)) && std::env::var_os("LIGHTAIR_GPUI_PREVIEW").is_none()
}

#[cfg(target_os = "macos")]
mod mac {
use std::collections::HashMap;
use std::ffi::c_void;

use std::ffi::{CStr, c_char};

use gpui_kit::{Bounds, Hsla, Pixels, Window};
use lightair_core::Id;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

type Id_ = *mut c_void;

/// `[target sel]` → id
unsafe fn send(target: Id_, sel: &CStr) -> Id_ {
    let f: unsafe extern "C" fn(Id_, *mut c_void) -> Id_ = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr())) }
}

/// `[target sel: arg]` with a pointer-sized argument
unsafe fn send_p(target: Id_, sel: &CStr, arg: *mut c_void) -> Id_ {
    let f: unsafe extern "C" fn(Id_, *mut c_void, *mut c_void) -> Id_ = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr()), arg) }
}

unsafe fn send_bool(target: Id_, sel: &CStr, arg: bool) {
    let f: unsafe extern "C" fn(Id_, *mut c_void, bool) = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr()), arg) }
}

unsafe fn send_u64(target: Id_, sel: &CStr, arg: u64) {
    let f: unsafe extern "C" fn(Id_, *mut c_void, u64) = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr()), arg) }
}

unsafe fn send_rect(target: Id_, sel: &CStr, r: NSRect) -> Id_ {
    let f: unsafe extern "C" fn(Id_, *mut c_void, NSRect) -> Id_ = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr()), r) }
}

unsafe fn get_rect(target: Id_, sel: &CStr) -> NSRect {
    let f: unsafe extern "C" fn(Id_, *mut c_void) -> NSRect = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr())) }
}

/// `[content addSubview: view positioned: NSWindowBelow relativeTo: other]`
unsafe fn add_below(content: Id_, view: Id_, other: Id_) {
    let f: unsafe extern "C" fn(Id_, *mut c_void, Id_, isize, Id_) = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(content, sel_registerName(c"addSubview:positioned:relativeTo:".as_ptr()), view, -1, other) }
}

fn class(name: &CStr) -> Id_ {
    unsafe { objc_getClass(name.as_ptr()) }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> *mut c_void;
    fn CGColorRelease(c: *mut c_void);
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

fn rect(b: Bounds<Pixels>) -> NSRect {
    NSRect {
        x: f32::from(b.origin.x) as f64,
        y: f32::from(b.origin.y) as f64,
        w: f32::from(b.size.width).max(0.0) as f64,
        h: f32::from(b.size.height).max(0.0) as f64,
    }
}

unsafe fn set_colour(layer: Id_, colour: Hsla) {
    let c = colour.to_rgb();
    unsafe {
        let cg = CGColorCreateSRGB(c.r as f64, c.g as f64, c.b as f64, c.a as f64);
        send_p(layer, c"setBackgroundColor:", cg);
        CGColorRelease(cg);
    }
}

unsafe fn new_layer() -> Id_ {
    unsafe { send(class(c"CALayer"), c"new") }
}

pub(super) fn hide_menu_bar() {
    // NSApplicationPresentationHideDock | NSApplicationPresentationHideMenuBar
    unsafe { send_u64(send(class(c"NSApplication"), c"sharedApplication"), c"setPresentationOptions:", (1 << 1) | (1 << 3)) };
}

pub(super) fn hide_cursor() {
    unsafe { send_bool(class(c"NSCursor"), c"setHiddenUntilMouseMoves:", true) };
}

/// Changes layers without Core Animation's implicit animation.
fn quietly(f: impl FnOnce()) {
    unsafe {
        let tx = class(c"CATransaction");
        send(tx, c"begin");
        send_bool(tx, c"setDisableActions:", true);
        f();
        send(tx, c"commit");
    }
}

/// The well and each canvas's picture, as last placed.
type Placed = (Option<Bounds<Pixels>>, Vec<(Id, Bounds<Pixels>)>);

/// The view under the UI, the well's view in it, and each canvas's picture: its view and the
/// layer it hosts (which the preview shows frames in).
pub struct NativePreview {
    view: Id_,
    host: Id_,
    well: Id_,
    pictures: HashMap<Id, (Id_, Id_)>,
    /// Where things were last placed (shown again as they were when the canvas comes back).
    last: Placed,
    visible: bool,
    /// The mixer's meters: each strip's views, and what the meter thread draws.
    meters: HashMap<Id, MeterViews>,
    meter_jobs: std::sync::Arc<parking_lot::Mutex<HashMap<Id, MeterJob>>>,
    meters_shown: bool,
    meters_active: bool,
    meter_thread: bool,
    scale: f64,
}

impl NativePreview {
    /// Puts the view under GPUI's in `window` (which must be transparent), painting `background`
    /// everywhere and `well` where the canvas well is.
    pub fn attach(window: &Window, background: Hsla, well: Hsla) -> Option<Self> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
        let gpui_view = h.ns_view.as_ptr() as Id_;
        unsafe {
            let content = send(gpui_view, c"superview");
            if content.is_null() {
                return None;
            }
            let bounds = get_rect(content, c"bounds");
            let view = send_rect(send(class(c"NSView"), c"alloc"), c"initWithFrame:", bounds);
            // width and height follow the window
            send_u64(view, c"setAutoresizingMask:", 2 | 16);
            let host = new_layer();
            set_colour(host, background);
            send_p(view, c"setLayer:", host);
            send_bool(view, c"setWantsLayer:", true);
            // the well: a view of its own (AppKit draws views; layers added under a view's layer
            // weren't), its pictures views in it
            let well_view = send_rect(send(class(c"NSView"), c"alloc"), c"initWithFrame:", NSRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
            let well_layer = new_layer();
            set_colour(well_layer, well);
            send_bool(well_layer, c"setMasksToBounds:", true);
            send_p(well_view, c"setLayer:", well_layer);
            send_bool(well_view, c"setWantsLayer:", true);
            // the pictures stay inside it, zoomed in or not (views stopped clipping their
            // subviews by default in macOS 14)
            send_bool(well_view, c"setClipsToBounds:", true);
            send_bool(well_view, c"setHidden:", true);
            send_p(view, c"addSubview:", well_view);
            add_below(content, view, gpui_view);
            Some(Self {
                view,
                host,
                well: well_view,
                pictures: HashMap::new(),
                last: (None, vec![]),
                visible: true,
                meters: HashMap::new(),
                meter_jobs: Default::default(),
                meters_shown: true,
                meters_active: true,
                meter_thread: false,
                scale: window.scale_factor() as f64,
            })
        }
    }

    /// The layer a canvas's preview shows its frames in (made the first time), for
    /// `Preview::set_layer`.
    pub fn picture(&mut self, canvas: Id) -> *mut c_void {
        let well = self.well;
        self.pictures
            .entry(canvas)
            .or_insert_with(|| unsafe {
                let view = send_rect(send(class(c"NSView"), c"alloc"), c"initWithFrame:", NSRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
                let layer = new_layer();
                let gravity = send_p(class(c"NSString"), c"stringWithUTF8String:", c"resize".as_ptr() as *mut c_void);
                send_p(layer, c"setContentsGravity:", gravity);
                send_p(view, c"setLayer:", layer);
                send_bool(view, c"setWantsLayer:", true);
                send_bool(view, c"setHidden:", true);
                send_p(well, c"addSubview:", view);
                (view, layer)
            })
            .1
    }

    /// Where the well is (window points), and each shown canvas's picture in it; `None`: not
    /// on screen (another place, the Library, Editor).
    pub fn place(&mut self, well: Option<Bounds<Pixels>>, pictures: &[(Id, Bounds<Pixels>)]) {
        self.last = (well, pictures.to_vec());
        let well = well.filter(|_| self.visible);
        let (view, well_view, pictures_now) = (self.view, self.well, &self.pictures);
        quietly(|| unsafe {
            match well {
                None => send_bool(well_view, c"setHidden:", true),
                Some(w) => {
                    // AppKit's views count up from the bottom
                    let host_h = get_rect(view, c"bounds").h;
                    let mut r = rect(w);
                    r.y = host_h - r.y - r.h;
                    send_rect(well_view, c"setFrame:", r);
                    send_bool(well_view, c"setHidden:", false);
                    for (id, (picture, _)) in pictures_now {
                        match pictures.iter().find(|(c, _)| c == id) {
                            Some((_, b)) => {
                                let mut p = rect(*b);
                                p.x -= rect(w).x;
                                p.y = (rect(w).y + rect(w).h) - (p.y + p.h);
                                send_rect(*picture, c"setFrame:", p);
                                send_bool(*picture, c"setHidden:", false);
                            }
                            None => send_bool(*picture, c"setHidden:", true),
                        }
                    }
                }
            }
        });
    }

    /// Core Animation scales the layer to its frame itself: nothing to follow.
    pub fn refit(&mut self) {}

    pub fn place_player(&mut self, _: Option<(Bounds<Pixels>, (u32, u32))>) {}
    pub fn show_player(&mut self, _: bool) {}

    /// The canvas is on screen (the Live place) or not; placed again as it last was.
    pub fn set_visible(&mut self, visible: bool) {
        if visible != self.visible {
            self.visible = visible;
            let (well, pictures) = self.last.clone();
            self.place(well, &pictures);
            self.apply_meters();
        }
    }

    /// A canvas gone: its layer goes too.
    pub fn forget(&mut self, canvas: Id) {
        if let Some((view, layer)) = self.pictures.remove(&canvas) {
            unsafe {
                send(view, c"removeFromSuperview");
                send(view, c"release");
                send(layer, c"release");
            }
        }
    }
}

/// A strip's meter as laid out (window points): its box, its two bars, its peak readout, and
/// the area that clips them (the strips scroll).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeterPlace {
    pub boxed: Bounds<Pixels>,
    pub bars: [Bounds<Pixels>; 2],
    pub readout: Bounds<Pixels>,
    pub clip: Bounds<Pixels>,
}

/// A strip's meter's views (main thread).
struct MeterViews {
    clip: Id_,
    boxed: Id_,
    readout: Id_,
    text: Id_,
    place: MeterPlace,
}

/// What the meter thread draws for a strip.
struct MeterJob {
    text: usize,
    /// The bars in the box: (x, y from the bottom, width, height) in points.
    bars: [(f64, f64, f64, f64); 2],
    /// Each bar's layers: its level in green, yellow and red, and its peak hold.
    layers: [[usize; 4]; 2],
    shown: Shown,
    /// What the layers show now (each bar's level and hold, in points): unchanged, untouched.
    drawn: Option<[(f64, f64); 2]>,
}

/// Peak hold (the marker every console leaves at the recent peak): held this long, then it
/// falls this fast.
const HOLD_S: f32 = 1.5;
const HOLD_FALL_DB_S: f32 = 12.0;
/// The peak readout: the held peak, changing at most this often (a number that moved every
/// frame couldn't be read).
const READOUT_S: f32 = 0.25;

/// A strip's meter as displayed.
#[derive(Clone, Copy)]
struct Shown {
    bars: [f32; 2],
    hold: [(f32, std::time::Instant); 2],
    readout: (f32, std::time::Instant),
    text: Option<f32>,
    last: std::time::Instant,
}

fn meter_frac(db: f32) -> f32 {
    if !db.is_finite() { 0.0 } else { ((db + 60.0) / 60.0).clamp(0.0, 1.0) }
}

fn rgb(c: u32) -> (f64, f64, f64) {
    (((c >> 16) & 0xFF) as f64 / 255.0, ((c >> 8) & 0xFF) as f64 / 255.0, (c & 0xFF) as f64 / 255.0)
}

/// The meters' colours (ui.rs's tokens): green below −20, yellow −20…−9, red above −9 dBFS.
const OK: u32 = 0x34D399;
const WARN: u32 = 0xFBBF24;
const ERROR: u32 = 0xF87171;
const BAR_BG: u32 = 0x18181B;
const BOX_BG: u32 = 0x0B0B0C;
const TEXT: u32 = 0xA9A9B1;

unsafe fn new_view(frame: NSRect) -> Id_ {
    unsafe { send_rect(send(class(c"NSView"), c"alloc"), c"initWithFrame:", frame) }
}

unsafe fn send_f64(target: Id_, sel: &CStr, arg: f64) {
    let f: unsafe extern "C" fn(Id_, *mut c_void, f64) = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { f(target, sel_registerName(sel.as_ptr()), arg) }
}

unsafe fn ns_string(s: &str) -> Id_ {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    unsafe { send_p(class(c"NSString"), c"stringWithUTF8String:", c.as_ptr() as *mut c_void) }
}

unsafe fn set_cg(layer: Id_, sel: &CStr, c: u32) {
    let (r, g, b) = rgb(c);
    unsafe {
        let cg = CGColorCreateSRGB(r, g, b, 1.0);
        send_p(layer, sel, cg);
        CGColorRelease(cg);
    }
}

impl NativePreview {
    /// Puts each strip's meter where the mixer laid it out (holes GPUI leaves in the mixer), or
    /// takes away those no longer there. Main thread.
    pub fn place_meters(&mut self, strips: &[(Id, MeterPlace)]) {
        let host_h = unsafe { get_rect(self.view, c"bounds").h };
        // AppKit's views count up from the bottom, and a view's frame is in its parent's terms
        let flip = |b: Bounds<Pixels>, h: f64| {
            let mut r = rect(b);
            r.y = h - r.y - r.h;
            r
        };
        let within = |inner: Bounds<Pixels>, outer: Bounds<Pixels>| {
            let r = rect(inner);
            let o = rect(outer);
            NSRect { x: r.x - o.x, y: (o.y + o.h) - (r.y + r.h), w: r.w, h: r.h }
        };
        let (scale, well) = (self.scale, self.well);
        let gone: Vec<Id> = self.meters.keys().copied().filter(|id| !strips.iter().any(|(s, _)| s == id)).collect();
        let mut jobs = self.meter_jobs.lock();
        quietly(|| unsafe {
            for id in gone {
                if let Some(m) = self.meters.remove(&id) {
                    send(m.clip, c"removeFromSuperview");
                    for v in [m.clip, m.boxed, m.readout, m.text] {
                        send(v, c"release");
                    }
                }
                jobs.remove(&id);
            }
            for (id, place) in strips {
                let m = self.meters.entry(*id).or_insert_with(|| {
                    // the clip, the box (its layer the meter thread's picture), the readout box and its text
                    let clip = new_view(flip(place.clip, host_h));
                    send_bool(clip, c"setWantsLayer:", true);
                    send_bool(clip, c"setClipsToBounds:", true);
                    let boxed = new_view(within(place.boxed, place.clip));
                    let layer = new_layer();
                    set_cg(layer, c"setBackgroundColor:", BOX_BG);
                    send_f64(layer, c"setCornerRadius:", 3.0);
                    send_f64(layer, c"setContentsScale:", scale);
                    send_p(boxed, c"setLayer:", layer);
                    send_bool(boxed, c"setWantsLayer:", true);
                    let readout = new_view(within(place.readout, place.clip));
                    let rl = new_layer();
                    set_cg(rl, c"setBackgroundColor:", BOX_BG);
                    send_f64(rl, c"setCornerRadius:", 3.0);
                    send_p(readout, c"setLayer:", rl);
                    send_bool(readout, c"setWantsLayer:", true);
                    // the text, centred on the readout (CATextLayer draws from its top)
                    let r = rect(place.readout);
                    let text = new_view(NSRect { x: 0.0, y: (r.h - 13.0) / 2.0, w: r.w, h: 13.0 });
                    let tl = send(class(c"CATextLayer"), c"new");
                    send_p(tl, c"setFont:", ns_string("Menlo"));
                    send_f64(tl, c"setFontSize:", 10.0);
                    send_p(tl, c"setAlignmentMode:", ns_string("center"));
                    send_f64(tl, c"setContentsScale:", scale);
                    set_cg(tl, c"setForegroundColor:", TEXT);
                    send_p(tl, c"setString:", ns_string("-inf"));
                    send_p(text, c"setLayer:", tl);
                    send_bool(text, c"setWantsLayer:", true);
                    send_p(readout, c"addSubview:", text);
                    send_p(clip, c"addSubview:", boxed);
                    send_p(clip, c"addSubview:", readout);
                    // under the canvas well: a strip the canvas grows over is covered by it
                    add_below(self.view, clip, well);
                    send_bool(clip, c"setHidden:", !self.meters_shown);
                    MeterViews { clip, boxed, readout, text, place: *place }
                });
                if m.place != *place || !jobs.contains_key(id) {
                    send_rect(m.clip, c"setFrame:", flip(place.clip, host_h));
                    send_rect(m.boxed, c"setFrame:", within(place.boxed, place.clip));
                    send_rect(m.readout, c"setFrame:", within(place.readout, place.clip));
                    m.place = *place;
                    let b = rect(place.boxed);
                    let bar = |c: usize| {
                        let r = rect(place.bars[c]);
                        (r.x - b.x, (b.y + b.h) - (r.y + r.h), r.w, r.h)
                    };
                    let now = std::time::Instant::now();
                    let quiet = f32::NEG_INFINITY;
                    let shown = jobs.get(id).map(|j| j.shown).unwrap_or(Shown {
                        bars: [quiet; 2],
                        hold: [(quiet, now); 2],
                        readout: (quiet, now),
                        text: None,
                        last: now,
                    });
                    // each bar: its track, then its level in three colours and its peak hold, as
                    // layers in the box (frames change, nothing is drawn again)
                    let boxed = send(m.boxed, c"layer");
                    let old: Id_ = send(boxed, c"sublayers");
                    if !old.is_null() {
                        let copy = send(old, c"copy");
                        let n: usize = {
                            let f: unsafe extern "C" fn(Id_, *mut c_void) -> usize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                            f(copy, sel_registerName(c"count".as_ptr()))
                        };
                        for k in 0..n {
                            let f: unsafe extern "C" fn(Id_, *mut c_void, usize) -> Id_ = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                            let l = f(copy, sel_registerName(c"objectAtIndex:".as_ptr()), k);
                            send(l, c"removeFromSuperlayer");
                        }
                        send(copy, c"release");
                    }
                    let mut layers = [[0usize; 4]; 2];
                    for (c, slot) in layers.iter_mut().enumerate() {
                        let (x, y, w, h) = bar(c);
                        let track = new_layer();
                        set_cg(track, c"setBackgroundColor:", BAR_BG);
                        send_rect(track, c"setFrame:", NSRect { x, y, w, h });
                        send_p(boxed, c"addSublayer:", track);
                        send(track, c"release");
                        for (k, colour) in [OK, WARN, ERROR, OK].into_iter().enumerate() {
                            let l = new_layer();
                            set_cg(l, c"setBackgroundColor:", colour);
                            send_bool(l, c"setHidden:", true);
                            send_p(boxed, c"addSublayer:", l);
                            send(l, c"release");
                            slot[k] = l as usize;
                        }
                    }
                    jobs.insert(*id, MeterJob {
                        text: send(m.text, c"layer") as usize,
                        bars: [bar(0), bar(1)],
                        layers,
                        shown,
                        drawn: None,
                    });
                }
            }
        });
        drop(jobs);
        if !self.meter_thread && !strips.is_empty() {
            self.meter_thread = true;
            let jobs = self.meter_jobs.clone();
            let _ = std::thread::Builder::new().name("lightair-meters".into()).spawn(move || meter_thread(jobs));
        }
    }

    /// The mixer's tab is the one shown (its meters show in the Live place).
    pub fn show_meters(&mut self, active: bool) {
        self.meters_active = active;
        self.apply_meters();
    }

    fn apply_meters(&mut self) {
        let shown = self.meters_active && self.visible;
        if shown == self.meters_shown {
            return;
        }
        self.meters_shown = shown;
        quietly(|| unsafe {
            for m in self.meters.values() {
                send_bool(m.clip, c"setHidden:", !shown);
            }
        });
    }
}

/// Draws every meter at the display's rate, straight from the audio levels, without the UI.
fn meter_thread(shared: std::sync::Arc<parking_lot::Mutex<HashMap<Id, MeterJob>>>) {
    let colours = [OK, WARN, ERROR].map(|c| {
        let (r, g, b) = rgb(c);
        unsafe { CGColorCreateSRGB(r, g, b, 1.0) }
    });
    loop {
        std::thread::sleep(std::time::Duration::from_micros(16_667));
        // the window gone: the thread goes too
        if std::sync::Arc::strong_count(&shared) == 1 {
            break;
        }
        let pool = unsafe { objc_autoreleasePoolPush() };
        let mut jobs = shared.lock();
        let now = std::time::Instant::now();
        quietly(|| {
            for (id, job) in jobs.iter_mut() {
                let levels = lightair_engine::meters::meters().get(*id).map(|c| c.levels());
                let sh = &mut job.shown;
                let dt = now.duration_since(sh.last).as_secs_f32().min(0.1);
                sh.last = now;
                let quiet = f32::NEG_INFINITY;
                for c in 0..2 {
                    let target = levels.map_or(quiet, |l| l.peak[c]);
                    // the bar: rises at once, falls at 24 dB/s
                    let v = &mut sh.bars[c];
                    let fallen = if v.is_finite() { *v - 24.0 * dt } else { quiet };
                    *v = if target > fallen { target } else { fallen.max(-90.0) };
                    // the hold: set by a new peak, kept a while, then falling slowly, never below the bar
                    let (level, since) = &mut sh.hold[c];
                    if target >= *level {
                        (*level, *since) = (target, now);
                    } else if now.duration_since(*since).as_secs_f32() > HOLD_S {
                        *level = (*level - HOLD_FALL_DB_S * dt).max(sh.bars[c]);
                        if *level <= -90.0 {
                            *level = quiet;
                        }
                    }
                }
                let held = sh.hold[0].0.max(sh.hold[1].0);
                if now.duration_since(sh.readout.1).as_secs_f32() >= READOUT_S || held > sh.readout.0 {
                    sh.readout = (held, now);
                }
                unsafe { draw_meter(job, &colours) };
            }
        });
        drop(jobs);
        unsafe { objc_autoreleasePoolPop(pool) };
    }
    for c in colours {
        unsafe { CGColorRelease(c) };
    }
}

/// Moves the bars' layers to the levels shown (only when they've changed by a point), and
/// changes the readout when its number did.
unsafe fn draw_meter(job: &mut MeterJob, colours: &[*mut c_void; 3]) {
    let (g_end, y_end) = (meter_frac(-20.0), meter_frac(-9.0));
    let mut now = [(0.0, 0.0); 2];
    for (c, n) in now.iter_mut().enumerate() {
        let (_, _, _, h) = job.bars[c];
        let held = job.shown.hold[c].0;
        *n = ((h * meter_frac(job.shown.bars[c]) as f64).round(), if held > -90.0 { (h * meter_frac(held) as f64).round() } else { -1.0 });
    }
    unsafe {
        if job.drawn != Some(now) {
            job.drawn = Some(now);
            for (c, &(level, top)) in now.iter().enumerate() {
                let (x, y, w, h) = job.bars[c];
                for (k, (from, to)) in [(0.0, g_end), (g_end, y_end), (y_end, 1.0)].into_iter().enumerate() {
                    let layer = job.layers[c][k] as Id_;
                    let (lo, hi) = ((h * from as f64).round(), (h * to as f64).round().min(level));
                    if hi > lo {
                        send_rect(layer, c"setFrame:", NSRect { x, y: y + lo, w, h: hi - lo });
                        send_bool(layer, c"setHidden:", false);
                    } else {
                        send_bool(layer, c"setHidden:", true);
                    }
                }
                // the peak hold: a 2-point line, coloured by where it is
                let hold = job.layers[c][3] as Id_;
                if top > 0.0 {
                    let f = top / h;
                    let zone = if f > y_end as f64 { 2 } else if f > g_end as f64 { 1 } else { 0 };
                    send_p(hold, c"setBackgroundColor:", colours[zone]);
                    send_rect(hold, c"setFrame:", NSRect { x, y: y + (top - 2.0).max(0.0), w, h: top.min(2.0) });
                    send_bool(hold, c"setHidden:", false);
                } else {
                    send_bool(hold, c"setHidden:", true);
                }
            }
        }
        let peak = job.shown.readout.0;
        if job.shown.text != Some(peak) {
            job.shown.text = Some(peak);
            let text = if peak > -90.0 { format!("{peak:.1}") } else { "-inf".into() };
            send_p(job.text as Id_, c"setString:", ns_string(&text));
            set_cg(job.text as Id_, c"setForegroundColor:", if peak > -1.0 { ERROR } else { TEXT });
        }
    }
}

impl Drop for NativePreview {
    fn drop(&mut self) {
        for (_, m) in self.meters.drain() {
            unsafe {
                send(m.clip, c"removeFromSuperview");
                for v in [m.clip, m.boxed, m.readout, m.text] {
                    send(v, c"release");
                }
            }
        }
        for (_, (view, layer)) in self.pictures.drain() {
            unsafe {
                send(view, c"removeFromSuperview");
                send(view, c"release");
                send(layer, c"release");
            }
        }
        unsafe {
            send(self.view, c"removeFromSuperview");
            send(self.view, c"release");
            send(self.well, c"release");
            send(self.host, c"release");
        }
    }
}
}

/// Elsewhere GPUI draws the preview.
#[cfg(not(any(target_os = "macos", windows)))]
pub struct NativePreview;

#[cfg(not(any(target_os = "macos", windows)))]
impl NativePreview {
    pub fn attach(_: &gpui_kit::Window, _: gpui_kit::Hsla, _: gpui_kit::Hsla) -> Option<Self> {
        None
    }
    pub fn picture(&mut self, _: lightair_core::Id) -> *mut std::ffi::c_void {
        std::ptr::null_mut()
    }
    pub fn place(&mut self, _: Option<gpui_kit::Bounds<gpui_kit::Pixels>>, _: &[(lightair_core::Id, gpui_kit::Bounds<gpui_kit::Pixels>)]) {}
    pub fn set_visible(&mut self, _: bool) {}
    pub fn refit(&mut self) {}
    pub fn player_visual(&mut self) -> *mut std::ffi::c_void {
        std::ptr::null_mut()
    }
    pub fn place_player(&mut self, _: Option<(gpui_kit::Bounds<gpui_kit::Pixels>, (u32, u32))>) {}
    pub fn show_player(&mut self, _: bool) {}
    pub fn forget(&mut self, _: lightair_core::Id) {}
    pub fn place_meters(&mut self, _: &[(lightair_core::Id, MeterPlace)]) {}
    pub fn show_meters(&mut self, _: bool) {}
}

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeterPlace {
    pub boxed: gpui_kit::Bounds<gpui_kit::Pixels>,
    pub bars: [gpui_kit::Bounds<gpui_kit::Pixels>; 2],
    pub readout: gpui_kit::Bounds<gpui_kit::Pixels>,
    pub clip: gpui_kit::Bounds<gpui_kit::Pixels>,
}
