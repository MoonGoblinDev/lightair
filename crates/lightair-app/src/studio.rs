//! The studio: document + persistence + engine, as one GPUI model.
//!
//! Every change in the app goes through [`Studio::execute`]: the command is validated and
//! applied to the document, appended to the workspace journal, and the resulting diff is sent to
//! the engine thread for reconciliation (docs/03-architecture.md, "Document flow").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{Context, SharedString};
use lightair_core::{Command, CommandError, Document, Id, Workspace, diff};
use lightair_engine::outputs::{
    ClipsConfig, Container, OutputKey, OutputState, RecordingConfig, StreamConfig, default_encoders, runtime,
};
use lightair_engine::{Engine, Stats};
use lightair_store::Package;
use lightair_store::recordings::{self, File as ItemFile, Index, Item, Kind, Marker};

/// A recording on its way into the Library (docs/plans/screen-studio.md, "Library"). Its entry
/// is written when it starts (so a crash still leaves it findable) and again when it finishes.
struct Pending {
    workspace: PathBuf,
    item: Item,
    /// The canvases recorded, with the canvas name that tells their files apart.
    canvases: Vec<(Id, Option<String>)>,
    stopped: bool,
    /// Multitrack files still being written.
    multitrack: bool,
    thumbnail_due: Option<std::time::Instant>,
}

/// A Studio studio_take in progress (docs/plans/screen-studio.md, 4.2): its folder, Library entry,
/// and the source it logs.
struct StudioTakeRun {
    workspace: Option<PathBuf>,
    folder: PathBuf,
    item: Item,
    started: std::time::Instant,
    recorder: Option<lightair_platform::input::Recorder>,
    log: Option<lightair_store::input_log::Writer>,
    tracks: Vec<lightair_engine::studio_take::Track>,
    /// Its track files, and whose sound each is (the mixer track's name).
    audio: Vec<(PathBuf, String)>,
    stopping: bool,
    thumbnail_due: Option<std::time::Instant>,
    /// Its pauses, from `started`: (from, to), `to` open while paused. The files leave them
    /// out, so the input log does too.
    pauses: Vec<(std::time::Duration, Option<std::time::Duration>)>,
}

impl StudioTakeRun {
    fn paused(&self) -> bool {
        self.pauses.last().is_some_and(|p| p.1.is_none())
    }

    /// Time paused before `at` (from `started`), the open pause counting up to `at`.
    fn paused_before(&self, at: std::time::Duration) -> std::time::Duration {
        self.pauses.iter().map(|&(a, b)| b.unwrap_or(at).min(at).saturating_sub(a)).sum()
    }

    /// How long it has recorded: the time since it started, less its pauses.
    fn elapsed(&self) -> std::time::Duration {
        let now = self.started.elapsed();
        now - self.paused_before(now)
    }

    /// An input sample's time in the files, or `None` when it fell in a pause.
    fn file_time(&self, at: std::time::Duration) -> Option<std::time::Duration> {
        if self.pauses.iter().any(|&(a, b)| at >= a && b.is_none_or(|b| at < b)) {
            return None;
        }
        Some(at - self.paused_before(at))
    }

    /// Input samples as log records, on the files' clock (paused ones dropped).
    fn records(&self, samples: Vec<lightair_platform::input::Sample>) -> Vec<lightair_store::input_log::Record> {
        samples
            .into_iter()
            .filter_map(|s| Some(lightair_store::input_log::Record { at: self.file_time(s.at)?, entry: log_entry(s.event) }))
            .collect()
    }
}

fn log_entry(e: lightair_platform::input::Event) -> lightair_store::input_log::Entry {
    use lightair_platform::input::Event as E;
    use lightair_store::input_log::Entry;
    match e {
        E::Move { x, y } => Entry::Move { x, y },
        E::Down { button, x, y } => Entry::Down { button, x, y },
        E::Up { button, x, y } => Entry::Up { button, x, y },
        E::Scroll { dx, dy } => Entry::Scroll { dx, dy },
        E::KeyDown { code, modifiers } => Entry::KeyDown { code, modifiers },
        E::KeyUp { code, modifiers } => Entry::KeyUp { code, modifiers },
        E::Shape(s) => Entry::Shape(s as u8),
    }
}

/// A still for the Library: the program as it is now, written to `path` (by the window, which
/// has the preview).
#[derive(Debug, Clone)]
pub struct ThumbnailRequest {
    pub path: PathBuf,
}

/// What a panel (the mixer) asks the window to show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiRequest {
    /// A strip's effect, open in the Inspector.
    OpenEffect(lightair_core::MixerStrip, Id),
    /// The effect catalog for a strip.
    AddEffect(lightair_core::MixerStrip),
}

pub struct Studio {
    /// The chat of the destinations' channels (docs/plans/live-integrations.md), read while the
    /// workspace is open.
    pub chat: lightair_live::Hub,
    /// Services read besides the destinations' chats (tips: Settings › Connections).
    pub services: Vec<lightair_live::Channel>,
    /// The widgets' server (docs/plans/live-integrations.md, 2.5): `None` if it couldn't start.
    pub widgets: Option<lightair_widgets::Widgets>,
    doc: Document,
    package: Option<Package>,
    engine: Arc<Engine>,
    /// The scene selected for editing (the program scene unless studio mode stages another).
    pub selected_scene: Option<Id>,
    pub selected_object: Option<Id>,
    /// Further selected objects (⇧-click); `selected_object` is the one the Inspector edits.
    pub also_selected: Vec<Id>,
    /// The mixer track the Inspector shows (instead of a object).
    pub selected_track: Option<Id>,
    /// Last user-visible problem, shown in the status bar.
    pub issue: Option<SharedString>,
    /// Something a panel asks the window to show (the window takes it when it sees it).
    pub ui_request: Option<UiRequest>,
    pub stats: Stats,
    /// Clips announced so far (to report each new one once).
    clips_seen: u32,
    /// Frames libobs lagged since the previous stats sample (health is about now, not startup).
    pub recent_lagged: u32,
    /// The newest saved clip, and replays playing on the canvas (scene, object, source).
    pub last_clip: Option<PathBuf>,
    /// What the virtual camera was started on (the workspace's choice then): it restarts when
    /// the choice changes (a click, undo, another workspace).
    virtual_camera_started: Option<lightair_core::VirtualCameraSettings>,
    /// What other apps call the virtual camera (whose extension is installed), looked up when it
    /// starts.
    pub virtual_camera_name: &'static str,
    replays: Vec<(Id, Id, Id)>,
    /// Microphone tracks' loudest peak (dBFS) for each of the last ten seconds.
    mic_history: std::collections::HashMap<Id, std::collections::VecDeque<f32>>,
    /// Imported objects whose size was guessed, waiting for their sources to report a size.
    refit: Option<(Vec<lightair_store::EstimatedObject>, std::time::Instant)>,
    /// The engine was asked which countdowns ended, and hasn't answered yet.
    timers_asked: bool,
    /// The recording on its way into the Library.
    pending: Option<Pending>,
    /// The Studio studio_take being recorded.
    studio_take: Option<StudioTakeRun>,
    /// The studio_take just saved (workspace, item), for the window's card.
    pub last_studio_take: Option<(PathBuf, Id)>,
    /// Stills the window should studio_take for the Library.
    pub thumbnails: Vec<ThumbnailRequest>,
    /// Bumped when a workspace's recordings index changes (the Library reloads).
    pub library_version: u64,
    /// Multitrack audio files being recorded: their base name and paths.
    /// Multitrack audio being recorded: its files' base name, and each file with whose sound it is.
    multitrack_files: Option<(String, Vec<(std::path::PathBuf, String)>)>,
    /// What a new workspace starts as.
    starter: fn() -> Workspace,
}

impl Studio {
    pub fn open(engine: Arc<Engine>, dir: PathBuf, starter: fn() -> Workspace) -> Self {
        let (workspace, package, issue) = match lightair_store::Package::open(&dir, starter) {
            Ok(opened) => {
                let issue = (opened.recovered > 0)
                    .then(|| SharedString::from(format!("Recovered {} unsaved change(s)", opened.recovered)));
                (opened.workspace, Some(opened.package), issue)
            }
            Err(e) => {
                tracing::error!(%e, "could not open the workspace; using an unsaved one");
                (Workspace::new("Untitled"), None, Some(SharedString::from(format!("Workspace not saved: {e}"))))
            }
        };
        let selected_scene = workspace.program.or(workspace.scenes.first().map(|s| s.id));
        let chat = lightair_live::Hub::new();
        // what came before (before anything listens: it isn't new)
        lightair_live::log::start(&chat, lightair_platform::paths::data_dir().join("chat"));
        let widgets = crate::widgets::start(&chat);
        let mut studio = Self {
            chat,
            services: Vec::new(),
            widgets,
            doc: Document::new(workspace),
            package,
            engine,
            selected_scene,
            selected_object: None,
            also_selected: Vec::new(),
            selected_track: None,
            ui_request: None,
            issue,
            stats: Stats::default(),
            clips_seen: 0,
            recent_lagged: 0,
            refit: None,
            timers_asked: false,
            pending: None,
            studio_take: None,
            last_studio_take: None,
            thumbnails: Vec::new(),
            library_version: 0,
            multitrack_files: None,
            mic_history: Default::default(),
            last_clip: None,
            virtual_camera_started: None,
            virtual_camera_name: VIRTUAL_CAMERA_NAMES[0],
            replays: Vec::new(),
            starter,
        };
        studio.migrate();
        // (before the scene's pages open: a widget's page is served once it's known)
        studio.sync_chat();
        studio.rebuild_engine();
        studio.sync_clips();
        studio
    }

    /// Brings an older workspace's effects up to date (saved, but not an undo step).
    fn migrate(&mut self) {
        let mut commands = lightair_core::catalog::migrate(self.doc.workspace());
        // tracks' libobs audio filters: the audio engine runs its own (LIGHTAIR_LEGACY_AUDIO
        // keeps libobs mixing, so the filters stay)
        if std::env::var("LIGHTAIR_LEGACY_AUDIO").map_or(true, |v| v != "1") {
            commands.extend(lightair_core::catalog::migrate_audio(self.doc.workspace()));
        }
        if commands.is_empty() {
            return;
        }
        for cmd in &commands {
            if let Err(e) = self.doc.apply(cmd) {
                tracing::warn!(%e, "couldn't update an effect");
                continue;
            }
            if let Some(pkg) = &mut self.package
                && let Err(e) = pkg.record(cmd)
            {
                tracing::warn!(%e, "couldn't save the updated effects");
            }
        }
        self.doc = Document::new(self.doc.workspace().as_ref().clone());
        tracing::info!(count = commands.len(), "effects updated to the effect stack");
    }

    /// Where the workspace lives on disk (`None` for an unsaved workspace).
    pub fn workspace_dir(&self) -> Option<std::path::PathBuf> {
        self.package.as_ref().map(|p| p.dir().to_path_buf())
    }

    /// The workspace's `assets` folder (prefabs, images, video, sound), made if missing; `None`
    /// for an unsaved workspace.
    pub fn assets_dir(&self) -> Option<std::path::PathBuf> {
        let dir = self.package.as_ref()?.dir().join("assets");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    pub fn workspace(&self) -> &Arc<Workspace> {
        self.doc.workspace()
    }

    /// Applies, journals and reconciles a command. Errors are shown in the status bar.
    pub fn execute(&mut self, command: Command, cx: &mut Context<Self>) -> Result<(), CommandError> {
        let command = with_page_cleanup(self.workspace(), command);
        match self.doc.apply(&command) {
            Ok(change) => {
                if let Some(pkg) = &mut self.package
                    && let Err(e) = pkg.record(&command)
                {
                    self.issue = Some(format!("Couldn't save: {e}").into());
                }
                self.push(change.before, change.after);
                self.fix_selection();
                self.sync_clips();
                self.sync_chat();
                self.sync_virtual_camera();
                cx.notify();
                Ok(())
            }
            Err(e) => {
                self.issue = Some(e.to_string().into());
                cx.notify();
                Err(e)
            }
        }
    }

    /// Merges an OBS scene collection into the workspace as one undo step; the report goes to
    /// the status bar (summary) and the log (details).
    pub fn import_obs(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let result = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|json| lightair_store::import_obs_collection(&json, self.workspace()).map_err(|e| e.to_string()));
        match result {
            Ok((command, report)) => {
                for (group, items) in [
                    ("converted", &report.converted),
                    ("replaced", &report.replaced),
                    ("kept as plugin", &report.kept_plugin),
                    ("needs attention", &report.attention),
                ] {
                    for item in items {
                        tracing::info!(target: "import", "{group}: {item}");
                    }
                }
                if self.execute(command, cx).is_ok() {
                    self.issue = Some(format!("Imported OBS scenes: {}", report.summary()).into());
                    if !report.estimated.is_empty() {
                        let mut sources: Vec<Id> = report.estimated.iter().map(|e| e.source).collect();
                        sources.dedup();
                        self.engine.post(move |s| s.reconciler.hold_sources(&sources));
                        self.refit = Some((report.estimated, std::time::Instant::now()));
                    }
                }
            }
            Err(e) => self.issue = Some(format!("Couldn't import: {e}").into()),
        }
        cx.notify();
    }

    /// Plays the newest clip full-frame on top of the live scene; it removes itself when done.
    pub fn replay_last_clip(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(scene)) = (self.last_clip.clone(), self.workspace().program) else {
            self.issue = Some("Save a clip first, then replay it.".into());
            cx.notify();
            return;
        };
        let p = self.workspace().clone();
        let source = lightair_core::Source::new(
            "Replay",
            "ffmpeg_source",
            serde_json::json!({
                "local_file": path.to_string_lossy(),
                "is_local_file": true,
                "looping": false,
                "restart_on_activate": true,
                "clear_on_media_end": true,
            }),
        );
        let layouts: Vec<(Id, lightair_core::Layout)> = p
            .canvases
            .iter()
            .map(|f| (f.id, lightair_core::Layout::boxed(0.0, 0.0, f.width as f32, f.height as f32)))
            .collect();
        let object = lightair_core::Object::new("Replay", source.id, layouts);
        let (object_id, source_id) = (object.id, source.id);
        let command = Command::Batch {
            label: "Replay clip".into(),
            commands: vec![Command::AddSource { source }, Command::AddObject { scene, object, index: None }],
        };
        if self.execute(command, cx).is_ok() {
            self.replays.push((scene, object_id, source_id));
        }
    }

    /// Removes replays whose clip has finished (called on the stats tick).
    pub fn end_replays(&mut self, cx: &mut Context<Self>) {
        let mut finished = Vec::new();
        for (scene, object, source) in &self.replays {
            let id = *source;
            let state = self.engine.call(move |s| s.reconciler.media_status(id)).map(|s| s.state);
            if matches!(
                state,
                None | Some(lightair_engine::media::MediaState::Ended | lightair_engine::media::MediaState::Error)
            ) {
                finished.push((*scene, *object, *source));
            }
        }
        if finished.is_empty() {
            return;
        }
        self.replays.retain(|r| !finished.contains(r));
        let p = self.workspace().clone();
        let commands: Vec<Command> = finished
            .into_iter()
            .filter(|(scene, object, _)| p.scene(*scene).is_some_and(|s| s.object(*object).is_some()))
            .flat_map(|(scene, object, source)| [Command::RemoveObject { scene, object }, Command::RemoveSource { source }])
            .collect();
        if !commands.is_empty() {
            let _ = self.execute(Command::Batch { label: "End replay".into(), commands }, cx);
        }
    }

    /// Plays the mixer again once a plugin insert has loaded (it passes audio through until then).
    pub fn refresh_plugins(&mut self) {
        if lightair_engine::plugins::changed() {
            let workspace = self.workspace().clone();
            self.engine.post(move |s| s.reconciler.refresh_audio(&workspace));
        }
        // the camera and microphone sources held back for macOS's prompt, once answered
        if lightair_engine::access::changed() {
            let workspace = self.workspace().clone();
            self.engine.post(move |s| s.reconciler.access_answered(&workspace));
        }
    }

    /// Runs the end action of countdowns that just ended ("Then go to" a scene). Asked without
    /// waiting (on every tick: the engine may be busy opening a camera, and the window would
    /// freeze meanwhile); acted on as the answer comes.
    pub fn run_timer_ends(&mut self, cx: &mut Context<Self>) {
        if self.timers_asked {
            return;
        }
        self.timers_asked = true;
        let (tx, rx) = futures_channel::oneshot::channel();
        self.engine.post(move |s| {
            let _ = tx.send(s.reconciler.take_ended_timers());
        });
        cx.spawn(async move |this, cx| {
            let ended = rx.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.timers_asked = false;
                this.timers_ended(ended, cx);
            });
        })
        .detach();
    }

    fn timers_ended(&mut self, ended: Vec<Id>, cx: &mut Context<Self>) {
        for id in ended {
            let target = self
                .workspace()
                .sources
                .get(&id)
                .and_then(|i| i.settings["end_scene"].as_str().map(str::to_string))
                .and_then(|s| self.workspace().scenes.iter().find(|sc| sc.id.to_string() == s).map(|sc| sc.id));
            if let Some(scene) = target
                && self.workspace().program != Some(scene)
            {
                let _ = self.execute(Command::SetProgram { scene: Some(scene) }, cx);
            }
        }
    }

    /// Corrects imported objects whose source size was guessed, once the sources report their
    /// real size (or after a few seconds, with whatever is known). Objects changed since the
    /// import are left alone. One undo step.
    pub fn refit_imported(&mut self, cx: &mut Context<Self>) {
        let Some((pending, started)) = &self.refit else { return };
        let sources: Vec<Id> = pending.iter().map(|e| e.source).collect();
        let sizes = self.engine.call(move |s| sources.iter().map(|id| s.reconciler.source_size(*id)).collect::<Vec<_>>());
        if sizes.iter().any(Option::is_none) && started.elapsed() < std::time::Duration::from_secs(8) {
            return;
        }
        let (pending, _) = self.refit.take().unwrap();
        self.engine.post(|s| s.reconciler.release_sources());
        let workspace = self.workspace().clone();
        let commands: Vec<Command> = pending
            .iter()
            .zip(sizes)
            .filter_map(|(est, size)| {
                let current = workspace.scene(est.scene)?.object(est.object)?.layouts.get(&est.canvas)?;
                let size = size.filter(|_| *current == est.layout)?;
                Some(Command::SetLayout {
                    scene: est.scene,
                    object: est.object,
                    canvas: est.canvas,
                    layout: est.refit(size),
                })
            })
            .collect();
        if commands.is_empty() {
            return;
        }
        let n = commands.len();
        if self.execute(Command::Batch { label: "Size imported objects".into(), commands }, cx).is_ok() {
            self.issue =
                Some(format!("Corrected the size of {n} imported object{}", if n == 1 { "" } else { "s" }).into());
        }
    }

    pub fn begin_gesture(&mut self) {
        self.doc.begin_gesture();
    }

    pub fn end_gesture(&mut self) {
        self.doc.end_gesture();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(change) = self.doc.undo() {
            self.issue = Some(format!("Undo: {}", change.label).into());
            self.push(change.before, change.after);
            self.sync_virtual_camera();
            self.sync_chat();
            self.compact();
            self.fix_selection();
            cx.notify();
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(change) = self.doc.redo() {
            self.issue = Some(format!("Redo: {}", change.label).into());
            self.push(change.before, change.after);
            self.sync_virtual_camera();
            self.sync_chat();
            self.compact();
            self.fix_selection();
            cx.notify();
        }
    }

    /// Writes the workspace files and empties the journal (on a timer, on quit, after undo).
    /// Opens another workspace in place of this one: this one is saved, the engine is rebuilt from
    /// the other. Not while recording or live.
    pub fn switch_to(&mut self, dir: &std::path::Path, cx: &mut Context<Self>) -> Result<(), String> {
        if self.is_recording() || self.studio_take.is_some() || runtime().any_stream_running() {
            return Err("Stop recording and streaming first.".into());
        }
        if self.workspace_dir().as_deref() == Some(dir) {
            return Ok(());
        }
        let opened = lightair_store::Package::open(dir, self.starter).map_err(|e| e.to_string())?;
        self.compact();
        self.package = Some(opened.package);
        self.doc = Document::new(opened.workspace);
        self.migrate();
        let workspace = self.doc.workspace().clone();
        self.selected_scene = workspace.program.or(workspace.scenes.first().map(|s| s.id));
        self.selected_object = None;
        self.also_selected.clear();
        self.selected_track = None;
        self.replays.clear();
        self.refit = None;
        self.mic_history.clear();
        self.last_clip = None;
        self.clips_seen = 0;
        self.issue = (opened.recovered > 0).then(|| format!("Recovered {} unsaved change(s)", opened.recovered).into());
        self.engine.call(move |s| {
            s.outputs.stop_clips();
            s.reconciler.rebuild(&workspace);
        });
        self.sync_clips();
        self.sync_chat();
        self.sync_virtual_camera();
        self.library_version += 1;
        // opened next time too (not by a copy started on a given workspace: tests, screenshots)
        if std::env::var_os("LIGHTAIR_PROJECT").is_none() {
            let mut prefs = crate::prefs::load_from(&crate::prefs::path());
            prefs.last_workspace = Some(dir.to_path_buf());
            let _ = crate::prefs::save_to(&crate::prefs::path(), &prefs);
        }
        cx.notify();
        Ok(())
    }

    /// Makes a new workspace called `name` beside the open one (in the Workspaces folder), from the
    /// starter, and returns its folder (not opened).
    pub fn create_workspace(&self, name: &str) -> Result<PathBuf, String> {
        let base = self.workspace_dir().and_then(|d| d.parent().map(Path::to_path_buf)).unwrap_or_else(lightair_platform::paths::workspaces_dir);
        let clean: String = name.trim().chars().map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c }).collect();
        let clean = if clean.is_empty() { "Untitled".to_string() } else { clean };
        let dir = (1..)
            .map(|n| if n == 1 { base.join(format!("{clean}.lightair")) } else { base.join(format!("{clean} {n}.lightair")) })
            .find(|d| !d.exists())
            .unwrap_or_else(|| base.join(format!("{clean}.lightair")));
        let starter = self.starter;
        let name = name.trim().to_string();
        lightair_store::Package::open(&dir, || {
            let mut p = starter();
            p.name = if name.is_empty() { "Untitled".into() } else { name };
            p
        })
        .map_err(|e| e.to_string())?;
        Ok(dir)
    }

    pub fn compact(&mut self) {
        if let Some(pkg) = &mut self.package
            && let Err(e) = pkg.compact(self.doc.workspace())
        {
            tracing::error!(%e, "compaction failed");
        }
    }

    /// Compacts and waits for it to be on disk (the package writes on its own thread): on quit.
    pub fn save_and_wait(&mut self) {
        self.compact();
        if let Some(pkg) = &self.package {
            pkg.wait();
        }
    }

    pub fn flush(&mut self) {
        if let Some(pkg) = &mut self.package {
            if pkg.pending() > 0 {
                let _ = pkg.flush();
            }
            if pkg.pending() > 200 {
                self.compact();
            }
        }
    }

    // ------------------------------------------------------------ outputs
    pub fn recording_folder(&self) -> PathBuf {
        self.workspace()
            .outputs
            .recording
            .folder
            .clone()
            .unwrap_or_else(lightair_platform::paths::recordings_dir)
    }

    /// Keeps the replay buffer running exactly when clips are on.
    /// Reads the chat of every destination with a chat channel set (and stops reading the ones
    /// no longer set).
    /// Reads `service` (a tip service) or stops reading it.
    pub fn set_service(&mut self, service: &str, on: bool) {
        self.services.retain(|c| c.platform != service);
        if on {
            self.services.push(lightair_live::Channel { platform: service.into(), channel: "tips".into() });
        }
        self.sync_chat();
    }

    fn sync_chat(&self) {
        let channels: Vec<lightair_live::Channel> = self
            .workspace()
            .outputs
            .destinations
            .iter()
            .filter_map(|d| {
                let channel = d.chat.as_ref().filter(|c| !c.trim().is_empty())?;
                Some(lightair_live::Channel { platform: d.platform.clone()?, channel: channel.trim().to_string() })
            })
            .chain(self.services.iter().cloned())
            .collect();
        self.chat.set_channels(&channels);
        if let Some(w) = &self.widgets {
            w.set_instances(self.workspace().sources.values().filter_map(|s| lightair_widgets::Instance::from_settings(&s.settings)).collect());
        }
    }

    fn sync_clips(&mut self) {
        let settings = self.workspace().outputs.clips;
        let running = runtime().get(OutputKey::Clips).state.is_running();
        if settings.enabled && !running {
            let cfg = ClipsConfig {
                folder: self.recording_folder().join("Clips"),
                seconds: settings.seconds,
                encoders: default_encoders(&self.engine.capabilities.encoders, 8000, false),
            };
            let canvas = settings.canvas;
            self.engine.post(move |s| {
                let video = canvas.and_then(|f| s.reconciler.canvas_video(f));
                if let Err(e) = s.outputs.start_clips(&cfg, video) {
                    tracing::error!(%e, "replay buffer failed to start");
                }
            });
        } else if !settings.enabled && running {
            self.engine.post(|s| s.outputs.stop_clips());
        }
    }

    // ------------------------------------------------------------ virtual camera

    /// The virtual camera's state now.
    pub fn virtual_camera(&self) -> OutputState {
        runtime().get(OutputKey::VirtualCamera).state
    }

    /// Starts the virtual camera, or stops it.
    pub fn toggle_virtual_camera(&mut self, cx: &mut Context<Self>) {
        if self.virtual_camera().is_running() || matches!(self.virtual_camera(), OutputState::Starting) {
            self.engine.post(|s| s.outputs.stop_virtual_camera());
        } else {
            self.start_virtual_camera();
        }
        cx.notify();
    }

    /// Windows: installs the virtual camera (Windows asks for administrator approval):
    /// Lightair's (Windows 11), used from the next start of the camera; OBS's elsewhere, there
    /// from Lightair's next start (libobs lists its outputs as it loads its modules).
    pub fn install_virtual_camera(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = lightair_engine::vcam::install() {
            tracing::warn!(%e, "virtual camera install");
        }
        cx.notify();
    }

    fn start_virtual_camera(&mut self) {
        self.virtual_camera_name = virtual_camera_name();
        let choice = self.workspace().outputs.virtual_camera;
        self.virtual_camera_started = Some(choice);
        let canvas = choice.canvas.filter(|c| self.workspace().canvas(*c).is_some());
        // one scene: on the chosen canvas (its size), else the primary
        let scene = choice.scene.filter(|id| self.workspace().scene(*id).is_some());
        let shape = canvas.and_then(|c| self.workspace().canvas(c)).unwrap_or_else(|| self.workspace().primary_canvas());
        let (canvas_id, size) = (shape.id, (shape.width, shape.height, shape.fps_num, shape.fps_den));
        self.engine.post(move |s| {
            let video = canvas.and_then(|f| s.reconciler.canvas_video(f));
            let scene = scene.and_then(|id| s.reconciler.scene_source(id, canvas_id)).map(|source| {
                lightair_engine::outputs::VirtualCameraScene { source, width: size.0, height: size.1, fps_num: size.2, fps_den: size.3 }
            });
            if let Err(e) = s.outputs.start_virtual_camera(video, scene) {
                tracing::warn!(%e, "virtual camera didn't start");
            }
        });
    }

    /// Which scene the virtual camera shows (`None`: the one on air), kept in the workspace.
    pub fn set_virtual_camera_scene(&mut self, scene: Option<Id>, cx: &mut Context<Self>) {
        let settings = lightair_core::VirtualCameraSettings { scene, ..self.workspace().outputs.virtual_camera };
        let _ = self.execute(Command::SetVirtualCamera { settings }, cx);
    }

    /// Which canvas the virtual camera shows (`None`: the primary), kept in the workspace.
    pub fn set_virtual_camera_canvas(&mut self, canvas: Option<Id>, cx: &mut Context<Self>) {
        let settings = lightair_core::VirtualCameraSettings { canvas, ..self.workspace().outputs.virtual_camera };
        let _ = self.execute(Command::SetVirtualCamera { settings }, cx);
    }

    /// A running virtual camera follows the workspace's choice (restarted when it changes).
    fn sync_virtual_camera(&mut self) {
        if self.virtual_camera().is_running() && self.virtual_camera_started != Some(self.workspace().outputs.virtual_camera) {
            self.engine.post(|s| s.outputs.stop_virtual_camera());
            self.start_virtual_camera();
        }
    }

    pub fn clips_ready(&self) -> bool {
        matches!(runtime().get(OutputKey::Clips).state, OutputState::Active { .. })
    }

    pub fn save_clip(&mut self, cx: &mut Context<Self>) {
        if !self.clips_ready() {
            self.issue = Some("Turn on Clips in Outputs to save clips.".into());
            cx.notify();
            return;
        }
        self.engine.post(|s| {
            s.outputs.save_clip();
        });
    }

    fn announce_clips(&mut self) {
        let status = runtime().get(OutputKey::Clips);
        if status.saved_clips > self.clips_seen {
            self.clips_seen = status.saved_clips;
            let name = status.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            self.issue = Some(format!("Clip saved: {}", name.unwrap_or_default()).into());
            self.last_clip = status.path.clone();
            if let (Some(dir), Some(path)) = (self.workspace_dir(), status.path.clone()) {
                let seconds = self.workspace().outputs.clips.seconds as u64;
                let started = chrono::Utc::now().timestamp_millis() - seconds as i64 * 1000;
                let title = format!("Clip · {}", chrono::Local::now().format("%-d %b, %H:%M:%S"));
                let mut item = Item::new(Kind::Clip, title, vec![ItemFile::new(path)], started);
                item.duration_ms = Some(seconds * 1000);
                let thumb = recordings::thumbnails_dir(&dir).join(format!("{}.png", item.id));
                item.thumbnail = Some(thumb.clone());
                self.thumbnails.push(ThumbnailRequest { path: thumb });
                self.index_add(&dir, item);
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        runtime().any_recording_running() || lightair_engine::audio_host::STEMS_RUNNING.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn toggle_recording(&mut self, cx: &mut Context<Self>) {
        if self.studio_take.is_some() {
            return self.toggle_studio_take(cx);
        }
        if self.is_recording() {
            let status = runtime().recording();
            if let Some(p) = &mut self.pending {
                p.stopped = true;
                if let OutputState::Active { since } | OutputState::Reconnecting { since } = status.state {
                    p.item.duration_ms = Some(status.recorded(since).as_millis() as u64);
                }
            }
            self.engine.post(|s| s.outputs.stop_recording());
            self.finish_multitrack(cx);
            return;
        }
        let workspace = self.workspace().clone();
        let settings = workspace.outputs.recording.clone();
        let container = match settings.container.as_str() {
            "mkv" => Container::Mkv,
            "mov" => Container::Mov,
            _ => Container::HybridMp4,
        };
        // Every chosen canvas, each to its own file; with several, the canvas's name tells
        // them apart ("Lightair 2026-11-12 10-00-00 Vertical.mp4").
        let targets = settings.targets(&workspace);
        let several = targets.len() > 1;
        let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
        // Make the journal durable before outputs start (docs/14, "Saving, autosave & recovery").
        self.flush();
        // Multitrack audio files (and, audio only, the program) beside the video.
        let mut multitrack = false;
        if settings.multitrack || settings.audio_only {
            let (p, folder, base) = (workspace.clone(), self.recording_folder(), format!("Lightair {stamp}"));
            let program = settings.audio_only;
            match self.engine.call(move |s| s.reconciler.start_multitrack(&p, &folder, &base, program)) {
                Ok(files) => {
                    self.multitrack_files = Some((format!("Lightair {stamp}"), files));
                    multitrack = true;
                }
                Err(e) => self.issue = Some(format!("Multitrack audio didn't start: {e}").into()),
            }
        }
        // its Library entry
        if let Some(dir) = self.workspace_dir() {
            let kind = if settings.audio_only { Kind::Multitrack } else { Kind::Recording };
            let title = format!("{} · {}", workspace.name, chrono::Local::now().format("%-d %b, %H:%M"));
            let mut item = Item::new(kind, title, Vec::new(), chrono::Utc::now().timestamp_millis());
            let thumbnail_due = (!settings.audio_only).then(|| {
                item.thumbnail = Some(recordings::thumbnails_dir(&dir).join(format!("{}.png", item.id)));
                std::time::Instant::now() + std::time::Duration::from_secs(3)
            });
            self.pending = Some(Pending {
                workspace: dir,
                item,
                canvases: if settings.audio_only {
                    Vec::new()
                } else {
                    targets.iter().map(|c| (*c, several.then(|| workspace.canvas(*c).map(|f| f.name.clone()).unwrap_or_default()))).collect()
                },
                stopped: false,
                multitrack,
                thumbnail_due,
            });
        }
        if settings.audio_only {
            cx.notify();
            return;
        }
        for canvas in targets {
            let label = several.then(|| workspace.canvas(canvas).map(|f| f.name.clone()).unwrap_or_default());
            let name = match &label {
                Some(l) => format!("Lightair {stamp} {}.{}", lightair_core::file_safe(l), container.extension()),
                None => format!("Lightair {stamp}.{}", container.extension()),
            };
            let Some(shape) = workspace.canvas(canvas) else { continue };
            let path = self.recording_folder().join(&name);
            if let Some(p) = &mut self.pending {
                p.item.files.push(ItemFile { canvas: label.clone(), ..ItemFile::new(path) });
            }
            let cfg = RecordingConfig {
                canvas,
                path: self.recording_folder().join(name),
                label,
                split_minutes: settings.split_minutes,
                container,
                encoders: crate::encoding::for_recording(&self.engine.capabilities.encoders, &settings, shape),
                // the engine plays the first five mixes into libobs mixes 1-5, in mixer order
                extra_mixes: settings.file_mixes.iter().filter_map(|m| crate::encoding::libobs_mix(&workspace, *m)).collect(),
            };
            self.engine.post(move |s| {
                let video = s.reconciler.canvas_video(canvas);
                if let Err(e) = s.outputs.start_recording(&cfg, video) {
                    tracing::error!(%e, "recording failed to start");
                }
            });
        }
        if let Some(p) = &self.pending {
            let (dir, item) = (p.workspace.clone(), p.item.clone());
            self.index_add(&dir, item);
        }
        cx.notify();
    }

    /// A Studio studio_take is being recorded (and isn't stopping yet).
    /// Whether the Studio take is paused.
    pub fn studio_take_paused(&self) -> bool {
        self.studio_take.as_ref().is_some_and(|t| !t.stopping && t.paused())
    }

    pub fn is_taking(&self) -> bool {
        self.studio_take.as_ref().is_some_and(|t| !t.stopping)
    }

    /// How long the studio_take has run.
    pub fn studio_take_elapsed(&self) -> Option<std::time::Duration> {
        self.studio_take.as_ref().filter(|t| !t.stopping).map(StudioTakeRun::elapsed)
    }

    /// Starts or stops a Studio take: each screen and camera in the program to its own file,
    /// the multitrack audio, and the pointer, clicks and keys, in a studio_take folder.
    pub fn toggle_studio_take(&mut self, cx: &mut Context<Self>) {
        if self.studio_take.is_some() {
            self.stop_studio_take(cx);
            return;
        }
        if self.is_recording() {
            self.issue = Some("Stop the recording first.".into());
            cx.notify();
            return;
        }
        let workspace = self.workspace().clone();
        let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
        let folder = self.recording_folder().join(format!("Studio take {stamp}.lightair-take"));
        let fps = (workspace.primary_canvas().fps_num, workspace.primary_canvas().fps_den);
        self.flush();
        let (f, p, caps) = (folder.clone(), workspace.clone(), self.engine.capabilities.encoders.clone());
        let started = self.engine.call(move |s| {
            let sources = s.reconciler.studio_take_sources(&p);
            let studio_take = lightair_engine::studio_take::StudioTake::start(sources, &f, fps, &mut s.outputs, &caps)?;
            let tracks = studio_take.tracks.clone();
            s.studio_take = Some(studio_take);
            Ok::<_, String>(tracks)
        });
        let tracks = match started {
            Ok(t) => t,
            Err(e) => {
                self.issue = Some(format!("The take didn't start: {e}").into());
                cx.notify();
                return;
            }
        };
        // the scene as it is: what the studio_take doesn't record (images, text, colours) rebuilt in
        // the Editor, and where everything was
        if let Some(scene) = lightair_store::studio_take_scene::StudioTakeScene::of(&workspace, &folder) {
            let _ = scene.write(&folder);
        }
        // the audio engine's track files, beside the video
        let (p, f) = (workspace.clone(), folder.clone());
        let audio = match self.engine.call(move |s| s.reconciler.start_multitrack(&p, &f, "Audio", false)) {
            Ok(files) => files,
            Err(e) => {
                self.issue = Some(format!("The take's audio didn't start: {e}").into());
                Vec::new()
            }
        };
        // the pointer, clicks and keys
        let recorder = lightair_platform::input::Recorder::start(120);
        let log = lightair_store::input_log::Writer::create(&folder.join("input.bin"), recorder.listening).ok().map(|mut w| {
            use lightair_store::input_log::{Entry, Record};
            // the screen recorded: its display, or its window where it is now (else the main display)
            let screen = workspace.sources.values().find(|s| s.kind == lightair_core::kinds::SCREEN && tracks.iter().any(|t| t.screen && t.source == s.id));
            let frame = screen.and_then(|s| match s.settings["mode"].as_i64().unwrap_or(lightair_core::kinds::DISPLAY) {
                lightair_core::kinds::WINDOW => lightair_platform::input::window_frame(s.settings["window"].to_string().trim_matches('"')),
                _ => s.settings["display"].as_str().and_then(lightair_platform::input::display_frame),
            });
            let (x, y, dw, dh) = frame.unwrap_or_else(lightair_platform::input::main_display);
            let at = std::time::Duration::ZERO;
            let _ = w.write(&[Record { at, entry: Entry::Display { x, y } }, Record { at, entry: Entry::DisplaySize { w: dw, h: dh } }]);
            w
        });
        let title = format!("Studio take · {}", chrono::Local::now().format("%-d %b, %H:%M"));
        let mut item = Item::new(Kind::StudioTake, title, Vec::new(), chrono::Utc::now().timestamp_millis());
        item.files = tracks.iter().map(|t| ItemFile { track: Some(t.name.clone()), ..ItemFile::new(t.file.clone()) }).collect();
        let workspace_dir = self.workspace_dir();
        if let Some(dir) = &workspace_dir {
            item.thumbnail = Some(recordings::thumbnails_dir(dir).join(format!("{}.png", item.id)));
            // written now, so a studio_take cut short is still in the Library
            self.index_add(dir, item.clone());
        }
        self.studio_take = Some(StudioTakeRun {
            workspace: workspace_dir,
            folder,
            item,
            started: std::time::Instant::now(),
            recorder: Some(recorder),
            log,
            tracks,
            audio,
            stopping: false,
            thumbnail_due: Some(std::time::Instant::now() + std::time::Duration::from_secs(3)),
            pauses: Vec::new(),
        });
        cx.notify();
    }

    fn stop_studio_take(&mut self, cx: &mut Context<Self>) {
        let Some(t) = &mut self.studio_take else { return };
        if t.stopping {
            return;
        }
        t.stopping = true;
        if t.paused() {
            let now = t.started.elapsed();
            if let Some(p) = t.pauses.last_mut() {
                p.1 = Some(now);
            }
        }
        t.item.duration_ms = Some(t.elapsed().as_millis() as u64);
        self.engine.post(|s| s.outputs.stop_recording());
        let report = self.engine.call(|s| s.reconciler.stop_multitrack());
        let Some(t) = &mut self.studio_take else { return };
        if let Some(r) = t.recorder.take() {
            let rest = t.records(r.stop());
            if let Some(w) = &mut t.log {
                let _ = w.write(&rest);
                let _ = w.flush();
            }
        }
        // the files written (each with its track's name, from when they started)
        if let Some(r) = &report {
            let named = std::mem::take(&mut t.audio);
            t.audio = r.files.iter().map(|f| (f.clone(), named.iter().find(|(p, _)| p == f).map_or_else(String::new, |(_, n)| n.clone()))).collect();
        }
        let sidecar = serde_json::json!({
            "started_ms": t.item.started_ms,
            "duration_ms": t.item.duration_ms,
            "video": t.tracks.iter().map(|k| serde_json::json!({
                "name": k.name,
                "kind": if k.screen { "screen" } else { "camera" },
                "file": k.file.file_name().map(|n| n.to_string_lossy().into_owned()),
                "input": k.source,
                "width": k.width,
                "height": k.height,
            })).collect::<Vec<_>>(),
            "audio": t.audio.iter().filter_map(|(f, _)| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect::<Vec<_>>(),
            // whose sound each track file is
            "audio_tracks": t.audio.iter().map(|(f, name)| serde_json::json!({
                "file": f.file_name().map(|n| n.to_string_lossy().into_owned()),
                "name": name,
            })).collect::<Vec<_>>(),
            "audio_start_ns": report.as_ref().map(|r| r.start_ns),
            "sample_rate": report.as_ref().map(|r| r.sample_rate),
            "input": "input.bin",
            "markers": t.item.markers,
            // where it was paused, on the files' clock (each a cut in the files)
            "pauses_ms": t.pauses.iter().map(|&(a, _)| (a - t.paused_before(a)).as_millis() as u64).collect::<Vec<_>>(),
        });
        let studio_take_json = t.folder.join("take.json");
        if let Ok(text) = serde_json::to_string_pretty(&sidecar) {
            let _ = std::fs::write(&studio_take_json, text);
        }
        t.item.files.extend(t.audio.iter().map(|(f, _)| ItemFile::new(f.clone())));
        t.item.files.push(ItemFile::new(t.folder.join("input.bin")));
        t.item.files.push(ItemFile::new(studio_take_json));
        cx.notify();
    }

    /// Follows the take: its source log to disk every second, the still, and its Library entry
    /// once its files have stopped.
    fn track_studio_take(&mut self) {
        let Some(t) = &mut self.studio_take else { return };
        if let Some(r) = &t.recorder {
            let records = t.records(r.take());
            if let Some(w) = &mut t.log {
                let _ = w.write(&records);
                let _ = w.flush();
            }
        }
        if t.thumbnail_due.is_some_and(|d| d <= std::time::Instant::now()) {
            t.thumbnail_due = None;
            if let Some(path) = t.item.thumbnail.clone() {
                self.thumbnails.push(ThumbnailRequest { path });
            }
        }
        let Some(t) = &self.studio_take else { return };
        if !t.stopping || !self.engine.call(|s| s.studio_take.as_ref().is_none_or(|t| t.finished())) {
            return;
        }
        // the per-source canvases go once their files have stopped
        self.engine.post(|s| s.studio_take = None);
        let Some(mut t) = self.studio_take.take() else { return };
        if t.thumbnail_due.is_some() {
            t.item.thumbnail = None;
        }
        t.item.files.retain(|f| f.path.exists());
        if let Some(dir) = t.workspace {
            let id = t.item.id;
            self.index_add(&dir, t.item);
            self.last_studio_take = Some((dir, id));
        }
        self.issue = Some(format!("Studio take saved: {}", t.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).into());
    }

    /// Writes an item to a workspace's recordings index (and tells the Library).
    fn index_add(&mut self, workspace: &std::path::Path, item: Item) {
        if let Err(e) = Index::add_to(workspace, item) {
            tracing::warn!(%e, "couldn't write the recordings index");
        }
        self.library_version += 1;
    }

    /// Follows the recording on its way into the Library: split files as they start, the
    /// still, and its entry once every output and the multitrack files are done.
    fn track_recording(&mut self) {
        let Some(p) = &mut self.pending else { return };
        for (canvas, label) in &p.canvases {
            if let Some(path) = runtime().get(OutputKey::Recording(*canvas)).path
                && !p.item.files.iter().any(|f| f.path == path)
            {
                p.item.files.push(ItemFile { canvas: label.clone(), ..ItemFile::new(path) });
            }
        }
        if p.thumbnail_due.is_some_and(|t| t <= std::time::Instant::now()) {
            p.thumbnail_due = None;
            if let Some(path) = p.item.thumbnail.clone() {
                self.thumbnails.push(ThumbnailRequest { path });
            }
        }
        let Some(p) = &self.pending else { return };
        let running = p.canvases.iter().any(|(c, _)| runtime().get(OutputKey::Recording(*c)).state.is_running());
        if !p.stopped || p.multitrack || running {
            return;
        }
        let Some(mut p) = self.pending.take() else { return };
        // a still that never came (stopped within seconds): none
        if p.thumbnail_due.is_some() {
            p.item.thumbnail = None;
        }
        // files that were never written (it failed to start)
        p.item.files.retain(|f| f.path.exists());
        if p.item.files.is_empty() {
            let mut index = Index::load(&p.workspace);
            index.items.retain(|i| i.id != p.item.id);
            let _ = index.save(&p.workspace);
            self.library_version += 1;
            return;
        }
        if p.item.duration_ms.is_none() {
            p.item.duration_ms = p.item.files.iter().find_map(|f| recordings::mp4_duration_ms(&f.path));
        }
        let (dir, item) = (p.workspace, p.item);
        self.index_add(&dir, item);
    }

    pub fn pause_recording(&mut self, cx: &mut Context<Self>) {
        let mut pause = !runtime().recording().paused;
        // a Studio take keeps its own pauses (its input log leaves them out)
        if let Some(t) = &mut self.studio_take
            && !t.stopping
        {
            pause = !t.paused();
            let now = t.started.elapsed();
            if pause {
                t.pauses.push((now, None));
            } else if let Some(p) = t.pauses.last_mut() {
                p.1 = Some(now);
            }
        }
        self.engine.post(move |s| {
            s.outputs.pause_recording(pause);
            s.reconciler.pause_multitrack(pause);
        });
        cx.notify();
    }

    /// Stops the multitrack audio files and writes what they hold beside them
    /// ("<name>.multitrack.json": the files, their rate and length, and when they started).
    fn finish_multitrack(&mut self, cx: &mut Context<Self>) {
        let Some((base, named)) = self.multitrack_files.take() else { return };
        let Some(report) = self.engine.call(|s| s.reconciler.stop_multitrack()) else {
            self.issue = Some("Multitrack audio couldn't be finished.".into());
            cx.notify();
            return;
        };
        let names: Vec<String> =
            report.files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
        let tracks: Vec<serde_json::Value> = report
            .files
            .iter()
            .map(|f| serde_json::json!({
                "file": f.file_name().map(|n| n.to_string_lossy().into_owned()),
                "name": named.iter().find(|(p, _)| p == f).map(|(_, n)| n.clone()),
            }))
            .collect();
        let sidecar = serde_json::json!({
            "files": names,
            // whose sound each file is
            "tracks": tracks,
            "sample_rate": report.sample_rate,
            "frames": report.frames,
            "seconds": report.frames as f64 / report.sample_rate as f64,
            "start_ns": report.start_ns,
        });
        let path = self.recording_folder().join(format!("{base}.multitrack.json"));
        if let Ok(text) = serde_json::to_string_pretty(&sidecar) {
            let _ = std::fs::write(&path, text);
        }
        if let Some(p) = &mut self.pending {
            p.item.files.extend(report.files.iter().map(|f| ItemFile::new(f.clone())));
            p.item.files.push(ItemFile::new(path));
            if p.item.kind == Kind::Multitrack {
                p.item.duration_ms = Some(report.frames * 1000 / report.sample_rate.max(1) as u64);
            }
            p.multitrack = false;
            p.stopped = true;
        }
        self.issue = Some(format!("Recorded {} audio files", report.files.len()).into());
        cx.notify();
    }

    pub fn add_marker(&mut self, cx: &mut Context<Self>) {
        let n = runtime().recording().markers + 1;
        let ok = self.engine.call(move |s| s.outputs.add_marker(&format!("Marker {n}")));
        let status = runtime().recording();
        if ok
            && let Some(p) = &mut self.pending
            && let OutputState::Active { since } = status.state
        {
            let at_ms = status.recorded(since).as_millis() as u64;
            p.item.markers.push(Marker { at_ms, label: format!("Marker {n}") });
        }
        if ok && let Some(t) = &mut self.studio_take {
            let at_ms = t.elapsed().as_millis() as u64;
            t.item.markers.push(Marker { at_ms, label: format!("Marker {n}") });
        }
        self.issue = Some(if ok {
            format!("Marker {n} added").into()
        } else {
            "Markers need a recording in MP4 or MOV.".into()
        });
        cx.notify();
    }

    pub fn split_recording(&mut self, cx: &mut Context<Self>) {
        if self.engine.call(|s| s.outputs.split_recording()) {
            self.issue = Some("Continuing in a new file".into());
        }
        cx.notify();
    }

    /// Starts every enabled destination.
    pub fn go_live(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace().clone();
        let enabled: Vec<_> = workspace.outputs.destinations.iter().filter(|d| d.enabled).cloned().collect();
        if enabled.is_empty() {
            self.issue = Some("Add a destination in Outputs to go live.".into());
            cx.notify();
            return;
        }
        self.flush();
        // Integrated loudness covers this show.
        lightair_engine::loudness::reset_stream_loudness();
        for d in enabled {
            let Some(key) = crate::secrets::stream_key(d.id) else {
                self.issue = Some(format!("No stream key saved for {}.", d.name).into());
                continue;
            };
            let reconnect = workspace.outputs.reconnect;
            let cfg = StreamConfig {
                id: d.id,
                server: d.server.clone(),
                key,
                encoders: crate::encoding::for_stream(&self.engine.capabilities.encoders, &workspace, &d),
                reconnect: Some((reconnect.retries, reconnect.delay_s)),
                // Twitch's VOD track
                vod_mix: d
                    .vod_mix
                    .filter(|_| crate::platforms::takes_vod_track(&d))
                    .and_then(|m| crate::encoding::libobs_mix(&workspace, m)),
            };
            let canvas = d.canvas;
            self.engine.post(move |s| {
                let video = canvas.and_then(|f| s.reconciler.canvas_video(f));
                if let Err(e) = s.outputs.start_stream(&cfg, video) {
                    tracing::error!(%e, "stream failed to start");
                }
            });
        }
        cx.notify();
    }

    pub fn end_stream(&mut self, cx: &mut Context<Self>) {
        self.engine.post(|s| s.outputs.stop_all_streams());
        cx.notify();
    }

    /// Surfaces output failures as issues (called on the stats tick).
    fn check_outputs(&mut self) {
        let mut keys = vec![OutputKey::Clips, OutputKey::VirtualCamera];
        keys.extend(self.workspace().canvases.iter().map(|f| OutputKey::Recording(f.id)));
        keys.extend(self.workspace().outputs.destinations.iter().map(|d| OutputKey::Stream(d.id)));
        for key in keys {
            if let OutputState::Failed(msg) = runtime().get(key).state {
                let label = match key {
                    OutputKey::Recording(canvas) => match self.workspace().canvas(canvas) {
                        Some(f) if self.workspace().canvases.len() > 1 => format!("Recording ({})", f.name),
                        _ => "Recording".to_string(),
                    },
                    OutputKey::Clips => "Clips".to_string(),
                    OutputKey::VirtualCamera => "Virtual camera".to_string(),
                    OutputKey::Stream(id) => self
                        .workspace()
                        .outputs
                        .destinations
                        .iter()
                        .find(|d| d.id == id)
                        .map(|d| d.name.clone())
                        .unwrap_or_default(),
                };
                let text = SharedString::from(format!("{label}: {msg}"));
                if self.issue.as_ref() != Some(&text) {
                    self.issue = Some(text);
                }
            }
        }
    }

    pub fn refresh_stats(&mut self) {
        self.engine.post(|s| {
            s.outputs.sample();
            s.reconciler.cleanup();
        });
        self.check_outputs();
        self.announce_clips();
        self.track_recording();
        self.track_studio_take();
        let stats = lightair_engine::read_stats();
        self.recent_lagged = stats.lagged_frames.saturating_sub(self.stats.lagged_frames);
        self.stats = stats;
        self.sample_mics();
    }

    fn mic_tracks(&self) -> Vec<(Id, String)> {
        let p = self.workspace();
        p.tracks
            .iter()
            .filter(|t| p.sources.get(&t.source).is_some_and(|i| lightair_core::kinds::is_mic(&i.kind)))
            .map(|t| (t.id, t.name.clone()))
            .collect()
    }

    fn sample_mics(&mut self) {
        let mics = self.mic_tracks();
        self.mic_history.retain(|id, _| mics.iter().any(|(m, _)| m == id));
        for (id, _) in mics {
            let Some(cell) = lightair_engine::meters::meters().get(id) else { continue };
            let h = self.mic_history.entry(id).or_default();
            h.push_back(cell.take_window_peak());
            while h.len() > 10 {
                h.pop_front();
            }
        }
    }

    /// ⇧⌘M: mutes every microphone, or unmutes them all if they're all muted already.
    pub fn toggle_mics(&mut self, cx: &mut Context<Self>) {
        let mics = self.mic_tracks();
        if mics.is_empty() {
            return;
        }
        let p = self.workspace().clone();
        let all_muted = mics.iter().all(|(id, _)| p.tracks.iter().any(|t| t.id == *id && t.muted));
        let commands = mics.iter().map(|(id, _)| Command::SetTrackMuted { track: *id, muted: !all_muted }).collect();
        let label = if all_muted { "Unmute microphones" } else { "Mute microphones" };
        if self.execute(Command::Batch { label: label.into(), commands }, cx).is_ok() {
            self.issue = Some(if all_muted { "Microphones on" } else { "Microphones muted" }.into());
        }
    }

    /// What the Go Live review checks. `has_key` is looked up once by the caller (Keychain).
    pub fn preflight_facts(&self, has_key: &HashMap<Id, bool>, record_too: bool) -> crate::preflight::Facts {
        let p = self.workspace();
        let enabled: Vec<_> = p.outputs.destinations.iter().filter(|d| d.enabled).collect();
        let encoders = default_encoders(&self.engine.capabilities.encoders, p.outputs.stream_bitrate_kbps, true);
        let encoder_ok = self.engine.capabilities.encoders.contains(&encoders.video_encoder);
        // Identical settings share an encoder: one per distinct canvas.
        let mut canvases: Vec<_> = enabled.iter().map(|d| d.canvas).collect();
        canvases.sort_by_key(|f| f.map(|id| id.to_string()));
        canvases.dedup();
        let target_fps = p.primary_canvas().fps_num as f64 / p.primary_canvas().fps_den.max(1) as f64;
        crate::preflight::Facts {
            destinations: p
                .outputs
                .destinations
                .iter()
                .map(|d| crate::preflight::Destination {
                    name: d.name.clone(),
                    enabled: d.enabled,
                    has_key: has_key.get(&d.id).copied().unwrap_or(false),
                })
                .collect(),
            encoder: encoder_ok.then(|| match encoders.video_encoder.as_str() {
                "obs_x264" => "x264 (software)".to_string(),
                id => format!("{} H.264", lightair_engine::outputs::encoder_maker(id)),
            }),
            encoders: canvases.len() as u32,
            video_kbps: encoders.video_bitrate_kbps,
            audio_kbps: encoders.audio_bitrate_kbps,
            lagged_recent: self.recent_lagged,
            fps: self.stats.active_fps,
            target_fps,
            mics: self
                .mic_tracks()
                .into_iter()
                .filter_map(|(id, name)| self.mic_history.get(&id).map(|h| (name, h.iter().copied().collect())))
                .collect(),
            record_too,
            free_gb: free_gb(&self.recording_folder()),
        }
    }

    fn push(&self, before: Arc<Workspace>, after: Arc<Workspace>) {
        let patches = diff(&before, &after);
        if patches.is_empty() {
            return;
        }
        self.engine.post(move |s| s.reconciler.apply(&after, &patches));
    }

    fn rebuild_engine(&self) {
        let workspace = self.doc.workspace().clone();
        let empty = Arc::new(workspace.empty_like());
        self.push(empty, workspace);
    }

    /// Every selected object, the Inspector's first.
    pub fn selection(&self) -> Vec<Id> {
        self.selected_object.into_iter().chain(self.also_selected.iter().copied()).collect()
    }

    /// Selects a scene for editing: the Inspector shows the scene (no object or mixer strip).
    pub fn select_scene(&mut self, scene: Id) {
        self.selected_scene = Some(scene);
        self.selected_object = None;
        self.also_selected.clear();
        self.selected_track = None;
    }

    /// Selects a mixer track for the Inspector (and clears the object selection).
    pub fn select_track(&mut self, track: Option<Id>) {
        self.selected_track = track;
        if track.is_some() {
            self.selected_object = None;
            self.also_selected.clear();
        }
    }

    /// ⇧-click: add a object to the selection, or studio_take it out.
    /// Adds a object to the selection (kept if it's there).
    pub fn add_selected(&mut self, object: Id) {
        if self.selected_object == Some(object) || self.also_selected.contains(&object) {
            return;
        }
        if self.selected_object.is_none() {
            self.selected_object = Some(object);
        } else {
            self.also_selected.push(object);
        }
    }

    pub fn toggle_selected(&mut self, object: Id) {
        if self.selected_object == Some(object) {
            self.selected_object = (!self.also_selected.is_empty()).then(|| self.also_selected.remove(0));
        } else if let Some(i) = self.also_selected.iter().position(|l| *l == object) {
            self.also_selected.remove(i);
        } else if self.selected_object.is_none() {
            self.selected_object = Some(object);
        } else {
            self.also_selected.push(object);
        }
    }

    fn fix_selection(&mut self) {
        let p = self.doc.workspace();
        // a mixer strip: a track, bus, mix or the master
        if self.selected_track.is_some_and(|t| lightair_core::MixerStrip::find(p, t).is_none()) {
            self.selected_track = None;
        }
        if self.selected_scene.is_none_or(|s| p.scene(s).is_none()) {
            self.selected_scene = p.program.or(p.scenes.first().map(|s| s.id));
        }
        if let (Some(scene), Some(object)) = (self.selected_scene, self.selected_object)
            && p.scene(scene).is_none_or(|s| s.object(object).is_none())
        {
            self.selected_object = self.also_selected.first().copied();
        }
        let scene = self.selected_scene.and_then(|s| p.scene(s));
        let primary = self.selected_object;
        self.also_selected.retain(|l| Some(*l) != primary && scene.is_some_and(|s| s.object(*l).is_some()));
        if self.selected_object.is_none() {
            self.also_selected.clear();
        }
    }
}

/// Free space on the volume holding `path` (or its nearest existing parent), in GB.
fn free_gb(path: &std::path::Path) -> Option<f64> {
    lightair_platform::files::free_bytes(path).map(|b| b as f64 / 1e9)
}

/// The virtual cameras Lightair sends to, by their device ids, and what apps call them: its own
/// (modules/lightair-virtual-camera), else OBS's (as lightair-effect-stack's virtualcam.m picks).
const VIRTUAL_CAMERAS: [&str; 1] = ["2DF109AC-DE07-42DF-A0D7-098033FFE986"];
pub const VIRTUAL_CAMERA_NAMES: [&str; 2] = ["Lightair Virtual Camera", "OBS Virtual Camera"];

fn virtual_camera_name() -> &'static str {
    // Windows: Lightair's camera (Windows 11) where it's installed, else OBS's
    if cfg!(windows) {
        return lightair_engine::vcam::camera().unwrap_or(lightair_engine::vcam::Camera::Lightair).name();
    }
    let ours = lightair_engine::kinds::cameras().iter().any(|(_, uid)| VIRTUAL_CAMERAS.iter().any(|v| uid.eq_ignore_ascii_case(v)));
    VIRTUAL_CAMERA_NAMES[usize::from(!ours)]
}

/// `command`, and when it removes the last object showing a web page, the page's mixer track
/// and the page too (one undo step): a page nobody sees shouldn't keep playing on air.
fn with_page_cleanup(before: &lightair_core::Workspace, command: Command) -> Command {
    fn removes(c: &Command) -> bool {
        match c {
            Command::RemoveObject { .. } | Command::RemoveScene { .. } => true,
            Command::Batch { commands, .. } => commands.iter().any(removes),
            _ => false,
        }
    }
    if !removes(&command) {
        return command;
    }
    let mut after = before.clone();
    if command.apply(&mut after).is_err() {
        return command;
    }
    let mut extra = vec![];
    for (id, source) in &before.sources {
        let gone = before.objects_using(*id).next().is_some() && after.objects_using(*id).next().is_none();
        if source.kind != lightair_engine::web::KIND || !gone || !after.sources.contains_key(id) {
            continue;
        }
        extra.extend(after.tracks.iter().filter(|t| t.source == *id).map(|t| Command::RemoveTrack { track: t.id }));
        extra.push(Command::RemoveSource { source: *id });
    }
    if extra.is_empty() {
        return command;
    }
    let label = command.label();
    let mut commands = vec![command];
    commands.extend(extra);
    Command::Batch { label, commands }
}

#[cfg(test)]
mod page_cleanup {
    use super::*;
    use lightair_core::{Document, Layout, Object, Scene, Source, Track, Workspace};

    /// Deleting the only object showing a web page takes its track and the page with it (one
    /// undo step); a page still shown elsewhere, or a microphone's track, stays.
    #[test]
    fn deleting_a_pages_last_object_removes_its_track() {
        let mut doc = Document::new(Workspace::new("Pages"));
        let f = doc.workspace().primary_canvas().id;
        let page = Source::new("Page", lightair_engine::web::KIND, serde_json::json!({"url": "https://example.com"}));
        let (pid, track) = (page.id, Track::new("Page", page.id));
        let mut a = Scene::new("A");
        let mut b = Scene::new("B");
        let (oa, ob) = (Object::new("Page", pid, [(f, Layout::boxed(0.0, 0.0, 10.0, 10.0))]), Object::new("Page", pid, [(f, Layout::boxed(0.0, 0.0, 10.0, 10.0))]));
        let (oa_id, ob_id, a_id, b_id) = (oa.id, ob.id, a.id, b.id);
        a.objects.push(oa);
        b.objects.push(ob);
        for cmd in [
            Command::AddSource { source: page },
            Command::AddScene { scene: a, index: None },
            Command::AddScene { scene: b, index: None },
            Command::AddTrack { track, index: None },
        ] {
            doc.apply(&cmd).unwrap();
        }
        // still shown in B: nothing more
        let first = with_page_cleanup(doc.workspace(), Command::RemoveObject { scene: a_id, object: oa_id });
        assert!(matches!(first, Command::RemoveObject { .. }));
        doc.apply(&first).unwrap();
        // the last one: the track and the page go too
        let last = with_page_cleanup(doc.workspace(), Command::RemoveObject { scene: b_id, object: ob_id });
        doc.apply(&last).unwrap();
        assert!(doc.workspace().tracks.is_empty(), "its track is gone");
        assert!(!doc.workspace().sources.contains_key(&pid), "the page is gone");
        doc.undo().unwrap();
        assert_eq!(doc.workspace().tracks.len(), 1, "one undo brings both back");
        assert!(doc.workspace().sources.contains_key(&pid));
    }
}
