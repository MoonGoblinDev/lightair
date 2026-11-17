//! The reconciler: makes libobs match the Lightair document (docs/04-engine-libobs.md,
//! "Reconciler"). It consumes patches from `lightair_core::diff`, runs on the engine thread,
//! and is idempotent: applying `diff(empty, workspace)` to an empty engine gives the same libobs
//! state as any sequence of edits ending at `workspace`.
//!
//! libobs objects are named by their Lightair id, so user-facing names never collide.

use std::collections::{HashMap, HashSet};

use libobs_sys as sys;
use lightair_core::catalog::lower;
use lightair_core::{
    Effect, Fit, Canvas, Id, Object, ObjectPatch, Layout, Patch, Workspace, Track, TransitionKind, TransitionSettings,
    diff_objects,
};
use parking_lot::Mutex;

use crate::timers::{TIMER_KIND, TIMER_SOURCE, TimerAction, TimerStatus, Timers, on_timers_tick, tick_param};
use crate::transition::{CanvasMove, MoveAnim, Tween, item_fade, match_object};

use crate::meters::{MeterCell, db_to_mul, meters};
use crate::obs::{ObsCanvas, Data, ObsScene, ObsSource, vec2};
use std::sync::Arc;

/// libobs state owned on behalf of the document.
///
/// Every document scene exists once per canvas: one libobs scene in that canvas's canvas,
/// all sharing the same source sources (docs/07-scenes-objects-canvas.md, "Formats").
#[derive(Default)]
pub struct Reconciler {
    sources: HashMap<Id, ObsSource>,
    /// Source effect chains (Lightair Effect Stack, colour correction, …).
    source_effects: HashMap<Id, Chain>,
    /// Screen captures zoomed and given a drawn pointer by their scene's `ScreenZoomPointer` (the
    /// pointer filter, last on the source, and the settings it was given).
    screen_zoom_pointers: HashMap<Id, (ObsSource, serde_json::Value)>,
    /// Canvases for additional canvases. The primary canvas uses libobs's main canvas.
    obs_canvases: HashMap<Id, ObsCanvas>,
    /// (scene, canvas) → libobs scene.
    scenes: HashMap<(Id, Id), ObsScene>,
    /// Scene items are owned by their scene; (scene, canvas, object) → item.
    items: HashMap<(Id, Id, Id), ItemPtr>,
    /// Shaped objects: the item above is a one-item group; this is the source's item inside it
    /// (carrying the crop) and the shape mask on the group.
    masks: HashMap<(Id, Id, Id), Masked>,
    /// The object being cropped (scene, canvas, object): shown whole and unshaped meanwhile.
    crop_preview: Option<(Id, Id, Id)>,
    /// Leaving crop view: the ghost is fading out.
    crop_fading: bool,
    canvases: Vec<Id>,
    primary_canvas: Option<Id>,
    program: Option<Id>,
    tracks: HashMap<Id, TrackState>,
    /// The audio engine playing the mixer (docs/plans/audio-engine.md); started with the first
    /// track. `LIGHTAIR_LEGACY_AUDIO=1` keeps libobs mixing the tracks instead.
    audio: Option<crate::audio_host::AudioHost>,
    /// The direct monitor wanted: output device ("" default) and whether it plays the program
    /// (else the cue mix). Applied whenever the audio engine runs.
    monitor: Option<(String, bool)>,
    /// Frames per audio block (`None`: the default).
    audio_block: Option<usize>,
    /// Each canvas's output channel goes through its own transition.
    transitions: HashMap<Id, ObsSource>,
    transition_settings: TransitionSettings,
    stinger: Option<lightair_core::Stinger>,
    /// The running Move, driven by a libobs tick callback.
    anim: Arc<Mutex<Option<MoveAnim>>>,
    tick_registered: bool,
    /// Sources held "showing" off air (e.g. to learn their size), released with `release_sources`.
    held: HashSet<Id>,
    /// Countdown/stopwatch/clock sources, driven by their own tick callback.
    timers: Arc<Timers>,
    timers_tick: bool,
    /// Sources held back until the user answers macOS's camera or microphone prompt
    /// ([`crate::access`]).
    awaiting_access: HashSet<Id>,
}

/// libobs state for one mixer track: its output channel, volume meter and effect filters.
struct TrackState {
    source: Id,
    /// Output channel (1..64) that keeps a sound-only source running whatever is on air; 0 for
    /// a source with a picture, which runs only while it's on air (as in OBS).
    channel: u32,
    volmeter: VolmeterPtr,
    meter: Arc<MeterCell>,
    filters: Vec<(Id, ObsSource)>,
}

struct VolmeterPtr(*mut sys::obs_volmeter_t);
unsafe impl Send for VolmeterPtr {}

#[derive(Clone, Copy)]
struct ItemPtr(*mut sys::obs_sceneitem_t);
unsafe impl Send for ItemPtr {}

/// The inside of a object drawn through Lightair's filters: the source's item in the group, the
/// shape mask, and the flip/3D transform (applied in that order).
pub(crate) struct Masked {
    pub inner: *mut sys::obs_sceneitem_t,
    pub filter: ObsSource,
    pub transform: Option<ObsSource>,
}
unsafe impl Send for Masked {}

impl Reconciler {
    /// Applies patches computed from two snapshots; `workspace` is the newer one.
    pub fn apply(&mut self, workspace: &Workspace, patches: &[Patch]) {
        self.primary_canvas = Some(workspace.primary_canvas().id);
        let transition_changed = self.transition_settings != workspace.transition || self.stinger != workspace.stinger;
        self.transition_settings = workspace.transition;
        self.stinger = workspace.stinger.clone();
        self.cleanup();
        // Canvases are synced from the snapshot itself, so a rebuild against `empty_like()`
        // (which keeps the canvases) still creates every canvas.
        let mut canvases_added = false;
        for canvas in &workspace.canvases {
            if !self.canvases.contains(&canvas.id) {
                self.add_canvas(workspace, canvas);
                canvases_added = true;
            }
        }
        for id in self.canvases.clone() {
            if workspace.canvas(id).is_none() {
                self.remove_canvas(id);
            }
        }
        // Set transitions up when they change, not at the switch: a stinger has to load its
        // video before it can play.
        if transition_changed || canvases_added {
            self.prepare_transitions();
        }
        let mut recreated = false;
        for patch in patches {
            match patch {
                Patch::CanvasAdded(_) | Patch::CanvasRemoved(_) => {}
                Patch::CanvasChanged(canvas) => self.resize_canvas(canvas),
                Patch::SourceAdded(source) => self.create_source(source),
                Patch::SourceChanged { before, after } => {
                    // a portable kind that plays as another libobs type now (Windows: a
                    // display capture becoming a window capture)
                    if crate::kinds::libobs_kind(&before.kind, &before.settings)
                        != crate::kinds::libobs_kind(&after.kind, &after.settings)
                    {
                        self.recreate_source(after, workspace);
                        recreated = true;
                    } else if let Some(src) = self.sources.get(&after.id) {
                        if before.settings != after.settings || draws_pointer(before) != draws_pointer(after) {
                            let drawn = self.screen_zoom_pointers.get(&after.id).is_some_and(|(_, s)| s["hide_capture"] == true);
                            src.update(&data_from_json(&source_settings(after, drawn)));
                            if after.kind == TIMER_KIND {
                                self.timers.configure(after.id, &after.settings);
                            }
                        }
                        let chain = self.source_effects.entry(after.id).or_default();
                        unsafe { chain_update(src.as_ptr(), chain, &lower(&before.effects), &lower(&after.effects)) };
                    }
                }
                Patch::SourceRemoved(id) => self.destroy_source(*id),
                // libobs draws the flattened hierarchy: pictures only, in world terms (a parent
                // moving changes its children's world layouts, so they update too).
                Patch::SceneAdded(scene) => {
                    let scene = scene.flatten();
                    for canvas in self.canvases.clone() {
                        self.add_scene_in(&scene, canvas);
                    }
                }
                Patch::SceneChanged { before, after } => {
                    let (before, after) = (&before.flatten(), &after.flatten());
                    for canvas in self.canvases.clone() {
                        for lp in diff_objects(before, after) {
                            match lp {
                                ObjectPatch::Added { object, index } => self.add_object(after.id, canvas, object, index),
                                ObjectPatch::Removed { object } => {
                                    self.masks.remove(&(after.id, canvas, object));
                                    if let Some(item) = self.items.remove(&(after.id, canvas, object)) {
                                        unsafe { sys::obs_sceneitem_remove(item.0) };
                                    }
                                }
                                ObjectPatch::Changed { object } => {
                                    let index = after.objects.iter().position(|l| l.id == object.id).unwrap_or(0);
                                    self.update_object(after.id, canvas, object, index)
                                }
                                ObjectPatch::Order(order) => self.reorder(after.id, canvas, &order),
                            }
                        }
                    }
                }
                Patch::SceneRemoved(id) => {
                    if self.program == Some(*id) {
                        self.set_program(None, workspace);
                    }
                    self.items.retain(|(scene, _, _), _| scene != id);
                    self.masks.retain(|(scene, _, _), _| scene != id);
                    self.scenes.retain(|(scene, _), _| scene != id);
                }
                Patch::ProgramChanged(program) => self.set_program(*program, workspace),
                Patch::TrackAdded(track) => self.add_track(track),
                Patch::TrackChanged { before, after } => self.update_track(before, after),
                Patch::TrackRemoved(id) => self.remove_track(*id),
                Patch::TrackOrder(_) => {}
                // The audio engine takes the mixer from the workspace (docs/plans/audio-engine.md).
                Patch::AudioSetup => {}
            }
        }
        let audio_changed = recreated || patches.iter().any(|p| {
            matches!(
                p,
                Patch::TrackAdded(_)
                    | Patch::TrackChanged { .. }
                    | Patch::TrackRemoved(_)
                    | Patch::AudioSetup
                    | Patch::SourceAdded(_)
                    | Patch::SourceRemoved(_)
            ) || matches!(p, Patch::SourceChanged { after, .. } if after.kind == crate::devices::DEVICE_KIND)
        });
        if audio_changed {
            self.sync_audio(workspace);
        }
        let screens_changed = patches.iter().any(|p| {
            matches!(
                p,
                Patch::SceneAdded(_)
                    | Patch::SceneChanged { .. }
                    | Patch::SceneRemoved(_)
                    | Patch::ProgramChanged(_)
                    | Patch::SourceAdded(_)
                    | Patch::SourceChanged { .. }
                    | Patch::SourceRemoved(_)
            )
        });
        if screens_changed || recreated {
            self.sync_screen_zoom_pointers(workspace);
        }
    }

    /// Puts each scene's `ScreenZoomPointer` on its screen captures: a pointer filter (zoom, drawn
    /// pointer, clicks) last on each, the capture's own pointer hidden while Lightair draws one.
    /// A capture in several scenes takes the look of the scene on air, else the first one's.
    fn sync_screen_zoom_pointers(&mut self, workspace: &Workspace) {
        let mut want: HashMap<Id, lightair_core::ScreenZoomPointer> = HashMap::new();
        let on_air = workspace.program.and_then(|p| workspace.scene(p));
        for scene in on_air.into_iter().chain(workspace.scenes.iter()) {
            let Some(look) = scene.screen else { continue };
            for object in &scene.objects {
                let Some(source) = object.source.and_then(|i| workspace.sources.get(&i)) else { continue };
                if is_screen(source) {
                    want.entry(source.id).or_insert(look);
                }
            }
        }
        // gone: the filter off, the capture's own pointer back
        let gone: Vec<Id> = self.screen_zoom_pointers.keys().copied().filter(|id| !want.contains_key(id)).collect();
        for id in gone {
            if let (Some((filter, was)), Some(src)) = (self.screen_zoom_pointers.remove(&id), self.sources.get(&id)) {
                unsafe { sys::obs_source_filter_remove(src.as_ptr(), filter.as_ptr()) };
                if let Some(source) = workspace.sources.get(&id).filter(|i| !draws_pointer(i))
                    && was["hide_capture"] == true
                {
                    src.update(&data_from_json(&source_settings(source, false)));
                }
            }
        }
        for (id, look) in want {
            let (Some(src), Some(source)) = (self.sources.get(&id), workspace.sources.get(&id)) else { continue };
            let settings = screen_zoom_pointer_settings(&look);
            // drawn before (a new filter: as the capture was made, with its own pointer)
            let hid = self.screen_zoom_pointers.get(&id).map_or(draws_pointer(source), |(_, s)| s["hide_capture"] == true || draws_pointer(source));
            match self.screen_zoom_pointers.get_mut(&id) {
                Some((_, was)) if *was == settings => {}
                Some((filter, was)) => {
                    filter.update(&data_from_json(&settings));
                    *was = settings;
                }
                None => {
                    let Some(filter) = ObsSource::create_private(crate::pointer::KIND, &format!("screen-zoom-pointer:{id}"), Some(&data_from_json(&settings))) else {
                        tracing::warn!("could not create the screen look");
                        continue;
                    };
                    unsafe { sys::obs_source_filter_add(src.as_ptr(), filter.as_ptr()) };
                    self.screen_zoom_pointers.insert(id, (filter, settings));
                }
            }
            // the capture's own pointer, hidden while Lightair draws one (only when that changes:
            // a capture given new settings may start again)
            let hide = look.pointer_drawn || look.pointer_hidden;
            if hid != (hide || draws_pointer(source)) {
                src.update(&data_from_json(&source_settings(source, hide)));
            }
            // last, so it zooms what the source's effects made
            if let Some((filter, _)) = self.screen_zoom_pointers.get(&id) {
                unsafe { sys::obs_source_filter_set_order(src.as_ptr(), filter.as_ptr(), sys::OBS_ORDER_MOVE_BOTTOM) };
            }
        }
    }

    /// Whether the audio engine plays the mixer (rather than libobs mixing the tracks).
    fn engine_audio() -> bool {
        std::env::var("LIGHTAIR_LEGACY_AUDIO").map_or(true, |v| v != "1")
    }

    /// Brings the audio engine up to date: captures every track's source, releases sources no
    /// track uses, and plays the workspace's mixer.
    fn sync_audio(&mut self, workspace: &Workspace) {
        if !Self::engine_audio() || (workspace.tracks.is_empty() && self.audio.is_none()) {
            return;
        }
        if self.audio.is_none() {
            let mut info = sys::obs_audio_info::default();
            let rate = if unsafe { sys::obs_get_audio_info(&mut info) } { info.samples_per_sec } else { 48_000 };
            crate::plugins::set_sample_rate(rate);
            let block = self.audio_block.unwrap_or(crate::audio_host::BLOCK);
            self.audio = crate::audio_host::AudioHost::start(rate, block);
        }
        let Some(host) = self.audio.as_mut() else { return };
        let monitor = self.monitor.clone().map(|(d, program)| (d, if program { lightair_audio::PROGRAM } else { crate::audio::CUE }));
        if let Err(e) = host.set_headphones(monitor) {
            tracing::warn!(%e, "direct monitoring unavailable; libobs monitors the cue");
        }
        let wanted: HashSet<Id> = workspace.tracks.iter().map(|t| t.source).collect();
        let is_device = |id: &Id| workspace.sources.get(id).is_some_and(|i| i.kind == crate::devices::DEVICE_KIND);
        let stale: Vec<Id> = host.captured().filter(|i| !wanted.contains(i) || is_device(i)).collect();
        for source in stale {
            host.release(source);
        }
        let stale: Vec<Id> = host.open_devices().filter(|i| !wanted.contains(i)).collect();
        for source in stale {
            host.close_device(source);
        }
        for source in &wanted {
            let Some(i) = workspace.sources.get(source) else { continue };
            if i.kind == crate::devices::DEVICE_KIND {
                // an interface: the engine opens it itself, every channel
                let device = i.settings["device"].as_str().unwrap_or("");
                if let Err(e) = host.open_device(*source, device) {
                    tracing::warn!(%e, device, "couldn't open the audio input");
                }
            } else if let Some(src) = self.sources.get(source) {
                host.capture(*source, src);
            }
        }
        host.set(crate::audio::spec(workspace));
        crate::plugins::retain(&crate::plugins::inserts(workspace));
        let mut cells: Vec<_> = workspace
            .tracks
            .iter()
            .filter_map(|t| meters().get(t.id).map(|cell| (crate::audio::node(t.id), cell)))
            .collect();
        // buses, mixes and the master show in the mixer too
        for id in workspace.audio.buses.iter().map(|b| b.id).chain(workspace.audio.mixes.iter().map(|m| m.id)) {
            let cell = meters().get(id).unwrap_or_else(|| meters().insert(id));
            cells.push((crate::audio::node(id), cell));
        }
        let master = meters().get(lightair_core::MASTER_ID).unwrap_or_else(|| meters().insert(lightair_core::MASTER_ID));
        cells.push((lightair_audio::PROGRAM, master));
        host.set_meter_cells(cells);
    }

    /// Starts recording multitrack files into `folder`, named "<base> - <strip>.wav": the
    /// tracks and buses marked Record (every track when none is), and the program mix when
    /// `program` (audio-only recordings). Returns the files.
    /// Sets the audio engine's block size (frames): smaller is lower latency, larger a lighter
    /// load. Restarts the engine if it's running (not while multitrack files record: false).
    pub fn set_audio_block(&mut self, frames: usize, workspace: &Workspace) -> bool {
        if self.audio_block == Some(frames) {
            return true;
        }
        if crate::audio_host::STEMS_RUNNING.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        self.audio_block = Some(frames);
        if self.audio.as_ref().is_some_and(|a| a.block() != frames) {
            self.audio = None;
            self.sync_audio(workspace);
        }
        true
    }

    /// The user answered macOS's camera or microphone prompt ([`crate::access::changed`]): the
    /// sources held back are made, in every scene that shows them, and the mixer opens its
    /// devices.
    pub fn access_answered(&mut self, workspace: &Workspace) {
        let waiting: Vec<Id> = self.awaiting_access.drain().collect();
        for id in waiting {
            if let Some(source) = workspace.sources.get(&id) {
                self.recreate_source(source, workspace);
            }
        }
        self.sync_audio(workspace);
    }

    /// Plays the mixer again (a plugin insert finished loading: [`crate::plugins::changed`]).
    pub fn refresh_audio(&mut self, workspace: &Workspace) {
        self.sync_audio(workspace);
    }

    pub fn start_multitrack(
        &mut self,
        workspace: &Workspace,
        folder: &std::path::Path,
        base: &str,
        program: bool,
    ) -> Result<Vec<(std::path::PathBuf, String)>, String> {
        let settings = &workspace.outputs.recording;
        let host = self.audio.as_mut().ok_or("the audio engine isn't running")?;
        let tap = match settings.multitrack_point {
            lightair_core::MultitrackPoint::Raw => lightair_audio::Tap::Input,
            lightair_core::MultitrackPoint::PreFader => lightair_audio::Tap::PreFader,
            lightair_core::MultitrackPoint::PostFader => lightair_audio::Tap::PostFader,
        };
        let float = settings.multitrack_format == lightair_core::MultitrackFormat::Float32;
        let marked = workspace.tracks.iter().any(|t| t.record) || workspace.audio.buses.iter().any(|b| b.record);
        let mut strips: Vec<(lightair_audio::NodeId, String, u16)> = workspace
            .tracks
            .iter()
            .filter(|t| t.record || !marked)
            .map(|t| {
                // a mono channel records as mono (before the pan it's the same on both sides)
                let ch = if matches!(t.channels, lightair_core::TrackChannels::Mono(_)) && tap != lightair_audio::Tap::PostFader {
                    1
                } else {
                    2
                };
                (crate::audio::node(t.id), t.name.clone(), ch)
            })
            .collect();
        strips.extend(workspace.audio.buses.iter().filter(|b| b.record).map(|b| (crate::audio::node(b.id), b.name.clone(), 2)));
        if program {
            strips.push((lightair_audio::PROGRAM, "Program".into(), 2));
        }
        if strips.is_empty() {
            return Err("no tracks to record".into());
        }
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let clean = lightair_core::file_safe;
        let mut used = HashSet::new();
        let mut files = Vec::new();
        let specs = strips
            .into_iter()
            .map(|(id, name, channels)| {
                let mut file = format!("{base} - {}.wav", clean(&name));
                let mut n = 2;
                while !used.insert(file.clone()) {
                    file = format!("{base} - {} {n}.wav", clean(&name));
                    n += 1;
                }
                // the file, and whose sound it is (its name as it was, not read back from the file's)
                files.push((folder.join(&file), name));
                crate::audio_host::TrackFileSpec { id, path: folder.join(file), channels, tap, float }
            })
            .collect::<Vec<_>>();
        host.start_multitrack(specs)?;
        Ok(files)
    }

    /// Monitoring straight from the audio engine to an output device (`None`: libobs's own,
    /// slower monitoring of the cue). `program`: hear the stream mix rather than the cue.
    pub fn set_headphones(&mut self, device: Option<String>, program: bool) {
        self.monitor = device.map(|d| (if d == "default" { String::new() } else { d }, program));
        if let Some(host) = self.audio.as_mut() {
            let m = self.monitor.clone().map(|(d, p)| (d, if p { lightair_audio::PROGRAM } else { crate::audio::CUE }));
            if let Err(e) = host.set_headphones(m) {
                tracing::warn!(%e, "direct monitoring unavailable; libobs monitors the cue");
            }
        }
    }

    /// A monitor the caller reads (tests): stereo, interleaved.
    pub fn audio_test_monitor(&mut self, program: bool) -> Option<rtrb::Consumer<f32>> {
        let src = if program { lightair_audio::PROGRAM } else { crate::audio::CUE };
        Some(self.audio.as_mut()?.test_monitor(src))
    }

    /// A source the caller feeds (tests): `channels` interleaved frames, like an interface.
    pub fn audio_test_input(&mut self, source: Id, channels: u16) -> Option<rtrb::Producer<f32>> {
        Some(self.audio.as_mut()?.test_input(source, channels))
    }

    /// Stops the multitrack files; what they hold.
    pub fn stop_multitrack(&mut self) -> Option<crate::audio_host::MultitrackReport> {
        self.audio.as_mut()?.stop_multitrack()
    }

    pub fn pause_multitrack(&mut self, pause: bool) {
        if let Some(h) = self.audio.as_mut() {
            h.pause_multitrack(pause);
        }
    }

    /// A track's, bus's, mix's or the program's meter from the audio engine.
    pub fn audio_meter(&mut self, id: Id) -> Option<Arc<lightair_audio::meter::Meter>> {
        Some(self.audio.as_mut()?.meter(crate::audio::node(id)))
    }

    /// The program's (master's) meter.
    pub fn program_meter(&mut self) -> Option<Arc<lightair_audio::meter::Meter>> {
        Some(self.audio.as_mut()?.meter(lightair_audio::PROGRAM))
    }

    // ------------------------------------------------------------ canvases
    fn add_canvas(&mut self, workspace: &Workspace, canvas: &Canvas) {
        if self.canvases.contains(&canvas.id) {
            return;
        }
        if Some(canvas.id) != self.primary_canvas {
            match ObsCanvas::create(&canvas.id.to_string(), canvas.width, canvas.height, canvas.fps_num, canvas.fps_den) {
                Some(obs_canvas) => {
                    self.obs_canvases.insert(canvas.id, obs_canvas);
                }
                None => {
                    tracing::error!(canvas = canvas.name, "could not create a canvas");
                    return;
                }
            }
        }
        self.canvases.push(canvas.id);
        if let Some(tr) = ObsSource::create_private("fade_transition", &format!("transition:{}", canvas.id), None) {
            match self.obs_canvases.get(&canvas.id) {
                Some(obs_canvas) => unsafe { sys::obs_canvas_set_channel(obs_canvas.as_ptr(), 0, tr.as_ptr()) },
                None => unsafe { sys::obs_set_output_source(0, tr.as_ptr()) },
            }
            self.transitions.insert(canvas.id, tr);
        }
        // Scenes that already exist get their version in the new canvas; scenes added later in
        // the same patch list arrive through SceneAdded.
        for scene in &workspace.scenes {
            if self.scenes.keys().any(|(s, _)| *s == scene.id) {
                self.add_scene_in(&scene.flatten(), canvas.id);
            }
        }
        if let Some(program) = self.program {
            self.land(Some(program));
        }
    }

    fn resize_canvas(&mut self, canvas: &Canvas) {
        if Some(canvas.id) == self.primary_canvas {
            // The main video can be reset while nothing records or streams from it.
            unsafe {
                let mut ovi = std::mem::zeroed::<sys::obs_video_info>();
                if !sys::obs_get_video_info(&mut ovi) {
                    return;
                }
                // Not read once graphics are up, but must point at something.
                ovi.graphics_module = if cfg!(windows) { c"libobs-d3d11".as_ptr() } else { c"libobs-opengl".as_ptr() };
                ovi.base_width = canvas.width;
                ovi.base_height = canvas.height;
                ovi.output_width = canvas.width;
                ovi.output_height = canvas.height;
                ovi.fps_num = canvas.fps_num;
                ovi.fps_den = canvas.fps_den.max(1);
                let code = sys::obs_reset_video(&mut ovi);
                if code != 0 {
                    tracing::warn!(code, "couldn't resize the main canvas (is something recording or live?)");
                }
            }
        } else if let Some(obs_canvas) = self.obs_canvases.get(&canvas.id) {
            obs_canvas.reset(canvas.width, canvas.height, canvas.fps_num, canvas.fps_den);
        }
    }

    fn remove_canvas(&mut self, id: Id) {
        self.finish_move();
        self.transitions.remove(&id);
        self.items.retain(|(_, f, _), _| *f != id);
        self.masks.retain(|(_, f, _), _| *f != id);
        self.scenes.retain(|(_, f), _| *f != id);
        self.obs_canvases.remove(&id);
        self.canvases.retain(|f| *f != id);
    }

    /// Makes a source's libobs source (as this platform plays its kind) and its effects.
    fn create_source(&mut self, source: &lightair_core::Source) {
        let timer = source.kind == TIMER_KIND;
        let resolved = crate::kinds::resolve(&source.kind, &source.settings);
        let kind = match &resolved {
            _ if timer => TIMER_SOURCE,
            Some((id, _)) => id,
            None => source.kind.as_str(),
        };
        // opening a camera or microphone macOS hasn't asked about yet would wait here, on the
        // prompt: it's asked without waiting, and the source made once answered
        if let Some(media) = crate::access::needed_by(kind)
            && crate::access::hold_back(media)
        {
            self.awaiting_access.insert(source.id);
            return;
        }
        let settings = data_from_json(&source_settings(source, false));
        match ObsSource::create(kind, &source.id.to_string(), Some(&settings)) {
            Some(src) => {
                if kind == "ffmpeg_source" && source.settings.get("restart_on_activate").and_then(serde_json::Value::as_bool) == Some(false) {
                    unsafe { pause_off_air(src.as_ptr()) };
                }
                if timer {
                    self.timers.add(source.id, src.as_ptr(), &source.settings);
                    self.ensure_timers_tick();
                }
                let chain = unsafe { chain_build(src.as_ptr(), &lower(&source.effects)) };
                self.source_effects.insert(source.id, chain);
                self.sources.insert(source.id, src);
            }
            None => tracing::warn!(kind, name = source.name, "could not create input"),
        }
    }

    fn destroy_source(&mut self, id: Id) {
        self.awaiting_access.remove(&id);
        self.timers.remove(id);
        if self.held.remove(&id)
            && let Some(src) = self.sources.get(&id)
        {
            unsafe { sys::obs_source_dec_showing(src.as_ptr()) };
        }
        if let (Some(src), Some(mut chain)) = (self.sources.get(&id), self.source_effects.remove(&id)) {
            unsafe { chain_remove(src.as_ptr(), &mut chain) };
        }
        if let (Some(src), Some((filter, _))) = (self.sources.get(&id), self.screen_zoom_pointers.remove(&id)) {
            unsafe { sys::obs_source_filter_remove(src.as_ptr(), filter.as_ptr()) };
        }
        self.sources.remove(&id);
    }

    /// A new source for a source whose libobs type changed, put back in every scene that shows
    /// it (the audio engine takes it again when audio syncs).
    fn recreate_source(&mut self, source: &lightair_core::Source, workspace: &Workspace) {
        let id = source.id;
        let scenes: Vec<lightair_core::Scene> = workspace.scenes.iter().map(|s| s.flatten()).collect();
        let using: Vec<(Id, Id)> = scenes
            .iter()
            .flat_map(|s| s.objects.iter().filter(|l| l.source == Some(id)).map(move |l| (s.id, l.id)))
            .collect();
        for (scene, object) in &using {
            for canvas in self.canvases.clone() {
                self.masks.remove(&(*scene, canvas, *object));
                if let Some(item) = self.items.remove(&(*scene, canvas, *object)) {
                    unsafe { sys::obs_sceneitem_remove(item.0) };
                }
            }
        }
        if let Some(host) = self.audio.as_mut() {
            host.release(id);
        }
        let held = self.held.contains(&id);
        self.destroy_source(id);
        self.create_source(source);
        if held && let Some(src) = self.sources.get(&id) {
            unsafe { sys::obs_source_inc_showing(src.as_ptr()) };
            self.held.insert(id);
        }
        for scene in &scenes {
            for (index, object) in scene.objects.iter().enumerate().filter(|(_, l)| l.source == Some(id)) {
                for canvas in self.canvases.clone() {
                    if self.scenes.contains_key(&(scene.id, canvas)) {
                        self.add_object(scene.id, canvas, object, index);
                    }
                }
            }
        }
    }

    fn add_scene_in(&mut self, scene: &lightair_core::Scene, canvas: Id) {
        if self.scenes.contains_key(&(scene.id, canvas)) {
            return;
        }
        let name = format!("{}:{}", scene.id, canvas);
        let obs_scene = match self.obs_canvases.get(&canvas) {
            Some(obs_canvas) => obs_canvas.create_scene(&name),
            None => ObsScene::create(&name),
        };
        let Some(obs_scene) = obs_scene else { return };
        self.scenes.insert((scene.id, canvas), obs_scene);
        for (index, object) in scene.objects.iter().enumerate() {
            self.add_object(scene.id, canvas, object, index);
        }
    }

    /// The canvas behind a canvas, as a new strong reference (for previews).
    /// The libobs source of a source (borrowed; valid while the source exists).
    pub(crate) fn source_ptr(&self, source: Id) -> Option<*mut sys::obs_source_t> {
        self.sources.get(&source).map(|s| s.as_ptr())
    }

    /// Whether libobs is running a source (on air, or a sound-only source on a track).
    pub fn source_active(&self, source: Id) -> bool {
        self.source_ptr(source).is_some_and(|s| unsafe { sys::obs_source_active(s) })
    }

    /// Shows a object whole while it's being cropped (crop mode), or `None` to end it. The
    /// document is untouched: this is only how libobs draws it meanwhile.
    pub fn set_crop_preview(&mut self, target: Option<(Id, Id, Id)>, workspace: &Workspace) {
        // The same target again while fading out: back in (the ghost fades in again).
        if self.crop_preview == target && !self.crop_fading {
            return;
        }
        let previous = std::mem::replace(&mut self.crop_preview, target);
        self.crop_fading = false;
        for (scene, canvas, object) in previous.into_iter().chain(target) {
            let Some(s) = workspace.scene(scene).map(|s| s.flatten()) else { continue };
            let Some(index) = s.objects.iter().position(|l| l.id == object) else { continue };
            let l = s.objects[index].clone();
            self.update_object(scene, canvas, &l, index);
        }
    }

    /// Fades the crop view's ghost out (before `set_crop_preview(None)`, so leaving crop view
    /// doesn't pop): the picture then looks just as it will once the crop is applied again.
    pub fn fade_crop_preview(&mut self, workspace: &Workspace) {
        let Some((scene, canvas, object)) = self.crop_preview else { return };
        self.crop_fading = true;
        let Some(s) = workspace.scene(scene).map(|s| s.flatten()) else { return };
        let Some(index) = s.objects.iter().position(|l| l.id == object) else { return };
        let l = s.objects[index].clone();
        self.update_object(scene, canvas, &l, index);
    }

    /// Whether the source's face effects see a face right now (`None`: it has none running).
    pub fn face_present(&self, source: Id) -> Option<bool> {
        self.studio_query(source, c"face_present")
    }

    /// Turns one of a source's effect filters on or off in libobs only, leaving the document
    /// alone (the eyedropper shows the picture without the effect being picked for). `filter` is
    /// the lowered chain's id: the effect's own, or `catalog::STUDIO_ID` for face effects.
    pub fn set_filter_live(&self, source: Id, filter: Id, enabled: bool) {
        if let Some(chain) = self.source_effects.get(&source)
            && let Some((_, f)) = chain.iter().find(|(id, _)| *id == filter)
        {
            unsafe { sys::obs_source_set_enabled(f.as_ptr(), enabled) };
        }
    }

    /// Whether the source's Lightair Effect Stack sees a person (for the background effects' hint);
    /// `None` when it has no Lightair Effect Stack running.
    pub fn person_present(&self, source: Id) -> Option<bool> {
        self.studio_query(source, c"person_present")
    }

    /// Calls a `void name(out bool present)` proc on the source's enabled Lightair Effect Stack filter.
    fn studio_query(&self, source: Id, name: &std::ffi::CStr) -> Option<bool> {
        let chain = self.source_effects.get(&source)?;
        chain.iter().find_map(|(_, filter)| unsafe {
            let id = std::ffi::CStr::from_ptr(sys::obs_source_get_id(filter.as_ptr()));
            if id.to_bytes() != b"lightair_camera_studio" || !sys::obs_source_enabled(filter.as_ptr()) {
                return None;
            }
            let ph = sys::obs_source_get_proc_handler(filter.as_ptr());
            let mut cd = sys::calldata_t::default();
            let mut present = false;
            if sys::proc_handler_call(ph, name.as_ptr(), &mut cd) {
                // calldata_get_bool is a header inline over calldata_get_data.
                sys::calldata_get_data(
                    &cd,
                    c"present".as_ptr(),
                    &mut present as *mut bool as *mut std::ffi::c_void,
                    std::mem::size_of::<bool>(),
                );
            }
            if !cd.fixed && !cd.stack.is_null() {
                sys::bfree(cd.stack as *mut std::ffi::c_void);
            }
            Some(present)
        })
    }

    /// Starts sources as if they were on screen (captures open, media loads) without putting
    /// them on air, until `release_sources`.
    pub fn hold_sources(&mut self, sources: &[Id]) {
        for id in sources {
            if let Some(src) = self.sources.get(id)
                && self.held.insert(*id)
            {
                unsafe { sys::obs_source_inc_showing(src.as_ptr()) };
            }
        }
    }

    pub fn release_sources(&mut self) {
        for id in std::mem::take(&mut self.held) {
            if let Some(src) = self.sources.get(&id) {
                unsafe { sys::obs_source_dec_showing(src.as_ptr()) };
            }
        }
    }

    /// The choices a source offers for a list setting (a camera's devices, a display capture's
    /// displays or windows, a microphone's devices): (label, value). Read from the source itself,
    /// with its current settings applied, since some sources fill their lists only then.
    pub fn source_options(&self, source: Id, property: &str) -> Vec<(String, serde_json::Value)> {
        self.sources.get(&source).map(|src| crate::obs::source_list_items(src.as_ptr(), property)).unwrap_or_default()
    }

    /// Every source's pixel size that's known (for the editor's frames and handles).
    pub fn source_sizes(&self) -> HashMap<Id, (u32, u32)> {
        self.sources.keys().filter_map(|id| self.source_size(*id).map(|s| (*id, s))).collect()
    }

    /// A source's pixel size, once it has one (async sources report it after their first frame).
    pub fn source_size(&self, source: Id) -> Option<(u32, u32)> {
        let src = self.sources.get(&source)?;
        let (w, h) = unsafe { (sys::obs_source_get_width(src.as_ptr()), sys::obs_source_get_height(src.as_ptr())) };
        (w > 0 && h > 0).then_some((w, h))
    }

    /// The libobs scene source for a document scene in a canvas, if it exists.
    pub fn scene_source(&self, scene: Id, canvas: Id) -> Option<*mut sys::obs_source_t> {
        self.scenes.get(&(scene, canvas)).map(|s| s.source_ptr())
    }

    /// A new strong reference to that scene source (the caller releases it), e.g. for Studio
    /// mode's preview of a scene that isn't on air.
    pub fn scene_source_ref(&self, scene: Id, canvas: Id) -> Option<*mut sys::obs_source_t> {
        self.scene_source(scene, canvas).map(|s| unsafe { sys::obs_source_get_ref(s) }).filter(|p| !p.is_null())
    }

    pub fn canvas_ref(&self, canvas: Id) -> Option<*mut sys::obs_canvas_t> {
        if Some(canvas) == self.primary_canvas {
            return Some(unsafe { sys::obs_get_main_canvas() });
        }
        self.obs_canvases.get(&canvas).map(|c| unsafe { sys::obs_canvas_get_ref(c.as_ptr()) })
    }

    /// The canvas's video output, for encoders recording or streaming a canvas.
    pub fn canvas_video(&self, canvas: Id) -> Option<crate::outputs::VideoOutput> {
        if Some(canvas) == self.primary_canvas {
            return Some(crate::outputs::VideoOutput(unsafe { sys::obs_get_video() }));
        }
        self.obs_canvases
            .get(&canvas)
            .map(|c| crate::outputs::VideoOutput(unsafe { sys::obs_canvas_get_video(c.as_ptr()) }))
    }

    fn dest(&self, program: Option<Id>, canvas: Id) -> *mut sys::obs_source_t {
        program.and_then(|id| self.scenes.get(&(id, canvas))).map_or(std::ptr::null_mut(), |s| s.source_ptr())
    }

    /// Puts `program` on air in every canvas instantly.
    fn land(&mut self, program: Option<Id>) {
        for canvas in self.canvases.clone() {
            if let Some(tr) = self.transitions.get(&canvas) {
                unsafe { crate::transition::set_now(tr.as_ptr(), self.dest(program, canvas)) };
            }
        }
    }

    fn set_program(&mut self, program: Option<Id>, workspace: &Workspace) {
        let previous = self.program;
        self.finish_move();
        // the incoming scene's own transition (and stinger video), else the workspace's
        let incoming = program.and_then(|b| workspace.scene(b));
        let settings = incoming.and_then(|s| s.transition).unwrap_or(self.transition_settings);
        let stinger = incoming.filter(|s| s.transition.is_some()).and_then(|s| s.stinger.clone()).or_else(|| self.stinger.clone());
        let kind = match (previous, program) {
            (Some(a), Some(b)) if a != b => settings.kind,
            _ => TransitionKind::Cut,
        };
        let ms = settings.duration_ms;
        match kind {
            TransitionKind::Cut => self.land(program),
            TransitionKind::Fade
            | TransitionKind::Slide
            | TransitionKind::Wipe
            | TransitionKind::Dip
            | TransitionKind::Stinger => {
                let (type_id, source_settings) = transition_source(&settings, stinger.as_ref());
                for canvas in self.canvases.clone() {
                    self.ensure_transition(canvas, type_id, &source_settings);
                    if let Some(tr) = self.transitions.get(&canvas) {
                        unsafe {
                            sys::obs_transition_start(
                                tr.as_ptr(),
                                sys::OBS_TRANSITION_MODE_AUTO,
                                ms,
                                self.dest(program, canvas),
                            )
                        };
                    }
                }
            }
            TransitionKind::Move => {
                let (Some(a), Some(b)) = (previous, program) else { unreachable!() };
                if !self.start_move(a, b, ms, workspace) {
                    self.land(program);
                }
            }
        }
        self.program = program;
    }

    fn prepare_transitions(&mut self) {
        if matches!(self.transition_settings.kind, TransitionKind::Cut | TransitionKind::Move) {
            return;
        }
        let (type_id, settings) = transition_source(&self.transition_settings, self.stinger.as_ref());
        for canvas in self.canvases.clone() {
            self.ensure_transition(canvas, type_id, &settings);
        }
    }

    /// Makes `canvas`'s output transition a `type_id` source with `settings`, swapping the
    /// source on the output channel when the type changes (libobs carries over what's showing).
    fn ensure_transition(&mut self, canvas: Id, type_id: &str, settings: &serde_json::Value) {
        let Some(current) = self.transitions.get(&canvas) else { return };
        let current_type = unsafe { std::ffi::CStr::from_ptr(sys::obs_source_get_id(current.as_ptr())) };
        let data = data_from_json(settings);
        if current_type.to_bytes() == type_id.as_bytes() {
            current.update(&data);
            return;
        }
        let Some(next) = ObsSource::create_private(type_id, &format!("transition:{canvas}"), Some(&data)) else {
            tracing::warn!(type_id, "transition type unavailable");
            return;
        };
        unsafe {
            sys::obs_transition_swap_begin(next.as_ptr(), current.as_ptr());
            match self.obs_canvases.get(&canvas) {
                Some(obs_canvas) => sys::obs_canvas_set_channel(obs_canvas.as_ptr(), 0, next.as_ptr()),
                None => sys::obs_set_output_source(0, next.as_ptr()),
            }
            sys::obs_transition_swap_end(next.as_ptr(), current.as_ptr());
        }
        self.transitions.insert(canvas, next);
    }

    /// Builds the temporary scenes for a Move from `a` to `b` and starts the animation.
    fn start_move(&mut self, a: Id, b: Id, ms: u32, workspace: &Workspace) -> bool {
        let (Some(scene_a), Some(scene_b)) =
            (workspace.scene(a).map(|s| s.flatten()), workspace.scene(b).map(|s| s.flatten()))
        else {
            return false;
        };
        let (scene_a, scene_b) = (&scene_a, &scene_b);
        // (ms: the transition's, the incoming scene's own or the workspace's)
        let mut canvases = Vec::new();
        for canvas in self.canvases.clone() {
            let (Some(tr), dest) = (self.transitions.get(&canvas), self.dest(Some(b), canvas)) else { continue };
            let name = format!("move:{canvas}:{}", Id::new());
            let temp = match self.obs_canvases.get(&canvas) {
                Some(obs_canvas) => obs_canvas.create_scene(&name),
                None => ObsScene::create(&name),
            };
            let Some(temp) = temp else { continue };
            // Active while it's built: libobs counts a source shown through an item's fade one way
            // when the scene activates mid-fade and another when the fade ends, and a video caught
            // by that stopped on landing. Built active, the counts match.
            unsafe { sys::obs_source_inc_active(temp.source_ptr()) };
            let mut tweens = Vec::new();
            let mut fades = Vec::new();
            let mut masks = Vec::new();
            // Which of A's objects continue into B.
            let mut used: Vec<Id> = Vec::new();
            let matches: Vec<Option<Object>> = scene_b
                .objects
                .iter()
                .map(|l| {
                    let m = match_object(scene_a, l).filter(|m| !used.contains(&m.id)).cloned();
                    if let Some(m) = &m {
                        used.push(m.id);
                    }
                    m
                })
                .collect();
            // Objects leaving: at the bottom, fading out.
            for l in scene_a.objects.iter().filter(|l| !used.contains(&l.id)) {
                let (Some(lo), Some(source)) = (l.layouts.get(&canvas), l.source.and_then(|i| self.sources.get(&i)))
                else {
                    continue;
                };
                if !lo.visible {
                    continue;
                }
                let Some((item, masked)) =
                    (unsafe { add_item(temp.as_ptr(), source, Some(lo), &format!("{}", l.id), false) })
                else {
                    continue;
                };
                unsafe { layout_item(item, masked.as_ref(), lo) };
                masks.extend(masked);
                item_fade(item, false, ms, &mut fades);
                unsafe {
                    sys::obs_sceneitem_set_visible(item, false);
                }
            }
            // B's objects: gliding from A's layout when they continue, fading in otherwise.
            for (l, m) in scene_b.objects.iter().zip(&matches) {
                let (Some(to), Some(source)) = (l.layouts.get(&canvas), l.source.and_then(|i| self.sources.get(&i)))
                else {
                    continue;
                };
                if !to.visible {
                    continue;
                }
                // The shape (if any) is B's: a object gliding into a circle is a circle on the way.
                let Some((item, masked)) =
                    (unsafe { add_item(temp.as_ptr(), source, Some(to), &format!("{}", l.id), false) })
                else {
                    continue;
                };
                match m.as_ref().and_then(|m| m.layouts.get(&canvas)).filter(|f| f.visible) {
                    Some(from) => {
                        // B's shape and flips throughout; tilt glides.
                        let from = Layout { shape: to.shape, flip_h: to.flip_h, flip_v: to.flip_v, ..*from };
                        unsafe { layout_item(item, masked.as_ref(), &from) };
                        tweens.push(Tween { item, masked, from, to: *to });
                    }
                    None => unsafe {
                        layout_item(item, masked.as_ref(), to);
                        masks.extend(masked);
                        sys::obs_sceneitem_set_visible(item, false);
                        item_fade(item, true, ms, &mut fades);
                        sys::obs_sceneitem_set_visible(item, true);
                    },
                }
            }
            unsafe {
                crate::transition::set_now(tr.as_ptr(), temp.source_ptr());
                sys::obs_source_dec_active(temp.source_ptr());
            }
            canvases.push(CanvasMove { temp, tweens, transition: tr.as_ptr(), dest, fades, masks });
        }
        if canvases.is_empty() {
            return false;
        }
        self.ensure_tick();
        *self.anim.lock() =
            Some(MoveAnim { canvases, start: std::time::Instant::now(), duration: ms as f32 / 1000.0, done: false });
        true
    }

    fn ensure_timers_tick(&mut self) {
        if !self.timers_tick {
            unsafe { sys::obs_add_tick_callback(Some(on_timers_tick), tick_param(&self.timers)) };
            self.timers_tick = true;
        }
    }

    pub fn timer_action(&self, source: Id, action: TimerAction) {
        self.timers.action(source, action);
    }

    pub fn timer_status(&self, source: Id) -> Option<TimerStatus> {
        self.timers.status(source)
    }

    /// The text a timer is showing.
    pub fn timer_text(&self, source: Id) -> Option<String> {
        self.timers.text(source)
    }

    /// Countdowns that ended since the last call (their end actions are the app's to run).
    pub fn take_ended_timers(&self) -> Vec<Id> {
        self.timers.take_ended()
    }

    fn ensure_tick(&mut self) {
        if !self.tick_registered {
            unsafe { sys::obs_add_tick_callback(Some(on_tick), Arc::as_ptr(&self.anim) as *mut _) };
            self.tick_registered = true;
        }
    }

    /// Lands a running Move on its destination immediately.
    fn finish_move(&mut self) {
        if let Some(anim) = self.anim.lock().as_mut() {
            anim.finish();
        }
        self.cleanup();
    }

    /// Releases a finished Move's temporary scenes (engine thread).
    pub fn cleanup(&mut self) {
        let mut guard = self.anim.lock();
        if guard.as_ref().is_some_and(|a| a.done) {
            guard.take();
        }
    }

    fn add_object(&mut self, scene: Id, canvas: Id, object: &Object, index: usize) {
        let (Some(obs_scene), Some(source)) =
            (self.scenes.get(&(scene, canvas)), object.source.and_then(|i| self.sources.get(&i)))
        else {
            tracing::warn!(%scene, object = %object.id, "layer references a missing scene or input");
            return;
        };
        let layout = object.layouts.get(&canvas);
        let key = (scene, canvas, object.id);
        let filtered = self.crop_preview == Some(key);
        let Some((item, masked)) =
            (unsafe { add_item(obs_scene.as_ptr(), source, layout, &format!("{}:{canvas}", object.id), filtered) })
        else {
            return;
        };
        unsafe { sys::obs_sceneitem_set_order_position(item, index as i32) };
        self.items.insert(key, ItemPtr(item));
        if let Some(m) = masked {
            self.masks.insert(key, m);
        }
        self.update_object(scene, canvas, object, index);
    }

    fn update_object(&mut self, scene: Id, canvas: Id, object: &Object, index: usize) {
        let key = (scene, canvas, object.id);
        let Some(item) = self.items.get(&key).copied() else { return };
        // Gaining or losing a shape changes the item's structure (so does crop view, which
        // needs the mask): rebuild it in place. The new item goes in before the old one comes
        // out, so no frame is drawn without the object.
        let previewing = self.crop_preview == Some(key);
        let shaped = previewing || object.layouts.get(&canvas).is_some_and(|l| l.needs_filters());
        if shaped != self.masks.contains_key(&key) {
            self.masks.remove(&key);
            self.items.remove(&key);
            self.add_object(scene, canvas, object, index);
            unsafe { sys::obs_sceneitem_remove(item.0) };
            return;
        }
        unsafe {
            sys::obs_sceneitem_set_locked(item.0, object.locked);
            let size = previewing.then(|| object.source.and_then(|i| self.source_size(i))).flatten();
            match object.layouts.get(&canvas) {
                // Being cropped: the whole picture, where it would sit uncropped, with the shape
                // in the crop window and the rest as a ghost that fades in.
                Some(l) if size.is_some() && self.masks.contains_key(&key) => {
                    let (w, h) = size.unwrap();
                    let masked = self.masks.get(&key);
                    layout_item(item.0, masked, &l.uncropped((w as f32, h as f32)));
                    if let Some(m) = masked {
                        let outside = if self.crop_fading { 0.0 } else { CROP_GHOST };
                        m.filter.update(&data_from_json(&crop_view_settings(l, outside)));
                        sys::obs_source_set_enabled(m.filter.as_ptr(), true);
                    }
                }
                Some(l) => layout_item(item.0, self.masks.get(&key), l),
                // No layout in this canvas: the object is hidden there.
                None => {
                    sys::obs_sceneitem_set_visible(item.0, false);
                }
            }
        }
    }

    fn reorder(&mut self, scene: Id, canvas: Id, order: &[Id]) {
        for (index, object) in order.iter().enumerate() {
            if let Some(item) = self.items.get(&(scene, canvas, *object)) {
                unsafe { sys::obs_sceneitem_set_order_position(item.0, index as i32) };
            }
        }
    }

    /// What libobs actually holds, read back from libobs (not from our maps): per scene and
    /// canvas, the items bottom-to-top with source, position, bounds, rotation and visibility.
    /// Used to check that incremental reconciliation and a full rebuild produce identical state.
    /// What a Studio take records: the screens and cameras in the program (visible in the
    /// primary canvas), each once, with its live source.
    pub fn studio_take_sources(&self, workspace: &Workspace) -> Vec<(lightair_core::Source, *mut sys::obs_source_t)> {
        let Some(scene) = workspace.program.and_then(|p| workspace.scene(p)) else { return Vec::new() };
        let canvas = workspace.primary_canvas().id;
        let mut out: Vec<(lightair_core::Source, *mut sys::obs_source_t)> = Vec::new();
        for object in scene.flatten().objects {
            let visible = object.layouts.get(&canvas).is_none_or(|l| l.visible);
            let Some(source) = object.source.and_then(|i| workspace.sources.get(&i)) else { continue };
            if !visible
                || !matches!(source.kind.as_str(), lightair_core::kinds::SCREEN | lightair_core::kinds::CAMERA)
                || out.iter().any(|(i, _)| i.id == source.id)
            {
                continue;
            }
            if let Some(src) = self.sources.get(&source.id) {
                out.push(((**source).clone(), src.as_ptr()));
            }
        }
        out
    }

    /// A source's live source (borrowed), for tests and the take.
    pub fn source_source(&self, source: Id) -> Option<*mut sys::obs_source_t> {
        self.source_ptr(source)
    }

    /// The libobs source type a source plays as (diagnostics and tests).
    pub fn source_type(&self, source: Id) -> Option<String> {
        let src = self.sources.get(&source)?;
        let id = unsafe { sys::obs_source_get_id(src.as_ptr()) };
        (!id.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(id) }.to_string_lossy().into_owned())
    }

    pub fn describe(&self) -> Vec<String> {
        unsafe extern "C" fn each(
            _: *mut sys::obs_scene_t,
            item: *mut sys::obs_sceneitem_t,
            param: *mut std::ffi::c_void,
        ) -> bool {
            let out = unsafe { &mut *(param as *mut Vec<String>) };
            unsafe {
                let src = sys::obs_sceneitem_get_source(item);
                let name = std::ffi::CStr::from_ptr(sys::obs_source_get_name(src)).to_string_lossy().into_owned();
                let mut pos = vec2(0.0, 0.0);
                let mut bounds = vec2(0.0, 0.0);
                sys::obs_sceneitem_get_pos(item, &mut pos);
                sys::obs_sceneitem_get_bounds(item, &mut bounds);
                let (p, b) = (pos.__bindgen_anon_1.__bindgen_anon_1, bounds.__bindgen_anon_1.__bindgen_anon_1);
                out.push(format!(
                    "  {name} pos=({:.1},{:.1}) bounds=({:.1},{:.1}) rot={:.1} visible={}",
                    p.x,
                    p.y,
                    b.x,
                    b.y,
                    sys::obs_sceneitem_get_rot(item),
                    sys::obs_sceneitem_visible(item)
                ));
            }
            true
        }
        let mut keys: Vec<&(Id, Id)> = self.scenes.keys().collect();
        keys.sort();
        let mut out = Vec::new();
        for key in keys {
            let (scene, canvas) = key;
            out.push(format!(
                "scene {scene} format {canvas}{}",
                if self.program == Some(*scene) { " (program)" } else { "" }
            ));
            unsafe { sys::obs_scene_enum_items(self.scenes[key].as_ptr(), Some(each), &mut out as *mut _ as *mut _) };
        }
        out
    }

    // ------------------------------------------------------------ audio tracks
    fn add_track(&mut self, track: &Track) {
        let Some(source) = self.sources.get(&track.source) else {
            tracing::warn!(track = track.name, "track references a missing input");
            return;
        };
        let used: std::collections::HashSet<u32> = self.tracks.values().map(|t| t.channel).filter(|c| *c != 0).collect();
        let Some(channel) = (1..crate::audio_host::OUT_CHANNELS.start).find(|c| !used.contains(c)) else {
            tracing::error!("no free output channel for track");
            return;
        };
        let src = source.as_ptr();
        let meter = meters().insert(track.id);
        unsafe {
            // As in OBS: a source with a picture (a video, a web page, a capture) runs while a
            // scene showing it is on air, and its sound with it; off air it stops (no decoding,
            // no painting). Sound-only sources (microphones, desktop and app audio) always run,
            // like OBS's global audio sources.
            let pictured = sys::obs_source_get_output_flags(src) & sys::OBS_SOURCE_VIDEO != 0;
            let channel = if pictured { 0 } else { channel };
            if channel != 0 {
                sys::obs_set_output_source(channel, src);
            }
            // with the engine, its post-fader meters fill the cell (sync_audio); otherwise libobs's
            let vm = if Self::engine_audio() {
                std::ptr::null_mut()
            } else {
                let vm = sys::obs_volmeter_create(sys::OBS_FADER_LOG);
                sys::obs_volmeter_attach_source(vm, src);
                sys::obs_volmeter_add_callback(vm, Some(on_levels), Arc::as_ptr(&meter) as *mut _);
                vm
            };
            let mut state =
                TrackState { source: track.source, channel, volmeter: VolmeterPtr(vm), meter, filters: vec![] };
            if Self::engine_audio() {
                // levels, mute and the engine's effects are the engine's; plugin filters stay
                state.filters = chain_build(src, &libobs_effects(&track.effects));
            } else {
                apply_track_levels(src, track);
                state.filters = chain_build(src, &track.effects);
            }
            self.tracks.insert(track.id, state);
        }
    }

    fn update_track(&mut self, before: &Track, after: &Track) {
        if before.source != after.source {
            self.remove_track(after.id);
            self.add_track(after);
            return;
        }
        let (Some(state), Some(source)) = (self.tracks.get_mut(&after.id), self.sources.get(&after.source)) else {
            return;
        };
        let src = source.as_ptr();
        if Self::engine_audio() {
            let (b, a) = (libobs_effects(&before.effects), libobs_effects(&after.effects));
            unsafe { chain_update(src, &mut state.filters, &b, &a) };
        } else {
            unsafe { apply_track_levels(src, after) };
            unsafe { chain_update(src, &mut state.filters, &before.effects, &after.effects) };
        }
    }

    fn remove_track(&mut self, id: Id) {
        let Some(mut state) = self.tracks.remove(&id) else { return };
        unsafe {
            if !state.volmeter.0.is_null() {
                sys::obs_volmeter_remove_callback(state.volmeter.0, Some(on_levels), Arc::as_ptr(&state.meter) as *mut _);
                sys::obs_volmeter_destroy(state.volmeter.0);
            }
            if state.channel != 0 {
                sys::obs_set_output_source(state.channel, std::ptr::null_mut());
            }
            if let Some(source) = self.sources.get(&state.source) {
                chain_remove(source.as_ptr(), &mut state.filters);
                sys::obs_source_set_monitoring_enabled(source.as_ptr(), false);
            }
        }
        meters().remove(id);
    }

    /// Items of what a canvas's canvas is showing right now (through its transition), as
    /// "name x y" lines with positions of the item centres. For tests and diagnostics.
    pub fn describe_on_air(&self, canvas: Id) -> Vec<String> {
        unsafe extern "C" fn each(
            _: *mut sys::obs_scene_t,
            item: *mut sys::obs_sceneitem_t,
            param: *mut std::ffi::c_void,
        ) -> bool {
            let out = unsafe { &mut *(param as *mut Vec<String>) };
            unsafe {
                let src = sys::obs_sceneitem_get_source(item);
                let name = std::ffi::CStr::from_ptr(sys::obs_source_get_name(src)).to_string_lossy().into_owned();
                let mut pos = vec2(0.0, 0.0);
                sys::obs_sceneitem_get_pos(item, &mut pos);
                let p = pos.__bindgen_anon_1.__bindgen_anon_1;
                out.push(format!("{name} {:.0} {:.0}", p.x, p.y));
            }
            true
        }
        let mut out = Vec::new();
        let Some(tr) = self.transitions.get(&canvas) else { return out };
        unsafe {
            let active = sys::obs_transition_get_active_source(tr.as_ptr());
            if active.is_null() {
                return out;
            }
            let scene = sys::obs_scene_from_source(active);
            if !scene.is_null() {
                sys::obs_scene_enum_items(scene, Some(each), &mut out as *mut _ as *mut _);
            }
            sys::obs_source_release(active);
        }
        out
    }

    /// Number of live libobs objects (tests and diagnostics).
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.sources.len(), self.scenes.len(), self.items.len())
    }

    /// Releases everything in dependency order (scenes before the sources they use).
    /// Everything from `workspace`, from scratch (another workspace opened): the main canvas takes
    /// its primary canvas's size.
    pub fn rebuild(&mut self, workspace: &Workspace) {
        self.clear();
        let patches = lightair_core::diff(&workspace.empty_like(), workspace);
        self.apply(workspace, &patches);
        let primary = workspace.primary_canvas();
        let mut ovi = unsafe { std::mem::zeroed::<sys::obs_video_info>() };
        let same = unsafe { sys::obs_get_video_info(&mut ovi) }
            && (ovi.base_width, ovi.base_height, ovi.fps_num, ovi.fps_den) == (primary.width, primary.height, primary.fps_num, primary.fps_den.max(1));
        if !same {
            self.resize_canvas(primary);
        }
    }

    pub fn clear(&mut self) {
        self.release_sources();
        if self.timers_tick {
            unsafe { sys::obs_remove_tick_callback(Some(on_timers_tick), tick_param(&self.timers)) };
            self.timers_tick = false;
        }
        self.timers.clear();
        self.finish_move();
        if self.tick_registered {
            unsafe { sys::obs_remove_tick_callback(Some(on_tick), Arc::as_ptr(&self.anim) as *mut _) };
            self.tick_registered = false;
        }
        let ids: Vec<Id> = self.tracks.keys().copied().collect();
        for id in ids {
            self.remove_track(id);
        }
        self.audio = None; // stops the audio thread, releases the captures
        unsafe { sys::obs_set_output_source(0, std::ptr::null_mut()) };
        for obs_canvas in self.obs_canvases.values() {
            unsafe { sys::obs_canvas_set_channel(obs_canvas.as_ptr(), 0, std::ptr::null_mut()) };
        }
        self.transitions.clear();
        for (id, mut chain) in self.source_effects.drain() {
            if let Some(src) = self.sources.get(&id) {
                unsafe { chain_remove(src.as_ptr(), &mut chain) };
            }
        }
        self.masks.clear();
        self.items.clear();
        self.scenes.clear();
        self.obs_canvases.clear();
        self.canvases.clear();
        self.sources.clear();
        self.program = None;
        // libobs destroys sources on its own thread. Wait until it has, so whatever is built
        // next gets the same names (a name still in use gets " 2" added). The render thread
        // can hold a scene one frame longer, so wait for a few quiet rounds.
        let mut quiet = 0;
        while quiet < 2 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if unsafe { sys::obs_wait_for_destroy_queue() } {
                quiet = 0;
            } else {
                quiet += 1;
            }
        }
    }
}

pub(crate) unsafe fn apply_layout(item: *mut sys::obs_sceneitem_t, l: &Layout) {
    unsafe {
        // Centre alignment: position is the box centre, and rotation turns around it.
        sys::obs_sceneitem_set_alignment(item, sys::OBS_ALIGN_CENTER);
        sys::obs_sceneitem_set_bounds_alignment(item, sys::OBS_ALIGN_CENTER);
        let (bounds, crop_to_bounds) = match l.fit {
            Fit::Fit => (sys::OBS_BOUNDS_SCALE_INNER, false),
            Fit::Fill => (sys::OBS_BOUNDS_SCALE_OUTER, true),
            Fit::Stretch => (sys::OBS_BOUNDS_STRETCH, false),
            Fit::Original => (sys::OBS_BOUNDS_NONE, false),
        };
        sys::obs_sceneitem_set_bounds_type(item, bounds);
        sys::obs_sceneitem_set_bounds_crop(item, crop_to_bounds);
        sys::obs_sceneitem_set_bounds(item, &vec2(l.width, l.height));
        if l.fit == Fit::Original {
            sys::obs_sceneitem_set_scale(item, &vec2(1.0, 1.0));
        }
        sys::obs_sceneitem_set_pos(item, &vec2(l.x + l.width / 2.0, l.y + l.height / 2.0));
        sys::obs_sceneitem_set_rot(item, l.rotation);
        let crop = sys::obs_sceneitem_crop {
            left: l.crop.left as i32,
            top: l.crop.top as i32,
            right: l.crop.right as i32,
            bottom: l.crop.bottom as i32,
        };
        sys::obs_sceneitem_set_crop(item, &crop);
        sys::obs_sceneitem_set_visible(item, l.visible);
    }
}

/// Shape-mask filter settings (Lightair's libobs module, `lightair_shape_mask`).
fn mask_settings(s: &lightair_core::Shape) -> serde_json::Value {
    let kind = match s.kind {
        lightair_core::ShapeKind::Rounded => 0,
        lightair_core::ShapeKind::Circle => 1,
        lightair_core::ShapeKind::Pill => 2,
    };
    serde_json::json!({
        "shape": kind,
        "radius": s.radius,
        "feather": s.feather,
        "border": s.border,
        "border_color": s.border_color,
        "window_left": 0.0,
        "window_top": 0.0,
        "window_right": 0.0,
        "window_bottom": 0.0,
        "outside": 0.0,
    })
}

/// How much of the cut-away picture shows around the crop while cropping.
const CROP_GHOST: f64 = 0.42;

/// The mask while a object is being cropped: the whole picture is laid out, the shape (or a plain
/// box) sits in the crop window, and what's outside it fades in as a ghost.
fn crop_view_settings(l: &Layout, outside: f64) -> serde_json::Value {
    let shape = l.shape.unwrap_or(lightair_core::Shape {
        radius: 0.0,
        feather: 0.0,
        border: 0.0,
        ..lightair_core::Shape::new(lightair_core::ShapeKind::Rounded)
    });
    let mut v = mask_settings(&shape);
    v["window_left"] = l.crop.left.into();
    v["window_top"] = l.crop.top.into();
    v["window_right"] = l.crop.right.into();
    v["window_bottom"] = l.crop.bottom.into();
    v["outside"] = outside.into();
    v
}

/// Flip/3D filter settings (Lightair's libobs module, `lightair_transform`).
fn transform_settings(l: &Layout) -> serde_json::Value {
    serde_json::json!({"flip_h": l.flip_h, "flip_v": l.flip_v, "tilt_x": l.tilt_x, "tilt_y": l.tilt_y})
}

/// Adds `source` to `scene` as a object's item. A shaped object becomes a one-item group: the source
/// sits inside (carrying the crop) and the shape mask is on the group, so the shape is cut from
/// the cropped picture and belongs to this object alone.
pub(crate) unsafe fn add_item(
    scene: *mut sys::obs_scene_t,
    source: &ObsSource,
    layout: Option<&Layout>,
    name: &str,
    always_filtered: bool,
) -> Option<(*mut sys::obs_sceneitem_t, Option<Masked>)> {
    unsafe {
        let Some(layout) = layout.filter(|l| always_filtered || l.needs_filters()) else {
            let item = sys::obs_scene_add(scene, source.as_ptr());
            return (!item.is_null()).then_some((item, None));
        };
        let group_name = std::ffi::CString::new(format!("shape {name} {}", Id::new())).unwrap_or_default();
        let group = sys::obs_scene_add_group2(scene, group_name.as_ptr(), false);
        if group.is_null() {
            return None;
        }
        let inner = sys::obs_scene_add(sys::obs_sceneitem_group_get_scene(group), source.as_ptr());
        let shape = layout.shape.unwrap_or(lightair_core::Shape::new(lightair_core::ShapeKind::Rounded));
        let filter = ObsSource::create_private(
            "lightair_shape_mask",
            &format!("mask {name}"),
            Some(&data_from_json(&mask_settings(&shape))),
        );
        let Some(filter) = filter else {
            tracing::warn!("shape mask unavailable (is the Lightair module loaded?)");
            sys::obs_sceneitem_remove(group);
            let item = sys::obs_scene_add(scene, source.as_ptr());
            return (!item.is_null()).then_some((item, None));
        };
        let group_source = sys::obs_sceneitem_get_source(group);
        sys::obs_source_filter_add(group_source, filter.as_ptr());
        let transform = ObsSource::create_private(
            "lightair_transform",
            &format!("transform {name}"),
            Some(&data_from_json(&transform_settings(layout))),
        );
        if let Some(t) = &transform {
            sys::obs_source_filter_add(group_source, t.as_ptr());
        }
        Some((group, Some(Masked { inner, filter, transform })))
    }
}

/// Lays out a object's item; for a shaped object the crop goes inside the group (so the mask is
/// computed on the cropped picture) and the box, fit and rotation on the group.
pub(crate) unsafe fn layout_item(item: *mut sys::obs_sceneitem_t, masked: Option<&Masked>, l: &Layout) {
    unsafe {
        let Some(m) = masked else { return apply_layout(item, l) };
        sys::obs_sceneitem_set_alignment(m.inner, sys::OBS_ALIGN_LEFT | sys::OBS_ALIGN_TOP);
        sys::obs_sceneitem_set_bounds_type(m.inner, sys::OBS_BOUNDS_NONE);
        sys::obs_sceneitem_set_scale(m.inner, &vec2(1.0, 1.0));
        sys::obs_sceneitem_set_rot(m.inner, 0.0);
        sys::obs_sceneitem_set_pos(m.inner, &vec2(0.0, 0.0));
        let crop = sys::obs_sceneitem_crop {
            left: l.crop.left as i32,
            top: l.crop.top as i32,
            right: l.crop.right as i32,
            bottom: l.crop.bottom as i32,
        };
        sys::obs_sceneitem_set_crop(m.inner, &crop);
        apply_layout(item, &Layout { crop: lightair_core::Crop::default(), ..*l });
        if let Some(shape) = &l.shape {
            m.filter.update(&data_from_json(&mask_settings(shape)));
        }
        sys::obs_source_set_enabled(m.filter.as_ptr(), l.shape.is_some());
        if let Some(t) = &m.transform {
            t.update(&data_from_json(&transform_settings(l)));
            sys::obs_source_set_enabled(t.as_ptr(), l.has_transform());
        }
    }
}

/// A track's effects that libobs runs (plugin filters): the engine runs its own (`audio.*`).
fn libobs_effects(effects: &[Effect]) -> Vec<Effect> {
    effects.iter().filter(|e| !e.kind.starts_with("audio.")).cloned().collect()
}

unsafe fn apply_track_levels(src: *mut sys::obs_source_t, track: &Track) {
    unsafe {
        sys::obs_source_set_volume(src, db_to_mul(track.volume_db));
        sys::obs_source_set_muted(src, track.muted);
        let monitoring =
            if track.cue { sys::OBS_MONITORING_TYPE_MONITOR_AND_OUTPUT } else { sys::OBS_MONITORING_TYPE_NONE };
        sys::obs_source_set_monitoring_type(src, monitoring);
    }
}

type Chain = Vec<(Id, ObsSource)>;

/// Creates libobs filters for an effect chain and attaches them to `src`, in order.
unsafe fn chain_build(src: *mut sys::obs_source_t, effects: &[Effect]) -> Chain {
    let mut chain = Chain::new();
    for effect in effects {
        let Some(filter) =
            ObsSource::create_private(&effect.kind, &effect.id.to_string(), Some(&data_from_json(&effect.settings)))
        else {
            tracing::warn!(kind = effect.kind, "could not create effect");
            continue;
        };
        unsafe {
            sys::obs_source_set_enabled(filter.as_ptr(), effect.enabled);
            sys::obs_source_filter_add(src, filter.as_ptr());
        }
        chain.push((effect.id, filter));
    }
    chain
}

unsafe fn chain_remove(src: *mut sys::obs_source_t, chain: &mut Chain) {
    for (_, filter) in chain.drain(..) {
        unsafe { sys::obs_source_filter_remove(src, filter.as_ptr()) };
    }
}

/// Brings a chain from `before` to `after`: rebuilt when effects were added, removed or
/// reordered; otherwise updated in place. Settings are replaced, not merged: the document holds
/// the whole intended state (a look resets everything it doesn't set).
unsafe fn chain_update(src: *mut sys::obs_source_t, chain: &mut Chain, before: &[Effect], after: &[Effect]) {
    let shape = |effects: &[Effect]| effects.iter().map(|e| (e.id, e.kind.clone())).collect::<Vec<_>>();
    if shape(before) != shape(after) {
        unsafe {
            chain_remove(src, chain);
            *chain = chain_build(src, after);
        }
        return;
    }
    for (old, new) in before.iter().zip(after) {
        let Some((_, filter)) = chain.iter().find(|(id, _)| *id == new.id) else { continue };
        if old.enabled != new.enabled {
            unsafe { sys::obs_source_set_enabled(filter.as_ptr(), new.enabled) };
        }
        if old.settings != new.settings {
            filter.reset(&data_from_json(&new.settings));
        }
    }
}

/// libobs tick (video thread): advance a running Move. Never blocks: if the engine thread holds
/// the lock, this frame is skipped.
unsafe extern "C" fn on_tick(param: *mut std::ffi::c_void, _seconds: f32) {
    let anim = unsafe { &*(param as *const Mutex<Option<MoveAnim>>) };
    if let Some(mut guard) = anim.try_lock()
        && let Some(a) = guard.as_mut()
    {
        a.step();
    }
}

/// Volume meter callback (libobs audio thread): store levels, nothing else.
unsafe extern "C" fn on_levels(
    param: *mut std::ffi::c_void,
    magnitude: *const f32,
    peak: *const f32,
    _source_peak: *const f32,
) {
    let cell = unsafe { &*(param as *const MeterCell) };
    let n = sys::MAX_AUDIO_CHANNELS as usize;
    let (magnitude, peak) = unsafe { (std::slice::from_raw_parts(magnitude, n), std::slice::from_raw_parts(peak, n)) };
    cell.store(peak, magnitude);
}

/// The libobs transition type and settings for a transition (Cut and Move use a fade source:
/// Cut sets it directly, Move lands through it).
fn transition_source(
    t: &TransitionSettings,
    stinger: Option<&lightair_core::Stinger>,
) -> (&'static str, serde_json::Value) {
    match t.kind {
        // Without a stinger video, a fade.
        TransitionKind::Stinger => match stinger {
            Some(s) => (
                "obs_stinger_transition",
                serde_json::json!({
                    "path": s.path,
                    "transition_point": s.cut_ms,
                    "tp_type": 0,
                    "hw_decode": true,
                    "preload": true,
                }),
            ),
            None => ("fade_transition", serde_json::json!({})),
        },
        TransitionKind::Slide => ("slide_transition", serde_json::json!({"direction": t.direction.as_str()})),
        TransitionKind::Wipe => {
            // Gradient images shipped with the transitions module; the dark end is revealed
            // first, so `linear-h` travels right and `linear-v` travels down.
            use lightair_core::Direction::*;
            let image = if matches!(t.direction, Left | Right) { "linear-h.png" } else { "linear-v.png" };
            let invert = matches!(t.direction, Left | Up);
            ("wipe_transition", serde_json::json!({"luma_image": image, "luma_invert": invert, "luma_softness": 0.05}))
        }
        TransitionKind::Dip => {
            ("fade_to_color_transition", serde_json::json!({"color": 0xFF000000u32, "switch_point": 50}))
        }
        TransitionKind::Cut | TransitionKind::Fade | TransitionKind::Move => ("fade_transition", serde_json::json!({})),
    }
}

/// A source's settings as its libobs source takes them (portable kinds translated).
/// A source's libobs settings; `drawn`: its scene's look draws the pointer (so the capture's
/// own goes, as with the drawn pointer effect).
fn source_settings(source: &lightair_core::Source, drawn: bool) -> serde_json::Value {
    let mut s = crate::kinds::resolve(&source.kind, &source.settings).map_or_else(|| source.settings.clone(), |(_, s)| s);
    // Lightair draws the pointer: the capture's own goes (every platform's name for it)
    if (drawn || draws_pointer(source))
        && let Some(o) = s.as_object_mut()
    {
        for key in ["show_cursor", "capture_cursor", "cursor"] {
            o.insert(key.into(), false.into());
        }
    }
    s
}

/// Whether a source is a screen capture (a display, a window, an app or a game).
fn is_screen(source: &lightair_core::Source) -> bool {
    source.kind == lightair_core::kinds::SCREEN
        || matches!(source.kind.as_str(), "screen_capture" | "display_capture" | "monitor_capture" | "window_capture" | "game_capture")
}

/// A `ScreenZoomPointer` as the pointer filter's settings.
fn screen_zoom_pointer_settings(l: &lightair_core::ScreenZoomPointer) -> serde_json::Value {
    serde_json::json!({
        "draw": l.pointer_drawn && !l.pointer_hidden,
        // the capture's own pointer goes when Lightair draws one, or when there's to be none
        "hide_capture": l.pointer_drawn || l.pointer_hidden,
        "style": l.pointer_style,
        "size": l.pointer_size,
        "smoothing": l.pointer_smoothing,
        "tilt": l.pointer_tilt,
        "idle_s": l.pointer_idle_s,
        // clicks are drawn with the pointer
        "clicks": if l.pointer_drawn && !l.pointer_hidden { l.clicks } else { 0 },
        "arrow_only": false,
        "color": l.pointer_color,
        "zoom": l.zoom,
        "zoom_scale": l.zoom_scale,
        "zoom_hold_s": l.zoom_hold_s,
        "zoom_motion": l.zoom_motion,
        "zoom_typing": l.zoom_typing,
        "zoom_badge": l.zoom_badge,
    })
}

/// Whether a source has the drawn pointer on.
fn draws_pointer(source: &lightair_core::Source) -> bool {
    source.effects.iter().any(|e| e.enabled && (e.kind == crate::pointer::KIND || e.kind == lightair_core::catalog::OLD_POINTER_KIND))
}

fn data_from_json(value: &serde_json::Value) -> Data {
    Data::from_json(value)
}

/// OBS's media source keeps playing off air when it doesn't restart on air: paused off air (nothing
/// decoded), played on when back on air (Lightair's own media source does this itself).
unsafe fn pause_off_air(src: *mut sys::obs_source_t) {
    static PAUSED: std::sync::LazyLock<parking_lot::Mutex<HashSet<usize>>> = std::sync::LazyLock::new(Default::default);
    unsafe extern "C" fn on_air(data: *mut std::ffi::c_void, _: *mut sys::calldata_t) {
        // only what it paused itself (not a pause of the user's)
        if PAUSED.lock().remove(&(data as usize)) {
            unsafe { sys::obs_source_media_play_pause(data as *mut sys::obs_source_t, false) };
        }
    }
    unsafe extern "C" fn off_air(data: *mut std::ffi::c_void, _: *mut sys::calldata_t) {
        let src = data as *mut sys::obs_source_t;
        if unsafe { sys::obs_source_media_get_state(src) } == sys::OBS_MEDIA_STATE_PLAYING {
            PAUSED.lock().insert(data as usize);
            unsafe { sys::obs_source_media_play_pause(src, true) };
        }
    }
    unsafe {
        let handler = sys::obs_source_get_signal_handler(src);
        sys::signal_handler_connect(handler, c"activate".as_ptr(), Some(on_air), src as *mut std::ffi::c_void);
        sys::signal_handler_connect(handler, c"deactivate".as_ptr(), Some(off_air), src as *mut std::ffi::c_void);
        // made off air: it starts paused (it plays from making it otherwise)
        if !sys::obs_source_active(src) {
            PAUSED.lock().insert(src as usize);
            sys::obs_source_media_play_pause(src, true);
        }
    }
}
