//! The pointer, clicks, scrolls and keys, for Studio takes and live cursor effects
//! (docs/plans/screen-studio.md, 4.1 "Input recorder").
//!
//! - **Pointer** position (global display points, top-left origin): no permission on either
//!   platform.
//! - **Clicks, scrolls and keys:** Windows reads them through Raw Input (no permission). macOS needs
//!   Input Monitoring for a listen-only event tap: [`can_listen`] checks without asking, and
//!   Lightair never changes the setting itself ([`open_input_monitoring_settings`] shows the
//!   pane). Without it the recorder logs the pointer only.
//!
//! Keys are logged as key codes and modifiers, never as text.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A pointer shape Lightair can redraw (its own set, per platform look).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Shape {
    Arrow = 0,
    IBeam = 1,
    Hand = 2,
    Crosshair = 3,
    ResizeHorizontal = 4,
    ResizeVertical = 5,
    Wait = 6,
    NotAllowed = 7,
    Move = 8,
}

impl Shape {
    pub fn from_u8(v: u8) -> Shape {
        match v {
            1 => Shape::IBeam,
            2 => Shape::Hand,
            3 => Shape::Crosshair,
            4 => Shape::ResizeHorizontal,
            5 => Shape::ResizeVertical,
            6 => Shape::Wait,
            7 => Shape::NotAllowed,
            8 => Shape::Move,
            _ => Shape::Arrow,
        }
    }
}

/// Modifier bits for keys.
pub mod modifiers {
    pub const SHIFT: u16 = 1;
    pub const CONTROL: u16 = 2;
    pub const ALT: u16 = 4;
    /// ⌘ on macOS, the Windows key on Windows.
    pub const COMMAND: u16 = 8;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Move { x: f32, y: f32 },
    /// Button 0 left, 1 right, 2 middle.
    Down { button: u8, x: f32, y: f32 },
    Up { button: u8, x: f32, y: f32 },
    Scroll { dx: f32, dy: f32 },
    /// The platform's key code and the modifiers held.
    KeyDown { code: u16, modifiers: u16 },
    KeyUp { code: u16, modifiers: u16 },
    Shape(Shape),
}

/// An event and when it happened, from the recorder's start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: Duration,
    pub event: Event,
}

/// The pointer, in global display points (top-left origin).
pub fn pointer() -> (f32, f32) {
    imp::pointer()
}

/// The main display's frame in global points: (x, y, width, height).
pub fn main_display() -> (f32, f32, f32, f32) {
    imp::main_display()
}

/// A display's frame in global points, by the id its screen capture uses (macOS: the display's
/// UUID); `None` when it isn't connected or can't be told (Windows: the capture's own monitor id).
pub fn display_frame(display: &str) -> Option<(f32, f32, f32, f32)> {
    imp::display_frame(display)
}

/// A window's frame in global points now, by the id its window capture uses (macOS: the
/// window's number; Windows: libobs's `title:class:exe`); `None` when it's gone.
pub fn window_frame(window: &str) -> Option<(f32, f32, f32, f32)> {
    imp::window_frame(window)
}

/// The pointer's shape now (an app's own pointer counts as the arrow).
pub fn shape() -> Shape {
    imp::shape()
}

/// Whether clicks, scrolls and keys can be read: always on Windows; on macOS when Input
/// Monitoring is on for Lightair (checked without asking).
pub fn can_listen() -> bool {
    imp::can_listen()
}

/// Shows the Input Monitoring pane of the OS's privacy settings (macOS), where the user turns
/// it on; Lightair never turns it on itself. Nothing to show on Windows.
pub fn open_input_monitoring_settings() {
    imp::open_settings()
}

/// Asks the OS for Input Monitoring (macOS), only when the user asks for it (a button): the
/// first time, macOS shows its own prompt; either way Lightair is then listed in the pane, off,
/// for the user to turn on. True if it's already on. Always true on Windows.
pub fn request_listen() -> bool {
    imp::request_listen()
}

/// Records input until stopped (or dropped): the pointer at `hz` (moves only), shape changes,
/// and clicks, scrolls and keys when [`can_listen`].
pub struct Recorder {
    running: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<Sample>>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub listening: bool,
}

impl Recorder {
    pub fn start(hz: u32) -> Recorder {
        let running = Arc::new(AtomicBool::new(true));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();
        let mut threads = Vec::new();
        {
            let (running, samples) = (running.clone(), samples.clone());
            let period = Duration::from_secs_f64(1.0 / hz.clamp(10, 240) as f64);
            threads.push(
                std::thread::Builder::new()
                    .name("lightair-pointer".into())
                    .spawn(move || {
                        let mut last = None;
                        let mut last_shape = None;
                        while running.load(Ordering::Relaxed) {
                            let at = started.elapsed();
                            let p = pointer();
                            let s = shape();
                            let mut out = samples.lock().unwrap();
                            if last != Some(p) {
                                out.push(Sample { at, event: Event::Move { x: p.0, y: p.1 } });
                                last = Some(p);
                            }
                            if last_shape != Some(s) {
                                out.push(Sample { at, event: Event::Shape(s) });
                                last_shape = Some(s);
                            }
                            drop(out);
                            std::thread::sleep(period);
                        }
                    })
                    .expect("pointer thread"),
            );
        }
        let listening = can_listen();
        if listening && let Some(t) = imp::listen(started, running.clone(), samples.clone()) {
            threads.push(t);
        }
        Recorder { running, samples, threads, listening }
    }

    /// The samples since the last call, in time order.
    pub fn take(&self) -> Vec<Sample> {
        let mut v = std::mem::take(&mut *self.samples.lock().unwrap());
        v.sort_by_key(|s| s.at);
        v
    }

    /// Stops, and returns what's left.
    pub fn stop(mut self) -> Vec<Sample> {
        self.halt();
        self.take()
    }

    fn halt(&mut self) {
        // (before waking: a thread not yet waiting sees it, `imp::listen`)
        self.running.store(false, Ordering::SeqCst);
        imp::wake();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.halt();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Event, Sample, Shape, modifiers};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    type CFMachPortRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CGEventRef = *mut c_void;
    type TapCallback = extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> CGEventRef;
        fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
        fn CGEventGetFlags(event: CGEventRef) -> u64;
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            mask: u64,
            callback: TapCallback,
            info: *mut c_void,
        ) -> CFMachPortRef;
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
        fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: CFMachPortRef, order: isize) -> CFRunLoopSourceRef;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
        fn CFRunLoopRun();
        fn CFRunLoopStop(rl: CFRunLoopRef);
        static kCFRunLoopCommonModes: *const c_void;
    }

    pub fn pointer() -> (f32, f32) {
        unsafe {
            let e = CGEventCreate(std::ptr::null());
            if e.is_null() {
                return (0.0, 0.0);
            }
            let p = CGEventGetLocation(e);
            CFRelease(e);
            (p.x as f32, p.y as f32)
        }
    }

    pub fn shape() -> Shape {
        crate::system_pointer::now().and_then(|p| p.shape).unwrap_or(Shape::Arrow)
    }

    #[repr(C)]
    struct CGRect {
        origin: CGPoint,
        size: CGPoint,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }

    pub fn main_display() -> (f32, f32, f32, f32) {
        let r = unsafe { CGDisplayBounds(CGMainDisplayID()) };
        (r.origin.x as f32, r.origin.y as f32, r.size.x as f32, r.size.y as f32)
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
        fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut CGRect) -> bool;
    }
    #[link(name = "ColorSync", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFUUIDCreateString(alloc: *const c_void, uuid: *const c_void) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, buf: *mut std::ffi::c_char, size: isize, encoding: u32) -> bool;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const std::ffi::c_char, encoding: u32) -> *const c_void;
    }
    const UTF8: u32 = 0x0800_0100;

    fn uuid_of(display: u32) -> Option<String> {
        unsafe {
            let uuid = CGDisplayCreateUUIDFromDisplayID(display);
            if uuid.is_null() {
                return None;
            }
            let text = CFUUIDCreateString(std::ptr::null(), uuid);
            let mut buf = [0 as std::ffi::c_char; 64];
            let ok = !text.is_null() && CFStringGetCString(text, buf.as_mut_ptr(), buf.len() as isize, UTF8);
            if !text.is_null() {
                CFRelease(text);
            }
            CFRelease(uuid);
            ok.then(|| std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }

    pub fn display_frame(display: &str) -> Option<(f32, f32, f32, f32)> {
        let mut ids = [0u32; 16];
        let mut n = 0u32;
        if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } != 0 {
            return None;
        }
        let id = ids[..n as usize].iter().copied().find(|d| uuid_of(*d).is_some_and(|u| u.eq_ignore_ascii_case(display)))?;
        let r = unsafe { CGDisplayBounds(id) };
        Some((r.origin.x as f32, r.origin.y as f32, r.size.x as f32, r.size.y as f32))
    }

    pub fn window_frame(window: &str) -> Option<(f32, f32, f32, f32)> {
        let number: u32 = window.parse().ok()?;
        unsafe {
            // kCGWindowListOptionIncludingWindow: just that window
            let list = CGWindowListCopyWindowInfo(1 << 3, number);
            if list.is_null() {
                return None;
            }
            let mut rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGPoint { x: 0.0, y: 0.0 } };
            let found = CFArrayGetCount(list) > 0 && {
                let info = CFArrayGetValueAtIndex(list, 0);
                let key = CFStringCreateWithCString(std::ptr::null(), c"kCGWindowBounds".as_ptr(), UTF8);
                let bounds = CFDictionaryGetValue(info, key);
                CFRelease(key);
                !bounds.is_null() && CGRectMakeWithDictionaryRepresentation(bounds, &mut rect)
            };
            CFRelease(list);
            found.then_some((rect.origin.x as f32, rect.origin.y as f32, rect.size.x as f32, rect.size.y as f32))
        }
    }

    pub fn can_listen() -> bool {
        unsafe { CGPreflightListenEventAccess() }
    }

    pub fn request_listen() -> bool {
        unsafe { CGRequestListenEventAccess() }
    }

    pub fn open_settings() {
        let _ = std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            .spawn();
    }

    struct Tap {
        started: Instant,
        samples: Arc<Mutex<Vec<Sample>>>,
    }

    static LOOP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    // event types
    const L_DOWN: u32 = 1;
    const L_UP: u32 = 2;
    const R_DOWN: u32 = 3;
    const R_UP: u32 = 4;
    const KEY_DOWN: u32 = 10;
    const KEY_UP: u32 = 11;
    const SCROLL: u32 = 22;
    const O_DOWN: u32 = 25;
    const O_UP: u32 = 26;
    // fields
    const KEYCODE: u32 = 9;
    const BUTTON: u32 = 3;
    const SCROLL_Y: u32 = 96;
    const SCROLL_X: u32 = 97;

    fn mods(flags: u64) -> u16 {
        let mut m = 0;
        if flags & 0x20000 != 0 {
            m |= modifiers::SHIFT;
        }
        if flags & 0x40000 != 0 {
            m |= modifiers::CONTROL;
        }
        if flags & 0x80000 != 0 {
            m |= modifiers::ALT;
        }
        if flags & 0x100000 != 0 {
            m |= modifiers::COMMAND;
        }
        m
    }

    extern "C" fn on_event(_: *mut c_void, kind: u32, event: CGEventRef, info: *mut c_void) -> CGEventRef {
        let tap = unsafe { &*(info as *const Tap) };
        let at = tap.started.elapsed();
        let p = unsafe { CGEventGetLocation(event) };
        let (x, y) = (p.x as f32, p.y as f32);
        let field = |f| unsafe { CGEventGetIntegerValueField(event, f) };
        let ev = match kind {
            L_DOWN => Some(Event::Down { button: 0, x, y }),
            L_UP => Some(Event::Up { button: 0, x, y }),
            R_DOWN => Some(Event::Down { button: 1, x, y }),
            R_UP => Some(Event::Up { button: 1, x, y }),
            O_DOWN => Some(Event::Down { button: field(BUTTON).clamp(2, 255) as u8, x, y }),
            O_UP => Some(Event::Up { button: field(BUTTON).clamp(2, 255) as u8, x, y }),
            SCROLL => Some(Event::Scroll { dx: field(SCROLL_X) as f32, dy: field(SCROLL_Y) as f32 }),
            KEY_DOWN => Some(Event::KeyDown { code: field(KEYCODE) as u16, modifiers: mods(unsafe { CGEventGetFlags(event) }) }),
            KEY_UP => Some(Event::KeyUp { code: field(KEYCODE) as u16, modifiers: mods(unsafe { CGEventGetFlags(event) }) }),
            _ => None,
        };
        if let Some(event) = ev {
            tap.samples.lock().unwrap().push(Sample { at, event });
        }
        event
    }

    pub fn listen(
        started: Instant,
        running: Arc<AtomicBool>,
        samples: Arc<Mutex<Vec<Sample>>>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("lightair-input".into())
            .spawn(move || unsafe {
                let tap = Box::into_raw(Box::new(Tap { started, samples }));
                let mask: u64 = [L_DOWN, L_UP, R_DOWN, R_UP, KEY_DOWN, KEY_UP, SCROLL, O_DOWN, O_UP]
                    .iter()
                    .fold(0, |m, t| m | (1u64 << t));
                // session tap, at the head, listen only
                let port = CGEventTapCreate(1, 0, 1, mask, on_event, tap as *mut c_void);
                if port.is_null() {
                    drop(Box::from_raw(tap));
                    return;
                }
                let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
                let rl = CFRunLoopGetCurrent();
                CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes);
                CGEventTapEnable(port, true);
                LOOP.store(rl, Ordering::SeqCst);
                if running.load(Ordering::Relaxed) {
                    CFRunLoopRun();
                }
                LOOP.store(std::ptr::null_mut(), Ordering::SeqCst);
                CGEventTapEnable(port, false);
                CFRelease(source);
                CFRelease(port);
                drop(Box::from_raw(tap));
            })
            .ok()
    }

    pub fn wake() {
        let rl = LOOP.load(Ordering::SeqCst);
        if !rl.is_null() {
            unsafe { CFRunLoopStop(rl) };
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{Event, Sample, Shape, modifiers};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK,
        RIM_TYPEKEYBOARD, RIM_TYPEMOUSE, RegisterRawInputDevices,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CURSORINFO, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorInfo,
        GetCursorPos, GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, IDC_APPSTARTING, IDC_ARROW, IDC_CROSS, IDC_HAND,
        IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENS, IDC_SIZEWE, IDC_WAIT, LoadCursorW, MSG, PostThreadMessageW,
        RI_KEY_BREAK, RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, RI_MOUSE_MIDDLE_BUTTON_DOWN,
        RI_MOUSE_MIDDLE_BUTTON_UP, RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, RI_MOUSE_WHEEL, RegisterClassW,
        SetWindowLongPtrW, WM_INPUT, WM_QUIT, WNDCLASSW,
    };

    pub fn pointer() -> (f32, f32) {
        let mut p = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut p) };
        (p.x as f32, p.y as f32)
    }

    pub fn shape() -> Shape {
        let mut info: CURSORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
        if unsafe { GetCursorInfo(&mut info) } == 0 {
            return Shape::Arrow;
        }
        let is = |id| unsafe { LoadCursorW(std::ptr::null_mut(), id) } == info.hCursor;
        [
            (IDC_IBEAM, Shape::IBeam),
            (IDC_HAND, Shape::Hand),
            (IDC_CROSS, Shape::Crosshair),
            (IDC_SIZEWE, Shape::ResizeHorizontal),
            (IDC_SIZENS, Shape::ResizeVertical),
            (IDC_WAIT, Shape::Wait),
            (IDC_APPSTARTING, Shape::Wait),
            (IDC_NO, Shape::NotAllowed),
            (IDC_SIZEALL, Shape::Move),
            (IDC_ARROW, Shape::Arrow),
        ]
        .into_iter()
        .find(|(id, _)| is(*id))
        .map_or(Shape::Arrow, |(_, s)| s)
    }

    pub fn main_display() -> (f32, f32, f32, f32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        (0.0, 0.0, w as f32, h as f32)
    }

    pub fn display_frame(_: &str) -> Option<(f32, f32, f32, f32)> {
        None
    }

    pub fn window_frame(window: &str) -> Option<(f32, f32, f32, f32)> {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect};
        // libobs's `title:class:exe`, `#3A` for ":" and `#22` for "#" inside each part
        let mut parts = window.splitn(3, ':').map(|p| p.replace("#3A", ":").replace("#22", "#"));
        let (title, class) = (parts.next()?, parts.next()?);
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (title, class) = (wide(&title), wide(&class));
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
            if hwnd.is_null() {
                return None;
            }
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            (GetWindowRect(hwnd, &mut r) != 0).then(|| (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32))
        }
    }

    pub fn can_listen() -> bool {
        true
    }

    pub fn open_settings() {}

    pub fn request_listen() -> bool {
        true
    }

    struct Sink {
        started: Instant,
        samples: Arc<Mutex<Vec<Sample>>>,
    }

    static THREAD: AtomicU32 = AtomicU32::new(0);

    fn mods() -> u16 {
        let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
        let mut m = 0;
        if down(VK_SHIFT) {
            m |= modifiers::SHIFT;
        }
        if down(VK_CONTROL) {
            m |= modifiers::CONTROL;
        }
        if down(VK_MENU) {
            m |= modifiers::ALT;
        }
        if down(VK_LWIN) || down(VK_RWIN) {
            m |= modifiers::COMMAND;
        }
        m
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_INPUT {
            let sink = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Sink).as_ref() };
            if let Some(sink) = sink {
                let mut raw: RAWINPUT = unsafe { std::mem::zeroed() };
                let mut size = std::mem::size_of::<RAWINPUT>() as u32;
                let got = unsafe {
                    GetRawInputData(
                        l as HRAWINPUT,
                        RID_INPUT,
                        &mut raw as *mut _ as *mut _,
                        &mut size,
                        std::mem::size_of::<RAWINPUTHEADER>() as u32,
                    )
                };
                if got != u32::MAX {
                    let at = sink.started.elapsed();
                    let (x, y) = pointer();
                    let mut events = Vec::new();
                    if raw.header.dwType == RIM_TYPEMOUSE {
                        let m = unsafe { raw.data.mouse };
                        let flags = unsafe { m.Anonymous.Anonymous.usButtonFlags } as u32;
                        let data = unsafe { m.Anonymous.Anonymous.usButtonData } as i16;
                        for (flag, ev) in [
                            (RI_MOUSE_LEFT_BUTTON_DOWN, Event::Down { button: 0, x, y }),
                            (RI_MOUSE_LEFT_BUTTON_UP, Event::Up { button: 0, x, y }),
                            (RI_MOUSE_RIGHT_BUTTON_DOWN, Event::Down { button: 1, x, y }),
                            (RI_MOUSE_RIGHT_BUTTON_UP, Event::Up { button: 1, x, y }),
                            (RI_MOUSE_MIDDLE_BUTTON_DOWN, Event::Down { button: 2, x, y }),
                            (RI_MOUSE_MIDDLE_BUTTON_UP, Event::Up { button: 2, x, y }),
                        ] {
                            if flags & flag != 0 {
                                events.push(ev);
                            }
                        }
                        if flags & RI_MOUSE_WHEEL != 0 {
                            events.push(Event::Scroll { dx: 0.0, dy: data as f32 / 120.0 });
                        }
                    } else if raw.header.dwType == RIM_TYPEKEYBOARD {
                        let k = unsafe { raw.data.keyboard };
                        let up = (k.Flags as u32 & RI_KEY_BREAK) != 0;
                        let (code, modifiers) = (k.VKey, mods());
                        events.push(if up { Event::KeyUp { code, modifiers } } else { Event::KeyDown { code, modifiers } });
                    }
                    let mut out = sink.samples.lock().unwrap();
                    out.extend(events.into_iter().map(|event| Sample { at, event }));
                }
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn listen(
        started: Instant,
        running: Arc<AtomicBool>,
        samples: Arc<Mutex<Vec<Sample>>>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("lightair-input".into())
            .spawn(move || unsafe {
                let class = wide("LightairInput");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: GetModuleHandleW(std::ptr::null()),
                    lpszClassName: class.as_ptr(),
                    ..std::mem::zeroed()
                };
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    wc.hInstance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return;
                }
                let sink = Box::into_raw(Box::new(Sink { started, samples }));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, sink as isize);
                // mouse and keyboard, delivered in the background too
                let devices = [
                    RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd },
                    RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd },
                ];
                RegisterRawInputDevices(devices.as_ptr(), 2, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
                THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
                let mut msg: MSG = std::mem::zeroed();
                // stopped before the thread could be woken (`wake` found no thread yet, and
                // `running` is cleared before it looks): nothing will post WM_QUIT
                while running.load(Ordering::SeqCst) && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    DispatchMessageW(&msg);
                }
                THREAD.store(0, Ordering::SeqCst);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DestroyWindow(hwnd);
                drop(Box::from_raw(sink));
            })
            .ok()
    }

    pub fn wake() {
        let t = THREAD.load(Ordering::SeqCst);
        if t != 0 {
            unsafe { PostThreadMessageW(t, WM_QUIT, 0, 0) };
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::{Sample, Shape};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    pub fn pointer() -> (f32, f32) {
        (0.0, 0.0)
    }
    pub fn shape() -> Shape {
        Shape::Arrow
    }
    pub fn main_display() -> (f32, f32, f32, f32) {
        (0.0, 0.0, 1920.0, 1080.0)
    }
    pub fn display_frame(_: &str) -> Option<(f32, f32, f32, f32)> {
        None
    }
    pub fn window_frame(_: &str) -> Option<(f32, f32, f32, f32)> {
        None
    }
    pub fn can_listen() -> bool {
        false
    }
    pub fn open_settings() {}

    pub fn request_listen() -> bool {
        true
    }
    pub fn listen(_: Instant, _: Arc<AtomicBool>, _: Arc<Mutex<Vec<Sample>>>) -> Option<std::thread::JoinHandle<()>> {
        None
    }
    pub fn wake() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pointer_is_logged_without_any_permission() {
        // the position is read, and the recorder logs it from the start
        let r = super::Recorder::start(120);
        std::thread::sleep(std::time::Duration::from_millis(60));
        let samples = r.stop();
        assert!(samples.iter().any(|s| matches!(s.event, super::Event::Move { .. })), "{samples:?}");
        assert!(samples.windows(2).all(|w| w[0].at <= w[1].at));
    }
}
