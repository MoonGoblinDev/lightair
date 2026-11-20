//! A Studio studio_take's video (docs/plans/screen-studio.md, 4.2): each screen and camera in the
//! program recorded to a file of its own, at its own size, as fragmented MP4 (a studio_take that stops
//! abruptly keeps what was written).
//!
//! - **Screens** are recorded from a capture of their own, with the cursor off and no effects:
//!   the Editor draws the pointer again from the studio_take's source log.
//! - **Cameras** are recorded as they look (their effects on).
//!
//! Each file comes from a private canvas the size of its source, holding just that source.
//! Audio goes to the studio_take's multitrack files (the audio engine's track_files), started beside this.

use std::path::{Path, PathBuf};

use lightair_model::{Id, Source, kinds};
use serde_json::json;

use crate::obs::{ObsCanvas, Data, ObsScene, ObsSource};
use crate::outputs::{Container, OutputKey, Outputs, RecordingConfig, default_encoders, runtime};
use crate::sys;

/// One recorded file.
#[derive(Debug, Clone)]
pub struct Track {
    /// "Screen", "Camera", "Camera 2"…
    pub name: String,
    /// A screen (else a camera): what it is, apart from its name.
    pub screen: bool,
    pub source: Id,
    pub file: PathBuf,
    pub width: u32,
    pub height: u32,
}

struct Part {
    key: Id,
    // dropped after the output stops (field order: scene, then canvas)
    _own: Option<ObsSource>,
    _scene: ObsScene,
    _canvas: ObsCanvas,
}

pub struct StudioTake {
    parts: Vec<Part>,
    pub tracks: Vec<Track>,
}

// SAFETY: libobs objects used on the engine thread only.
unsafe impl Send for StudioTake {}

fn even(v: u32) -> u32 {
    (v.max(2)) & !1
}

impl StudioTake {
    /// Starts recording `sources` (sources and their live sources) into `folder`.
    pub fn start(
        sources: Vec<(Source, *mut sys::obs_source_t)>,
        folder: &Path,
        fps: (u32, u32),
        outputs: &mut Outputs,
        encoders: &[String],
    ) -> Result<StudioTake, String> {
        std::fs::create_dir_all(folder).map_err(|e| format!("Can't create the take's folder: {e}"))?;
        let mut parts = Vec::new();
        let mut tracks = Vec::new();
        let (mut screens, mut cameras) = (0, 0);
        for (source, live) in sources {
            let screen = source.kind == kinds::SCREEN;
            // the screen without the cursor or effects; the camera as it is
            let own = if screen {
                let mut settings = source.settings.clone();
                settings["cursor"] = json!(false);
                let Some((kind, s)) = crate::kinds::resolve(&source.kind, &settings) else { continue };
                ObsSource::create_private(kind, &format!("take:{}", source.id), Some(&Data::from_json(&s)))
            } else {
                None
            };
            let src = own.as_ref().map_or(live, |s| s.as_ptr());
            if src.is_null() {
                continue;
            }
            let (w, h) = unsafe { (sys::obs_source_get_width(live), sys::obs_source_get_height(live)) };
            let (w, h) = if w > 0 && h > 0 { (even(w), even(h)) } else { (1920, 1080) };
            let key = Id::new();
            let Some(obs_canvas) = ObsCanvas::create(&format!("take:{key}"), w, h, fps.0, fps.1) else { continue };
            let Some(scene) = obs_canvas.create_scene(&format!("take:{key}")) else { continue };
            unsafe {
                sys::obs_scene_add(scene.as_ptr(), src);
                sys::obs_canvas_set_channel(obs_canvas.as_ptr(), 0, scene.source_ptr());
            }
            let name = if screen {
                screens += 1;
                if screens == 1 { "Screen".to_string() } else { format!("Screen {screens}") }
            } else {
                cameras += 1;
                if cameras == 1 { "Camera".to_string() } else { format!("Camera {cameras}") }
            };
            let file = folder.join(format!("{}.mp4", name.to_lowercase().replace(' ', "-")));
            // a generous bitrate: the Editor re-encodes from these
            let kbps = ((w as u64 * h as u64 * fps.0 as u64 / fps.1.max(1) as u64) / 40_000).clamp(6_000, 80_000) as u32;
            let mut enc = default_encoders(encoders, kbps, false);
            enc.keyint_sec = 1;
            let cfg = RecordingConfig {
                canvas: key,
                path: file.clone(),
                label: None,
                split_minutes: None,
                container: Container::HybridMp4,
                encoders: enc,
                extra_mixes: Vec::new(),
            };
            let video = crate::outputs::VideoOutput(unsafe { sys::obs_canvas_get_video(obs_canvas.as_ptr()) });
            outputs.start_recording(&cfg, Some(video))?;
            tracks.push(Track { name, screen, source: source.id, file, width: w, height: h });
            parts.push(Part { key, _own: own, _scene: scene, _canvas: obs_canvas });
        }
        if parts.is_empty() {
            return Err("The program has no screen or camera to record.".into());
        }
        Ok(StudioTake { parts, tracks })
    }

    /// Whether every file has stopped (the studio_take can be let go).
    pub fn finished(&self) -> bool {
        self.parts.iter().all(|p| !runtime().get(OutputKey::Recording(p.key)).state.is_running())
    }
}
