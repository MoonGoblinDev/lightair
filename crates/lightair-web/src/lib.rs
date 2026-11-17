//! Web pages in Lightair: one Chromium on every platform (CEF's official build, pinned in this
//! crate's Cargo.toml and bundled with Lightair), so a page renders the same on macOS and
//! Windows.
//!
//! - Chromium starts the first time a page is opened, on the main thread, and is driven from
//!   Lightair's own loop (CEF's external message pump: [`pump`], called by the app). No window:
//!   each page renders off-screen at its own size (one CSS pixel a pixel) and every frame it
//!   paints (BGRA) goes to the page's sink, which hands it to the scene.
//! - Pages are asked for from any thread ([`Page::open`]); the main thread makes them.
//! - Chromium's helper processes are Lightair's own: the `lightair-web-helper` binary, which
//!   runs [`helper_main`] (on macOS, as Lightair's four helper apps inside its bundle).
//!
//! Where Chromium isn't bundled (tests run outside Lightair.app, the Windows check made from a
//! Mac), pages simply never paint ([`available`] says so).

use std::sync::Arc;

/// Where a page's frames go: premultiplied BGRA rows, `width × 4` bytes each, and the size.
pub type FrameSink = Arc<dyn Fn(&[u8], u32, u32) + Send + Sync>;

/// Where a page's frames go when Chromium shares them as textures (macOS: an `IOSurfaceRef`,
/// premultiplied BGRA, valid during the call: retain it to keep it), and its size. No pixel
/// crosses to the CPU (`Page::open_shared`).
pub type TextureSink = Arc<dyn Fn(*mut std::ffi::c_void, u32, u32) + Send + Sync>;

/// Where a page's sound goes: one plane of 32-bit float samples per channel (all the same
/// length), and the sample rate. Chromium sends sound here instead of playing it.
pub type AudioSink = Arc<dyn Fn(&[&[f32]], u32) + Send + Sync>;

/// Where an agent page's messages go (`Page::open_agent`): what its script sent.
pub type MessageSink = Arc<dyn Fn(String) + Send + Sync>;

/// What an agent page's script puts before a message for Lightair:
/// `console.log("\u0001lightair:" + text)`.
pub const MESSAGE_PREFIX: &str = "\u{1}lightair:";

/// Called when a window page is closed (by the person, or dropped).
pub type OnClosed = Arc<dyn Fn() + Send + Sync>;

/// Gets the cookies asked for: (name, value).
pub type CookiesDone = Box<dyn FnOnce(Vec<(String, String)>) + Send>;

/// A cookie to give Chromium (a sign-in brought over from another browser).
#[derive(Debug, Clone)]
pub struct CookieSpec {
    /// The address it's for ("https://www.twitch.tv").
    pub url: String,
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// Chromium's own time: microseconds since 1601 (0: for this session only).
    pub expires: i64,
    /// 0 unspecified, 1 none, 2 lax, 3 strict (Chromium's numbers + 1).
    pub same_site: i32,
}

/// Gives Chromium these cookies; `done` with how many it took.
pub fn set_cookies(list: Vec<CookieSpec>, done: impl FnOnce(usize) + Send + 'static) {
    imp::request(imp::Request::SetCookies { list, done: Box::new(done) });
}

static PROFILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The GPU Chromium renders on (Windows: an adapter LUID, "high,low"): the one libobs draws
/// with, so the textures it shares open on libobs's device (on a laptop with two GPUs Chromium
/// could otherwise pick the other).
static GPU_ADAPTER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Has Chromium render on the adapter with this LUID (before the first page opens).
pub fn set_gpu_adapter(high: i32, low: u32) {
    let _ = GPU_ADAPTER.set(format!("{high},{low}"));
}

/// Where Chromium keeps its profile (cookies, storage), so sign-ins last (set once, before the
/// first page; the default is a temporary folder).
pub fn set_profile_dir(dir: std::path::PathBuf) {
    let _ = PROFILE.set(dir);
}

/// The cookies Chromium would send to `url` (name, value), given to `done` (on Chromium's
/// thread; empty when there are none or Chromium isn't here).
pub fn cookies(url: &str, done: impl FnOnce(Vec<(String, String)>) + Send + 'static) {
    imp::request(imp::Request::Cookies { url: url.into(), done: Box::new(done) });
}

type Resolver = Box<dyn Fn(&str) -> String + Send + Sync>;
static RESOLVER: std::sync::OnceLock<Resolver> = std::sync::OnceLock::new();

/// Turns addresses of the app's own (a widget's `lightair-widget://…`) into what Chromium
/// loads, when a page opens or changes (set once, by the app).
pub fn set_url_resolver(f: impl Fn(&str) -> String + Send + Sync + 'static) {
    let _ = RESOLVER.set(Box::new(f));
}

fn resolved(mut o: PageOptions) -> PageOptions {
    if let Some(r) = RESOLVER.get() {
        o.url = r(&o.url);
    }
    o
}

/// A page's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct PageOptions {
    pub url: String,
    pub width: u32,
    pub height: u32,
    /// Frames a second at most (Chromium paints only what changes).
    pub fps: u32,
    /// Page zoom, as a browser's (1.0: 100%): the page lays out `width / zoom` CSS pixels
    /// wide, drawn sharp at `zoom` pixels each, so the frame stays `width × height`.
    pub zoom: f32,
    /// Style sheet added to the page each time it loads (after its own, so it wins ties).
    pub css: String,
}

impl PageOptions {
    /// The page's view in CSS pixels (what it lays out in).
    pub fn view(&self) -> (u32, u32) {
        let z = self.zoom.clamp(0.25, 5.0);
        (((self.width as f32 / z).round() as u32).max(1), ((self.height as f32 / z).round() as u32).max(1))
    }
}

/// A web page rendering off-screen; closed when dropped.
pub struct Page {
    id: u64,
}

impl Page {
    /// A page that isn't drawn, for reading a site as a person's browser would (a platform's
    /// chat, its key page: docs/plans/live-integrations.md, 2.10). `script` runs after every
    /// load of its main frame; what it logs with [`MESSAGE_PREFIX`] goes to `messages`. It shares
    /// Lightair's cookies, so a site signed in to once stays signed in.
    pub fn open_agent(url: &str, script: String, messages: MessageSink) -> Page {
        Self::open_agent_early(url, String::new(), script, messages)
    }

    /// `open_agent`, with `early` run as each load starts (before the page's own scripts get
    /// far: to listen in on its connections).
    pub fn open_agent_early(url: &str, early: String, script: String, messages: MessageSink) -> Page {
        let id = imp::next_id();
        let options = PageOptions { url: url.into(), width: 1024, height: 768, fps: 1, zoom: 1.0, css: String::new() };
        let none: FrameSink = Arc::new(|_, _, _| {});
        let silent: AudioSink = Arc::new(|_, _| {});
        imp::request(imp::Request::Open { id, options, sink: none, texture: None, audio: silent, agent: Some((early, script, messages)), window: None });
        // never throttled as a hidden page would be
        imp::request(imp::Request::Visible { id, visible: true });
        Page { id }
    }

    /// A page in a window of its own, for a person to use (signing in to a platform): shares
    /// every page's cookies. `on_closed` runs when it's closed.
    pub fn open_window(url: &str, title: &str, size: (u32, u32), on_closed: OnClosed) -> Page {
        let id = imp::next_id();
        let options = PageOptions { url: url.into(), width: size.0, height: size.1, fps: 60, zoom: 1.0, css: String::new() };
        let none: FrameSink = Arc::new(|_, _, _| {});
        let silent: AudioSink = Arc::new(|_, _| {});
        imp::request(imp::Request::Open { id, options, sink: none, texture: None, audio: silent, agent: None, window: Some((title.into(), on_closed)) });
        Page { id }
    }

    /// Runs `code` in the page's main frame now.
    pub fn eval(&self, code: &str) {
        imp::request(imp::Request::Eval { id: self.id, code: code.into() });
    }

    /// Goes to `url`.
    pub fn navigate(&self, url: &str) {
        imp::request(imp::Request::Navigate { id: self.id, url: url.into() });
    }

    /// Opens `options` (made on the main thread soon after); its frames go to `sink`, its sound
    /// to `audio`.
    pub fn open(options: PageOptions, sink: FrameSink, audio: AudioSink) -> Page {
        let options = resolved(options);
        let id = imp::next_id();
        imp::request(imp::Request::Open { id, options, sink, texture: None, audio, agent: None, window: None });
        Page { id }
    }

    /// Like [`Page::open`], but where Chromium can share its frames as GPU textures (macOS:
    /// IOSurfaces; Windows: Direct3D 11 NT handles, valid during the call), they go to `texture`
    /// instead of being read back into memory for `sink`.
    pub fn open_shared(options: PageOptions, texture: TextureSink, sink: FrameSink, audio: AudioSink) -> Page {
        let options = resolved(options);
        let id = imp::next_id();
        let texture = (cfg!(target_os = "macos") || cfg!(windows)).then_some(texture);
        imp::request(imp::Request::Open { id, options, sink, texture, audio, agent: None, window: None });
        Page { id }
    }

    /// New settings: another address, size or rate.
    pub fn update(&self, options: PageOptions) {
        let options = resolved(options);
        imp::request(imp::Request::Update { id: self.id, options });
    }

    /// Loads the page again, skipping the cache.
    pub fn reload(&self) {
        imp::request(imp::Request::Reload { id: self.id });
    }

    /// Whether the page is on screen somewhere (on air, previewed). A hidden page doesn't paint
    /// (its sound plays on), as OBS's browser source; pages start hidden.
    pub fn set_visible(&self, visible: bool) {
        imp::request(imp::Request::Visible { id: self.id, visible });
    }

    /// Restyles the page now with `css` (while it's being typed; [`Page::update`] keeps it).
    pub fn restyle(&self, css: String) {
        imp::request(imp::Request::Style { id: self.id, css });
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        imp::request(imp::Request::Close { id: self.id });
    }
}

/// Whether Chromium is bundled here (pages can paint).
pub fn available() -> bool {
    imp::available()
}

/// Loads Chromium's library now, while this is the only thread. On macOS Chromium's allocator
/// takes over the process's memory zone as its library loads; loaded later, with libobs's threads
/// already allocating, a free racing the takeover crashed it now and then at startup. Call first
/// thing in `main` (cheap: the library is mapped, nothing starts).
pub fn preload() {
    imp::preload()
}

/// Runs Chromium's helper process when this process is one (`--type=` on its command line):
/// its exit code; `None` for any other process. The helper binary's `main` calls it first.
pub fn helper_main() -> Option<i32> {
    imp::helper_main()
}

/// Drives Chromium: makes, changes and closes the pages asked for, and does its pending work.
/// Main thread only, every few milliseconds (cheap while there's nothing to do).
pub fn pump() {
    imp::pump()
}

/// Whether [`pump`] should come back soon (pages are open, or about to be).
pub fn busy() -> bool {
    imp::busy()
}

/// How long until [`pump`] is due: when Chromium asked for its work to be done (it schedules
/// it as pages animate, load or play), at most 33 ms while pages are open (CEF's own fallback),
/// at once with requests waiting.
pub fn next_pump() -> std::time::Duration {
    imp::next_pump()
}

/// Called (from any thread) when [`pump`] is due sooner than [`next_pump`] last said: a request
/// came in, or Chromium asked for its work sooner. The pump's loop wakes on it.
pub fn set_waker(wake: impl Fn() + Send + Sync + 'static) {
    imp::set_waker(Box::new(wake))
}

#[cfg(any(target_os = "macos", all(windows, not(lightair_cross_check))))]
mod imp {
    use super::{AudioSink, CookieSpec, CookiesDone, FrameSink, MessageSink, OnClosed, PageOptions, TextureSink};
    use cef::*;
    use parking_lot::Mutex;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    pub enum Request {
        Open {
            id: u64,
            options: PageOptions,
            sink: FrameSink,
            texture: Option<TextureSink>,
            audio: AudioSink,
            /// An agent page: its script and where its messages go.
            agent: Option<(String, String, MessageSink)>,
            /// A page in a window of its own: its title, and who to tell when it closes.
            window: Option<(String, OnClosed)>,
        },
        Cookies { url: String, done: CookiesDone },
        SetCookies { list: Vec<CookieSpec>, done: Box<dyn FnOnce(usize) + Send> },
        Update { id: u64, options: PageOptions },
        Eval { id: u64, code: String },
        Navigate { id: u64, url: String },
        Reload { id: u64 },
        Style { id: u64, css: String },
        Visible { id: u64, visible: bool },
        Close { id: u64 },
    }

    static NEXT: AtomicU64 = AtomicU64::new(1);
    static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());
    static OPEN_PAGES: AtomicU64 = AtomicU64::new(0);
    static STARTED: AtomicBool = AtomicBool::new(false);
    /// Chromium's browser context is set up (pages can be made; before that, they wait).
    static READY: AtomicBool = AtomicBool::new(false);

    pub fn next_id() -> u64 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    pub fn request(r: Request) {
        if matches!(r, Request::Open { .. }) {
            OPEN_PAGES.fetch_add(1, Ordering::Relaxed);
        }
        REQUESTS.lock().push(r);
        wake();
    }

    pub fn busy() -> bool {
        OPEN_PAGES.load(Ordering::Relaxed) > 0 || !REQUESTS.lock().is_empty()
    }

    // ------------------------------------------------------------ pump scheduling

    /// When Chromium wants its work done next (nanoseconds after `EPOCH`; `u64::MAX`: not asked).
    static DUE: AtomicU64 = AtomicU64::new(u64::MAX);
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    static WAKER: Mutex<Option<Box<dyn Fn() + Send + Sync>>> = Mutex::new(None);

    fn now_ns() -> u64 {
        EPOCH.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
    }

    fn wake() {
        if let Some(w) = WAKER.lock().as_ref() {
            w();
        }
    }

    pub fn set_waker(wake: Box<dyn Fn() + Send + Sync>) {
        *WAKER.lock() = Some(wake);
    }

    /// Chromium asks for its work in `delay_ms` (any thread): the sooner of that and what's due.
    fn schedule(delay_ms: i64) {
        let at = now_ns() + delay_ms.max(0) as u64 * 1_000_000;
        if DUE.fetch_min(at, Ordering::AcqRel) > at {
            wake();
        }
    }

    pub fn next_pump() -> std::time::Duration {
        if !REQUESTS.lock().is_empty() {
            return std::time::Duration::ZERO;
        }
        let cap = if OPEN_PAGES.load(Ordering::Relaxed) > 0 { 33 } else { 1000 };
        let due = DUE.load(Ordering::Acquire);
        let wait = if due == u64::MAX { cap * 1_000_000 } else { due.saturating_sub(now_ns()).min(cap * 1_000_000) };
        std::time::Duration::from_nanos(wait)
    }

    // ------------------------------------------------------------ where Chromium is

    /// Chromium's framework (macOS): Lightair.app/Contents/Frameworks, next to the executable's
    /// folder; from a helper app, three folders up.
    #[cfg(target_os = "macos")]
    fn framework(helper: bool) -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let up = if helper { "../../.." } else { "../Frameworks" };
        let path = exe.parent()?.join(up).join("Chromium Embedded Framework.framework");
        path.canonicalize().ok().filter(|p| p.exists())
    }

    pub fn available() -> bool {
        #[cfg(target_os = "macos")]
        return framework(false).is_some();
        #[cfg(windows)]
        return std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("libcef.dll").exists())).unwrap_or(false);
    }

    /// Loads Chromium's library (macOS loads it at run time, from the bundle), then declares the
    /// CEF API version this was built against, which CEF needs before anything else.
    pub fn preload() {
        if available() {
            load(false);
        }
    }

    fn load(helper: bool) -> bool {
        static LOADED: AtomicBool = AtomicBool::new(false);
        if LOADED.load(Ordering::Relaxed) {
            return true;
        }
        #[cfg(target_os = "macos")]
        let loaded = {
            use std::os::unix::ffi::OsStrExt;
            let Some(dir) = framework(helper) else { return false };
            let lib = dir.join("Chromium Embedded Framework");
            let Ok(name) = std::ffi::CString::new(lib.as_os_str().as_bytes()) else { return false };
            // (loaded once; kept for the process's life)
            unsafe { load_library(Some(&*name.as_ptr().cast())) == 1 }
        };
        #[cfg(windows)]
        let loaded = {
            let _ = helper;
            true
        };
        if loaded {
            let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
            LOADED.store(true, Ordering::Relaxed);
        }
        loaded
    }

    pub fn helper_main() -> Option<i32> {
        if !std::env::args().any(|a| a.starts_with("--type=")) {
            return None;
        }
        if !load(true) {
            return Some(1);
        }
        let args = args::Args::new();
        let code = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
        Some(code.max(0))
    }

    // ------------------------------------------------------------ the browser process

    /// Where pages look names up: DNS over HTTPS first (Cloudflare's, then Google's), the
    /// network's own resolver if neither answers. Some networks' resolvers hide sites Lightair
    /// needs (an Indonesian ISP sends kick.com to a block page).
    const DNS_OVER_HTTPS: &str = "https://chrome.cloudflare-dns.com/dns-query https://dns.google/dns-query{?dns}";
    /// Their hosts, looked at before Chromium starts (`probe_dns_over_https`).
    const DNS_OVER_HTTPS_HOSTS: [&str; 2] = ["chrome.cloudflare-dns.com:443", "dns.google:443"];

    /// Whether a DNS-over-HTTPS server answered: 0 not known yet, 1 yes, 2 no.
    static DNS_OVER_HTTPS_REACHABLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    /// Chromium's "secure" mode uses only the servers it's given, so on a network that blocks
    /// them every page (scene browser sources too) would stay blank; its "automatic" mode ignores
    /// servers of one's own. So: a connection to each first, off the main thread, and the
    /// network's resolver if neither answers. Pages wait for the answer (`pump`): tens of ms,
    /// sooner than Chromium is ready, except where they're blocked (at most 1.5 s, once).
    fn probe_dns_over_https() {
        std::thread::spawn(|| {
            use std::net::ToSocketAddrs;
            let tries: Vec<_> = DNS_OVER_HTTPS_HOSTS
                .iter()
                .map(|host| {
                    std::thread::spawn(move || {
                        host.to_socket_addrs()
                            .ok()
                            .and_then(|mut a| a.next())
                            .is_some_and(|addr| std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(1500)).is_ok())
                    })
                })
                .collect();
            let answers = tries.into_iter().any(|t| t.join().unwrap_or(false));
            DNS_OVER_HTTPS_REACHABLE.store(if answers { 1 } else { 2 }, Ordering::Relaxed);
            if !answers {
                tracing::info!("no DNS-over-HTTPS server answers here: web pages use the network's DNS");
            }
            wake();
        });
    }

    fn secure_dns() {
        let Some(prefs) = preference_manager_get_global() else { return };
        let mode = if DNS_OVER_HTTPS_REACHABLE.load(Ordering::Relaxed) == 1 { "secure" } else { "off" };
        tracing::debug!(mode, "Chromium's DNS over HTTPS");
        for (name, value) in [("dns_over_https.templates", DNS_OVER_HTTPS), ("dns_over_https.mode", mode)] {
            let Some(mut v) = value_create() else { continue };
            v.set_string(Some(&CefString::from(value)));
            // (an empty string goes to Chromium as no string at all, and the call is refused)
            let mut error = CefString::from(" ");
            if prefs.set_preference(Some(&CefString::from(name)), Some(&mut v), Some(&mut error)) != 1 {
                tracing::warn!(name, error = %error, "Chromium didn't take a DNS setting");
            }
        }
    }

    wrap_browser_process_handler! {
        struct LightairBrowserProcess;
        impl BrowserProcessHandler {
            fn on_context_initialized(&self) {
                tracing::debug!("Chromium ready for pages");
                READY.store(true, Ordering::Relaxed);
                wake();
            }
            fn on_schedule_message_pump_work(&self, delay_ms: i64) {
                schedule(delay_ms);
            }
        }
    }

    wrap_app! {
        struct LightairApp;
        impl App {
            fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
                Some(LightairBrowserProcess::new())
            }
            fn on_before_command_line_processing(&self, _process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
                let Some(line) = command_line else { return };
                // no window of its own, no keychain prompts, no first-run UI; sound plays without
                // a click (nobody clicks a page in a show)
                for switch in [
                    "use-mock-keychain",
                    "disable-features=MacAppCodeSignClone",
                    "no-first-run",
                    "no-default-browser-check",
                    "autoplay-policy=no-user-gesture-required",
                ]
                .into_iter()
                .map(String::from)
                // a developer's own, ";"-separated (host-resolver-rules, logging)
                .chain(super::GPU_ADAPTER.get().map(|luid| format!("use-adapter-luid={luid}")))
                .chain(std::env::var("LIGHTAIR_WEB_SWITCHES").unwrap_or_default().split(';').filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string()))
                {
                    match switch.split_once('=') {
                        Some((name, value)) => line.append_switch_with_value(Some(&CefString::from(name)), Some(&CefString::from(value))),
                        None => line.append_switch(Some(&CefString::from(switch.as_str()))),
                    }
                }
            }
        }
    }

    /// Chromium's cache (cookies, storage): one folder, which only one Chromium can use at a
    /// time. Another Lightair running alongside (a second copy, a test) gets one of its own,
    /// removed by a later start once that copy has gone.
    fn cache_dir() -> std::path::PathBuf {
        let (temp, shared) = match super::PROFILE.get() {
            Some(dir) => (dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(std::env::temp_dir), dir.clone()),
            None => (std::env::temp_dir(), std::env::temp_dir().join("lightair-web")),
        };
        #[cfg(not(target_os = "macos"))]
        let _ = temp;
        #[cfg(target_os = "macos")]
        {
            let alive = |pid: i32| pid > 0 && unsafe { libc::kill(pid, 0) } == 0;
            let me = std::process::id() as i32;
            // (Chromium's lock: a link to "host-pid")
            let holder = std::fs::read_link(shared.join("SingletonLock"))
                .ok()
                .and_then(|t| t.to_string_lossy().rsplit('-').next().and_then(|p| p.parse::<i32>().ok()));
            if holder.is_some_and(|pid| pid != me && alive(pid)) {
                for entry in std::fs::read_dir(&temp).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(pid) = name.strip_prefix("lightair-web-").and_then(|p| p.parse::<i32>().ok())
                        && pid != me
                        && !alive(pid)
                    {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
                return temp.join(format!("lightair-web-{me}"));
            }
        }
        shared
    }

    /// Starts Chromium (the first time a page is opened): false where it isn't bundled.
    fn start() -> bool {
        if STARTED.load(Ordering::Relaxed) {
            return true;
        }
        if !available() || !load(false) {
            tracing::warn!("Chromium isn't bundled here: web pages won't paint");
            return false;
        }
        probe_dns_over_https();
        #[cfg(target_os = "macos")]
        mac::adopt_app_protocol();
        let args = args::Args::new();
        let mut settings = Settings {
            no_sandbox: 1,
            external_message_pump: 1,
            windowless_rendering_enabled: 1,
            log_severity: LogSeverity::WARNING,
            // transparent where a page doesn't paint (overlays)
            background_color: 0x0000_0000,
            ..Default::default()
        };
        #[cfg(target_os = "macos")]
        {
            let exe = std::env::current_exe().unwrap_or_default();
            let frameworks = exe.parent().map(|d| d.join("../Frameworks")).unwrap_or_default();
            let bundle = exe.parent().and_then(|d| d.parent()).and_then(|d| d.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
            let helper = frameworks.join("Lightair Helper.app/Contents/MacOS/Lightair Helper");
            settings.browser_subprocess_path = CefString::from(helper.to_string_lossy().as_ref());
            settings.framework_dir_path = CefString::from(frameworks.join("Chromium Embedded Framework.framework").to_string_lossy().as_ref());
            settings.main_bundle_path = CefString::from(bundle.to_string_lossy().as_ref());
        }
        // Windows: Chromium's processes run as lightair-web-helper.exe beside Lightair (else
        // Chromium starts Lightair itself for each, which hands them to `helper_main`)
        #[cfg(windows)]
        if let Some(helper) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("lightair-web-helper.exe")))
            .filter(|h| h.exists())
        {
            settings.browser_subprocess_path = CefString::from(helper.to_string_lossy().as_ref());
        }
        let cache = cache_dir();
        settings.root_cache_path = CefString::from(cache.to_string_lossy().as_ref());
        let mut app = LightairApp::new();
        let ok = initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) == 1;
        if !ok {
            tracing::warn!(code = get_exit_code(), "Chromium didn't start");
            return false;
        }
        STARTED.store(true, Ordering::Relaxed);
        tracing::info!("Chromium started");
        true
    }

    // ------------------------------------------------------------ pages

    /// A page on the main thread: its browser (once made), its view (CSS size and zoom, read by
    /// the renderer).
    struct Live {
        browser: Option<Browser>,
        view: Arc<Mutex<((u32, u32), f32)>>,
        /// The custom style sheet, read by the load handler.
        css: Arc<Mutex<String>>,
        options: PageOptions,
        /// On screen somewhere (hidden pages don't paint).
        visible: bool,
    }

    /// `s` as a JavaScript string literal.
    fn js_string(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '<' => out.push_str("\\u003c"),
                '\u{2028}' => out.push_str("\\u2028"),
                '\u{2029}' => out.push_str("\\u2029"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// Puts `css` in the page's own style element (made the first time), last in the document.
    fn inject(frame: &Frame, css: &str) {
        let code = format!(
            "(function(){{var e=document.getElementById('lightair-css');if(!e){{e=document.createElement('style');e.id='lightair-css';(document.head||document.documentElement).appendChild(e);}}e.textContent={};}})();",
            js_string(css)
        );
        frame.execute_java_script(Some(&CefString::from(code.as_str())), Some(&CefString::from("lightair://style")), 0);
    }

    fn restyle(browser: &Browser, css: &str) {
        tracing::debug!(len = css.len(), "restyling a web page");
        if let Some(frame) = browser.main_frame() {
            inject(&frame, css);
        }
    }

    fn view_of(o: &PageOptions) -> ((u32, u32), f32) {
        (o.view(), o.zoom.clamp(0.25, 5.0))
    }

    thread_local! {
        static PAGES: RefCell<HashMap<u64, Live>> = RefCell::new(HashMap::new());
    }

    wrap_render_handler! {
        struct PageRender {
            view: Arc<Mutex<((u32, u32), f32)>>,
            sink: FrameSink,
            texture: Option<TextureSink>,
        }
        impl RenderHandler {
            fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
                if let Some(rect) = rect {
                    let ((w, h), _) = *self.view.lock();
                    *rect = Rect { x: 0, y: 0, width: w.max(1) as i32, height: h.max(1) as i32 };
                }
            }
            fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
                // pixels a CSS pixel: the page's zoom, whatever display Lightair is on
                if let Some(info) = info {
                    info.device_scale_factor = self.view.lock().1;
                    return 1;
                }
                0
            }
            fn on_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, _dirty: Option<&[Rect]>, buffer: *const u8, width: ::std::os::raw::c_int, height: ::std::os::raw::c_int) {
                if type_ != PaintElementType::VIEW || buffer.is_null() || width <= 0 || height <= 0 {
                    return;
                }
                let bytes = unsafe { std::slice::from_raw_parts(buffer, width as usize * height as usize * 4) };
                (self.sink)(bytes, width as u32, height as u32);
            }
            fn on_accelerated_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, _dirty: Option<&[Rect]>, info: Option<&AcceleratedPaintInfo>) {
                let (Some(texture), Some(info)) = (&self.texture, info) else { return };
                let surface = shared_surface(info);
                if type_ != PaintElementType::VIEW || surface.is_null() {
                    return;
                }
                let size = &info.extra.coded_size;
                texture(surface, size.width.max(0) as u32, size.height.max(0) as u32);
            }
        }
    }

    thread_local! {
        /// Window pages' close callbacks.
        static WINDOWS: RefCell<HashMap<u64, OnClosed>> = RefCell::new(HashMap::new());
    }

    wrap_life_span_handler! {
        struct PageLife {
            id: u64,
        }
        impl LifeSpanHandler {
            fn on_before_close(&self, _browser: Option<&mut Browser>) {
                let id = self.id;
                if let Some(f) = WINDOWS.with(|w| w.borrow_mut().remove(&id)) {
                    PAGES.with(|pages| pages.borrow_mut().remove(&id));
                    f();
                }
            }
            fn on_after_created(&self, browser: Option<&mut Browser>) {
                let Some(browser) = browser else { return };
                let id = self.id;
                tracing::debug!(id, "web page made");
                let browser = browser.clone();
                PAGES.with(|pages| match pages.borrow_mut().get_mut(&id) {
                    Some(live) => {
                        if let Some(host) = browser.host() {
                            host.was_hidden((!live.visible) as i32);
                        }
                        live.browser = Some(browser)
                    }
                    // closed before it was made
                    None => {
                        if let Some(host) = browser.host() {
                            host.close_browser(1);
                        }
                    }
                });
            }
        }
    }

    // A page's sound: asked for as 48 kHz stereo; Chromium says what it sends when a stream
    // starts (rate, channels), then sends planar float packets.
    wrap_audio_handler! {
        struct PageAudio {
            format: Arc<Mutex<(u32, usize)>>,
            sink: AudioSink,
        }
        impl AudioHandler {
            fn audio_parameters(&self, _browser: Option<&mut Browser>, params: Option<&mut AudioParameters>) -> ::std::os::raw::c_int {
                let Some(params) = params else { return 0 };
                params.channel_layout = ChannelLayout::LAYOUT_STEREO;
                params.sample_rate = 48_000;
                if params.frames_per_buffer <= 0 {
                    params.frames_per_buffer = 1024;
                }
                1
            }
            fn on_audio_stream_started(&self, _browser: Option<&mut Browser>, params: Option<&AudioParameters>, channels: ::std::os::raw::c_int) {
                let rate = params.map_or(48_000, |p| p.sample_rate.max(1) as u32);
                *self.format.lock() = (rate, channels.clamp(1, 8) as usize);
            }
            fn on_audio_stream_packet(&self, _browser: Option<&mut Browser>, data: *mut *const f32, frames: ::std::os::raw::c_int, _pts: i64) {
                let (rate, channels) = *self.format.lock();
                if data.is_null() || frames <= 0 {
                    return;
                }
                let mut planes: [&[f32]; 8] = [&[]; 8];
                for (c, plane) in planes.iter_mut().enumerate().take(channels) {
                    let ptr = unsafe { *data.add(c) };
                    if ptr.is_null() {
                        return;
                    }
                    *plane = unsafe { std::slice::from_raw_parts(ptr, frames as usize) };
                }
                (self.sink)(&planes[..channels], rate);
            }
        }
    }

    wrap_load_handler! {
        struct PageLoad {
            css: Arc<Mutex<String>>,
            // run as each load starts, before the page's own scripts get far (hooks)
            early: String,
            script: String,
        }
        impl LoadHandler {
            fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
                if let Some(frame) = frame
                    && frame.is_main() == 1
                    && !self.early.is_empty()
                {
                    frame.execute_java_script(Some(&CefString::from(self.early.as_str())), Some(&CefString::from("lightair://early")), 0);
                }
            }
            fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: ::std::os::raw::c_int) {
                let Some(frame) = frame else { return };
                if frame.is_main() != 1 {
                    return;
                }
                if !self.script.is_empty() {
                    tracing::debug!(url = %CefString::from(&frame.url()), status = http_status_code, "agent page loaded");
                }
                let css = self.css.lock().clone();
                if !css.trim().is_empty() {
                    inject(frame, &css);
                }
                if !self.script.is_empty() {
                    frame.execute_java_script(Some(&CefString::from(self.script.as_str())), Some(&CefString::from("lightair://agent")), 0);
                }
            }
        }
    }

    /// Hands what was collected over when Chromium lets go of the visitor (after the last
    /// cookie, or at once when there are none).
    struct CookieJar {
        found: Mutex<Vec<(String, String)>>,
        done: Mutex<Option<CookiesDone>>,
    }

    impl Drop for CookieJar {
        fn drop(&mut self) {
            if let Some(done) = self.done.get_mut().take() {
                done(std::mem::take(self.found.get_mut()));
            }
        }
    }

    wrap_cookie_visitor! {
        struct CookieCollector {
            jar: Arc<CookieJar>,
        }
        impl CookieVisitor {
            fn visit(&self, cookie: Option<&Cookie>, _count: ::std::os::raw::c_int, _total: ::std::os::raw::c_int, _delete_cookie: Option<&mut ::std::os::raw::c_int>) -> ::std::os::raw::c_int {
                if let Some(c) = cookie {
                    self.jar.found.lock().push((c.name.to_string(), c.value.to_string()));
                }
                1
            }
        }
    }

    fn set_cookies(list: Vec<CookieSpec>, done: Box<dyn FnOnce(usize) + Send>) {
        let Some(manager) = cookie_manager_get_global_manager(None) else {
            done(0);
            return;
        };
        let mut taken = 0;
        for c in list {
            let cookie = Cookie {
                name: CefString::from(c.name.as_str()),
                value: CefString::from(c.value.as_str()),
                domain: CefString::from(c.domain.as_str()),
                path: CefString::from(c.path.as_str()),
                secure: c.secure as i32,
                httponly: c.http_only as i32,
                has_expires: (c.expires > 0) as i32,
                expires: Basetime { val: c.expires },
                same_site: match c.same_site {
                    1 => CookieSameSite::NO_RESTRICTION,
                    2 => CookieSameSite::LAX_MODE,
                    3 => CookieSameSite::STRICT_MODE,
                    _ => CookieSameSite::UNSPECIFIED,
                },
                ..Default::default()
            };
            if manager.set_cookie(Some(&CefString::from(c.url.as_str())), Some(&cookie), None) == 1 {
                taken += 1;
            }
        }
        done(taken);
    }

    fn cookies(url: String, done: CookiesDone) {
        let jar = Arc::new(CookieJar { found: Mutex::new(Vec::new()), done: Mutex::new(Some(done)) });
        if let Some(manager) = cookie_manager_get_global_manager(None) {
            let mut visitor = CookieCollector::new(jar.clone());
            manager.visit_url_cookies(Some(&CefString::from(url.as_str())), 1, Some(&mut visitor));
        }
        // the visitor's copy (Chromium's) and this one: the last to go answers
        drop(jar);
    }

    // An agent page's messages: console lines with Lightair's prefix (the rest are the page's own)
    wrap_display_handler! {
        struct PageDisplay {
            messages: Option<MessageSink>,
        }
        impl DisplayHandler {
            fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, _source: Option<&CefString>, _line: ::std::os::raw::c_int) -> ::std::os::raw::c_int {
                let Some(message) = message else { return 0 };
                let text = message.to_string();
                let Some(sink) = &self.messages else {
                    tracing::debug!(%text, line = _line, "page console");
                    return 0;
                };
                match text.strip_prefix(crate::MESSAGE_PREFIX) {
                    Some(m) => {
                        sink(m.to_string());
                        1
                    }
                    None => {
                        tracing::debug!(%text, "agent page console");
                        0
                    }
                }
            }
        }
    }

    wrap_client! {
        struct PageClient {
            render: RenderHandler,
            life: LifeSpanHandler,
            audio: AudioHandler,
            load: LoadHandler,
            display: DisplayHandler,
        }
        impl Client {
            fn display_handler(&self) -> Option<DisplayHandler> {
                Some(self.display.clone())
            }
            fn load_handler(&self) -> Option<LoadHandler> {
                Some(self.load.clone())
            }
            fn audio_handler(&self) -> Option<AudioHandler> {
                Some(self.audio.clone())
            }
            fn render_handler(&self) -> Option<RenderHandler> {
                Some(self.render.clone())
            }
            fn life_span_handler(&self) -> Option<LifeSpanHandler> {
                Some(self.life.clone())
            }
        }
    }

    /// The IOSurface a shared-texture paint is in (macOS only).
    #[cfg(target_os = "macos")]
    fn shared_surface(info: &AcceleratedPaintInfo) -> *mut std::ffi::c_void {
        info.shared_texture_io_surface
    }

    /// The Direct3D 11 texture's NT handle (Windows), valid during the paint call.
    #[cfg(windows)]
    fn shared_surface(info: &AcceleratedPaintInfo) -> *mut std::ffi::c_void {
        info.shared_texture_handle
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    fn shared_surface(_: &AcceleratedPaintInfo) -> *mut std::ffi::c_void {
        std::ptr::null_mut()
    }

    #[allow(clippy::too_many_arguments)]
    fn open(id: u64, options: PageOptions, sink: FrameSink, texture: Option<TextureSink>, audio: AudioSink, agent: Option<(String, String, MessageSink)>, window_of: Option<(String, OnClosed)>) {
        let (early, script, messages) = agent.map_or((String::new(), String::new(), None), |(e, s, m)| (e, s, Some(m)));
        let view = Arc::new(Mutex::new(view_of(&options)));
        let format = Arc::new(Mutex::new((48_000, 2)));
        let css = Arc::new(Mutex::new(options.css.clone()));
        let mut client = PageClient::new(
            PageRender::new(view.clone(), sink, texture.clone()),
            PageLife::new(id),
            PageAudio::new(format, audio),
            PageLoad::new(css.clone(), early, script),
            PageDisplay::new(messages),
        );
        // shared textures: Chromium's GPU frames go to the scene as they are (no readback)
        let window = match &window_of {
            // a window of its own, in the middle of the screen
            Some((title, closed)) => {
                WINDOWS.with(|w| w.borrow_mut().insert(id, closed.clone()));
                WindowInfo {
                    window_name: CefString::from(title.as_str()),
                    bounds: Rect { x: 160, y: 120, width: options.width as i32, height: options.height as i32 },
                    runtime_style: RuntimeStyle::ALLOY,
                    ..Default::default()
                }
            }
            None => WindowInfo { windowless_rendering_enabled: 1, shared_texture_enabled: texture.is_some() as i32, ..Default::default() },
        };
        let browser_settings = BrowserSettings {
            windowless_frame_rate: options.fps.clamp(1, 60) as i32,
            background_color: 0x0000_0000,
            ..Default::default()
        };
        let url = CefString::from(options.url.as_str());
        let visible = window_of.is_some();
        PAGES.with(|pages| pages.borrow_mut().insert(id, Live { browser: None, view, css, options, visible }));
        if browser_host_create_browser(Some(&window), Some(&mut client), Some(&url), Some(&browser_settings), None, None) != 1 {
            tracing::warn!("Chromium didn't open a page");
        }
    }

    fn update(id: u64, options: PageOptions) {
        PAGES.with(|pages| {
            let mut pages = pages.borrow_mut();
            let Some(live) = pages.get_mut(&id) else { return };
            let before = std::mem::replace(&mut live.options, options.clone());
            *live.view.lock() = view_of(&options);
            *live.css.lock() = options.css.clone();
            let Some(browser) = &live.browser else { return };
            if before.css != options.css && before.url == options.url {
                restyle(browser, &options.css);
            }
            if let Some(host) = browser.host() {
                if before.zoom != options.zoom {
                    host.notify_screen_info_changed();
                }
                if (before.width, before.height, before.zoom) != (options.width, options.height, options.zoom) {
                    host.was_resized();
                }
                if before.fps != options.fps {
                    host.set_windowless_frame_rate(options.fps.clamp(1, 60) as i32);
                }
            }
            if before.url != options.url
                && let Some(frame) = browser.main_frame()
            {
                frame.load_url(Some(&CefString::from(options.url.as_str())));
            }
        });
    }

    fn reload(id: u64) {
        PAGES.with(|pages| {
            if let Some(browser) = pages.borrow().get(&id).and_then(|live| live.browser.clone()) {
                browser.reload_ignore_cache();
            }
        });
    }

    fn style(id: u64, css: String) {
        PAGES.with(|pages| {
            let pages = pages.borrow();
            let Some(live) = pages.get(&id) else { return };
            *live.css.lock() = css.clone();
            if let Some(browser) = &live.browser {
                restyle(browser, &css);
            }
        });
    }

    fn close(id: u64) {
        OPEN_PAGES.fetch_sub(1, Ordering::Relaxed);
        PAGES.with(|pages| {
            if let Some(live) = pages.borrow_mut().remove(&id)
                && let Some(host) = live.browser.and_then(|b| b.host())
            {
                host.close_browser(1);
            }
        });
    }

    pub fn pump() {
        if !REQUESTS.lock().is_empty() && !start() {
            // not bundled: forget them (their pages stay blank)
            REQUESTS.lock().clear();
            return;
        }
        if STARTED.load(Ordering::Relaxed) {
            // done now: Chromium asks again for what comes next
            DUE.store(u64::MAX, Ordering::Release);
            do_message_loop_work();
        }
        // pages are made once Chromium's context is ready (macOS insists), and its DNS is set
        if !READY.load(Ordering::Relaxed) || DNS_OVER_HTTPS_REACHABLE.load(Ordering::Relaxed) == 0 {
            return;
        }
        static DNS_SET: AtomicBool = AtomicBool::new(false);
        if !DNS_SET.swap(true, Ordering::Relaxed) {
            secure_dns();
        }
        let requests = std::mem::take(&mut *REQUESTS.lock());
        for r in requests {
            match r {
                Request::Open { id, options, sink, texture, audio, agent, window } => open(id, options, sink, texture, audio, agent, window),
                Request::Cookies { url, done } => cookies(url, done),
                Request::SetCookies { list, done } => set_cookies(list, done),
                Request::Eval { id, code } => PAGES.with(|pages| {
                    if let Some(frame) = pages.borrow().get(&id).and_then(|l| l.browser.as_ref()).and_then(|b| b.main_frame()) {
                        frame.execute_java_script(Some(&CefString::from(code.as_str())), Some(&CefString::from("lightair://agent")), 0);
                    }
                }),
                Request::Navigate { id, url } => PAGES.with(|pages| {
                    if let Some(frame) = pages.borrow().get(&id).and_then(|l| l.browser.as_ref()).and_then(|b| b.main_frame()) {
                        frame.load_url(Some(&CefString::from(url.as_str())));
                    }
                }),
                Request::Update { id, options } => update(id, options),
                Request::Reload { id } => reload(id),
                Request::Style { id, css } => style(id, css),
                Request::Visible { id, visible } => PAGES.with(|pages| {
                    let mut pages = pages.borrow_mut();
                    let Some(live) = pages.get_mut(&id) else { return };
                    live.visible = visible;
                    if let Some(host) = live.browser.as_ref().and_then(|b| b.host()) {
                        host.was_hidden((!visible) as i32);
                    }
                }),
                Request::Close { id } => close(id),
            }
        }
    }

    /// Chromium on macOS expects the application object to say whether it's in the middle of
    /// sending an event (CrAppControlProtocol); Lightair's window toolkit owns that object, so
    /// the two methods are added to its class.
    #[cfg(target_os = "macos")]
    mod mac {
        use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
        use objc2::{msg_send, sel};
        use std::sync::atomic::{AtomicBool, Ordering};

        static HANDLING: AtomicBool = AtomicBool::new(false);

        extern "C" fn is_handling(_: &AnyObject, _: Sel) -> Bool {
            Bool::new(HANDLING.load(Ordering::Relaxed))
        }

        extern "C" fn set_handling(_: &AnyObject, _: Sel, on: Bool) {
            HANDLING.store(on.as_bool(), Ordering::Relaxed);
        }

        unsafe extern "C" {
            fn class_addMethod(cls: *const AnyClass, name: Sel, imp: *const std::ffi::c_void, types: *const std::ffi::c_char) -> Bool;
            fn objc_getProtocol(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn class_addProtocol(cls: *const AnyClass, protocol: *const std::ffi::c_void) -> Bool;
        }

        pub fn adopt_app_protocol() {
            let Some(app_class) = AnyClass::get(c"NSApplication") else { return };
            let app: *mut AnyObject = unsafe { msg_send![app_class, sharedApplication] };
            if app.is_null() {
                return;
            }
            let class: *const AnyClass = unsafe { msg_send![app, class] };
            unsafe {
                class_addMethod(class, sel!(isHandlingSendEvent), is_handling as *const _, c"c@:".as_ptr());
                class_addMethod(class, sel!(setHandlingSendEvent:), set_handling as *const _, c"v@:c".as_ptr());
                for name in [c"CrAppProtocol", c"CrAppControlProtocol", c"CefAppProtocol"] {
                    let p = objc_getProtocol(name.as_ptr());
                    let added = !p.is_null() && class_addProtocol(class, p).as_bool();
                    let conforms: Bool = if p.is_null() { Bool::NO } else { msg_send![app, conformsToProtocol: p as *const AnyObject] };
                    tracing::debug!(protocol = ?name, found = !p.is_null(), added, conforms = conforms.as_bool(), class = ?(*class).name(), "Chromium app protocol");
                }
            }
        }
    }
}

/// No Chromium here (the Windows check made from a Mac): pages never paint.
#[cfg(not(any(target_os = "macos", all(windows, not(lightair_cross_check)))))]
mod imp {
    use super::{AudioSink, CookieSpec, CookiesDone, FrameSink, MessageSink, OnClosed, PageOptions, TextureSink};

    #[allow(dead_code)] // (asked for, never made here)
    pub enum Request {
        Open { id: u64, options: PageOptions, sink: FrameSink, texture: Option<TextureSink>, audio: AudioSink, agent: Option<(String, String, MessageSink)>, window: Option<(String, OnClosed)> },
        Cookies { url: String, done: CookiesDone },
        SetCookies { list: Vec<CookieSpec>, done: Box<dyn FnOnce(usize) + Send> },
        Update { id: u64, options: PageOptions },
        Eval { id: u64, code: String },
        Navigate { id: u64, url: String },
        Reload { id: u64 },
        Style { id: u64, css: String },
        Visible { id: u64, visible: bool },
        Close { id: u64 },
    }

    pub fn next_id() -> u64 {
        0
    }

    pub fn request(r: Request) {
        match r {
            Request::Cookies { done, .. } => done(Vec::new()),
            Request::SetCookies { done, .. } => done(0),
            _ => {}
        }
    }

    pub fn available() -> bool {
        false
    }

    pub fn preload() {}

    pub fn helper_main() -> Option<i32> {
        None
    }

    pub fn pump() {}

    pub fn busy() -> bool {
        false
    }

    pub fn next_pump() -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    pub fn set_waker(_: Box<dyn Fn() + Send + Sync>) {}
}
