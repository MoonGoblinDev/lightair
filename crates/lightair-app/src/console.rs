//! The console: what in the show needs attention now, each with its fix. A microphone or
//! camera the show uses that isn't connected (unplugged, renamed, another computer's), a
//! stream or recording that stopped on an error. Shown as a count at the footer's left; the
//! console lists them (shell/console_panel).

use lightair_core::{Id, Workspace, kinds};
use lightair_engine::outputs::{OutputKey, OutputState, runtime};

/// The devices connected now: (id, name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Connected {
    pub mics: Vec<(String, String)>,
    pub cameras: Vec<(String, String)>,
}

impl Connected {
    /// Asks the system (cameras through the engine's thread). Not on the UI thread.
    pub fn now(engine: &lightair_engine::Engine) -> Connected {
        let mics = lightair_engine::devices::input_devices().into_iter().map(|d| (d.id, d.name)).collect();
        // (name, id) from the engine
        let cameras = engine.call(|_| lightair_engine::kinds::cameras()).into_iter().map(|(name, id)| (id, name)).collect();
        Connected { mics, cameras }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// Point the source at another device (a microphone, or a camera).
    Device { source: Id, camera: bool },
    /// Nothing to do here: the message says what happened.
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attention {
    /// Stable, for the UI's ids.
    pub key: String,
    pub title: String,
    pub detail: String,
    pub fix: Fix,
}

/// Whether a source is used: by a track (audio) or an object in some scene.
fn used(w: &Workspace, source: Id) -> bool {
    w.tracks.iter().any(|t| t.source == source) || w.scenes.iter().any(|s| s.objects.iter().any(|o| o.source == Some(source)))
}

/// What needs attention in `w`, given what's connected (None: not looked yet, no device items).
pub fn look(w: &Workspace, connected: Option<&Connected>) -> Vec<Attention> {
    let mut out = Vec::new();
    if let Some(c) = connected {
        for s in w.sources.values() {
            let device = s.settings["device"].as_str().unwrap_or("");
            if device.is_empty() || !used(w, s.id) {
                continue;
            }
            let (missing, camera) = match s.kind.as_str() {
                kinds::AUDIO_DEVICE | kinds::MIC => (!lightair_engine::devices::is_system_default(device) && !c.mics.iter().any(|(id, _)| id == device), false),
                kinds::CAMERA => (!c.cameras.iter().any(|(id, _)| id == device), true),
                _ => continue,
            };
            if missing {
                let what = if camera { "camera" } else { "microphone or audio interface" };
                out.push(Attention {
                    key: format!("device-{}", s.id),
                    title: format!("“{}” isn't connected", s.name),
                    detail: format!("Its {what} is unplugged, off, or on another computer. Plug it in, or use another one."),
                    fix: Fix::Device { source: s.id, camera },
                });
            }
        }
    }
    for d in &w.outputs.destinations {
        if let OutputState::Failed(why) = &runtime().get(OutputKey::Stream(d.id)).state {
            out.push(Attention { key: format!("stream-{}", d.id), title: format!("Streaming to {} stopped", d.name), detail: why.clone(), fix: Fix::None });
        }
    }
    if let OutputState::Failed(why) = &runtime().recording().state {
        out.push(Attention { key: "recording".into(), title: "Recording stopped".into(), detail: why.clone(), fix: Fix::None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightair_core::Source;
    use std::sync::Arc;

    #[test]
    fn a_used_device_thats_gone_needs_attention() {
        let mut w = Workspace::new("Show");
        let source = |name: &str, kind: &str, device: &str| Source { id: Id::new(), name: name.into(), kind: kind.into(), settings: serde_json::json!({"device": device}), effects: vec![] };
        let mic = source("RGBMIC-USB audio", kinds::MIC, "usb:2");
        let spare = source("Old cam", kinds::CAMERA, "cam:9");
        w.sources.insert(mic.id, Arc::new(mic.clone()));
        w.sources.insert(spare.id, Arc::new(spare));
        w.tracks.push(Arc::new(lightair_core::Track::new("Mic", mic.id)));
        let gone = Connected { mics: vec![("builtin".into(), "MacBook Mic".into())], cameras: vec![] };
        let found = look(&w, Some(&gone));
        // the unused camera isn't listed
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fix, Fix::Device { source: mic.id, camera: false });
        assert!(found[0].title.contains("RGBMIC-USB audio"));
        let back = Connected { mics: vec![("usb:2".into(), "RGBMIC".into())], cameras: vec![] };
        assert!(look(&w, Some(&back)).is_empty());
    }
}
