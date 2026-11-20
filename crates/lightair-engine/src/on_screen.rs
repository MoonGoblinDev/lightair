//! Captures that run only while something shows them (macOS). OBS's screen capture
//! (ScreenCaptureKit) and camera capture start capturing the moment they're made and never stop,
//! shown or not. A running screen capture has the window server compose a copy of the whole
//! display every frame (a workspace with a display capture in each of five scenes kept the
//! window server at twice its load, the machine sluggish); a running camera keeps the camera
//! and its pipeline busy. Nothing off air should cost anything.
//!
//! Each wrapper makes its capture when something shows it (on air, a preview, a thumbnail) and
//! lets it go shortly after nothing does (a second: a scene being rebuilt or a transition's
//! hand-over doesn't restart it). While stopped it keeps its last size, so layouts don't jump.
//! A camera's sound (capture cards) is passed through.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::sys;

/// A wrapper type: its libobs id, the capture it runs, and whether the capture has sound.
pub struct Kind {
    pub id: &'static CStr,
    inner: &'static CStr,
    name: &'static CStr,
    sound: bool,
    icon: sys::obs_icon_type,
    /// The access macOS asks the user for before it opens (`crate::access`).
    access: Option<crate::access::Media>,
}

pub static SCREEN: Kind = Kind {
    id: c"lightair_screen_capture",
    inner: c"screen_capture",
    name: c"Screen capture",
    sound: false,
    icon: sys::OBS_ICON_TYPE_DESKTOP_CAPTURE,
    access: None,
};

pub static CAMERA: Kind = Kind {
    id: c"lightair_camera_capture",
    inner: c"macos-avcapture",
    name: c"Camera",
    sound: true,
    icon: sys::OBS_ICON_TYPE_CAMERA,
    access: Some(crate::access::Media::Camera),
};

/// The capture kind a wrapper stands for ("screen_capture" for "lightair_screen_capture").
pub(crate) fn inner_id(id: &str) -> Option<&'static str> {
    [&SCREEN, &CAMERA].into_iter().find(|k| k.id.to_bytes() == id.as_bytes()).and_then(|k| k.inner.to_str().ok())
}

/// Runs `f` with the capture a wrapper source stands for, to read what it offers (its displays,
/// windows, devices): the running capture, or a private one made with the wrapper's settings
/// for the moment (wrappers have no list of their own: they make their capture only while
/// shown). `None` for any other source.
pub(crate) fn with_capture<R>(source: *mut sys::obs_source_t, f: impl FnOnce(*mut sys::obs_source_t) -> R) -> Option<R> {
    let id = unsafe { CStr::from_ptr(sys::obs_source_get_id(source)) };
    let kind = [&SCREEN, &CAMERA].into_iter().find(|k| k.id == id)?;
    let shared = unsafe { &*(sys::obs_obj_get_data(source as *mut c_void) as *const Arc<Shared>) };
    let running = shared.child.lock().as_ref().map(|c| unsafe { sys::obs_source_get_ref(c.0) }).filter(|c| !c.is_null());
    let capture = match running {
        Some(c) => c,
        // a camera isn't opened just to list devices (its light would come on): the caller lists them
        None if kind.access.is_some() => return None,
        None => {
            let settings = shared.settings();
            unsafe { sys::obs_source_create_private(kind.inner.as_ptr(), c"Lightair choices".as_ptr(), settings) }
        }
    };
    if capture.is_null() {
        return None;
    }
    let out = f(capture);
    unsafe { sys::obs_source_release(capture) };
    Some(out)
}

/// How long a capture keeps running once nothing shows it.
const LINGER: Duration = Duration::from_secs(1);

/// Devices that came back (an iPhone that went away and returned): (unique id, when, as a
/// count of reconnections). A camera capture of one that's running starts again, as if new:
/// OBS's own reconnection doesn't always catch an iPhone's return.
static RECONNECTED: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
static RECONNECTIONS: AtomicU64 = AtomicU64::new(0);

/// A camera (or a phone's screen) with this unique id has (re)connected.
pub fn device_reconnected(uid: &str) {
    let n = RECONNECTIONS.fetch_add(1, Ordering::AcqRel) + 1;
    let mut list = RECONNECTED.lock();
    list.retain(|(u, _)| u != uid);
    list.push((uid.to_string(), n));
}

/// The device a camera capture's settings name.
fn device_of(settings: *mut sys::obs_data_t) -> String {
    let s = unsafe { sys::obs_data_get_string(settings, c"device".as_ptr()) };
    if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
}

struct Child(*mut sys::obs_source_t);
// SAFETY: libobs sources are reference counted and thread-safe.
unsafe impl Send for Child {}

struct Shared {
    kind: &'static Kind,
    source: usize,
    /// Its current settings (a reference held here).
    settings: Mutex<usize>,
    child: Mutex<Option<Child>>,
    /// A capture is being made or let go of (on a thread of its own).
    busy: AtomicBool,
    hidden_since: Mutex<Option<Instant>>,
    /// The last size seen (width << 32 | height).
    size: AtomicU64,
    /// Reconnections already looked at (`RECONNECTIONS`).
    reconnections: AtomicU64,
    /// A weak reference to the wrapper: where the capture's sound goes (`sound`).
    weak: usize,
}

impl Shared {
    fn settings(&self) -> *mut sys::obs_data_t {
        *self.settings.lock() as *mut sys::obs_data_t
    }

    /// Makes the capture (a worker thread).
    fn start(self: &Arc<Self>) {
        let parent = self.source as *mut sys::obs_source_t;
        let name = unsafe { CStr::from_ptr(sys::obs_source_get_name(parent)) }.to_string_lossy().into_owned();
        let name = CString::new(format!("{name} (capture)")).unwrap_or_default();
        // (held while it's used: an update may replace them meanwhile)
        let settings = {
            let s = self.settings.lock();
            unsafe { sys::obs_data_addref(*s as *mut sys::obs_data_t) };
            *s as *mut sys::obs_data_t
        };
        let child = unsafe { sys::obs_source_create_private(self.kind.inner.as_ptr(), name.as_ptr(), settings) };
        unsafe { sys::obs_data_release(settings) };
        if !child.is_null() {
            unsafe {
                // it draws only while it counts as shown
                sys::obs_source_inc_showing(child);
                if self.kind.sound {
                    sys::obs_source_add_audio_capture_callback(child, Some(sound), self.weak as *mut c_void);
                }
            }
            *self.child.lock() = Some(Child(child));
        }
        self.busy.store(false, Ordering::Release);
    }

    /// Lets the capture go (a worker thread, or destroy).
    fn stop(self: &Arc<Self>) {
        let child = self.child.lock().take();
        if let Some(Child(c)) = child {
            unsafe {
                if self.kind.sound {
                    sys::obs_source_remove_audio_capture_callback(c, Some(sound), self.weak as *mut c_void);
                }
                sys::obs_source_dec_showing(c);
                sys::obs_source_release(c);
            }
        }
        self.busy.store(false, Ordering::Release);
    }

    fn on_worker(self: &Arc<Self>, start: bool) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        // (making or releasing a source from inside libobs's tick isn't safe, and opening a
        // device can wait on a permission prompt: a thread of its own)
        let spawned = std::thread::Builder::new().name("lightair-capture".into()).spawn(move || {
            if start { me.start() } else { me.stop() }
        });
        if spawned.is_err() {
            self.busy.store(false, Ordering::Release);
        }
    }
}

/// A stretch of a capture's sound on its way to its wrapper.
struct Sound {
    /// The wrapper (a weak reference of its own, let go once it's passed on).
    to: usize,
    planes: Vec<Vec<f32>>,
    frames: u32,
    timestamp: u64,
    speakers: sys::speaker_layout,
    samples_per_sec: u32,
}

// SAFETY: the weak reference is libobs's (thread-safe), the samples are owned.
unsafe impl Send for Sound {}

/// The capture's sound, out of the wrapper (libobs mixes the wrapper, not its private capture).
/// Copied and passed on by a thread of its own: libobs calls this holding the capture's locks,
/// and the wrapper's would be taken under them, the other way round from the graphics thread
/// (which sizes the wrapper, then its capture): the two waited on each other for good.
unsafe extern "C" fn sound(param: *mut c_void, _: *mut sys::obs_source_t, audio: *const sys::audio_data, _muted: bool) {
    let weak = param as *mut sys::obs_weak_source_t;
    let a = unsafe { &*audio };
    let mut info = sys::obs_audio_info::default();
    if !unsafe { sys::obs_get_audio_info(&mut info) } || a.frames == 0 {
        return;
    }
    // (a speaker layout's number is its channel count)
    let channels = (info.speakers as usize).min(a.data.len());
    let planes = a.data[..channels]
        .iter()
        .take_while(|d| !d.is_null())
        .map(|d| unsafe { std::slice::from_raw_parts(*d as *const f32, a.frames as usize) }.to_vec())
        .collect::<Vec<_>>();
    if planes.is_empty() {
        return;
    }
    unsafe { sys::obs_weak_source_addref(weak) };
    let sound = Sound {
        to: weak as usize,
        planes,
        frames: a.frames,
        timestamp: a.timestamp,
        speakers: info.speakers,
        samples_per_sec: info.samples_per_sec,
    };
    // (full, the forwarder is stuck: dropped rather than piled up)
    if let Err(e) = sounds().try_send(sound) {
        unsafe { sys::obs_weak_source_release(e.into_inner().to as *mut sys::obs_weak_source_t) };
    }
}

/// Where captures' sound waits to be passed on to their wrappers (the thread starts with it).
fn sounds() -> &'static crossbeam_channel::Sender<Sound> {
    static SENDER: OnceLock<crossbeam_channel::Sender<Sound>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::bounded::<Sound>(512);
        let spawned = std::thread::Builder::new().name("lightair-capture-sound".into()).spawn(move || {
            for s in rx {
                let weak = s.to as *mut sys::obs_weak_source_t;
                let parent = unsafe { sys::obs_weak_source_get_source(weak) };
                if !parent.is_null() {
                    let mut out = sys::obs_source_audio {
                        frames: s.frames,
                        speakers: s.speakers,
                        format: sys::AUDIO_FORMAT_FLOAT_PLANAR,
                        samples_per_sec: s.samples_per_sec,
                        timestamp: s.timestamp,
                        ..Default::default()
                    };
                    for (o, p) in out.data.iter_mut().zip(&s.planes) {
                        *o = p.as_ptr() as *const u8;
                    }
                    unsafe {
                        sys::obs_source_output_audio(parent, &out);
                        sys::obs_source_release(parent);
                    }
                }
                unsafe { sys::obs_weak_source_release(weak) };
            }
        });
        if spawned.is_err() {
            tracing::warn!("couldn't start the capture sound thread");
        }
        tx
    })
}

fn shared<'a>(data: *mut c_void) -> &'a Arc<Shared> {
    unsafe { &*(data as *const Arc<Shared>) }
}

unsafe extern "C" fn get_name(type_data: *mut c_void) -> *const c_char {
    let kind = unsafe { &*(type_data as *const Kind) };
    kind.name.as_ptr()
}

unsafe extern "C" fn create(settings: *mut sys::obs_data_t, source: *mut sys::obs_source_t) -> *mut c_void {
    let kind = unsafe { &*(sys::obs_source_get_type_data(source) as *const Kind) };
    unsafe { sys::obs_data_addref(settings) };
    let s = Arc::new(Shared {
        kind,
        source: source as usize,
        settings: Mutex::new(settings as usize),
        child: Mutex::new(None),
        busy: AtomicBool::new(false),
        hidden_since: Mutex::new(None),
        size: AtomicU64::new(0),
        reconnections: AtomicU64::new(RECONNECTIONS.load(Ordering::Acquire)),
        weak: unsafe { sys::obs_source_get_weak_source(source) } as usize,
    });
    Box::into_raw(Box::new(s)) as *mut c_void
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    let s = unsafe { Box::from_raw(data as *mut Arc<Shared>) };
    // a capture being made finishes first
    while s.busy.swap(true, Ordering::AcqRel) {
        std::thread::sleep(Duration::from_millis(2));
    }
    s.stop();
    unsafe {
        sys::obs_data_release(s.settings());
        sys::obs_weak_source_release(s.weak as *mut sys::obs_weak_source_t);
    }
}

unsafe extern "C" fn update(data: *mut c_void, settings: *mut sys::obs_data_t) {
    let s = shared(data);
    unsafe { sys::obs_data_addref(settings) };
    let old = std::mem::replace(&mut *s.settings.lock(), settings as usize);
    unsafe { sys::obs_data_release(old as *mut sys::obs_data_t) };
    if let Some(Child(c)) = s.child.lock().as_ref() {
        unsafe { sys::obs_source_update(*c, settings) };
    }
}

/// Starts the capture when something shows it, stops it shortly after nothing does.
unsafe extern "C" fn video_tick(data: *mut c_void, _seconds: f32) {
    let s = shared(data);
    let shown = unsafe { sys::obs_source_showing(s.source as *mut sys::obs_source_t) };
    let running = s.child.lock().is_some();
    // its device came back: the capture let go, and made again (next tick) as if new
    if s.kind.access.is_some() {
        let now = RECONNECTIONS.load(Ordering::Acquire);
        let seen = s.reconnections.swap(now, Ordering::AcqRel);
        if now != seen && running {
            let device = device_of(s.settings());
            if !device.is_empty() && RECONNECTED.lock().iter().any(|(u, at)| *at > seen && *u == device) {
                s.on_worker(false);
                return;
            }
        }
    }
    if shown {
        *s.hidden_since.lock() = None;
        // (not while macOS is asking the user for the camera: it's made once they answer)
        if !running && !s.kind.access.is_some_and(crate::access::hold_back) {
            s.on_worker(true);
        }
    } else if running {
        let mut since = s.hidden_since.lock();
        let at = *since.get_or_insert_with(Instant::now);
        if at.elapsed() >= LINGER {
            *since = None;
            drop(since);
            s.on_worker(false);
        }
    }
}

unsafe extern "C" fn video_render(data: *mut c_void, _effect: *mut sys::gs_effect_t) {
    let s = shared(data);
    // (never waits on a capture being made)
    let Some(child) = s.child.try_lock() else { return };
    if let Some(Child(c)) = child.as_ref() {
        unsafe { sys::obs_source_video_render(*c) };
    }
}

fn size(data: *mut c_void) -> (u32, u32) {
    let s = shared(data);
    if let Some(child) = s.child.try_lock()
        && let Some(Child(c)) = child.as_ref()
    {
        let (w, h) = unsafe { (sys::obs_source_get_width(*c), sys::obs_source_get_height(*c)) };
        if w > 0 && h > 0 {
            s.size.store(((w as u64) << 32) | h as u64, Ordering::Relaxed);
        }
    }
    let packed = s.size.load(Ordering::Relaxed);
    ((packed >> 32) as u32, packed as u32)
}

unsafe extern "C" fn get_width(data: *mut c_void) -> u32 {
    size(data).0
}

unsafe extern "C" fn get_height(data: *mut c_void) -> u32 {
    size(data).1
}

/// Registers the wrappers with libobs (once; after the modules load).
pub fn register() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        for kind in [&SCREEN, &CAMERA] {
            unsafe {
                let mut info: sys::obs_source_info = std::mem::zeroed();
                info.id = kind.id.as_ptr();
                info.type_ = sys::OBS_SOURCE_TYPE_INPUT;
                info.output_flags = sys::OBS_SOURCE_VIDEO | sys::OBS_SOURCE_CUSTOM_DRAW | sys::OBS_SOURCE_DO_NOT_DUPLICATE;
                if kind.sound {
                    info.output_flags |= sys::OBS_SOURCE_AUDIO;
                }
                info.type_data = kind as *const Kind as *mut c_void;
                info.get_name = Some(get_name);
                info.create = Some(create);
                info.destroy = Some(destroy);
                info.update = Some(update);
                info.video_tick = Some(video_tick);
                info.video_render = Some(video_render);
                info.get_width = Some(get_width);
                info.get_height = Some(get_height);
                info.icon_type = kind.icon;
                sys::obs_register_source_s(&info, std::mem::size_of::<sys::obs_source_info>());
            }
        }
    });
}
