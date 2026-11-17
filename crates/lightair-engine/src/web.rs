//! Web pages as sources (`lightair_web`): a page Chromium renders off-screen
//! (`lightair_web`, one Chromium on every platform, bundled with Lightair), its frames given to
//! libobs as they're painted, like a camera's; its sound goes to libobs too (Chromium doesn't
//! play it), so it reaches the mixer like any other source's.
//!
//! Settings: `url`; `width`, `height` (the frame's size: the page's in CSS pixels at 100%);
//! `fps`; `zoom` (percent, as a browser's); `css` (added to the page each time it loads).
//!
//! On macOS Chromium shares its frames as IOSurfaces, and on Windows as Direct3D 11 shared
//! textures, which libobs draws as they are (as OBS's browser source does): no frame is read
//! back to memory, copied or uploaded again. Elsewhere frames arrive as pixels, given to libobs
//! like a camera's.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::sys;

pub const KIND: &str = "lightair_web";

/// The source a page paints into, cleared when the source goes (a paint after that is dropped).
#[derive(Clone)]
struct Target(Arc<Mutex<Option<usize>>>);

/// How long a page stays open once nothing shows it (a scene being rebuilt, a transition's
/// hand-over).
const LINGER: std::time::Duration = std::time::Duration::from_secs(1);

/// A page is open only while something shows it (on air, a preview): off screen it's closed,
/// costing nothing (OBS's "shut down when not visible"), and opened again when shown.
struct Web {
    page: Mutex<Option<lightair_web::Page>>,
    /// On screen somewhere (libobs's show/hide).
    showing: std::sync::atomic::AtomicBool,
    hidden_since: Mutex<Option<std::time::Instant>>,
    target: Target,
    options: Mutex<lightair_web::PageOptions>,
    #[cfg(any(target_os = "macos", windows))]
    shared: Arc<shared::Frames>,
}

fn string(settings: *mut sys::obs_data_t, key: &CStr) -> String {
    let s = unsafe { sys::obs_data_get_string(settings, key.as_ptr()) };
    if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
}

fn options(settings: *mut sys::obs_data_t) -> lightair_web::PageOptions {
    unsafe {
        lightair_web::PageOptions {
            url: string(settings, c"url"),
            width: sys::obs_data_get_int(settings, c"width".as_ptr()).clamp(16, 7680) as u32,
            height: sys::obs_data_get_int(settings, c"height".as_ptr()).clamp(16, 7680) as u32,
            fps: sys::obs_data_get_int(settings, c"fps".as_ptr()).clamp(1, 60) as u32,
            zoom: sys::obs_data_get_int(settings, c"zoom".as_ptr()).clamp(25, 500) as f32 / 100.0,
            css: string(settings, c"css"),
        }
    }
}

fn open(
    o: &lightair_web::PageOptions,
    target: &Target,
    #[cfg(any(target_os = "macos", windows))] shared: Option<Arc<shared::Frames>>,
) -> Option<lightair_web::Page> {
    if o.url.trim().is_empty() {
        return None;
    }
    let audio_target = target.clone();
    let target = target.clone();
    let sink: lightair_web::FrameSink = Arc::new(move |bgra: &[u8], w: u32, h: u32| {
        let held = target.0.lock();
        let Some(source) = *held else { return };
        unsafe {
            let mut frame: sys::obs_source_frame = std::mem::zeroed();
            frame.data[0] = bgra.as_ptr() as *mut u8;
            frame.linesize[0] = w * 4;
            frame.width = w;
            frame.height = h;
            frame.format = sys::VIDEO_FORMAT_BGRA;
            frame.full_range = true;
            frame.timestamp = sys::os_gettime_ns();
            // (libobs copies it)
            sys::obs_source_output_video(source as *mut sys::obs_source_t, &frame);
        }
    });
    let target = audio_target;
    let audio: lightair_web::AudioSink = Arc::new(move |planes: &[&[f32]], rate: u32| {
        let held = target.0.lock();
        let Some(source) = *held else { return };
        let Some(frames) = planes.first().map(|p| p.len()) else { return };
        unsafe {
            let mut audio: sys::obs_source_audio = std::mem::zeroed();
            for (slot, plane) in audio.data.iter_mut().zip(planes) {
                *slot = plane.as_ptr() as *const u8;
            }
            audio.frames = frames as u32;
            audio.speakers = match planes.len() {
                1 => sys::SPEAKERS_MONO,
                2 => sys::SPEAKERS_STEREO,
                3 => sys::SPEAKERS_2POINT1,
                4 => sys::SPEAKERS_4POINT0,
                5 => sys::SPEAKERS_4POINT1,
                6 => sys::SPEAKERS_5POINT1,
                _ => sys::SPEAKERS_7POINT1,
            };
            audio.format = sys::AUDIO_FORMAT_FLOAT_PLANAR;
            audio.samples_per_sec = rate;
            audio.timestamp = sys::os_gettime_ns();
            // (libobs copies it)
            sys::obs_source_output_audio(source as *mut sys::obs_source_t, &audio);
        }
    });
    #[cfg(any(target_os = "macos", windows))]
    if let Some(frames) = shared {
        let texture: lightair_web::TextureSink = Arc::new(move |surface, _w, _h| frames.put(surface));
        return Some(lightair_web::Page::open_shared(o.clone(), texture, sink, audio));
    }
    Some(lightair_web::Page::open(o.clone(), sink, audio))
}

/// Runs `f` on a web page source's open page (`source` is its libobs source; any other kind,
/// or a page closed off screen: nothing).
fn with_page(source: *mut sys::obs_source_t, f: impl FnOnce(&lightair_web::Page)) {
    let web = unsafe {
        let id = sys::obs_source_get_id(source);
        if id.is_null() || CStr::from_ptr(id).to_bytes() != KIND.as_bytes() {
            return;
        }
        (sys::obs_obj_get_data(source as *mut c_void) as *const Web).as_ref()
    };
    if let Some(web) = web
        && let Some(page) = web.page.lock().as_ref()
    {
        f(page);
    }
}

/// Loads a web page source's page again.
pub(crate) fn reload(source: *mut sys::obs_source_t) {
    with_page(source, |p| p.reload());
}

/// Restyles a web page source's page now (custom CSS being typed, not yet its setting).
pub(crate) fn restyle(source: *mut sys::obs_source_t, css: String) {
    with_page(source, |p| p.restyle(css));
}

impl Web {
    /// Opens the page if it isn't (shown again).
    fn ensure_open(&self) {
        let mut page = self.page.lock();
        if page.is_none() {
            let o = self.options.lock().clone();
            #[cfg(any(target_os = "macos", windows))]
            {
                *page = open(&o, &self.target, Some(self.shared.clone()));
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            {
                *page = open(&o, &self.target);
            }
        }
        if let Some(p) = page.as_ref() {
            p.set_visible(true);
        }
    }
}

unsafe extern "C" fn get_name(_: *mut c_void) -> *const c_char {
    c"Browser".as_ptr()
}

unsafe extern "C" fn create(settings: *mut sys::obs_data_t, source: *mut sys::obs_source_t) -> *mut c_void {
    // a page paints only when it changes, irregularly: its newest frame shows at once (libobs's
    // buffering, made for steady video, would hold it for a second or more)
    unsafe { sys::obs_source_set_async_unbuffered(source, true) };
    let target = Target(Arc::new(Mutex::new(Some(source as usize))));
    // (opened when something first shows it)
    let web = Web {
        page: Mutex::new(None),
        showing: Default::default(),
        hidden_since: Mutex::new(None),
        target,
        options: Mutex::new(options(settings)),
        #[cfg(any(target_os = "macos", windows))]
        shared: Arc::new(shared::Frames::default()),
    };
    Box::into_raw(Box::new(web)) as *mut c_void
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    let web = unsafe { Box::from_raw(data as *mut Web) };
    // no frame after this
    *web.target.0.lock() = None;
    drop(web);
}

unsafe extern "C" fn update(data: *mut c_void, settings: *mut sys::obs_data_t) {
    let web = unsafe { &*(data as *const Web) };
    let o = options(settings);
    if o == *web.options.lock() {
        return;
    }
    *web.options.lock() = o.clone();
    let mut page = web.page.lock();
    match page.as_ref() {
        Some(p) if !o.url.trim().is_empty() => p.update(o),
        // no address now: closed; a closed page opens with these when shown
        _ => {
            *page = None;
            drop(page);
            if web.showing.load(std::sync::atomic::Ordering::Relaxed) {
                web.ensure_open();
            }
        }
    }
}

unsafe extern "C" fn show(data: *mut c_void) {
    let web = unsafe { &*(data as *const Web) };
    web.showing.store(true, std::sync::atomic::Ordering::Relaxed);
    *web.hidden_since.lock() = None;
    web.ensure_open();
}

unsafe extern "C" fn hide(data: *mut c_void) {
    let web = unsafe { &*(data as *const Web) };
    web.showing.store(false, std::sync::atomic::Ordering::Relaxed);
    *web.hidden_since.lock() = Some(std::time::Instant::now());
    if let Some(page) = web.page.lock().as_ref() {
        page.set_visible(false);
    }
}

/// Closes the page once nothing has shown it for a moment.
unsafe extern "C" fn video_tick(data: *mut c_void, _seconds: f32) {
    let web = unsafe { &*(data as *const Web) };
    if web.showing.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut since = web.hidden_since.lock();
    if since.is_some_and(|t| t.elapsed() >= LINGER) {
        *since = None;
        drop(since);
        // (closing only asks Chromium's thread to)
        web.page.lock().take();
    }
}

unsafe extern "C" fn get_defaults(settings: *mut sys::obs_data_t) {
    unsafe {
        sys::obs_data_set_default_int(settings, c"width".as_ptr(), 1920);
        sys::obs_data_set_default_int(settings, c"height".as_ptr(), 1080);
        sys::obs_data_set_default_int(settings, c"fps".as_ptr(), 30);
        sys::obs_data_set_default_int(settings, c"zoom".as_ptr(), 100);
    }
}

/// Registers the source with libobs (once; after startup).
pub fn register() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        let mut info: sys::obs_source_info = std::mem::zeroed();
        info.id = c"lightair_web".as_ptr();
        info.type_ = sys::OBS_SOURCE_TYPE_INPUT;
        // frames as they're painted, kept until the next (a still page paints once); its sound
        info.output_flags = sys::OBS_SOURCE_ASYNC_VIDEO | sys::OBS_SOURCE_AUDIO | sys::OBS_SOURCE_DO_NOT_DUPLICATE;
        // macOS and Windows: Chromium's shared textures, drawn as they are
        #[cfg(any(target_os = "macos", windows))]
        {
            info.output_flags = sys::OBS_SOURCE_VIDEO | sys::OBS_SOURCE_AUDIO | sys::OBS_SOURCE_DO_NOT_DUPLICATE | sys::OBS_SOURCE_CUSTOM_DRAW;
            info.video_render = Some(shared::video_render);
            info.get_width = Some(shared::get_width);
            info.get_height = Some(shared::get_height);
        }
        info.get_name = Some(get_name);
        info.create = Some(create);
        info.destroy = Some(destroy);
        info.update = Some(update);
        info.show = Some(show);
        info.hide = Some(hide);
        info.video_tick = Some(video_tick);
        info.get_defaults = Some(get_defaults);
        sys::obs_register_source_s(&info, std::mem::size_of::<sys::obs_source_info>());
    });
}

/// Chromium's frames as shared IOSurfaces (macOS): the newest one is kept (retained, and marked
/// in use so Chromium doesn't paint into it again meanwhile) until libobs draws the next.
#[cfg(target_os = "macos")]
mod shared {
    use std::ffi::c_void;

    use parking_lot::Mutex;

    use super::Web;
    use crate::sys;

    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceIncrementUseCount(surface: *mut c_void);
        fn IOSurfaceDecrementUseCount(surface: *mut c_void);
        fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
        fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRetain(cf: *const c_void) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    /// A retained IOSurface, in use until dropped.
    struct Surface(*mut c_void);

    impl Surface {
        fn hold(surface: *mut c_void) -> Self {
            unsafe {
                CFRetain(surface);
                IOSurfaceIncrementUseCount(surface);
            }
            Surface(surface)
        }
        fn size(&self) -> (u32, u32) {
            unsafe { (IOSurfaceGetWidth(self.0) as u32, IOSurfaceGetHeight(self.0) as u32) }
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            unsafe {
                IOSurfaceDecrementUseCount(self.0);
                CFRelease(self.0);
            }
        }
    }

    // SAFETY: IOSurfaces are reference-counted kernel objects usable from any thread.
    unsafe impl Send for Surface {}

    struct Texture(*mut sys::gs_texture_t);
    // SAFETY: only touched on libobs's graphics thread (destroyed there too).
    unsafe impl Send for Texture {}

    /// The surface on screen, the texture over it, its size.
    type Shown = (Option<Surface>, Option<Texture>, (u32, u32));

    #[derive(Default)]
    pub struct Frames {
        /// Painted, not yet drawn.
        next: Mutex<Option<Surface>>,
        /// What the texture shows (kept in use while it does), the texture, its size.
        shown: Mutex<Shown>,
    }

    impl Frames {
        /// A new frame (main thread, during Chromium's paint).
        pub fn put(&self, surface: *mut c_void) {
            if surface.is_null() {
                return;
            }
            let held = Surface::hold(surface);
            // the one it replaces (never drawn) goes back to Chromium
            *self.next.lock() = Some(held);
        }
    }

    impl Drop for Frames {
        fn drop(&mut self) {
            if let Some(Texture(t)) = self.shown.get_mut().1.take() {
                unsafe {
                    sys::obs_enter_graphics();
                    sys::gs_texture_destroy(t);
                    sys::obs_leave_graphics();
                }
            }
        }
    }

    fn frames<'a>(data: *mut c_void) -> &'a Frames {
        unsafe { &(*(data as *const Web)).shared }
    }

    pub unsafe extern "C" fn video_render(data: *mut c_void, _effect: *mut sys::gs_effect_t) {
        let f = frames(data);
        let mut shown = f.shown.lock();
        if let Some(next) = f.next.lock().take() {
            let size = next.size();
            let rebound = match &shown.1 {
                Some(Texture(t)) if shown.2 == size => unsafe { sys::gs_texture_rebind_iosurface(*t, next.0) },
                _ => false,
            };
            if !rebound {
                if let Some(Texture(t)) = shown.1.take() {
                    unsafe { sys::gs_texture_destroy(t) };
                }
                let t = unsafe { sys::gs_texture_create_from_iosurface(next.0) };
                shown.1 = (!t.is_null()).then_some(Texture(t));
            }
            shown.2 = size;
            // the previous surface goes back to Chromium now that the texture shows this one
            shown.0 = Some(next);
        }
        let Some(Texture(tex)) = &shown.1 else { return };
        let (w, h) = shown.2;
        unsafe {
            // an IOSurface is a rectangle texture; Chromium's pixels are premultiplied (the
            // blend says so)
            // (Metal has no rectangle textures)
            let rect = sys::gs_get_device_type() as u32 != sys::GS_DEVICE_METAL;
            let effect = sys::obs_get_base_effect(if rect { sys::OBS_EFFECT_DEFAULT_RECT } else { sys::OBS_EFFECT_DEFAULT });
            let image = sys::gs_effect_get_param_by_name(effect, c"image".as_ptr());
            sys::gs_effect_set_texture(image, *tex);
            sys::gs_blend_state_push();
            sys::gs_blend_function(sys::GS_BLEND_ONE, sys::GS_BLEND_INVSRCALPHA);
            while sys::gs_effect_loop(effect, c"Draw".as_ptr()) {
                sys::gs_draw_sprite(*tex, 0, w, h);
            }
            sys::gs_blend_state_pop();
        }
    }

    fn size(data: *mut c_void) -> (u32, u32) {
        let web = unsafe { &*(data as *const Web) };
        let shown = web.shared.shown.lock().2;
        if shown.0 > 0 {
            shown
        } else {
            let o = web.options.lock();
            (o.width, o.height)
        }
    }

    pub unsafe extern "C" fn get_width(data: *mut c_void) -> u32 {
        size(data).0
    }

    pub unsafe extern "C" fn get_height(data: *mut c_void) -> u32 {
        size(data).1
    }
}

/// Chromium's frames as Direct3D 11 shared textures (Windows): Chromium hands each paint's
/// texture over as an NT handle valid during its paint call, so it's duplicated there and
/// opened by libobs (on its own device) when it next draws; the texture opened last is drawn
/// until a newer one arrives.
#[cfg(windows)]
mod shared {
    use std::ffi::c_void;

    use parking_lot::Mutex;
    use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use super::Web;
    use crate::sys;

    /// A handle of our own to a shared texture, closed when dropped.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    // SAFETY: a kernel handle, usable from any thread.
    unsafe impl Send for Handle {}

    struct Texture(*mut sys::gs_texture_t);
    // SAFETY: only touched on libobs's graphics thread (destroyed there too).
    unsafe impl Send for Texture {}

    #[derive(Default)]
    pub struct Frames {
        /// Painted, not yet opened.
        next: Mutex<Option<Handle>>,
        /// The texture drawn, and its size.
        shown: Mutex<(Option<Texture>, (u32, u32))>,
    }

    impl Frames {
        /// A new frame (Chromium's thread, during its paint): `handle` is Chromium's.
        pub fn put(&self, handle: *mut c_void) {
            if handle.is_null() {
                return;
            }
            let mut own: HANDLE = std::ptr::null_mut();
            let ok = unsafe {
                let me = GetCurrentProcess();
                DuplicateHandle(me, handle as HANDLE, me, &mut own, 0, 0, DUPLICATE_SAME_ACCESS)
            };
            if ok != 0 && !own.is_null() {
                // the one it replaces (never drawn) is let go
                *self.next.lock() = Some(Handle(own));
            }
        }
    }

    impl Drop for Frames {
        fn drop(&mut self) {
            if let Some(Texture(t)) = self.shown.get_mut().0.take() {
                unsafe {
                    sys::obs_enter_graphics();
                    sys::gs_texture_destroy(t);
                    sys::obs_leave_graphics();
                }
            }
        }
    }

    fn frames<'a>(data: *mut c_void) -> &'a Frames {
        unsafe { &(*(data as *const Web)).shared }
    }

    pub unsafe extern "C" fn video_render(data: *mut c_void, _effect: *mut sys::gs_effect_t) {
        let f = frames(data);
        let mut shown = f.shown.lock();
        if let Some(next) = f.next.lock().take() {
            // (NT handles are small numbers: libobs takes them as 32 bits)
            let t = unsafe { sys::gs_texture_open_nt_shared(next.0 as usize as u32) };
            if !t.is_null() {
                if let Some(Texture(old)) = shown.0.take() {
                    unsafe { sys::gs_texture_destroy(old) };
                }
                shown.1 = unsafe { (sys::gs_texture_get_width(t), sys::gs_texture_get_height(t)) };
                shown.0 = Some(Texture(t));
            }
        }
        let Some(Texture(tex)) = &shown.0 else { return };
        let (w, h) = shown.1;
        unsafe {
            // Chromium's pixels are premultiplied (the blend says so)
            let effect = sys::obs_get_base_effect(sys::OBS_EFFECT_DEFAULT);
            let image = sys::gs_effect_get_param_by_name(effect, c"image".as_ptr());
            sys::gs_effect_set_texture(image, *tex);
            sys::gs_blend_state_push();
            sys::gs_blend_function(sys::GS_BLEND_ONE, sys::GS_BLEND_INVSRCALPHA);
            while sys::gs_effect_loop(effect, c"Draw".as_ptr()) {
                sys::gs_draw_sprite(*tex, 0, w, h);
            }
            sys::gs_blend_state_pop();
        }
    }

    fn size(data: *mut c_void) -> (u32, u32) {
        let web = unsafe { &*(data as *const Web) };
        let shown = web.shared.shown.lock().1;
        if shown.0 > 0 {
            shown
        } else {
            let o = web.options.lock();
            (o.width, o.height)
        }
    }

    pub unsafe extern "C" fn get_width(data: *mut c_void) -> u32 {
        size(data).0
    }

    pub unsafe extern "C" fn get_height(data: *mut c_void) -> u32 {
        size(data).1
    }
}
