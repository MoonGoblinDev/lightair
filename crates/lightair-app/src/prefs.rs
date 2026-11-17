//! App preferences (docs/14-data-persistence-settings.md, "Settings"): how Lightair behaves on
//! this Mac, as opposed to workspace settings, which travel with the workspace.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Show the Go Live review sheet before going live.
    pub review_before_live: bool,
    /// Ending a stream takes a second click.
    pub confirm_end_stream: bool,
    /// Audio sample rate (Hz); used when Lightair starts.
    pub sample_rate: u32,
    /// "Listen in headphones" device: (name, id); `None`: the system default.
    #[serde(alias = "monitor_device")]
    pub headphones_device: Option<(String, String)>,
    /// Monitor straight from the audio engine (a few ms) rather than through libobs (~100 ms).
    #[serde(alias = "direct_monitor")]
    pub low_latency_headphones: bool,
    /// The monitor plays the stream mix rather than the cued tracks.
    #[serde(alias = "monitor_program")]
    pub headphones_hear_program: bool,
    /// The loudness the LUFS readout aims for.
    pub loudness_target: i32,
    /// Frames the audio engine processes at a time (128: low latency for music; 512: lighter).
    pub audio_block: usize,
    /// Defaults for newly added cameras.
    pub camera: CameraDefaults,
    /// Shortcuts that work while Lightair isn't in front.
    #[serde(alias = "global_hotkeys")]
    pub global_shortcuts: Vec<GlobalShortcut>,
    /// What the record button records.
    pub record_mode: RecordMode,
    /// The workspace open last: the one Lightair opens next.
    #[serde(alias = "last_project")]
    pub last_workspace: Option<PathBuf>,
    /// macOS's Backgrounds picture for Lightair before Background removal set it to green (put
    /// back when nothing keys the green any more; `system_effects`).
    pub macos_background_before: Option<String>,
    /// The remote API: obs-websocket, JSON-RPC and MCP (Settings › Remote API).
    pub remote: RemoteApi,
    /// Look for a new Lightair at launch and every few hours (Settings › About).
    pub check_updates: bool,
}

/// The remote API's server (its password is in the Keychain: `secrets`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteApi {
    pub enabled: bool,
    pub port: u16,
    /// Other devices on the network may connect (otherwise this Mac only).
    pub lan: bool,
    /// Clients must send the password (always, with LAN access on).
    pub require_password: bool,
}

impl Default for RemoteApi {
    fn default() -> Self {
        Self { enabled: true, port: lightair_remote::DEFAULT_PORT, lan: false, require_password: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraDefaults {
    /// Short side in pixels (720, 1080, 2160); `None`: the camera's own best.
    pub height: Option<u32>,
    /// New cameras start with Camera Studio on (its first look).
    pub studio_on: bool,
}

impl Default for CameraDefaults {
    fn default() -> Self {
        Self { height: Some(1080), studio_on: false }
    }
}

impl CameraDefaults {
    /// A new camera's settings (`lightair_core::kinds::CAMERA`): its device, at `height`.
    pub fn settings(&self, device: &str) -> serde_json::Value {
        let mut s = serde_json::json!({ "device": device });
        if let Some(h) = self.height {
            s["height"] = h.into();
        }
        s
    }
}

/// What the record button records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordMode {
    /// The program, as it's on air.
    #[default]
    Program,
    /// A Studio take: each source on its own, for the Editor.
    #[serde(alias = "take")]
    StudioTake,
}

/// A shortcut that works in any app: what it does and its keys ("cmd-shift-r").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalShortcut {
    pub action: crate::shortcuts::GlobalAction,
    pub keys: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            review_before_live: true,
            confirm_end_stream: true,
            sample_rate: 48_000,
            headphones_device: None,
            low_latency_headphones: true,
            headphones_hear_program: false,
            loudness_target: -14,
            audio_block: 256,
            camera: CameraDefaults::default(),
            global_shortcuts: Vec::new(),
            last_workspace: None,
            record_mode: RecordMode::Program,
            macos_background_before: None,
            remote: RemoteApi::default(),
            check_updates: true,
        }
    }
}

/// The sample rate libobs started with (a change applies next launch).
pub static RUNNING_SAMPLE_RATE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(48_000);

/// The loudness target, for the mixer's readout.
pub static LOUDNESS_TARGET: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-14);

pub fn path() -> PathBuf {
    lightair_platform::paths::data_dir().join("preferences.json")
}

/// Reads preferences, falling back to defaults for a missing or unreadable file.
pub fn load_from(path: &Path) -> Preferences {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Writes preferences atomically.
pub fn save_to(path: &Path, prefs: &Preferences) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(prefs)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("lightair-prefs-{}", std::process::id()));
        let file = dir.join("preferences.json");
        assert_eq!(load_from(&file), Preferences::default(), "missing file → defaults");
        let p = Preferences { review_before_live: false, confirm_end_stream: false, ..Preferences::default() };
        save_to(&file, &p).unwrap();
        assert_eq!(load_from(&file), p);
        std::fs::write(&file, b"{}").unwrap();
        assert!(load_from(&file).review_before_live, "missing keys take their defaults");
        let _ = std::fs::remove_dir_all(dir);
    }
}
