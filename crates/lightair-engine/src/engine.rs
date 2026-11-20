use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::thread::JoinHandle;

use crossbeam_channel::{Sender, bounded, unbounded};
use libobs_sys as sys;

use crate::obs::{ObsScene, ObsSource};
use crate::runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    OpenGl,
    Metal,
    /// Windows.
    D3D11,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modules {
    /// Bundle when running inside Lightair.app, otherwise the plugin directory next to libobs.
    Auto,
    /// Core modules from the app bundle's `Contents/PlugIns` (`obs_load_core_modules`).
    Bundle,
    /// Every `<module>.plugin` in a directory (`obs_load_plugins`).
    Directory(PathBuf),
    /// Load nothing (tests of the engine itself).
    None,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub canvas: (u32, u32),
    pub output: (u32, u32),
    pub fps: (u32, u32),
    pub sample_rate: u32,
    pub graphics: Graphics,
    pub modules: Modules,
    /// Allowlist passed to `obs_add_safe_module`; nothing outside it loads.
    pub allowed_modules: Vec<String>,
    pub locale: String,
    /// Where OBS modules keep their own config (`obs_startup`'s module config path).
    pub module_config_dir: PathBuf,
    /// Further directories of `<module>.plugin` bundles (Lightair's in-tree modules).
    pub extra_plugin_dirs: Vec<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            canvas: (1920, 1080),
            output: (1920, 1080),
            fps: (60, 1),
            sample_rate: 48_000,
            graphics: if cfg!(windows) {
                Graphics::D3D11
            } else if std::env::var("LIGHTAIR_GRAPHICS").is_ok_and(|g| g.eq_ignore_ascii_case("metal")) {
                // OBS's Metal renderer (experimental there), for trying it
                Graphics::Metal
            } else {
                Graphics::OpenGl
            },
            modules: Modules::Auto,
            allowed_modules: crate::DEFAULT_MODULES
                .iter()
                .filter(|m| !skipped_modules().any(|skip| skip == **m))
                .map(|s| s.to_string())
                .collect(),
            locale: "en-US".into(),
            module_config_dir: lightair_platform::paths::data_dir().join("plugin_config"),
            extra_plugin_dirs: Vec::new(),
        }
    }
}

/// Modules left out of the default set, from `LIGHTAIR_SKIP_MODULES` (comma-separated). The
/// test runner (`cargo xtask test`) uses it to skip camera and capture-card modules, whose
/// start-up waits seconds on system services, in tests that don't use them.
fn skipped_modules() -> impl Iterator<Item = String> {
    std::env::var("LIGHTAIR_SKIP_MODULES")
        .unwrap_or_default()
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("libobs runtime not found: {0}")]
    Runtime(String),
    #[error("obs_startup failed")]
    Startup,
    #[error("audio reset failed")]
    Audio,
    #[error("video reset failed ({0})")]
    Video(&'static str),
    #[error("the engine thread stopped")]
    Stopped,
}

/// Source, filter, transition, encoder, output and service types available on this machine.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub sources: Vec<String>,
    pub filters: Vec<String>,
    pub transitions: Vec<String>,
    pub encoders: Vec<String>,
    pub outputs: Vec<String>,
    pub services: Vec<String>,
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub active_fps: f64,
    pub average_frame_time_ms: f64,
    pub total_frames: u32,
    pub lagged_frames: u32,
}

/// State owned by the engine thread. Jobs receive `&mut EngineState`.
#[derive(Default)]
pub struct EngineState {
    /// Ad-hoc libobs objects (tests, diagnostics). The document lives in `reconciler`.
    pub sources: HashMap<String, ObsSource>,
    pub scenes: HashMap<String, ObsScene>,
    /// libobs objects mirroring the Lightair document.
    pub reconciler: crate::Reconciler,
    /// Running recordings and streams.
    pub outputs: crate::outputs::Outputs,
    /// A Studio take's per-source recordings, until they've stopped.
    pub studio_take: Option<crate::studio_take::StudioTake>,
}

impl EngineState {
    /// Sets the scene that is on air on output channel 0 (the primary canvas).
    pub fn set_program(&mut self, scene: &str) -> bool {
        match self.scenes.get(scene) {
            Some(s) => {
                unsafe { sys::obs_set_output_source(0, s.source_ptr()) };
                true
            }
            None => false,
        }
    }

    pub fn stats(&self) -> Stats {
        unsafe {
            Stats {
                active_fps: sys::obs_get_active_fps(),
                average_frame_time_ms: sys::obs_get_average_frame_time_ns() as f64 / 1e6,
                total_frames: sys::obs_get_total_frames(),
                lagged_frames: sys::obs_get_lagged_frames(),
            }
        }
    }
}

/// A piece of work for the engine thread, and where it was asked for (named when it's slow).
type Job = (Box<dyn FnOnce(&mut EngineState) + Send>, &'static std::panic::Location<'static>);

/// A job (or a wait for one) longer than this is logged with where it came from: the UI waits
/// on the engine's jobs, so a slow one is a frozen window.
const SLOW_MS: u128 = 60;

/// Owner of libobs. Dropping it shuts the engine down on its thread.
pub struct Engine {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
    pub runtime: Runtime,
    pub capabilities: Capabilities,
    pub version: String,
}

impl Engine {
    /// Starts libobs on the engine thread and waits until it is ready.
    pub fn start(config: EngineConfig) -> Result<Self, EngineError> {
        let (tx, rx) = unbounded::<Job>();
        let (ready_tx, ready_rx) = bounded(1);
        let thread = std::thread::Builder::new()
            .name("lightair-engine".into())
            .spawn(move || match unsafe { bootstrap(&config) } {
                Ok(ready) => {
                    let _ = ready_tx.send(Ok(ready));
                    let mut state = EngineState::default();
                    for (job, at) in rx {
                        let t = std::time::Instant::now();
                        job(&mut state);
                        let ms = t.elapsed().as_millis();
                        if ms >= SLOW_MS {
                            tracing::warn!(ms, at = %at, "slow engine job");
                        }
                    }
                    unsafe { shutdown(state) };
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    unsafe {
                        if sys::obs_initialized() {
                            sys::obs_shutdown();
                        }
                    }
                }
            })
            .expect("spawn engine thread");
        let (runtime, capabilities, version) = ready_rx.recv().map_err(|_| EngineError::Stopped)??;
        Ok(Self { tx: Some(tx), thread: Some(thread), runtime, capabilities, version })
    }

    /// Runs `f` on the engine thread without waiting.
    #[track_caller]
    pub fn post(&self, f: impl FnOnce(&mut EngineState) + Send + 'static) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((Box::new(f), std::panic::Location::caller()));
        }
    }

    /// Runs `f` on the engine thread and returns its result (the caller waits: behind whatever
    /// the engine is doing first, too).
    #[track_caller]
    pub fn call<R: Send + 'static>(&self, f: impl FnOnce(&mut EngineState) -> R + Send + 'static) -> R {
        let at = std::panic::Location::caller();
        let t = std::time::Instant::now();
        let (tx, rx) = bounded(1);
        if let Some(q) = &self.tx {
            let _ = q.send((
                Box::new(move |s: &mut EngineState| {
                    let _ = tx.send(f(s));
                }),
                at,
            ));
        }
        let r = rx.recv().expect("engine thread stopped");
        let ms = t.elapsed().as_millis();
        if ms >= SLOW_MS {
            tracing::warn!(ms, at = %at, thread = std::thread::current().name().unwrap_or("?"), "waited on the engine");
        }
        r
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn c(s: &str) -> CString {
    CString::new(s).expect("NUL in string")
}

unsafe fn bootstrap(cfg: &EngineConfig) -> Result<(Runtime, Capabilities, String), EngineError> {
    crate::log::install();
    let runtime = Runtime::detect().map_err(EngineError::Runtime)?;
    tracing::info!(frameworks = %runtime.frameworks.display(), plugins = %runtime.plugins.display(),
        in_bundle = runtime.in_bundle, "libobs runtime");

    let _ = std::fs::create_dir_all(&cfg.module_config_dir);
    // Windows: OBS's modules find their own files from the working folder, OBS's `bin\64bit`
    // (game capture's hooks in `..\..\core\win-capture\data`), as OBS sets it when it starts
    #[cfg(windows)]
    if let Err(e) = std::env::set_current_dir(&runtime.frameworks) {
        tracing::warn!(%e, "couldn't work from libobs's folder: game capture may not hook");
    }
    let locale = c(&cfg.locale);
    let config_dir = c(&cfg.module_config_dir.to_string_lossy());
    if !unsafe { sys::obs_startup(locale.as_ptr(), config_dir.as_ptr(), std::ptr::null_mut()) } {
        return Err(EngineError::Startup);
    }
    // Windows: libobs looks for its effects in `../../data/libobs/` from the working folder,
    // which is OBS's own `bin\64bit` only when OBS runs; Lightair says where they are.
    if let Some(data) = runtime.libobs_data() {
        let data = c(&format!("{}/", data.display()));
        unsafe { sys::obs_add_data_path(data.as_ptr()) };
    }
    let version = unsafe { CStr::from_ptr(sys::obs_get_version_string()) }.to_string_lossy().into_owned();

    let audio = sys::obs_audio_info2 {
        samples_per_sec: cfg.sample_rate,
        speakers: sys::SPEAKERS_STEREO,
        max_buffering_ms: 0,
        fixed_buffering: false,
    };
    if !unsafe { sys::obs_reset_audio2(&audio) } {
        return Err(EngineError::Audio);
    }
    crate::loudness::attach(cfg.sample_rate, 2);

    let module = c(&runtime.graphics_module(cfg.graphics).to_string_lossy());
    let (adapter, luid) = crate::gpu::preferred_adapter();
    // Chromium renders on the same GPU, so the textures it shares open on libobs's device
    if let Some((high, low)) = luid {
        lightair_web::set_gpu_adapter(high, low);
    }
    let mut video = sys::obs_video_info {
        graphics_module: module.as_ptr(),
        fps_num: cfg.fps.0,
        fps_den: cfg.fps.1,
        base_width: cfg.canvas.0,
        base_height: cfg.canvas.1,
        output_width: cfg.output.0,
        output_height: cfg.output.1,
        output_format: sys::VIDEO_FORMAT_NV12,
        adapter,
        gpu_conversion: true,
        colorspace: sys::VIDEO_CS_709,
        range: sys::VIDEO_RANGE_PARTIAL,
        scale_type: sys::OBS_SCALE_BICUBIC,
    };
    match unsafe { sys::obs_reset_video(&mut video) } {
        0 => {}
        -1 => return Err(EngineError::Video("generic failure")),
        -2 => return Err(EngineError::Video("graphics API not supported")),
        -3 => return Err(EngineError::Video("invalid parameters")),
        -4 => return Err(EngineError::Video("video output already active")),
        -5 => return Err(EngineError::Video("graphics adapter not found")),
        _ => return Err(EngineError::Video("unknown error")),
    }

    unsafe { load_modules(cfg, &runtime) };
    let capabilities = discover(cfg);
    tracing::info!(
        version,
        sources = capabilities.sources.len(),
        encoders = capabilities.encoders.len(),
        modules = capabilities.modules.len(),
        "engine ready"
    );
    Ok((runtime, capabilities, version))
}

unsafe fn load_modules(cfg: &EngineConfig, runtime: &Runtime) {
    if cfg.modules == Modules::None {
        return;
    }
    for name in &cfg.allowed_modules {
        unsafe { sys::obs_add_safe_module(c(name).as_ptr()) };
    }
    let mode = match &cfg.modules {
        // libobs's modules where libobs is: the app's own PlugIns, or an installed OBS's
        Modules::Auto => Modules::Directory(runtime.plugins.clone()),
        other => other.clone(),
    };
    match mode {
        Modules::Bundle => {
            if !unsafe { sys::obs_load_core_modules() } {
                tracing::warn!("some core modules failed to load");
            }
        }
        Modules::Directory(dir) => unsafe { load_plugin_dir(runtime, &dir) },
        Modules::Auto | Modules::None => {}
    }
    for dir in &cfg.extra_plugin_dirs {
        unsafe { load_plugin_dir(runtime, dir) };
    }
    unsafe {
        sys::obs_post_load_modules();
        sys::obs_log_loaded_modules();
    }
    // Lightair's own source types (the audio engine's mix outputs, a test tone)
    crate::audio_host::register_types();
    crate::pointer::register();
    crate::backdrop::register();
    crate::web::register();
    crate::vcam::register();
    #[cfg(target_os = "macos")]
    crate::media_source::register();
    #[cfg(target_os = "macos")]
    crate::on_screen::register();
    #[cfg(target_os = "macos")]
    crate::system_audio::register();
}

/// Loads every module in `dir` (subject to the safe-module allowlist): `<module>.plugin` bundles
/// on macOS, `<module>.dll` on Windows.
unsafe fn load_plugin_dir(runtime: &Runtime, dir: &std::path::Path) {
    let (bin, data) = runtime.module_patterns(dir);
    let (bin, data) = (c(&bin), c(&data));
    let mut info = sys::obs_runtime_module_info {
        path_info: sys::obs_runtime_module_path { binary: bin.as_ptr(), data: data.as_ptr() },
        type_: sys::OBS_MODULE_TYPE_PLUGIN,
        name: std::ptr::null(),
    };
    let mut failures = sys::obs_module_failure_info { failed_modules: std::ptr::null_mut(), count: 0 };
    unsafe {
        sys::obs_load_plugins(&mut info, &mut failures);
        if failures.count > 0 {
            tracing::warn!(count = failures.count, dir = %dir.display(), "modules failed to load");
        }
        sys::obs_module_failure_info_free(&mut failures);
    }
}

fn enumerate(f: unsafe extern "C" fn(usize, *mut *const c_char) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut id: *const c_char = std::ptr::null();
    let mut i = 0;
    while unsafe { f(i, &mut id) } {
        if !id.is_null() {
            out.push(unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned());
        }
        i += 1;
    }
    out
}

fn discover(_cfg: &EngineConfig) -> Capabilities {
    let mut modules = Vec::new();
    unsafe extern "C" fn collect(param: *mut std::ffi::c_void, module: *mut sys::obs_module_t) {
        let list = unsafe { &mut *(param as *mut Vec<String>) };
        let name = unsafe { sys::obs_get_module_file_name(module) };
        if !name.is_null() {
            list.push(unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned());
        }
    }
    unsafe { sys::obs_enum_modules(Some(collect), &mut modules as *mut _ as *mut _) };
    Capabilities {
        sources: enumerate(sys::obs_enum_source_types),
        filters: enumerate(sys::obs_enum_filter_types),
        transitions: enumerate(sys::obs_enum_transition_types),
        encoders: enumerate(sys::obs_enum_encoder_types),
        outputs: enumerate(sys::obs_enum_output_types),
        services: enumerate(sys::obs_enum_service_types),
        modules,
    }
}

/// Lists sources still alive before `obs_shutdown`; anything here is a reference leak.
fn log_surviving_sources() {
    unsafe extern "C" fn each(_: *mut std::ffi::c_void, src: *mut sys::obs_source_t) -> bool {
        let text = |p: *const c_char| {
            if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
        };
        let name = text(unsafe { sys::obs_source_get_name(src) });
        let id = text(unsafe { sys::obs_source_get_id(src) });
        tracing::warn!(name, id, "source still alive at shutdown");
        true
    }
    unsafe {
        sys::obs_enum_all_sources(Some(each), std::ptr::null_mut());
        sys::obs_enum_scenes(Some(each), std::ptr::null_mut());
    }
}

unsafe fn shutdown(state: EngineState) {
    unsafe {
        for channel in 0..64 {
            sys::obs_set_output_source(channel, std::ptr::null_mut());
        }
    }
    // Scenes drop before the sources they hold, then libobs finishes destroying them on its
    // destruction thread; shutting down before that queue drains crashes inside scene_destroy.
    // The render thread can hold a scene for one more frame after its channel is cleared, so
    // let a few frames pass and keep draining until nothing is left to destroy.
    crate::loudness::detach();
    let EngineState { sources, scenes, mut reconciler, mut outputs, studio_take } = state;
    outputs.shutdown();
    drop(studio_take);
    reconciler.clear();
    drop(scenes);
    drop(sources);
    let mut idle_rounds = 0;
    while idle_rounds < 3 {
        std::thread::sleep(std::time::Duration::from_millis(34));
        if unsafe { sys::obs_wait_for_destroy_queue() } {
            idle_rounds = 0;
        } else {
            idle_rounds += 1;
        }
    }
    log_surviving_sources();
    unsafe { sys::obs_shutdown() };
    let leaks = unsafe { sys::bnum_allocs() };
    if leaks > 1 {
        tracing::warn!(leaks, "libobs reported unfreed allocations at shutdown");
    }
    tracing::info!("engine stopped");
}

/// Reads frame statistics directly from libobs. Safe from any thread (libobs exposes these as
/// atomics), so the UI never waits on the engine thread for numbers.
pub fn read_stats() -> Stats {
    unsafe {
        if !sys::obs_initialized() {
            return Stats::default();
        }
        Stats {
            active_fps: sys::obs_get_active_fps(),
            average_frame_time_ms: sys::obs_get_average_frame_time_ns() as f64 / 1e6,
            total_frames: sys::obs_get_total_frames(),
            lagged_frames: sys::obs_get_lagged_frames(),
        }
    }
}
