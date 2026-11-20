//! Plugin hosting (docs/plans/audio-engine.md, Plugins): Audio Units (v2 and v3) through the
//! AudioToolbox C API, and VST3 ([`vst3`]). AUv3 units load out of process when the system
//! chooses to, so a crashing one is reported rather than fatal.
//!
//! A plugin insert in the workspace ("audio.plugin": format, id, name, vendor, state) gets its
//! instance here, keyed by the effect's id. Loading is asynchronous: until it's ready the insert
//! passes audio through, and [`changed`] tells the reconciler to play the mixer again.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use lightair_audio::dsp::Plugin;
use lightair_model::{Effect, Id};
use parking_lot::Mutex;

#[cfg(target_os = "macos")]
pub mod au;
pub mod vst3;

#[cfg(target_os = "macos")]
pub use au::{AudioUnitPlugin, load_audio_unit, scan_audio_units};

/// Audio Units exist on macOS only.
#[cfg(not(target_os = "macos"))]
pub fn scan_audio_units() -> Vec<PluginInfo> {
    Vec::new()
}

/// An installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    /// "au" or "vst3".
    pub format: String,
    /// The format's identifier ("au:<type>:<subtype>:<manufacturer>", hex).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// An AUv3 app extension (out of process).
    pub v3: bool,
    /// A VST3's bundle (empty for Audio Units, which the system finds).
    #[serde(default)]
    pub path: String,
}

// ------------------------------------------------------------------ the main thread

type Job = Box<dyn FnOnce() + Send>;

static MAIN: std::sync::OnceLock<Box<dyn Fn(Job) + Send + Sync>> = std::sync::OnceLock::new();

/// How work reaches the main thread (the app's run loop). Without one, it runs where it's asked
/// (tests, which run on the main thread).
pub fn set_main_thread(run: impl Fn(Job) + Send + Sync + 'static) {
    let _ = MAIN.set(Box::new(run));
}

pub(crate) fn on_main(job: Job) {
    match MAIN.get() {
        Some(run) => run(job),
        None => job(),
    }
}

// ------------------------------------------------------------------ the workspace's inserts

/// A running plugin of either format.
#[derive(Clone)]
pub enum Hosted {
    #[cfg(target_os = "macos")]
    Au(Arc<AudioUnitPlugin>),
    Vst3(Arc<vst3::Vst3Plugin>),
}

impl Hosted {
    fn plugin(&self) -> Arc<dyn Plugin> {
        match self {
            #[cfg(target_os = "macos")]
            Hosted::Au(p) => p.clone(),
            Hosted::Vst3(p) => p.clone(),
        }
    }

    fn failed(&self) -> bool {
        match self {
            #[cfg(target_os = "macos")]
            Hosted::Au(p) => p.crashed(),
            Hosted::Vst3(p) => p.failed(),
        }
    }

    fn latency(&self) -> usize {
        match self {
            #[cfg(target_os = "macos")]
            Hosted::Au(p) => p.latency(),
            Hosted::Vst3(p) => {
                p.refresh_latency();
                p.latency()
            }
        }
    }

    /// Its state, base64 (VST3: main thread).
    pub fn state(&self) -> Option<String> {
        match self {
            #[cfg(target_os = "macos")]
            Hosted::Au(p) => p.state(),
            Hosted::Vst3(p) => p.state(),
        }
    }

    fn set_state(&self, s: &str) {
        match self {
            #[cfg(target_os = "macos")]
            Hosted::Au(p) => {
                p.set_state(s);
            }
            Hosted::Vst3(p) => {
                let (p, s) = (p.clone(), s.to_string());
                on_main(Box::new(move || {
                    p.set_state(&s);
                }));
            }
        }
    }

    pub fn name(&self) -> &str {
        match self {
            #[cfg(target_os = "macos")]
            Hosted::Au(p) => &p.info.name,
            Hosted::Vst3(p) => &p.info.name,
        }
    }
}

/// Where a plugin insert stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready { latency: usize },
    Failed(String),
    /// Not a plugin insert the host knows.
    None,
}

enum Slot {
    Loading,
    Ready(Hosted),
    Failed(String),
}

struct Entry {
    id: String,
    slot: Slot,
    /// The state last given to (or read from) the instance.
    applied: Option<String>,
}

static ENTRIES: LazyLock<Mutex<HashMap<Id, Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static GENERATION: AtomicU64 = AtomicU64::new(0);
static SEEN: AtomicU64 = AtomicU64::new(0);
static SAMPLE_RATE: AtomicU32 = AtomicU32::new(48_000);

/// The rate plugins are set up for (the engine's).
pub fn set_sample_rate(rate: u32) {
    SAMPLE_RATE.store(rate, Ordering::Relaxed);
}

/// Whether a plugin finished loading (or failed) since the last call: the mixer should be
/// played again to pick it up.
pub fn changed() -> bool {
    let g = GENERATION.load(Ordering::Acquire);
    SEEN.swap(g, Ordering::AcqRel) != g
}

fn setting<'a>(e: &'a Effect, key: &str) -> Option<&'a str> {
    e.settings.get(key).and_then(serde_json::Value::as_str)
}

/// The installed VST3 bundle that has the class `id` (after a move or reinstall).
fn find_vst3(id: &str) -> Option<String> {
    vst3::bundles().into_iter().find_map(|b| {
        vst3::from_module_info(&b)
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.path)
    })
}

/// The running instance for a plugin insert, starting it if needed (`None` until it's ready).
/// A changed "state" setting (undo, a preset) is given to the running instance.
pub fn instance(e: &Effect) -> Option<Arc<dyn Plugin>> {
    let (format, id) = (setting(e, "format")?, setting(e, "id")?);
    if format != "au" && format != "vst3" {
        return None;
    }
    let state = setting(e, "state").map(str::to_string);
    {
        let mut entries = ENTRIES.lock();
        if let Some(entry) = entries.get_mut(&e.id)
            && entry.id == id
        {
            return match &entry.slot {
                Slot::Ready(p) => {
                    if let Some(s) = &state
                        && entry.applied.as_ref() != Some(s)
                    {
                        p.set_state(s);
                        entry.applied = Some(s.clone());
                    }
                    Some(p.plugin())
                }
                Slot::Loading | Slot::Failed(_) => None,
            };
        }
        entries.insert(e.id, Entry { id: id.to_string(), slot: Slot::Loading, applied: None });
    }
    // outside the lock: a plugin may finish loading right inside this call
    let mut info = PluginInfo {
        format: format.to_string(),
        id: id.to_string(),
        name: setting(e, "name").unwrap_or("Plugin").to_string(),
        vendor: setting(e, "vendor").unwrap_or("").to_string(),
        version: String::new(),
        v3: false,
        path: setting(e, "path").unwrap_or("").to_string(),
    };
    let effect = e.id;
    let rate = SAMPLE_RATE.load(Ordering::Relaxed) as f64;
    let (pid, pname) = (info.id.clone(), info.name.clone());
    let finish = move |result: Result<Hosted, String>| {
        let mut entries = ENTRIES.lock();
        let Some(entry) = entries.get_mut(&effect).filter(|en| en.id == pid) else { return };
        match result {
            Ok(p) => {
                if let Some(s) = &state {
                    p.set_state(s);
                }
                entry.applied = state;
                tracing::info!(plugin = %pname, latency = p.latency(), "plugin loaded");
                entry.slot = Slot::Ready(p);
            }
            Err(why) => {
                tracing::warn!(plugin = %pname, %why, "plugin didn't load");
                entry.slot = Slot::Failed(why);
            }
        }
        GENERATION.fetch_add(1, Ordering::AcqRel);
    };
    if format == "au" {
        #[cfg(target_os = "macos")]
        load_audio_unit(&info, rate, move |r| finish(r.map(Hosted::Au)));
        #[cfg(not(target_os = "macos"))]
        finish(Err("Audio Units are macOS only".into()));
    } else {
        on_main(Box::new(move || {
            if !std::path::Path::new(&info.path).exists()
                && let Some(path) = find_vst3(&info.id)
            {
                info.path = path;
            }
            finish(vst3::Vst3Plugin::load(&info, rate).map(Hosted::Vst3))
        }));
    }
    None
}

/// Where an insert's plugin stands.
pub fn status(effect: Id) -> Status {
    match ENTRIES.lock().get(&effect).map(|e| &e.slot) {
        Some(Slot::Loading) => Status::Loading,
        Some(Slot::Ready(p)) if p.failed() => Status::Failed("it stopped working".into()),
        Some(Slot::Ready(p)) => Status::Ready { latency: p.latency() },
        Some(Slot::Failed(why)) => Status::Failed(why.clone()),
        None => Status::None,
    }
}

/// An insert's running plugin (its editor, its state).
pub fn hosted(effect: Id) -> Option<Hosted> {
    match ENTRIES.lock().get(&effect).map(|e| &e.slot) {
        Some(Slot::Ready(p)) => Some(p.clone()),
        _ => None,
    }
}

/// Reads an insert's current state from its instance (what its editor changed). Remembered as
/// applied, so writing it into the workspace doesn't give it back to the instance. Main thread.
pub fn take_state(effect: Id) -> Option<String> {
    let p = hosted(effect)?;
    let s = p.state()?;
    if let Some(entry) = ENTRIES.lock().get_mut(&effect) {
        entry.applied = Some(s.clone());
    }
    Some(s)
}

/// Lets go of instances whose inserts are gone.
pub fn retain(keep: &HashSet<Id>) {
    let gone: Vec<Entry> = {
        let mut entries = ENTRIES.lock();
        let ids: Vec<Id> = entries.keys().filter(|id| !keep.contains(id)).copied().collect();
        ids.into_iter().filter_map(|id| entries.remove(&id)).collect()
    };
    // disposed outside the lock
    drop(gone);
}

/// The plugin inserts in a workspace (tracks, buses, mixes, master).
pub fn inserts(workspace: &lightair_model::Workspace) -> HashSet<Id> {
    let a = &workspace.audio;
    workspace
        .tracks
        .iter()
        .flat_map(|t| t.effects.iter())
        .chain(a.buses.iter().flat_map(|b| b.effects.iter()))
        .chain(a.mixes.iter().flat_map(|m| m.effects.iter()))
        .chain(a.master.effects.iter())
        .filter(|e| e.kind == lightair_model::catalog::PLUGIN_KIND)
        .map(|e| e.id)
        .collect()
}
