//! The Phone camera layer's picture and sound (docs/07-scenes-layers-canvas.md, LY-16;
//! lightair-live's docs/05 §6). A phone in Lightair Live's Camera mode sends MPEG-TS over SRT;
//! the app receives it (`lightair-phone-camera`) and hands each datagram to a UDP port on this
//! machine, where a private `ffmpeg_source` decodes it (libobs's FFmpeg has no SRT of its own).
//!
//! `lightair_phone_camera` wraps that decoder:
//!
//! - a new feed (the phone connected again, or was asked for another codec or size) makes a new
//!   decoder, so nothing of the last stream is kept;
//! - a feed with nothing new for [`GONE_AFTER`] shows a clean placeholder (a dark slate and a
//!   line saying what it waits for) until pictures come again;
//! - the phone encodes as its sensor sees and says which way is up: the wrapper turns the
//!   picture (90° steps) and reports the turned size;
//! - the phone's microphone is passed through as the wrapper's sound (a mixer track).
//!
//! The app talks to a source's [`Feed`] by the source's id (its libobs name).

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::json;

use crate::obs::{Data, ObsSource};
use crate::sys;

/// The workspace kind (and libobs id) of a Phone camera source.
pub const KIND: &str = "lightair_phone_camera";
const ID: &CStr = c"lightair_phone_camera";

/// How long a feed may bring nothing before the placeholder shows (the phone dropped).
pub const GONE_AFTER: Duration = Duration::from_millis(700);

/// The placeholder's typeface (the system's own).
#[cfg(windows)]
const FACE: &str = "Segoe UI";
#[cfg(not(windows))]
const FACE: &str = "Helvetica Neue";

/// The size shown before any picture came (a 1080p landscape camera).
const DEFAULT_SIZE: (u32, u32) = (1920, 1080);

fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    // (+1: zero means "never")
    START.get_or_init(Instant::now).elapsed().as_micros() as u64 + 1
}

/// One Phone camera source's feed, as the app sees it: where its stream goes, and what the
/// wrapper should know about it.
pub struct Feed {
    /// The UDP port on 127.0.0.1 the decoder listens on.
    port: u16,
    /// Bumped for each new stream: the wrapper makes a new decoder.
    generation: AtomicU64,
    /// When data last came (`now_us`), 0 for never.
    last_data: AtomicU64,
    /// Clockwise degrees that turn the picture upright (0, 90, 180, 270).
    rotation: AtomicU32,
    /// What the placeholder says.
    message: Mutex<String>,
    /// Bytes received, for the Inspector's rate.
    bytes: AtomicU64,
    /// Data came since the decoder was last made.
    streamed: AtomicBool,
}

impl Feed {
    fn new() -> Feed {
        // a free port, found by taking one and letting it go (the decoder binds it next)
        let port = std::net::UdpSocket::bind("127.0.0.1:0").and_then(|s| s.local_addr()).map_or(0, |a| a.port());
        Feed {
            port,
            generation: AtomicU64::new(0),
            last_data: AtomicU64::new(0),
            rotation: AtomicU32::new(0),
            message: Mutex::new("Waiting for the phone".into()),
            bytes: AtomicU64::new(0),
            streamed: AtomicBool::new(false),
        }
    }

    /// Where the stream goes: datagrams of MPEG-TS packets, as they came.
    pub fn addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    /// A new stream starts (a phone connected, or changed codec or size): a new decoder.
    pub fn restart(&self) {
        self.streamed.store(false, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// A stream started: a new decoder, unless the one there has had nothing yet (it's ready).
    pub fn new_stream(&self) {
        if self.streamed.swap(false, Ordering::AcqRel) {
            self.restart();
        }
    }

    /// Data arrived (`n` bytes).
    pub fn data(&self, n: usize) {
        self.streamed.store(true, Ordering::Release);
        self.last_data.store(now_us(), Ordering::Release);
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// Bytes received so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Whether pictures are coming (data within [`GONE_AFTER`]).
    pub fn live(&self) -> bool {
        let at = self.last_data.load(Ordering::Acquire);
        at != 0 && now_us().saturating_sub(at) < GONE_AFTER.as_micros() as u64
    }

    /// The phone's `rotation`: clockwise degrees that turn the picture upright.
    pub fn set_rotation(&self, degrees: i64) {
        let r = (degrees.rem_euclid(360) as u32 + 45) / 90 * 90 % 360;
        self.rotation.store(r, Ordering::Release);
    }

    pub fn rotation(&self) -> u32 {
        self.rotation.load(Ordering::Acquire)
    }

    /// What the placeholder says ("Waiting for Sam's iPhone").
    pub fn set_message(&self, text: &str) {
        let mut m = self.message.lock();
        if *m != text {
            *m = text.to_owned();
        }
    }
}

static FEEDS: Mutex<Option<HashMap<String, Arc<Feed>>>> = Mutex::new(None);

/// The feed of the Phone camera source with this id (made the first time it's asked for).
pub fn feed(source: &str) -> Arc<Feed> {
    FEEDS
        .lock()
        .get_or_insert_with(HashMap::new)
        .entry(source.to_owned())
        .or_insert_with(|| Arc::new(Feed::new()))
        .clone()
}

/// Lets a source's feed go (the source was removed).
pub fn forget(source: &str) {
    if let Some(f) = FEEDS.lock().as_mut() {
        f.remove(source);
    }
}

/// The decoder's settings for a feed on `port`: MPEG-TS over UDP from this machine, played as it
/// comes (no buffering). A phone that drops leaves it waiting; a new stream gets a new decoder.
fn decoder_settings(port: u16) -> serde_json::Value {
    json!({
        "is_local_file": false,
        "input": format!(
            "udp://127.0.0.1:{port}?fifo_size=262144&overrun_nonfatal=1&buffer_size=8388608"
        ),
        "input_format": "mpegts",
        "buffering_mb": 0,
        "hw_decode": true,
        "restart_on_activate": false,
        "close_when_inactive": false,
        "clear_on_media_end": true,
        "reconnect_delay_sec": 1,
        // a short look before playing: what's read to learn the stream is played in real time
        // afterwards, so a long look (it was 1.5 s) stays as delay for good. The phone puts
        // parameter sets before every keyframe and sends one as soon as a stream starts.
        "ffmpeg_options": "probesize=32768 analyzeduration=200000 fflags=nobuffer",
    })
}

struct Child(ObsSource);

struct Shared {
    source: usize,
    /// A weak reference to the wrapper: where the decoder's sound goes.
    weak: usize,
    feed: Arc<Feed>,
    decoder: Mutex<Option<Child>>,
    /// The placeholder's line, and what it says.
    text: Mutex<Option<(Child, String)>>,
    /// The feed generation the decoder was made for.
    seen: AtomicU64,
    /// A worker is making or letting go of a decoder.
    busy: AtomicBool,
    /// The last size shown (width << 32 | height), before turning.
    size: AtomicU64,
}

impl Shared {
    /// A new decoder for the feed's current stream (a worker thread).
    fn renew(self: &Arc<Self>, generation: u64) {
        self.drop_decoder();
        let name = unsafe { CStr::from_ptr(sys::obs_source_get_name(self.source as *mut sys::obs_source_t)) }
            .to_string_lossy()
            .into_owned();
        let settings = Data::from_json(&decoder_settings(self.feed.port));
        if let Some(child) =
            ObsSource::create_private("ffmpeg_source", &format!("{name} (phone feed)"), Some(&settings))
        {
            unsafe {
                sys::obs_source_add_audio_capture_callback(child.as_ptr(), Some(sound), self.weak as *mut c_void)
            };
            // the newest picture as soon as it's decoded: libobs otherwise holds a stream's
            // frames back to their timestamps (the phone's mic is lined up by the track's offset)
            unsafe { sys::obs_source_set_async_unbuffered(child.as_ptr(), true) };
            *self.decoder.lock() = Some(Child(child));
        }
        self.seen.store(generation, Ordering::Release);
        self.busy.store(false, Ordering::Release);
    }

    fn drop_decoder(&self) {
        if let Some(Child(c)) = self.decoder.lock().take() {
            unsafe { sys::obs_source_remove_audio_capture_callback(c.as_ptr(), Some(sound), self.weak as *mut c_void) };
        }
    }

    /// The placeholder's line, made or changed (a worker thread: text is drawn when it's set).
    fn retext(self: &Arc<Self>, message: String) {
        let settings = Data::from_json(&json!({
            "text": message,
            "font": {"face": FACE, "size": 64, "flags": 0},
            "color1": 0xFFB4B4B4u32,
            "color2": 0xFFB4B4B4u32,
        }));
        let mut text = self.text.lock();
        match text.as_mut() {
            Some((Child(t), said)) => {
                t.update(&settings);
                *said = message;
            }
            None => {
                if let Some(t) =
                    ObsSource::create_private("text_ft2_source_v2", "Phone camera placeholder", Some(&settings))
                {
                    *text = Some((Child(t), message));
                }
            }
        }
        drop(text);
        self.busy.store(false, Ordering::Release);
    }

    fn on_worker(self: &Arc<Self>, f: impl FnOnce(&Arc<Self>) + Send + 'static) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        // (making or releasing a source from inside libobs's tick isn't safe)
        let spawned = std::thread::Builder::new().name("lightair-phone-camera".into()).spawn(move || f(&me));
        if spawned.is_err() {
            self.busy.store(false, Ordering::Release);
        }
    }

    /// The decoder's picture size, before turning (the last seen while it has none).
    fn picture_size(&self) -> (u32, u32) {
        if let Some(d) = self.decoder.try_lock()
            && let Some(Child(c)) = d.as_ref()
        {
            let (w, h) = (c.width(), c.height());
            if w > 0 && h > 0 && self.feed.live() {
                self.size.store(((w as u64) << 32) | h as u64, Ordering::Relaxed);
            }
        }
        let packed = self.size.load(Ordering::Relaxed);
        ((packed >> 32) as u32, packed as u32)
    }

    /// The size the layer has: the picture's, turned upright.
    fn shown_size(&self) -> (u32, u32) {
        let (w, h) = self.picture_size();
        if self.feed.rotation() % 180 == 90 { (h, w) } else { (w, h) }
    }
}

// SAFETY: libobs sources are reference counted and thread-safe; the rest is locked or atomic.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// A stretch of the decoder's sound on its way to the wrapper.
struct Sound {
    to: usize,
    planes: Vec<Vec<f32>>,
    frames: u32,
    timestamp: u64,
    speakers: sys::speaker_layout,
    samples_per_sec: u32,
}

// SAFETY: the weak reference is libobs's (thread-safe), the samples are owned.
unsafe impl Send for Sound {}

/// The decoder's sound, out of the wrapper (libobs mixes the wrapper, not its private decoder).
/// Passed on by a thread of its own: libobs calls this holding the decoder's locks.
unsafe extern "C" fn sound(param: *mut c_void, _: *mut sys::obs_source_t, audio: *const sys::audio_data, _muted: bool) {
    let weak = param as *mut sys::obs_weak_source_t;
    let a = unsafe { &*audio };
    let mut info = sys::obs_audio_info::default();
    if !unsafe { sys::obs_get_audio_info(&mut info) } || a.frames == 0 {
        return;
    }
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
    if let Err(e) = sounds().try_send(sound) {
        unsafe { sys::obs_weak_source_release(e.into_inner().to as *mut sys::obs_weak_source_t) };
    }
}

fn sounds() -> &'static crossbeam_channel::Sender<Sound> {
    static SENDER: OnceLock<crossbeam_channel::Sender<Sound>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::bounded::<Sound>(512);
        let spawned = std::thread::Builder::new().name("lightair-phone-sound".into()).spawn(move || {
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
            tracing::warn!("couldn't start the phone camera sound thread");
        }
        tx
    })
}

fn shared<'a>(data: *mut c_void) -> &'a Arc<Shared> {
    unsafe { &*(data as *const Arc<Shared>) }
}

unsafe extern "C" fn get_name(_: *mut c_void) -> *const c_char {
    c"Phone camera".as_ptr()
}

unsafe extern "C" fn create(settings: *mut sys::obs_data_t, source: *mut sys::obs_source_t) -> *mut c_void {
    let name = unsafe { CStr::from_ptr(sys::obs_source_get_name(source)) }.to_string_lossy().into_owned();
    let s = Arc::new(Shared {
        source: source as usize,
        weak: unsafe { sys::obs_source_get_weak_source(source) } as usize,
        feed: feed(&name),
        decoder: Mutex::new(None),
        text: Mutex::new(None),
        // (the first tick makes a decoder)
        seen: AtomicU64::new(u64::MAX),
        busy: AtomicBool::new(false),
        size: AtomicU64::new(0),
    });
    unsafe { update(&s as *const Arc<Shared> as *mut c_void, settings) };
    Box::into_raw(Box::new(s)) as *mut c_void
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    let s = unsafe { Box::from_raw(data as *mut Arc<Shared>) };
    while s.busy.swap(true, Ordering::AcqRel) {
        std::thread::sleep(Duration::from_millis(2));
    }
    s.drop_decoder();
    s.text.lock().take();
    unsafe { sys::obs_weak_source_release(s.weak as *mut sys::obs_weak_source_t) };
}

/// The size asked of the phone stands in until a picture comes.
unsafe extern "C" fn update(data: *mut c_void, settings: *mut sys::obs_data_t) {
    let s = shared(data);
    let get = |k: &CStr| unsafe { sys::obs_data_get_int(settings, k.as_ptr()) };
    let (w, h) = (get(c"width"), get(c"height"));
    let (w, h) = if w > 0 && h > 0 { (w as u32, h as u32) } else { DEFAULT_SIZE };
    if !s.feed.live() {
        s.size.store(((w as u64) << 32) | h as u64, Ordering::Relaxed);
    }
}

unsafe extern "C" fn video_tick(data: *mut c_void, _seconds: f32) {
    let s = shared(data);
    let generation = s.feed.generation.load(Ordering::Acquire);
    if s.seen.load(Ordering::Acquire) != generation {
        s.on_worker(move |s| s.renew(generation));
        return;
    }
    let message = s.feed.message.lock().clone();
    let said = s.text.try_lock().map(|t| t.as_ref().map(|(_, m)| m.clone()));
    if let Some(said) = said
        && said.as_deref() != Some(message.as_str())
    {
        s.on_worker(move |s| s.retext(message));
    }
}

/// Fills `w` × `h` with a colour (r, g, b, a in 0–1).
unsafe fn fill(w: u32, h: u32, rgba: [f32; 4]) {
    unsafe {
        let solid = sys::obs_get_base_effect(sys::OBS_EFFECT_SOLID);
        let colour = sys::gs_effect_get_param_by_name(solid, c"color".as_ptr());
        let tech = sys::gs_effect_get_technique(solid, c"Solid".as_ptr());
        let v = sys::vec4 { __bindgen_anon_1: sys::vec4__bindgen_ty_1 { ptr: rgba } };
        sys::gs_effect_set_vec4(colour, &v);
        sys::gs_technique_begin(tech);
        sys::gs_technique_begin_pass(tech, 0);
        sys::gs_draw_sprite(std::ptr::null_mut(), 0, w, h);
        sys::gs_technique_end_pass(tech);
        sys::gs_technique_end(tech);
    }
}

unsafe extern "C" fn video_render(data: *mut c_void, _effect: *mut sys::gs_effect_t) {
    let s = shared(data);
    let (pw, ph) = s.picture_size();
    let live = s.feed.live();
    if live
        && let Some(d) = s.decoder.try_lock()
        && let Some(Child(c)) = d.as_ref()
        && c.width() > 0
    {
        let (fw, fh) = (pw as f32, ph as f32);
        unsafe {
            sys::gs_matrix_push();
            match s.feed.rotation() {
                90 => {
                    sys::gs_matrix_translate3f(fh, 0.0, 0.0);
                    sys::gs_matrix_rotaa4f(0.0, 0.0, 1.0, std::f32::consts::FRAC_PI_2);
                }
                180 => {
                    sys::gs_matrix_translate3f(fw, fh, 0.0);
                    sys::gs_matrix_rotaa4f(0.0, 0.0, 1.0, std::f32::consts::PI);
                }
                270 => {
                    sys::gs_matrix_translate3f(0.0, fw, 0.0);
                    sys::gs_matrix_rotaa4f(0.0, 0.0, 1.0, 3.0 * std::f32::consts::FRAC_PI_2);
                }
                _ => {}
            }
            sys::obs_source_video_render(c.as_ptr());
            sys::gs_matrix_pop();
        }
        return;
    }
    // the placeholder: a dark slate, the line in the middle (at most 80 % of the width)
    let (w, h) = s.shown_size();
    if w == 0 || h == 0 {
        return;
    }
    unsafe { fill(w, h, [0.07, 0.07, 0.08, 1.0]) };
    if let Some(t) = s.text.try_lock()
        && let Some((Child(t), _)) = t.as_ref()
    {
        let (tw, th) = (t.width() as f32, t.height() as f32);
        if tw > 0.0 && th > 0.0 {
            // the line about a twentieth of the height, never wider than 80 % of the picture
            let scale = (h as f32 * 0.05 / th).min(w as f32 * 0.8 / tw);
            unsafe {
                sys::gs_matrix_push();
                sys::gs_matrix_translate3f((w as f32 - tw * scale) / 2.0, (h as f32 - th * scale) / 2.0, 0.0);
                sys::gs_matrix_scale3f(scale, scale, 1.0);
                sys::obs_source_video_render(t.as_ptr());
                sys::gs_matrix_pop();
            }
        }
    }
}

unsafe extern "C" fn get_width(data: *mut c_void) -> u32 {
    shared(data).shown_size().0
}

unsafe extern "C" fn get_height(data: *mut c_void) -> u32 {
    shared(data).shown_size().1
}

/// Registers the Phone camera source type with libobs (once; after the modules load).
pub fn register() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        let mut info: sys::obs_source_info = std::mem::zeroed();
        info.id = ID.as_ptr();
        info.type_ = sys::OBS_SOURCE_TYPE_INPUT;
        info.output_flags = sys::OBS_SOURCE_VIDEO
            | sys::OBS_SOURCE_AUDIO
            | sys::OBS_SOURCE_CUSTOM_DRAW
            | sys::OBS_SOURCE_DO_NOT_DUPLICATE;
        info.get_name = Some(get_name);
        info.create = Some(create);
        info.destroy = Some(destroy);
        info.update = Some(update);
        info.video_tick = Some(video_tick);
        info.video_render = Some(video_render);
        info.get_width = Some(get_width);
        info.get_height = Some(get_height);
        info.icon_type = sys::OBS_ICON_TYPE_CAMERA;
        sys::obs_register_source_s(&info, std::mem::size_of::<sys::obs_source_info>());
    });
}

/// The placeholder line for a feed: what it waits for.
pub fn waiting_for(phone: Option<&str>) -> String {
    match phone {
        Some(p) if !p.is_empty() => format!("Waiting for {p}"),
        _ => "Pair a phone in the Inspector".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_snaps_to_quarter_turns() {
        let f = Feed::new();
        for (given, want) in [(0, 0), (90, 90), (-90, 270), (270, 270), (360, 0), (100, 90), (179, 180), (450, 90)] {
            f.set_rotation(given);
            assert_eq!(f.rotation(), want, "{given}");
        }
    }

    #[test]
    fn a_feed_is_live_only_while_data_comes() {
        let f = Feed::new();
        assert!(!f.live());
        f.data(1316);
        assert!(f.live());
        assert_eq!(f.bytes(), 1316);
        std::thread::sleep(GONE_AFTER + Duration::from_millis(50));
        assert!(!f.live());
    }

    #[test]
    fn feeds_are_kept_by_source_and_have_their_own_ports() {
        let (a, b) = (feed("phone-camera-test-a"), feed("phone-camera-test-b"));
        assert!(Arc::ptr_eq(&a, &feed("phone-camera-test-a")));
        assert_ne!(a.addr().port(), 0);
        assert_ne!(a.addr().port(), b.addr().port());
        forget("phone-camera-test-a");
        assert!(!Arc::ptr_eq(&a, &feed("phone-camera-test-a")));
    }
}
