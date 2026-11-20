//! Outputs: recording and streaming (docs/11-outputs-streaming-recording.md).
//!
//! Each running output owns its libobs output, encoders and (for streams) service. libobs
//! output signals update a process-wide [`OutputsRuntime`] that the UI reads; nothing about
//! outputs is in the undo history (it is runtime state, docs/03-architecture.md).

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use libobs_sys as sys;
use lightair_model::Id;
use parking_lot::Mutex;

use crate::obs::Data;

// ---------------------------------------------------------------- runtime status
#[derive(Debug, Clone, PartialEq)]
pub enum OutputState {
    Idle,
    Starting,
    Active {
        since: Instant,
    },
    Reconnecting {
        since: Instant,
    },
    Stopping,
    /// Stopped with an error the user should see.
    Failed(String),
}

impl OutputState {
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            OutputState::Starting
                | OutputState::Active { .. }
                | OutputState::Reconnecting { .. }
                | OutputState::Stopping
        )
    }
}

#[derive(Debug, Clone)]
pub struct OutputStatus {
    pub state: OutputState,
    /// File being written (recordings).
    pub path: Option<PathBuf>,
    pub bytes: u64,
    pub dropped_frames: i32,
    pub total_frames: i32,
    /// Clips saved since the replay buffer started.
    pub saved_clips: u32,
    /// Recording: paused (the file holds no time while paused).
    pub paused: bool,
    /// Recording: time spent paused before the current pause, and when the current one began.
    pub paused_for: std::time::Duration,
    pub paused_since: Option<Instant>,
    /// Recording: markers added (chapters in MP4/MOV).
    pub markers: u32,
    /// Recording: files written so far (1 + splits); `path` is the current one.
    pub files: u32,
}

impl OutputStatus {
    /// Time actually recorded since `since` (excludes pauses).
    pub fn recorded(&self, since: Instant) -> std::time::Duration {
        let paused = self.paused_for + self.paused_since.map_or_default(|t| t.elapsed());
        since.elapsed().saturating_sub(paused)
    }
}

impl Default for OutputStatus {
    fn default() -> Self {
        Self {
            state: OutputState::Idle,
            path: None,
            bytes: 0,
            dropped_frames: 0,
            total_frames: 0,
            saved_clips: 0,
            paused: false,
            paused_for: std::time::Duration::ZERO,
            paused_since: None,
            markers: 0,
            files: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKey {
    /// A recording of one canvas (by its canvas id); several can run at once.
    Recording(Id),
    Stream(Id),
    /// The replay buffer behind the Clip button.
    Clips,
    /// The virtual camera: a canvas as a camera other apps can pick (macOS: through the OBS
    /// Virtual Camera extension, lightair-effect-stack's `lightair_virtualcam` output).
    VirtualCamera,
}

#[derive(Default)]
pub struct OutputsRuntime {
    status: Mutex<HashMap<OutputKey, OutputStatus>>,
}

impl OutputsRuntime {
    pub fn get(&self, key: OutputKey) -> OutputStatus {
        self.status.lock().get(&key).cloned().unwrap_or_default()
    }

    pub fn any_recording_running(&self) -> bool {
        self.status.lock().iter().any(|(k, s)| matches!(k, OutputKey::Recording(_)) && s.state.is_running())
    }

    /// The recordings as one: the longest-running one while any run (its time, pause and
    /// markers stand for all), else one starting, stopping or failed, else idle.
    pub fn recording(&self) -> OutputStatus {
        let status = self.status.lock();
        let rank = |s: &OutputStatus| match s.state {
            OutputState::Active { .. } | OutputState::Reconnecting { .. } => 0,
            OutputState::Starting | OutputState::Stopping => 1,
            OutputState::Failed(_) => 2,
            OutputState::Idle => 3,
        };
        let since = |s: &OutputStatus| match s.state {
            OutputState::Active { since } | OutputState::Reconnecting { since } => Some(since),
            _ => None,
        };
        status
            .iter()
            .filter(|(k, _)| matches!(k, OutputKey::Recording(_)))
            .map(|(_, s)| s)
            .min_by_key(|s| (rank(s), since(s)))
            .cloned()
            .unwrap_or_default()
    }

    pub fn any_stream_running(&self) -> bool {
        self.status.lock().iter().any(|(k, s)| matches!(k, OutputKey::Stream(_)) && s.state.is_running())
    }

    /// Earliest start among running streams (for the Go Live timer).
    /// Each stream destination's status, by its id.
    pub fn streams(&self) -> Vec<(Id, OutputStatus)> {
        self.status
            .lock()
            .iter()
            .filter_map(|(k, s)| match k {
                OutputKey::Stream(id) => Some((*id, s.clone())),
                _ => None,
            })
            .collect()
    }

        pub fn stream_since(&self) -> Option<Instant> {
        self.status
            .lock()
            .iter()
            .filter(|(k, _)| matches!(k, OutputKey::Stream(_)))
            .filter_map(|(_, s)| match s.state {
                OutputState::Active { since } | OutputState::Reconnecting { since } => Some(since),
                _ => None,
            })
            .min()
    }

    fn update(&self, key: OutputKey, f: impl FnOnce(&mut OutputStatus)) {
        f(self.status.lock().entry(key).or_default());
    }
}

pub fn runtime() -> &'static OutputsRuntime {
    static RUNTIME: OnceLock<OutputsRuntime> = OnceLock::new();
    RUNTIME.get_or_init(OutputsRuntime::default)
}

/// A canvas's video output, as returned by `Reconciler::canvas_video`. Only the engine creates
/// these, so the pointer is always a live libobs video output.
#[derive(Clone, Copy)]
pub struct VideoOutput(pub(crate) *mut sys::video_t);

// ---------------------------------------------------------------- configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// OBS's hybrid MP4: crash-safe while recording, a normal MP4 when finished.
    HybridMp4,
    Mkv,
    Mov,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::HybridMp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Mov => "mov",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EncoderConfig {
    /// libobs video encoder id (e.g. `com.apple.videotoolbox.videoencoder.ave.avc`, `obs_x264`).
    pub video_encoder: String,
    pub video_bitrate_kbps: u32,
    pub audio_encoder: String,
    pub audio_bitrate_kbps: u32,
    /// CBR for streaming; quality-oriented VBR for recording.
    pub constant_bitrate: bool,
    /// B-frames (WebRTC/WHIP can't carry them).
    pub bframes: bool,
    /// Seconds between keyframes.
    pub keyint_sec: u32,
    /// Encode at this size instead of the canvas's (scaled on the GPU).
    pub scaled: Option<(u32, u32)>,
    /// The libobs mix its audio is (0: Program, the master; 1–5: the audio engine's mixes).
    pub audio_mix: usize,
}

#[derive(Debug, Clone)]
pub struct RecordingConfig {
    /// The canvas (canvas id) recorded; it keys the output.
    pub canvas: Id,
    pub path: PathBuf,
    /// Added to split files' names so canvases recorded together don't collide ("Vertical").
    pub label: Option<String>,
    /// Start a new file every this many minutes.
    pub split_minutes: Option<u32>,
    pub container: Container,
    pub encoders: EncoderConfig,
    /// libobs mixes recorded as extra audio tracks after the program (the audio engine's mixes,
    /// 1-5).
    pub extra_mixes: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ClipsConfig {
    pub folder: PathBuf,
    pub seconds: u32,
    pub encoders: EncoderConfig,
}

/// How a destination is reached, from its URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamProtocol {
    /// rtmp:// and rtmps://
    Rtmp,
    /// srt://, rist://, udp://, tcp:// (MPEG-TS). The key is the SRT stream ID.
    MpegTs,
    /// http(s):// WHIP (WebRTC). The key is the bearer token; audio is Opus.
    Whip,
}

impl StreamProtocol {
    pub fn of(server: &str) -> Option<Self> {
        let s = server.trim().to_ascii_lowercase();
        let has = |p: &str| s.starts_with(p);
        if has("rtmp://") || has("rtmps://") {
            Some(Self::Rtmp)
        } else if has("srt://") || has("rist://") || has("udp://") || has("tcp://") {
            Some(Self::MpegTs)
        } else if has("http://") || has("https://") {
            Some(Self::Whip)
        } else {
            None
        }
    }

    pub fn label(server: &str) -> &'static str {
        let s = server.trim().to_ascii_lowercase();
        match Self::of(&s) {
            Some(Self::Rtmp) if s.starts_with("rtmps") => "RTMPS",
            Some(Self::Rtmp) => "RTMP",
            Some(Self::MpegTs) if s.starts_with("srt") => "SRT",
            Some(Self::MpegTs) if s.starts_with("rist") => "RIST",
            Some(Self::MpegTs) => "MPEG-TS",
            Some(Self::Whip) => "WHIP",
            None => "?",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub id: Id,
    pub server: String,
    pub key: String,
    pub encoders: EncoderConfig,
    /// (retries, seconds apart) when the connection drops; `None`: libobs's defaults.
    pub reconnect: Option<(u32, u32)>,
    /// A second audio track (Twitch's VOD track): the libobs mix it carries (RTMP only).
    pub vod_mix: Option<usize>,
}

// ---------------------------------------------------------------- libobs objects
struct Running {
    output: *mut sys::obs_output_t,
    video: *mut sys::obs_encoder_t,
    audio: *mut sys::obs_encoder_t,
    service: *mut sys::obs_service_t,
    /// Heap-pinned key passed to signal callbacks; freed after disconnecting.
    signal_param: *mut OutputKey,
    signals: &'static [(&'static str, SignalCb)],
    /// What its encoders were made for; outputs with the same key share them.
    share: (usize, EncoderConfig),
    /// Audio encoders of the extra tracks (recordings carrying mixes).
    extra: Vec<*mut sys::obs_encoder_t>,
}

unsafe impl Send for Running {}

impl Running {
    unsafe fn release(self) {
        unsafe {
            let handler = sys::obs_output_get_signal_handler(self.output);
            for (name, cb) in self.signals {
                let n = CString::new(*name).unwrap();
                sys::signal_handler_disconnect(handler, n.as_ptr(), Some(*cb), self.signal_param as *mut c_void);
            }
            if sys::obs_output_active(self.output) {
                sys::obs_output_force_stop(self.output);
            }
            sys::obs_output_release(self.output);
            sys::obs_encoder_release(self.video);
            sys::obs_encoder_release(self.audio);
            for e in self.extra {
                sys::obs_encoder_release(e);
            }
            if !self.service.is_null() {
                sys::obs_service_release(self.service);
            }
            drop(Box::from_raw(self.signal_param));
        }
    }
}

type SignalCb = unsafe extern "C" fn(*mut c_void, *mut sys::calldata_t);
/// Signals each kind of output declares (libobs warns about connecting to undeclared ones).
fn signals_for(key: OutputKey) -> &'static [(&'static str, SignalCb)] {
    match key {
        OutputKey::Recording(_) => &[("start", on_start), ("stop", on_stop), ("file_changed", on_file_changed)],
        OutputKey::Stream(_) => {
            &[("start", on_start), ("stop", on_stop), ("reconnect", on_reconnect), ("reconnect_success", on_start)]
        }
        OutputKey::Clips => &[("start", on_start), ("stop", on_stop), ("saved", on_saved)],
        OutputKey::VirtualCamera => &[("start", on_start), ("stop", on_stop)],
    }
}

/// The recording moved on to a new file (a split).
unsafe extern "C" fn on_file_changed(param: *mut c_void, data: *mut sys::calldata_t) {
    let key = unsafe { *(param as *const OutputKey) };
    let mut p: *const std::ffi::c_char = std::ptr::null();
    let path = unsafe { sys::calldata_get_string(data, c"next_file".as_ptr(), &mut p) && !p.is_null() }
        .then(|| PathBuf::from(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()));
    runtime().update(key, |s| {
        if path.is_some() {
            s.path = path;
        }
        s.files += 1;
    });
}

/// Replay buffer saved a clip: record its path (the UI shows it and counts clips).
unsafe extern "C" fn on_saved(param: *mut c_void, _: *mut sys::calldata_t) {
    let key = unsafe { *(param as *const OutputKey) };
    let path = CLIPS_OUTPUT.lock().as_ref().and_then(|p| unsafe { last_replay(p.0) });
    runtime().update(key, |s| {
        s.path = path;
        s.saved_clips += 1;
    });
}

struct OutputPtr(*mut sys::obs_output_t);
unsafe impl Send for OutputPtr {}
static CLIPS_OUTPUT: Mutex<Option<OutputPtr>> = Mutex::new(None);

unsafe fn proc_call(output: *mut sys::obs_output_t, name: &str, cd: &mut sys::calldata_t) -> bool {
    unsafe {
        let ph = sys::obs_output_get_proc_handler(output);
        sys::proc_handler_call(ph, c(name).as_ptr(), cd)
    }
}

unsafe fn last_replay(output: *mut sys::obs_output_t) -> Option<PathBuf> {
    let mut cd = sys::calldata_t::default();
    let mut path = None;
    unsafe {
        if proc_call(output, "get_last_replay", &mut cd) {
            let mut p: *const std::ffi::c_char = std::ptr::null();
            if sys::calldata_get_string(&cd, c"path".as_ptr(), &mut p) && !p.is_null() {
                path = Some(PathBuf::from(CStr::from_ptr(p).to_string_lossy().into_owned()));
            }
        }
        if !cd.fixed && !cd.stack.is_null() {
            sys::bfree(cd.stack as *mut c_void);
        }
    }
    path
}

unsafe extern "C" fn on_start(param: *mut c_void, _: *mut sys::calldata_t) {
    let key = unsafe { *(param as *const OutputKey) };
    runtime().update(key, |s| {
        let since = match s.state {
            OutputState::Reconnecting { since } => since,
            _ => Instant::now(),
        };
        s.state = OutputState::Active { since };
    });
}

unsafe extern "C" fn on_reconnect(param: *mut c_void, _: *mut sys::calldata_t) {
    let key = unsafe { *(param as *const OutputKey) };
    runtime().update(key, |s| {
        let since = match s.state {
            OutputState::Active { since } | OutputState::Reconnecting { since } => since,
            _ => Instant::now(),
        };
        s.state = OutputState::Reconnecting { since };
    });
}

unsafe extern "C" fn on_stop(param: *mut c_void, data: *mut sys::calldata_t) {
    let key = unsafe { *(param as *const OutputKey) };
    let mut code: i64 = 0;
    let mut output: *mut sys::obs_output_t = std::ptr::null_mut();
    unsafe {
        let name = c"code";
        sys::calldata_get_data(data, name.as_ptr(), &mut code as *mut i64 as *mut c_void, std::mem::size_of::<i64>());
        let name = c"output";
        sys::calldata_get_data(
            data,
            name.as_ptr(),
            &mut output as *mut _ as *mut c_void,
            std::mem::size_of::<*mut c_void>(),
        );
    }
    let state = if code == 0 {
        OutputState::Idle
    } else {
        let detail = if output.is_null() { None } else { last_error(output) };
        OutputState::Failed(describe_stop(code as i32, detail))
    };
    runtime().update(key, |s| s.state = state);
}

fn last_error(output: *mut sys::obs_output_t) -> Option<String> {
    let p = unsafe { sys::obs_output_get_last_error(output) };
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
}

/// Plain-language reason for an output stopping (docs/06-design-system.md, Writing style).
fn describe_stop(code: i32, detail: Option<String>) -> String {
    let base = match code {
        sys::OBS_OUTPUT_BAD_PATH => "The recording folder can't be written to.",
        sys::OBS_OUTPUT_CONNECT_FAILED => {
            "Couldn't connect to the server. Check the server address and your connection."
        }
        sys::OBS_OUTPUT_INVALID_STREAM => "The server rejected the stream key.",
        sys::OBS_OUTPUT_DISCONNECTED => "The connection to the server was lost.",
        sys::OBS_OUTPUT_UNSUPPORTED => "This server doesn't support the chosen format.",
        sys::OBS_OUTPUT_NO_SPACE => "The disk is full.",
        sys::OBS_OUTPUT_ENCODE_ERROR => "The encoder stopped working. Try a different encoder.",
        _ => "The output stopped unexpectedly.",
    };
    match detail {
        Some(d) => format!("{base} ({d})"),
        None => base.to_string(),
    }
}

/// Running outputs, owned by the engine thread.
#[derive(Default)]
pub struct Outputs {
    recordings: HashMap<Id, Running>,
    streams: HashMap<Id, Running>,
    clips: Option<Running>,
    virtual_camera: Option<Running>,
    /// The virtual camera's own canvas, while it shows one scene rather than what's on air.
    virtual_camera_canvas: Option<crate::obs::ObsCanvas>,
}

/// One scene for the virtual camera (instead of a canvas's program): its scene source and the
/// size and rate of the canvas it belongs to.
#[derive(Debug, Clone, Copy)]
pub struct VirtualCameraScene {
    pub source: *mut sys::obs_source_t,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

unsafe impl Send for VirtualCameraScene {}

fn c(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

unsafe fn create_encoders(
    name: &str,
    cfg: &EncoderConfig,
    video_output: *mut sys::video_t,
) -> Result<(*mut sys::obs_encoder_t, *mut sys::obs_encoder_t), String> {
    let mut v = Data::new();
    let prores = cfg.video_encoder.contains("prores");
    let hevc = cfg.video_encoder.contains("hevc");
    v.set_int("keyint_sec", cfg.keyint_sec.max(1) as i64);
    let apple = cfg.video_encoder.contains("videotoolbox");
    if !prores {
        // VideoToolbox and x264 average over the file; the Windows encoders call it VBR
        let variable = if apple || cfg.video_encoder == "obs_x264" { "ABR" } else { "VBR" };
        v.set_int("bitrate", cfg.video_bitrate_kbps as i64)
            .set_str("rate_control", if cfg.constant_bitrate { "CBR" } else { variable })
            .set_str("profile", if hevc { "main" } else { "high" });
    }
    if cfg.video_encoder == "obs_x264" {
        v.set_str("preset", "veryfast");
        if !cfg.bframes {
            v.set_str("x264opts", "bframes=0");
        }
    } else if apple {
        v.set_bool("bframes", cfg.bframes);
    } else if cfg.video_encoder.contains("qsv") {
        v.set_int("bframes", if cfg.bframes { 3 } else { 0 });
    } else {
        // NVENC and AMF: B-frames between references
        v.set_int("bf", if cfg.bframes { 2 } else { 0 });
    }
    let mut a = Data::new();
    a.set_int("bitrate", cfg.audio_bitrate_kbps as i64);
    unsafe {
        let video = sys::obs_video_encoder_create(
            c(&cfg.video_encoder).as_ptr(),
            c(&format!("{name} video")).as_ptr(),
            v.as_ptr(),
            std::ptr::null_mut(),
        );
        if video.is_null() {
            return Err(format!("Couldn't create the video encoder ({}).", cfg.video_encoder));
        }
        let audio = sys::obs_audio_encoder_create(
            c(&cfg.audio_encoder).as_ptr(),
            c(&format!("{name} audio")).as_ptr(),
            a.as_ptr(),
            cfg.audio_mix,
            std::ptr::null_mut(),
        );
        if audio.is_null() {
            sys::obs_encoder_release(video);
            return Err(format!("Couldn't create the audio encoder ({}).", cfg.audio_encoder));
        }
        // The canvas's canvas (main canvas when null), scaled when asked.
        sys::obs_encoder_set_video(video, if video_output.is_null() { sys::obs_get_video() } else { video_output });
        if let Some((w, h)) = cfg.scaled {
            sys::obs_encoder_set_scaled_size(video, w & !1, h & !1);
        }
        sys::obs_encoder_set_audio(audio, sys::obs_get_audio());
        Ok((video, audio))
    }
}

#[allow(clippy::too_many_arguments)] // each output kind sets a different few
unsafe fn start(
    key: OutputKey,
    output_type: &str,
    output_settings: &Data,
    encoders: &EncoderConfig,
    service: *mut sys::obs_service_t,
    video_output: *mut sys::video_t,
    shared: Option<(*mut sys::obs_encoder_t, *mut sys::obs_encoder_t)>,
    extra_mixes: &[usize],
) -> Result<Running, String> {
    let name = match key {
        OutputKey::Recording(canvas) => format!("Recording {canvas}"),
        OutputKey::Stream(id) => format!("Stream {id}"),
        OutputKey::Clips => "Clips".to_string(),
        OutputKey::VirtualCamera => "Virtual camera".to_string(),
    };
    unsafe {
        let release_service = || {
            if !service.is_null() {
                sys::obs_service_release(service);
            }
        };
        // `shared` carries strong references to another output's encoders.
        let made = match shared {
            Some(pair) => Ok(pair),
            None => create_encoders(&name, encoders, video_output),
        };
        let (video, audio) = match made {
            Ok(v) => v,
            Err(e) => {
                release_service();
                return Err(e);
            }
        };
        let output = sys::obs_output_create(
            c(output_type).as_ptr(),
            c(&name).as_ptr(),
            output_settings.as_ptr(),
            std::ptr::null_mut(),
        );
        if output.is_null() {
            sys::obs_encoder_release(video);
            sys::obs_encoder_release(audio);
            release_service();
            return Err(format!("Couldn't create the {output_type} output."));
        }
        sys::obs_output_set_video_encoder(output, video);
        sys::obs_output_set_audio_encoder(output, audio, 0);
        // more audio tracks: one encoder per libobs mix, in order after the program
        let mut extra = Vec::new();
        for (track, &mix) in extra_mixes.iter().enumerate().take(5) {
            let mut a = Data::new();
            a.set_int("bitrate", encoders.audio_bitrate_kbps as i64);
            let enc = sys::obs_audio_encoder_create(
                c(&encoders.audio_encoder).as_ptr(),
                c(&format!("{name} audio {}", track + 2)).as_ptr(),
                a.as_ptr(),
                mix,
                std::ptr::null_mut(),
            );
            if enc.is_null() {
                continue;
            }
            sys::obs_encoder_set_audio(enc, sys::obs_get_audio());
            sys::obs_output_set_audio_encoder(output, enc, track + 1);
            extra.push(enc);
        }
        if !service.is_null() {
            sys::obs_output_set_service(output, service);
        }
        let signal_param = Box::into_raw(Box::new(key));
        let handler = sys::obs_output_get_signal_handler(output);
        let signals = signals_for(key);
        for (sig, cb) in signals {
            sys::signal_handler_connect(handler, c(sig).as_ptr(), Some(*cb), signal_param as *mut c_void);
        }
        let share = (video_output as usize, encoders.clone());
        let running = Running { output, video, audio, service, signal_param, signals, share, extra };
        runtime().update(key, |s| *s = OutputStatus { state: OutputState::Starting, ..Default::default() });
        if !sys::obs_output_start(output) {
            let err = last_error(output).unwrap_or_else(|| "The output couldn't start.".into());
            running.release();
            runtime().update(key, |s| s.state = OutputState::Failed(err.clone()));
            return Err(err);
        }
        Ok(running)
    }
}

impl Outputs {
    /// Starts recording one canvas. `video` is that canvas's canvas video
    /// (`Reconciler::canvas_video`); `None` means the primary canvas. Several canvases can
    /// record at once, each its own output and file.
    pub fn start_recording(&mut self, cfg: &RecordingConfig, video: Option<VideoOutput>) -> Result<(), String> {
        let video_output = video.map_or(std::ptr::null_mut(), |v| v.0);
        let key = OutputKey::Recording(cfg.canvas);
        if self.recordings.get(&cfg.canvas).is_some_and(|r| unsafe { sys::obs_output_active(r.output) }) {
            return Ok(());
        }
        if let Some(old) = self.recordings.remove(&cfg.canvas) {
            unsafe { old.release() };
        }
        if let Some(dir) = cfg.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Can't create the recording folder: {e}"))?;
        }
        let (output_type, path_key) = match cfg.container {
            Container::HybridMp4 => ("mp4_output", "path"),
            Container::Mov => ("mov_output", "path"),
            Container::Mkv => ("ffmpeg_muxer", "path"),
        };
        let mut settings = Data::new();
        settings.set_str(path_key, &cfg.path.to_string_lossy());
        // Splits on request, or every N minutes; later files go next to the first one.
        settings.set_bool("split_file", true);
        settings.set_int("max_time_sec", cfg.split_minutes.map_or(0, |m| m as i64 * 60));
        settings.set_int("max_size_mb", 0);
        if let Some(dir) = cfg.path.parent() {
            settings.set_str("directory", &dir.to_string_lossy());
        }
        let pattern = match &cfg.label {
            Some(label) => format!("Lightair %CCYY-%MM-%DD %hh-%mm-%ss {label}"),
            None => "Lightair %CCYY-%MM-%DD %hh-%mm-%ss".to_string(),
        };
        settings.set_str("format", &pattern);
        settings.set_str("extension", cfg.container.extension());
        settings.set_bool("allow_spaces", true);
        let running =
            unsafe { start(key, output_type, &settings, &cfg.encoders, std::ptr::null_mut(), video_output, None, &cfg.extra_mixes)? };
        runtime().update(key, |s| {
            s.path = Some(cfg.path.clone());
            s.files = 1;
        });
        self.recordings.insert(cfg.canvas, running);
        Ok(())
    }

    /// Stops every recording.
    pub fn stop_recording(&mut self) {
        for (canvas, r) in &self.recordings {
            runtime().update(OutputKey::Recording(*canvas), |s| {
                if s.state.is_running() {
                    s.state = OutputState::Stopping
                }
            });
            unsafe { sys::obs_output_stop(r.output) };
        }
    }

    /// The running recordings: canvas and output.
    fn active_recordings(&self) -> Vec<(Id, *mut sys::obs_output_t)> {
        self.recordings
            .iter()
            .map(|(c, r)| (*c, r.output))
            .filter(|(_, o)| unsafe { sys::obs_output_active(*o) })
            .collect()
    }

    /// Pauses or resumes every recording. Returns whether they are all now in that state.
    pub fn pause_recording(&mut self, pause: bool) -> bool {
        let active = self.active_recordings();
        let mut all = !active.is_empty();
        // libobs applies a pause at the next video frame; resuming before it has leaves the
        // encoder waiting forever (and the recording can't stop). Give a fresh pause time.
        if !pause {
            let newest = active
                .iter()
                .filter_map(|(c, _)| runtime().get(OutputKey::Recording(*c)).paused_since)
                .map(|t| t.elapsed())
                .min();
            if let Some(age) = newest.filter(|a| *a < Duration::from_millis(150)) {
                std::thread::sleep(Duration::from_millis(150) - age);
            }
        }
        for (canvas, output) in active {
            let ok = unsafe { sys::obs_output_pause(output, pause) };
            let paused = unsafe { sys::obs_output_paused(output) };
            runtime().update(OutputKey::Recording(canvas), |s| {
                match (s.paused, paused) {
                    (false, true) => s.paused_since = Some(Instant::now()),
                    (true, false) => s.paused_for += s.paused_since.take().map_or_default(|t| t.elapsed()),
                    _ => {}
                }
                s.paused = paused;
            });
            all &= ok && paused == pause;
        }
        all
    }

    /// Ends every recording's current file and continues in a new one, without a gap.
    pub fn split_recording(&mut self) -> bool {
        let mut any = false;
        for (_, output) in self.active_recordings() {
            any |= unsafe { split_file(output) };
        }
        any
    }

    /// Adds a marker (an MP4/MOV chapter) at the current time to every recording that can hold
    /// one. Returns false when none can (MKV) or none is running.
    pub fn add_marker(&mut self, name: &str) -> bool {
        let mut any = false;
        for (canvas, output) in self.active_recordings() {
            if unsafe { add_chapter(output, name) } {
                runtime().update(OutputKey::Recording(canvas), |s| s.markers += 1);
                any = true;
            }
        }
        any
    }

    pub fn start_stream(&mut self, cfg: &StreamConfig, video: Option<VideoOutput>) -> Result<(), String> {
        let video_output = video.map_or(std::ptr::null_mut(), |v| v.0);
        let key = OutputKey::Stream(cfg.id);
        if self.streams.get(&cfg.id).is_some_and(|r| unsafe { sys::obs_output_active(r.output) }) {
            return Ok(());
        }
        if let Some(old) = self.streams.remove(&cfg.id) {
            unsafe { old.release() };
        }
        let protocol = StreamProtocol::of(&cfg.server)
            .ok_or_else(|| "Use an rtmp://, rtmps://, srt://, rist:// or https:// (WHIP) address.".to_string())?;
        let (service_type, output_type) = match protocol {
            StreamProtocol::Rtmp => ("rtmp_custom", "rtmp_output"),
            StreamProtocol::MpegTs => ("rtmp_custom", "ffmpeg_mpegts_muxer"),
            StreamProtocol::Whip => ("whip_custom", "whip_output"),
        };
        let mut encoders = cfg.encoders.clone();
        if protocol == StreamProtocol::Whip {
            // WebRTC carries Opus audio, and B-frames aren't allowed.
            encoders.audio_encoder = "ffmpeg_opus".into();
            encoders.bframes = false;
        }
        let mut service_settings = Data::new();
        service_settings.set_str("server", &cfg.server);
        match protocol {
            StreamProtocol::Whip => service_settings.set_str("bearer_token", &cfg.key),
            _ => service_settings.set_str("key", &cfg.key),
        };
        let service = unsafe {
            sys::obs_service_create(
                c(service_type).as_ptr(),
                c(&format!("Service {}", cfg.id)).as_ptr(),
                service_settings.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        if service.is_null() {
            return Err("Couldn't set up the streaming service.".into());
        }
        let mut output_settings = Data::new();
        output_settings.set_bool("low_latency_mode_enabled", false);
        // On failure `start` releases everything it was given, the service included.
        // Destinations with identical encoder settings on the same canvas share one encoder pair.
        let share_key = (video_output as usize, encoders.clone());
        let shared = self
            .streams
            .values()
            .find(|r| r.share == share_key)
            .map(|r| unsafe { (sys::obs_encoder_get_ref(r.video), sys::obs_encoder_get_ref(r.audio)) });
        let shared = shared.filter(|(v, a)| !v.is_null() && !a.is_null());
        // Twitch's VOD track: a second audio track, as OBS sends it (RTMP's multitrack audio)
        let vod: Vec<usize> = cfg.vod_mix.filter(|_| protocol == StreamProtocol::Rtmp).into_iter().collect();
        let running = unsafe { start(key, output_type, &output_settings, &encoders, service, video_output, shared, &vod)? };
        if let Some((retries, delay)) = cfg.reconnect {
            unsafe { sys::obs_output_set_reconnect_settings(running.output, retries as i32, delay as i32) };
        }
        self.streams.insert(cfg.id, running);
        Ok(())
    }

    /// How many distinct video encoders the streams use (for the UI's "N encoders" note).
    pub fn stream_encoder_count(&self) -> usize {
        let mut seen: Vec<usize> = self.streams.values().map(|r| r.video as usize).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }

    /// The video encoder a stream uses (diagnostics and tests).
    /// A stream's audio tracks, in order: the libobs mix each encodes (diagnostics, tests).
    pub fn stream_audio_tracks(&self, id: Id) -> Vec<usize> {
        let Some(r) = self.streams.get(&id) else { return Vec::new() };
        (0..6)
            .map_while(|k| {
                let enc = unsafe { sys::obs_output_get_audio_encoder(r.output, k) };
                (!enc.is_null()).then(|| unsafe { sys::obs_encoder_get_mixer_index(enc) })
            })
            .collect()
    }

    pub fn stream_video_encoder(&self, id: Id) -> Option<usize> {
        self.streams.get(&id).map(|r| r.video as usize)
    }

    pub fn stop_stream(&mut self, id: Id) {
        if let Some(r) = self.streams.get(&id) {
            runtime().update(OutputKey::Stream(id), |s| {
                if s.state.is_running() {
                    s.state = OutputState::Stopping
                }
            });
            unsafe { sys::obs_output_stop(r.output) };
        }
    }

    pub fn stop_all_streams(&mut self) {
        let ids: Vec<Id> = self.streams.keys().copied().collect();
        for id in ids {
            self.stop_stream(id);
        }
    }

    /// Starts the virtual camera: what a canvas has on air (`video`: its canvas video; `None` the
    /// primary), or one scene (`scene`: on a canvas of the camera's own). Raw frames, no
    /// encoders. Its status is `OutputKey::VirtualCamera`'s.
    pub fn start_virtual_camera(&mut self, video: Option<VideoOutput>, scene: Option<VirtualCameraScene>) -> Result<(), String> {
        if self.virtual_camera.as_ref().is_some_and(|r| unsafe { sys::obs_output_active(r.output) }) {
            return Ok(());
        }
        self.drop_virtual_camera();
        let video = match scene {
            Some(sc) => {
                let canvas = crate::obs::ObsCanvas::create("Virtual camera", sc.width, sc.height, sc.fps_num, sc.fps_den)
                    .ok_or_else(|| "Couldn't make the virtual camera's canvas.".to_string())?;
                unsafe { sys::obs_canvas_set_channel(canvas.as_ptr(), 0, sc.source) };
                let v = VideoOutput(unsafe { sys::obs_canvas_get_video(canvas.as_ptr()) });
                self.virtual_camera_canvas = Some(canvas);
                Some(v)
            }
            None => video,
        };
        let key = OutputKey::VirtualCamera;
        unsafe {
            // Lightair's camera (macOS: the camera extension, lightair-effect-stack's output;
            // Windows 11: lightair_camera.dll, crate::vcam's output), else OBS's DirectShow camera
            // ("OBS Virtual Camera"), registered only where it's installed
            let id = match crate::vcam::camera() {
                Some(crate::vcam::Camera::Obs) => c"virtualcam_output",
                _ => c"lightair_virtualcam",
            };
            let output = sys::obs_output_create(id.as_ptr(), c"Virtual camera".as_ptr(), std::ptr::null_mut(), std::ptr::null_mut());
            if output.is_null() {
                let err = if cfg!(windows) {
                    crate::vcam::NOT_INSTALLED.to_string()
                } else {
                    "The virtual camera isn't available on this system.".to_string()
                };
                runtime().update(key, |s| s.state = OutputState::Failed(err.clone()));
                return Err(err);
            }
            let video_output = video.map_or_else(|| sys::obs_get_video(), |v| v.0);
            sys::obs_output_set_media(output, video_output, sys::obs_get_audio());
            let signal_param = Box::into_raw(Box::new(key));
            let handler = sys::obs_output_get_signal_handler(output);
            let signals = signals_for(key);
            for (sig, cb) in signals {
                sys::signal_handler_connect(handler, c(sig).as_ptr(), Some(*cb), signal_param as *mut c_void);
            }
            let running = Running {
                output,
                video: std::ptr::null_mut(),
                audio: std::ptr::null_mut(),
                service: std::ptr::null_mut(),
                signal_param,
                signals,
                share: (
                    video_output as usize,
                    EncoderConfig {
                        video_encoder: String::new(),
                        video_bitrate_kbps: 0,
                        audio_encoder: String::new(),
                        audio_bitrate_kbps: 0,
                        constant_bitrate: false,
                        bframes: false,
                        keyint_sec: 0,
                        scaled: None,
                        audio_mix: 0,
                    },
                ),
                extra: Vec::new(),
            };
            runtime().update(key, |s| *s = OutputStatus { state: OutputState::Starting, ..Default::default() });
            if !sys::obs_output_start(output) {
                let err = last_error(output).unwrap_or_else(|| "The virtual camera didn't start.".into());
                running.release();
                self.virtual_camera_canvas = None;
                runtime().update(key, |s| s.state = OutputState::Failed(err.clone()));
                return Err(err);
            }
            self.virtual_camera = Some(running);
        }
        Ok(())
    }

    pub fn stop_virtual_camera(&mut self) {
        if let Some(r) = &self.virtual_camera {
            runtime().update(OutputKey::VirtualCamera, |s| {
                if s.state.is_running() {
                    s.state = OutputState::Stopping
                }
            });
            unsafe { sys::obs_output_stop(r.output) };
        }
        // (a raw output stops at once: its canvas can go)
        self.virtual_camera_canvas = None;
    }

    fn drop_virtual_camera(&mut self) {
        if let Some(old) = self.virtual_camera.take() {
            unsafe { old.release() };
        }
        self.virtual_camera_canvas = None;
    }

    /// Starts the replay buffer (docs/11-outputs-streaming-recording.md, "Clips").
    pub fn start_clips(&mut self, cfg: &ClipsConfig, video: Option<VideoOutput>) -> Result<(), String> {
        let video_output = video.map_or(std::ptr::null_mut(), |v| v.0);
        if self.clips.as_ref().is_some_and(|r| unsafe { sys::obs_output_active(r.output) }) {
            return Ok(());
        }
        self.drop_clips();
        std::fs::create_dir_all(&cfg.folder).map_err(|e| format!("Can't create the clips folder: {e}"))?;
        let mut settings = Data::new();
        settings
            .set_str("directory", &cfg.folder.to_string_lossy())
            .set_str("format", "Clip %CCYY-%MM-%DD %hh-%mm-%ss")
            .set_str("extension", "mp4")
            .set_int("max_time_sec", cfg.seconds as i64)
            .set_int("max_size_mb", 0);
        let running = unsafe {
            start(
                OutputKey::Clips,
                "replay_buffer",
                &settings,
                &cfg.encoders,
                std::ptr::null_mut(),
                video_output,
                None,
                &[],
            )?
        };
        *CLIPS_OUTPUT.lock() = Some(OutputPtr(running.output));
        self.clips = Some(running);
        Ok(())
    }

    pub fn stop_clips(&mut self) {
        if let Some(r) = &self.clips {
            runtime().update(OutputKey::Clips, |s| {
                if s.state.is_running() {
                    s.state = OutputState::Stopping
                }
            });
            unsafe { sys::obs_output_stop(r.output) };
        }
    }

    fn drop_clips(&mut self) {
        CLIPS_OUTPUT.lock().take();
        if let Some(old) = self.clips.take() {
            unsafe { old.release() };
        }
    }

    /// Saves the last N seconds as a clip; the "saved" signal reports the file.
    pub fn save_clip(&mut self) -> bool {
        let Some(r) = &self.clips else { return false };
        if !unsafe { sys::obs_output_active(r.output) } {
            return false;
        }
        let mut cd = sys::calldata_t::default();
        let ok = unsafe { proc_call(r.output, "save", &mut cd) };
        if !cd.fixed && !cd.stack.is_null() {
            unsafe { sys::bfree(cd.stack as *mut c_void) };
        }
        ok
    }

    /// Updates byte and frame counters for running outputs (called by the stats sampler).
    pub fn sample(&self) {
        let sample = |key: OutputKey, r: &Running| unsafe {
            let (bytes, dropped, total) = (
                sys::obs_output_get_total_bytes(r.output),
                sys::obs_output_get_frames_dropped(r.output),
                sys::obs_output_get_total_frames(r.output),
            );
            runtime().update(key, |s| {
                s.bytes = bytes;
                s.dropped_frames = dropped;
                s.total_frames = total;
            });
        };
        for (canvas, r) in &self.recordings {
            sample(OutputKey::Recording(*canvas), r);
        }
        for (id, r) in &self.streams {
            sample(OutputKey::Stream(*id), r);
        }
    }

    /// Stops and releases everything (engine shutdown).
    pub fn shutdown(&mut self) {
        if let Some(r) = &self.clips {
            unsafe { sys::obs_output_stop(r.output) };
        }
        self.drop_clips();
        if let Some(r) = &self.virtual_camera {
            unsafe { sys::obs_output_stop(r.output) };
        }
        self.drop_virtual_camera();
        for (_, r) in self.recordings.drain() {
            unsafe {
                sys::obs_output_stop(r.output);
                r.release();
            }
        }
        for (_, r) in self.streams.drain() {
            unsafe {
                sys::obs_output_stop(r.output);
                r.release();
            }
        }
    }
}

/// H.264 encoders, best first: Apple's (macOS); NVIDIA, AMD and Intel hardware (Windows); x264.
pub const H264_ENCODERS: &[&str] = &[
    "com.apple.videotoolbox.videoencoder.ave.avc",
    "com.apple.videotoolbox.videoencoder.h264",
    "obs_nvenc_h264_tex",
    "jim_nvenc",
    "h264_texture_amf",
    "obs_qsv11_v2",
    "obs_x264",
];

/// HEVC encoders, best first.
pub const HEVC_ENCODERS: &[&str] = &[
    "com.apple.videotoolbox.videoencoder.ave.hevc",
    "com.apple.videotoolbox.videoencoder.hevc.vcp",
    "obs_nvenc_hevc_tex",
    "jim_hevc_nvenc",
    "h265_texture_amf",
    "obs_qsv11_hevc",
];

/// ProRes encoders (Apple's; Windows has none in libobs).
pub const PRORES_ENCODERS: &[&str] =
    &["com.apple.videotoolbox.videoencoder.appleproreshw.422", "com.apple.videotoolbox.videoencoder.prores-422"];

/// The first of `ids` that's available.
pub fn first_available(available: &[String], ids: &[&str]) -> Option<String> {
    ids.iter().find(|id| available.iter().any(|e| e == *id)).map(|id| id.to_string())
}

/// The AAC encoder: Apple's on macOS, FFmpeg's elsewhere.
pub fn aac_encoder(available: &[String]) -> String {
    first_available(available, &["CoreAudio_AAC"]).unwrap_or_else(|| "ffmpeg_aac".into())
}

/// Who makes an encoder, for people: "Apple hardware", "NVIDIA NVENC", "x264 (software)".
pub fn encoder_maker(id: &str) -> &'static str {
    if id.contains("videotoolbox") {
        "Apple hardware"
    } else if id.contains("nvenc") {
        "NVIDIA NVENC"
    } else if id.contains("amf") {
        "AMD AMF"
    } else if id.contains("qsv") {
        "Intel Quick Sync"
    } else if id == "obs_x264" {
        "x264 (software)"
    } else {
        "Encoder"
    }
}

/// Picks the best available encoders: hardware H.264 when present.
pub fn default_encoders(available: &[String], video_bitrate_kbps: u32, constant_bitrate: bool) -> EncoderConfig {
    let video = first_available(available, H264_ENCODERS).unwrap_or_else(|| "obs_x264".into());
    let audio = aac_encoder(available);
    EncoderConfig {
        video_encoder: video,
        video_bitrate_kbps,
        audio_encoder: audio,
        audio_bitrate_kbps: 160,
        constant_bitrate,
        bframes: true,
        keyint_sec: 2,
        scaled: None,
        audio_mix: 0,
    }
}

/// Asks a recording to continue in a new file; whether it will.
unsafe fn split_file(output: *mut sys::obs_output_t) -> bool {
    let mut cd = sys::calldata_t::default();
    let mut enabled = false;
    unsafe {
        if proc_call(output, "split_file", &mut cd) {
            // calldata_get_bool is a header inline over calldata_get_data.
            sys::calldata_get_data(
                &cd,
                c"split_file_enabled".as_ptr(),
                &mut enabled as *mut bool as *mut c_void,
                std::mem::size_of::<bool>(),
            );
        }
        if !cd.fixed && !cd.stack.is_null() {
            sys::bfree(cd.stack as *mut c_void);
        }
    }
    enabled
}

/// Adds a chapter to an MP4/MOV recording; false for other containers.
unsafe fn add_chapter(output: *mut sys::obs_output_t, name: &str) -> bool {
    let id = unsafe { CStr::from_ptr(sys::obs_output_get_id(output)) };
    if !matches!(id.to_bytes(), b"mp4_output" | b"mov_output") {
        return false;
    }
    let mut cd = sys::calldata_t::default();
    let name = c(name);
    unsafe {
        // calldata_set_string is a header inline: the string with its terminator.
        let bytes = name.as_bytes_with_nul();
        sys::calldata_set_data(&mut cd, c"chapter_name".as_ptr(), bytes.as_ptr() as *const c_void, bytes.len());
        let ok = proc_call(output, "add_chapter", &mut cd);
        if !cd.fixed && !cd.stack.is_null() {
            sys::bfree(cd.stack as *mut c_void);
        }
        ok
    }
}
