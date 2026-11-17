//! Lightair's engine adapter over libobs.
//!
//! libobs is a process-wide singleton. [`Engine`] owns it on a dedicated **engine actor**
//! thread: every mutating libobs call happens there, in order (docs/03-architecture.md,
//! "Threading model"). Other threads send jobs with [`Engine::call`] / [`Engine::post`].

pub mod access;
pub mod audio;
pub mod audio_host;
pub mod backdrop;
pub mod pointer;
#[cfg(target_os = "macos")]
pub mod on_screen;
#[cfg(target_os = "macos")]
pub mod system_audio;
pub mod devices;
pub mod displays;
pub mod kinds;
mod engine;
pub mod gpu;
pub mod vcam;
mod log;
pub mod loudness;
pub mod media;
#[cfg(target_os = "macos")]
pub mod media_source;
pub mod meters;
pub mod monitoring;
pub mod obs;
pub mod outputs;
pub mod web;
pub mod player;
pub mod plugins;
mod reconcile;
mod runtime;
pub mod studio_take;
#[doc(hidden)]
pub mod testing;
pub mod timers;
mod transition;

pub use engine::{Capabilities, Engine, EngineConfig, EngineError, EngineState, Graphics, Modules, Stats, read_stats};
pub use libobs_sys as sys;
pub use reconcile::Reconciler;
pub use runtime::Runtime;

/// OBS modules Lightair loads from an OBS 33 plugin set: the ones that only need libobs.
/// Modules that link Qt or obs-frontend-api are handled by the compatibility object
/// (docs/13-extensibility-and-compatibility.md).
#[cfg(not(windows))]
pub const DEFAULT_MODULES: &[&str] = &[
    "aja",
    "coreaudio-encoder",
    "decklink",
    "image-source",
    "lightair-camera-fx",
    "mac-avcapture",
    "mac-avcapture-legacy",
    "mac-capture",
    "mac-syphon",
    "mac-videotoolbox",
    "obs-ffmpeg",
    "obs-filters",
    "obs-outputs",
    "obs-transitions",
    "obs-webrtc",
    "obs-x264",
    "rtmp-services",
    "text-freetype2",
    "vlc-video",
];

/// Windows: display, window and game capture; cameras and capture cards (DirectShow); WASAPI
/// devices, desktop and per-app audio; NVENC and QSV (AMF is in obs-ffmpeg).
#[cfg(windows)]
pub const DEFAULT_MODULES: &[&str] = &[
    "aja",
    "decklink",
    "image-source",
    "lightair-camera-fx",
    "obs-ffmpeg",
    "obs-filters",
    "obs-nvenc",
    "obs-outputs",
    "obs-qsv11",
    "obs-transitions",
    "obs-webrtc",
    "obs-x264",
    "rtmp-services",
    "text-freetype2",
    "vlc-video",
    "win-capture",
    "win-dshow",
    "win-wasapi",
];
