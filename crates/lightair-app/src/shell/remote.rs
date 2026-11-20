//! The remote API's methods (docs/remote-api.md): `lightair-remote` carries calls from
//! obs-websocket, JSON-RPC and MCP clients; they're answered here, on the main thread, against
//! the Studio, as the UI would do them (commands, so every change is undoable and labelled with
//! who made it). Changes to the Studio go back out as events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use gpui_kit::*;
use lightair_model::{Command, Id, Layout, Object, Scene, Source, Track, Workspace};
use lightair_engine::outputs::{OutputKey, OutputState, runtime};
use lightair_preview::Preview;
use lightair_remote::{ApiError, Call, Config, ErrorKind, Handler, Reply, Server};
use serde_json::{Value, json};

use super::StudioWindow;

type Job = (Call, std::sync::mpsc::Sender<Reply>);
/// Where calls go to be answered on the main thread.
pub(super) type JobSender = futures_channel::mpsc::UnboundedSender<Job>;

/// How long a client waits for the app to answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);

/// The server, and what the window keeps for it.
pub(crate) struct Remote {
    server: Option<Server>,
    /// Why it isn't running when it should be (the port taken, …).
    pub error: Option<String>,
    jobs: futures_channel::mpsc::UnboundedSender<Job>,
    /// What clients were last told (`None`: no clients to tell).
    told: Option<Told>,
    /// A preview kept for screenshots (dropped when they stop).
    shot: Option<Shot>,
    /// Where `cpu_percent` was last sampled from.
    cpu: Option<(Instant, f64)>,
    /// Settings shows the password.
    pub show_password: bool,
    /// The password from the Keychain, once read (`Some(None)`: it gave none). Read off the
    /// main thread: the first read can wait on a Keychain prompt (another build of Lightair
    /// saved it), which mustn't freeze the window.
    pub password: Option<Option<String>>,
    reading_password: bool,
    /// Lightair Live on phones: the TLS port, pairing and mDNS (phones.rs).
    pub(super) phones: super::phones::Phones,
}

struct Shot {
    preview: Preview,
    scene: Option<Id>,
    used: Instant,
}

impl Remote {
    /// A way in for the app's own callers (automations).
    pub(super) fn jobs(&self) -> JobSender {
        self.jobs.clone()
    }

    pub fn running(&self) -> Option<&Server> {
        self.server.as_ref()
    }

    /// Stops the server (to start again with new settings or a new password).
    pub fn restart(&mut self, cx: &mut Context<StudioWindow>) {
        self.server = None;
        cx.notify();
    }
}

/// The state events are about.
#[derive(Clone, PartialEq, Default)]
struct Told {
    program: Option<Id>,
    scenes: Vec<(Id, String)>,
    recording: (bool, bool),
    streaming: bool,
    vcam: bool,
    clips_saved: u32,
    /// Track: (source, volume, muted).
    tracks: HashMap<Id, (Id, f32, bool)>,
    /// (scene, object): visible in the primary canvas.
    visible: HashMap<(Id, Id), bool>,
    workspace: usize,
}

fn err(kind: ErrorKind, m: impl Into<String>) -> ApiError {
    ApiError::new(kind, m)
}

fn id_of(s: &str) -> Option<Id> {
    serde_json::from_value(json!(s)).ok()
}

fn text<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn need<'a>(p: &'a Value, k: &str) -> Result<&'a str, ApiError> {
    text(p, k).ok_or_else(|| ApiError::invalid(format!("`{k}` is needed")))
}

fn num(p: &Value, k: &str) -> Option<f32> {
    p.get(k).and_then(Value::as_f64).map(|v| v as f32)
}

/// By id, by name, then by name in any case.
fn pick<'a, T>(items: impl Iterator<Item = &'a T> + Clone, key: &str, id: impl Fn(&T) -> Id, name: impl Fn(&T) -> &str) -> Option<&'a T>
where
    T: 'a,
{
    let as_id = id_of(key);
    let mut all = items.clone();
    if let Some(i) = as_id
        && let Some(t) = all.find(|t| id(t) == i)
    {
        return Some(t);
    }
    items.clone().find(|t| name(t) == key).or_else(|| items.clone().find(|t| name(t).eq_ignore_ascii_case(key)))
}

fn scene_by<'a>(w: &'a Workspace, key: &str) -> Result<&'a Arc<Scene>, ApiError> {
    pick(w.scenes.iter(), key, |s| s.id, |s| s.name.as_str()).ok_or_else(|| ApiError::not_found(format!("no scene {key}")))
}

fn source_by<'a>(w: &'a Workspace, key: &str) -> Result<&'a Arc<Source>, ApiError> {
    pick(w.sources.values(), key, |s| s.id, |s| s.name.as_str()).ok_or_else(|| ApiError::not_found(format!("no source {key}")))
}

fn object_by<'a>(scene: &'a Scene, key: &str) -> Result<&'a Object, ApiError> {
    pick(scene.objects.iter(), key, |o| o.id, |o| o.name.as_str())
        .ok_or_else(|| ApiError::not_found(format!("no object {key} in {}", scene.name)))
}

/// A track by its id or name, or by its source's.
fn track_by<'a>(w: &'a Workspace, key: &str) -> Result<&'a Arc<Track>, ApiError> {
    if let Some(t) = pick(w.tracks.iter(), key, |t| t.id, |t| t.name.as_str()) {
        return Ok(t);
    }
    let source = source_by(w, key).map_err(|_| ApiError::not_found(format!("no track {key}")))?;
    w.tracks.iter().find(|t| t.source == source.id).ok_or_else(|| ApiError::not_found(format!("{key} has no sound")))
}

fn scene_json(w: &Workspace, s: &Scene, canvas: Id) -> Value {
    let objects: Vec<Value> = s
        .objects
        .iter()
        .map(|o| {
            let l = o.layouts.get(&canvas).cloned().unwrap_or_else(|| Layout::boxed(0.0, 0.0, 0.0, 0.0));
            let kind = o.source.and_then(|id| w.sources.get(&id)).map_or("group", |src| src.kind.as_str());
            json!({
                "id": o.id, "name": o.name, "kind": kind, "source": o.source, "parent": o.parent,
                "visible": l.visible, "locked": o.locked,
                "x": l.x, "y": l.y, "width": l.width, "height": l.height,
                "rotation": l.rotation, "opacity": l.opacity,
            })
        })
        .collect();
    json!({"id": s.id, "name": s.name, "on_air": w.program == Some(s.id), "objects": objects})
}

fn output_json(key: Option<OutputKey>) -> Value {
    let status = match key {
        Some(k) => runtime().get(k),
        None => runtime().recording(),
    };
    let since = match status.state {
        OutputState::Active { since } | OutputState::Reconnecting { since } => Some(since),
        _ => None,
    };
    let state = match &status.state {
        OutputState::Idle => "idle",
        OutputState::Starting => "starting",
        OutputState::Active { .. } => "active",
        OutputState::Reconnecting { .. } => "reconnecting",
        OutputState::Stopping => "stopping",
        OutputState::Failed(_) => "failed",
    };
    let mut v = json!({
        "active": status.state.is_running(),
        "state": state,
        "seconds": since.map_or(0.0, |s| status.recorded(s).as_secs_f64()),
    });
    if let OutputState::Failed(m) = &status.state {
        v["error"] = m.clone().into();
    }
    v
}

impl StudioWindow {
    /// Makes the remote API's state and the task answering its calls.
    pub(super) fn new_remote(window: &mut Window, cx: &mut Context<Self>) -> Remote {
        let (jobs, mut queue) = futures_channel::mpsc::unbounded::<Job>();
        cx.spawn_in(window, async move |this, cx| {
            use futures_util::StreamExt;
            while let Some((call, reply)) = queue.next().await {
                if call.method == "screenshot" {
                    let answer = Self::screenshot(this.clone(), call.params, cx).await;
                    let _ = reply.send(answer);
                    continue;
                }
                // phones' preview frames and scene thumbnails (phones.rs)
                if matches!(call.method.as_str(), "preview.frame" | "scene.thumbnails") {
                    let answer = Self::phone_picture(this.clone(), call, cx).await;
                    let _ = reply.send(answer);
                    continue;
                }
                let answer = this
                    .update(cx, |view, cx| view.remote_call(&call, cx))
                    .unwrap_or_else(|_| Err(ApiError::failed("Lightair's window is closed")));
                let _ = reply.send(answer);
            }
        })
        .detach();
        let cpu = Some((Instant::now(), process_usage().0));
        let phones = Self::new_phones(window, cx);
        Remote { server: None, error: None, jobs, told: None, shot: None, cpu, show_password: false, password: None, reading_password: false, phones }
    }

    /// Starts, restarts or stops the server to match the preferences.
    /// Reads the remote API's password from the Keychain (or makes a new one, `new`) on a
    /// background thread, then starts the server with it.
    pub(super) fn read_remote_password(&mut self, new: bool, cx: &mut Context<Self>) {
        if self.remote.reading_password {
            return;
        }
        self.remote.reading_password = true;
        cx.spawn(async move |this, cx| {
            let password = cx
                .background_executor()
                .spawn(async move { if new { crate::secrets::new_remote_password() } else { crate::secrets::remote_password() } })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.remote.reading_password = false;
                this.remote.password = Some(password);
                if new {
                    this.remote.restart(cx);
                }
                this.sync_remote(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn sync_remote(&mut self, cx: &mut Context<Self>) {
        let p = self.prefs.remote;
        // a test copy keeps off the running app's port
        let port = std::env::var("LIGHTAIR_REMOTE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(p.port);
        let needs = p.enabled && (p.require_password || p.lan) && std::env::var_os("LIGHTAIR_REMOTE_OFF").is_none();
        let password = if needs {
            match self.remote.password.clone().or_else(crate::secrets::early_remote_password) {
                Some(p) => {
                    self.remote.password = Some(p.clone());
                    p
                }
                None => {
                    // started once the Keychain answers
                    self.read_remote_password(false, cx);
                    return;
                }
            }
        } else {
            None
        };
        if needs && password.is_none() {
            self.remote.server = None;
            self.remote.error = Some(format!("The {} didn't give Lightair a password for the remote API.", crate::ui::KEYCHAIN));
            return;
        }
        let config = Config { port, lan: p.lan, password };
        if !p.enabled || std::env::var_os("LIGHTAIR_REMOTE_OFF").is_some() {
            self.remote.server = None;
            self.remote.error = None;
            return;
        }
        if self.remote.server.as_ref().is_some_and(|s| *s.config() == config) {
            return;
        }
        // the old one lets go of the port first
        self.remote.server = None;
        let jobs = self.remote.jobs.clone();
        let engine = self.studio.read(cx).engine().clone();
        let chat = self.studio.read(cx).chat.clone();
        let handler: Handler = Arc::new(move |call: Call| {
            // sending takes a moment: on the server's thread, not the main one
            if call.method == "chat.send" {
                return crate::chat_send::send(&chat, &call.params).map_err(|e| ApiError::new(ErrorKind::State, e));
            }
            // the platforms answer slowly: on the server's thread (phones.rs)
            if call.method == "stream.info.get" {
                return super::phones::stream_info(&chat, &call.params);
            }
            // phones' name for `stream.edit`
            let method = if call.method == "stream.info.set" { "stream.edit" } else { call.method.as_str() };
            if let Some(out) = lightair_live::integrations::call(method, &call.params) {
                return out.map_err(|e| ApiError::new(ErrorKind::State, e));
            }
            // asking the system for windows and devices takes a moment: not on the main thread
            if call.method == "capture.targets" {
                return capture_targets(&call.params, &engine);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            if jobs.unbounded_send((call, tx)).is_err() {
                return Err(ApiError::failed("Lightair is closing"));
            }
            rx.recv_timeout(ANSWER_WITHIN).unwrap_or_else(|_| Err(ApiError::failed("Lightair didn't answer in time")))
        });
        // the phones' port belongs to the server it shares a handler with
        self.remote.phones.stop_listening();
        match Server::start(config, handler) {
            Ok(s) => {
                self.remote.server = Some(s);
                self.remote.error = None;
                self.sync_phones(cx);
            }
            Err(e) => {
                tracing::warn!(%e, port, "remote API couldn't start");
                self.remote.error = Some(if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("Port {port} is in use (OBS or another app?). Choose another port.")
                } else {
                    e.to_string()
                });
            }
        }
        cx.notify();
    }

    /// Tells clients what changed in the Studio since they were last told.
    pub(super) fn tell_remote(&mut self, cx: &mut Context<Self>) {
        // mDNS says whether this desktop is live
        self.sync_phones(cx);
        let Some(server) = &self.remote.server else { return };
        if server.clients().is_empty() {
            self.remote.told = None;
            return;
        }
        let studio = self.studio.read(cx);
        let w = studio.workspace().clone();
        let canvas = w.primary_canvas().id;
        let rec = runtime().recording();
        let now = Told {
            program: w.program,
            scenes: w.scenes.iter().map(|s| (s.id, s.name.clone())).collect(),
            recording: (rec.state.is_running(), rec.paused),
            streaming: runtime().any_stream_running(),
            vcam: studio.virtual_camera().is_running(),
            clips_saved: runtime().get(OutputKey::Clips).saved_clips,
            tracks: w.tracks.iter().map(|t| (t.id, (t.source, t.volume_db, t.muted))).collect(),
            visible: w
                .scenes
                .iter()
                .flat_map(|s| s.objects.iter().map(move |o| ((s.id, o.id), o.layouts.get(&canvas).is_some_and(|l| l.visible))))
                .collect(),
            workspace: Arc::as_ptr(&w) as usize,
        };
        let Some(before) = self.remote.told.replace(now.clone()) else { return };
        if before == now {
            return;
        }
        let name_of = |id: Id| w.scene(id).map(|s| s.name.clone());
        if before.program != now.program
            && let Some(p) = now.program
        {
            server.emit("scene.program", json!({"id": p, "name": name_of(p)}));
        }
        if before.scenes != now.scenes {
            let scenes: Vec<Value> = now.scenes.iter().map(|(id, name)| json!({"id": id, "name": name})).collect();
            server.emit("scene.list", json!({"scenes": scenes}));
        }
        if before.recording != now.recording {
            let path = rec.path.as_ref().map(|p| p.to_string_lossy().into_owned());
            server.emit("record.state", json!({"active": now.recording.0, "paused": now.recording.1, "path": path}));
        }
        if before.streaming != now.streaming {
            server.emit("stream.state", json!({"active": now.streaming}));
        }
        if before.vcam != now.vcam {
            server.emit("vcam.state", json!({"active": now.vcam}));
        }
        if now.clips_saved > before.clips_saved {
            let path = runtime().get(OutputKey::Clips).path.map(|p| p.to_string_lossy().into_owned());
            server.emit("clip.saved", json!({"path": path}));
        }
        for (track, (source, volume, muted)) in &now.tracks {
            let Some((_, v0, m0)) = before.tracks.get(track) else { continue };
            let source_name = w.sources.get(source).map(|s| s.name.clone());
            if m0 != muted {
                server.emit("audio.mute", json!({"track": track, "source": source, "source_name": source_name, "muted": muted}));
            }
            if (v0 - volume).abs() > 0.001 {
                server.emit("audio.volume", json!({"track": track, "source": source, "source_name": source_name, "volume_db": volume}));
            }
        }
        for ((scene, object), visible) in &now.visible {
            if before.visible.get(&(*scene, *object)).is_some_and(|v| v != visible) {
                server.emit(
                    "object.visible",
                    json!({"scene": scene, "scene_name": name_of(*scene), "object": object, "visible": visible}),
                );
            }
        }
        if before.workspace != now.workspace {
            server.emit("workspace.changed", json!({}));
        }
    }

    /// Runs commands as one undoable change named for who asked.
    pub(super) fn remote_exec(&mut self, client: &str, label: &str, commands: Vec<Command>, cx: &mut Context<Self>) -> Result<(), ApiError> {
        if commands.is_empty() {
            return Ok(());
        }
        // one step, named for who made it ("MCP: Add Text")
        let cmd = Command::Batch { label: format!("{client}: {label}"), commands };
        self.studio.update(cx, |s, cx| s.execute(cmd, cx)).map_err(|e| ApiError::invalid(e.to_string()))
    }

    /// The scene a call is about: `scene`, or the one being edited, or the program.
    fn scene_param(&self, p: &Value, cx: &App) -> Result<Arc<Scene>, ApiError> {
        let studio = self.studio.read(cx);
        let w = studio.workspace();
        match text(p, "scene") {
            Some(key) => scene_by(w, key).cloned(),
            None => studio
                .selected_scene
                .or(w.program)
                .and_then(|id| w.scene(id).cloned())
                .ok_or_else(|| ApiError::not_found("the workspace has no scenes")),
        }
    }

    pub(super) fn remote_call(&mut self, call: &Call, cx: &mut Context<Self>) -> Reply {
        // what this call changes is told against how things were before it
        self.tell_remote(cx);
        if let Some(reply) = self.phone_call(call, cx) {
            return reply;
        }
        if let Some(reply) = self.live_call(call, cx) {
            return reply;
        }
        let p = &call.params;
        let who = call.client;
        let w = self.studio.read(cx).workspace().clone();
        let canvas = match text(p, "canvas") {
            Some(c) => pick(w.canvases.iter(), c, |c| c.id, |c| c.name.as_str())
                .map(|c| c.id)
                .ok_or_else(|| ApiError::not_found(format!("no canvas {c}")))?,
            None => w.primary_canvas().id,
        };
        let ok = || Ok(json!({}));
        match call.method.as_str() {
            "app.version" => Ok(json!({"name": "Lightair", "version": env!("CARGO_PKG_VERSION"), "workspace": w.name})),
            "state.get" => Ok(self.state_json(cx)),
            "workspace.get" => serde_json::to_value(&*w).map_err(|e| ApiError::failed(e.to_string())),
            "stats.get" => Ok(self.stats_json(cx)),

            "scene.switch" => {
                let scene = scene_by(&w, need(p, "scene")?)?.clone();
                let cue = self.cue_scenes;
                self.studio.update(cx, |s, _| {
                    if !cue {
                        s.select_scene(scene.id);
                    }
                });
                if w.program != Some(scene.id) {
                    self.remote_exec(who, &format!("Switch to {}", scene.name), vec![Command::SetProgram { scene: Some(scene.id) }], cx)?;
                }
                Ok(json!({"id": scene.id, "name": scene.name}))
            }
            "scene.create" => {
                let name = need(p, "name")?;
                let scene = Scene::new(name);
                let id = scene.id;
                let mut cmds = vec![Command::AddScene { scene, index: None }];
                if p.get("switch").and_then(Value::as_bool) == Some(true) {
                    cmds.push(Command::SetProgram { scene: Some(id) });
                }
                self.remote_exec(who, &format!("Add scene {name}"), cmds, cx)?;
                Ok(json!({"id": id, "name": name}))
            }
            "scene.rename" => {
                let scene = scene_by(&w, need(p, "scene")?)?;
                let name = need(p, "name")?.to_string();
                self.remote_exec(who, "Rename scene", vec![Command::RenameScene { scene: scene.id, name }], cx)?;
                ok()
            }
            "scene.remove" => {
                let scene = scene_by(&w, need(p, "scene")?)?;
                if w.scenes.len() == 1 {
                    return Err(ApiError::state("a workspace keeps at least one scene"));
                }
                self.remote_exec(who, &format!("Remove {}", scene.name), vec![Command::RemoveScene { scene: scene.id }], cx)?;
                ok()
            }

            "object.add" => self.remote_add(who, p, cx),
            "object.update" => {
                let scene = self.scene_param(p, cx)?;
                let o = object_by(&scene, need(p, "object")?)?.clone();
                let mut cmds = vec![];
                if let Some(name) = text(p, "name") {
                    cmds.push(Command::RenameObject { scene: scene.id, object: o.id, name: name.to_string() });
                }
                let moved = ["x", "y", "width", "height", "rotation", "opacity"].iter().any(|k| p.get(*k).is_some());
                if moved {
                    let mut l = o.layouts.get(&canvas).cloned().unwrap_or_else(|| Layout::boxed(0.0, 0.0, 100.0, 100.0));
                    l.x = num(p, "x").unwrap_or(l.x);
                    l.y = num(p, "y").unwrap_or(l.y);
                    l.width = num(p, "width").unwrap_or(l.width).max(1.0);
                    l.height = num(p, "height").unwrap_or(l.height).max(1.0);
                    l.rotation = num(p, "rotation").unwrap_or(l.rotation);
                    l.opacity = num(p, "opacity").unwrap_or(l.opacity).clamp(0.0, 1.0);
                    cmds.push(Command::SetLayout { scene: scene.id, object: o.id, canvas, layout: l });
                }
                if let Some(visible) = p.get("visible").and_then(Value::as_bool) {
                    // everywhere it's laid out, unless a canvas is named
                    let canvases: Vec<Id> = if text(p, "canvas").is_some() { vec![canvas] } else { o.layouts.keys().copied().collect() };
                    for c in canvases {
                        cmds.push(Command::SetObjectVisible { scene: scene.id, object: o.id, canvas: c, visible });
                    }
                }
                if let Some(locked) = p.get("locked").and_then(Value::as_bool) {
                    cmds.push(Command::SetObjectLocked { scene: scene.id, object: o.id, locked });
                }
                if let Some(settings) = p.get("settings").filter(|s| s.is_object()) {
                    let src = o.source.and_then(|s| w.sources.get(&s)).ok_or_else(|| ApiError::invalid(format!("{} has no source", o.name)))?;
                    cmds.push(Command::SetSourceSettings { source: src.id, settings: merged(&src.settings, settings) });
                }
                self.remote_exec(who, &format!("Change {}", o.name), cmds, cx)?;
                let w = self.studio.read(cx).workspace().clone();
                let scene = w.scene(scene.id).cloned().ok_or_else(|| ApiError::not_found("the scene went"))?;
                let all = scene_json(&w, &scene, canvas);
                Ok(all["objects"].as_array().and_then(|l| l.iter().find(|x| x["id"] == json!(o.id)).cloned()).unwrap_or_default())
            }
            "object.remove" => {
                let scene = self.scene_param(p, cx)?;
                let o = object_by(&scene, need(p, "object")?)?;
                self.remote_exec(who, &format!("Remove {}", o.name), vec![Command::RemoveObject { scene: scene.id, object: o.id }], cx)?;
                ok()
            }
            "object.move" => {
                let scene = self.scene_param(p, cx)?;
                let o = object_by(&scene, need(p, "object")?)?;
                let siblings: Vec<Id> = scene.objects.iter().filter(|x| x.parent == o.parent).map(|x| x.id).collect();
                let at = siblings.iter().position(|x| *x == o.id).unwrap_or(0);
                let last = siblings.len().saturating_sub(1);
                let to = match (text(p, "position"), p.get("to").and_then(Value::as_u64)) {
                    (Some("front"), _) => last,
                    (Some("back"), _) => 0,
                    (Some("forward"), _) => (at + 1).min(last),
                    (Some("backward"), _) => at.saturating_sub(1),
                    (None, Some(i)) => (i as usize).min(last),
                    _ => return Err(ApiError::invalid("`position` is front, back, forward or backward (or `to`: an index from the back)")),
                };
                if to != at {
                    self.remote_exec(who, &format!("Move {}", o.name), vec![Command::MoveObject { scene: scene.id, object: o.id, to }], cx)?;
                }
                Ok(json!({"index": to}))
            }

            "source.get" => {
                let s = source_by(&w, need(p, "source")?)?;
                Ok(json!({"id": s.id, "name": s.name, "kind": s.kind, "settings": s.settings}))
            }
            "source.update" => {
                let s = source_by(&w, need(p, "source")?)?.clone();
                let mut cmds = vec![];
                if let Some(name) = text(p, "name") {
                    cmds.push(Command::RenameSource { source: s.id, name: name.to_string() });
                }
                if let Some(settings) = p.get("settings").filter(|v| v.is_object()) {
                    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(false);
                    let settings = if replace { settings.clone() } else { merged(&s.settings, settings) };
                    cmds.push(Command::SetSourceSettings { source: s.id, settings });
                }
                self.remote_exec(who, &format!("Change {}", s.name), cmds, cx)?;
                ok()
            }
            "source.remove" => {
                let s = source_by(&w, need(p, "source")?)?;
                // with every object showing it and its track
                let mut cmds: Vec<Command> = w
                    .scenes
                    .iter()
                    .flat_map(|sc| sc.objects.iter().filter(|o| o.source == Some(s.id)).map(|o| Command::RemoveObject { scene: sc.id, object: o.id }))
                    .collect();
                cmds.extend(w.tracks.iter().filter(|t| t.source == s.id).map(|t| Command::RemoveTrack { track: t.id }));
                cmds.push(Command::RemoveSource { source: s.id });
                self.remote_exec(who, &format!("Remove {}", s.name), cmds, cx)?;
                ok()
            }

            "media.status" => {
                let src = source_by(&w, need(p, "source")?)?.id;
                let st = self.studio.read(cx).engine().call(move |e| e.reconciler.media_status(src));
                let st = st.ok_or_else(|| ApiError::invalid("not a media source"))?;
                use lightair_engine::media::MediaState as M;
                let state = match st.state {
                    M::None => "none",
                    M::Playing => "playing",
                    M::Opening => "opening",
                    M::Buffering => "buffering",
                    M::Paused => "paused",
                    M::Stopped => "stopped",
                    M::Ended => "ended",
                    M::Error => "error",
                };
                Ok(json!({"state": state, "time_ms": st.time_ms, "duration_ms": st.duration_ms}))
            }
            "media.action" => {
                use lightair_engine::media::MediaAction as A;
                let src = source_by(&w, need(p, "source")?)?.id;
                let action = match need(p, "action")? {
                    "play" => A::Play,
                    "pause" => A::Pause,
                    "restart" => A::Restart,
                    "stop" => A::Stop,
                    "seek" => A::Seek(p.get("ms").and_then(Value::as_i64).ok_or_else(|| ApiError::invalid("`ms` is needed to seek"))?),
                    a => return Err(ApiError::invalid(format!("no media action {a} (play, pause, restart, stop, seek)"))),
                };
                self.studio.read(cx).engine().call(move |e| e.reconciler.media_action(src, action));
                ok()
            }
            "audio.set" => {
                let t = track_by(&w, need(p, "track")?)?;
                let mut cmds = vec![];
                if let Some(v) = num(p, "volume_db") {
                    cmds.push(Command::SetTrackVolume { track: t.id, volume_db: v.clamp(-100.0, 26.0) });
                }
                if let Some(m) = p.get("muted").and_then(Value::as_bool) {
                    cmds.push(Command::SetTrackMuted { track: t.id, muted: m });
                }
                self.remote_exec(who, &format!("Change {}", t.name), cmds, cx)?;
                ok()
            }
            "audio.add" => self.remote_audio_add(who, p, cx),

            "record.status" => Ok(self.recording_json(cx)),
            "record.start" | "record.stop" | "record.toggle" => {
                let on = self.studio.read(cx).is_recording();
                let want = match call.method.as_str() {
                    "record.start" => true,
                    "record.stop" => false,
                    _ => !on,
                };
                if want == on {
                    return Err(ApiError::state(if on { "already recording" } else { "not recording" }));
                }
                let path = runtime().recording().path.map(|p| p.to_string_lossy().into_owned());
                self.studio.update(cx, |s, cx| s.toggle_recording(cx));
                Ok(json!({"active": want, "path": if want { None } else { path }}))
            }
            "record.pause" | "record.resume" => {
                let s = self.studio.read(cx);
                if !s.is_recording() {
                    return Err(ApiError::state("not recording"));
                }
                let paused = runtime().recording().paused;
                if paused != (call.method == "record.resume") {
                    return Err(ApiError::state(if paused { "already paused" } else { "not paused" }));
                }
                self.studio.update(cx, |s, cx| s.pause_recording(cx));
                ok()
            }
            "record.marker" | "record.split" => {
                if !self.studio.read(cx).is_recording() {
                    return Err(ApiError::state("not recording"));
                }
                let marker = call.method == "record.marker";
                self.studio.update(cx, |s, cx| if marker { s.add_marker(cx) } else { s.split_recording(cx) });
                ok()
            }
            "stream.status" => Ok(self.streaming_json(cx)),
            "stream.start" | "stream.stop" | "stream.toggle" => {
                let on = runtime().any_stream_running();
                let want = match call.method.as_str() {
                    "stream.start" => true,
                    "stream.stop" => false,
                    _ => !on,
                };
                if want == on {
                    return Err(ApiError::state(if on { "already live" } else { "not live" }));
                }
                if want {
                    let enabled: Vec<_> = w.outputs.destinations.iter().filter(|d| d.enabled).collect();
                    if enabled.is_empty() {
                        return Err(ApiError::state("no destination: add one in Outputs to go live"));
                    }
                    if let Some(d) = enabled.iter().find(|d| crate::secrets::stream_key(d.id).is_none()) {
                        return Err(ApiError::state(format!("no stream key saved for {}", d.name)));
                    }
                    self.studio.update(cx, |s, cx| s.go_live(cx));
                } else {
                    self.studio.update(cx, |s, cx| s.end_stream(cx));
                }
                Ok(json!({"active": want}))
            }
            "vcam.status" => Ok(json!({"active": self.studio.read(cx).virtual_camera().is_running()})),
            "vcam.start" | "vcam.stop" | "vcam.toggle" => {
                let on = self.studio.read(cx).virtual_camera().is_running();
                let want = match call.method.as_str() {
                    "vcam.start" => true,
                    "vcam.stop" => false,
                    _ => !on,
                };
                if want == on {
                    return Err(ApiError::state(if on { "the virtual camera is on" } else { "the virtual camera is off" }));
                }
                self.studio.update(cx, |s, cx| s.toggle_virtual_camera(cx));
                Ok(json!({"active": want}))
            }
            "clip.status" => {
                let s = self.studio.read(cx);
                Ok(json!({"active": s.clips_ready(), "last": s.last_clip}))
            }
            "clip.save" => {
                if !self.studio.read(cx).clips_ready() {
                    return Err(ApiError::state("Clips are off: turn them on in Outputs"));
                }
                self.studio.update(cx, |s, cx| s.save_clip(cx));
                ok()
            }

            "history.undo" | "history.redo" => {
                let undo = call.method == "history.undo";
                let label = self.studio.update(cx, |s, cx| {
                    if undo { s.undo(cx) } else { s.redo(cx) }
                    s.issue.clone()
                });
                Ok(json!({"done": label.map(|l| l.to_string())}))
            }
            "commands.execute" => {
                let list = match (p.get("commands").and_then(Value::as_array), p.get("command")) {
                    (Some(l), _) => l.clone(),
                    (None, Some(c)) => vec![c.clone()],
                    _ => return Err(ApiError::invalid("`commands` is needed: a list of commands")),
                };
                let commands = list
                    .into_iter()
                    .map(|c| serde_json::from_value::<Command>(c).map_err(|e| ApiError::invalid(format!("not a command: {e}"))))
                    .collect::<Result<Vec<_>, _>>()?;
                let label = text(p, "label").map(str::to_owned).unwrap_or_else(|| match commands.as_slice() {
                    [one] => one.label(),
                    _ => format!("{} changes", commands.len()),
                });
                self.remote_exec(who, &label, commands, cx)?;
                ok()
            }
            "ui.select" => {
                let scene = self.scene_param(p, cx)?;
                let object = text(p, "object").map(|o| object_by(&scene, o).map(|o| o.id)).transpose()?;
                self.studio.update(cx, |s, cx| {
                    s.select_scene(scene.id);
                    cx.notify();
                });
                if object.is_some() {
                    self.select_object(object, cx);
                }
                ok()
            }
            m => Err(err(ErrorKind::Unknown, format!("Lightair has no method {m}"))),
        }
    }

    fn state_json(&self, cx: &App) -> Value {
        let studio = self.studio.read(cx);
        let w = studio.workspace();
        let canvas = w.primary_canvas().id;
        let program = w.program.and_then(|id| w.scene(id)).map(|s| json!({"id": s.id, "name": s.name}));
        json!({
            "workspace": w.name,
            "program": program,
            "editing": studio.selected_scene,
            "canvases": w.canvases.iter().map(|c| json!({
                "id": c.id, "name": c.name, "width": c.width, "height": c.height,
                "fps": c.fps_num as f64 / c.fps_den.max(1) as f64,
            })).collect::<Vec<_>>(),
            "scenes": w.scenes.iter().map(|s| scene_json(w, s, canvas)).collect::<Vec<_>>(),
            "sources": w.sources.values().map(|s| json!({"id": s.id, "name": s.name, "kind": s.kind})).collect::<Vec<_>>(),
            "tracks": w.tracks.iter().map(|t| json!({
                "id": t.id, "name": t.name, "source": t.source, "volume_db": t.volume_db, "muted": t.muted,
            })).collect::<Vec<_>>(),
            "outputs": {
                "recording": self.recording_json(cx),
                "streaming": self.streaming_json(cx),
                "virtual_camera": {"active": studio.virtual_camera().is_running()},
                "clips": {"active": studio.clips_ready()},
            },
        })
    }

    fn recording_json(&self, cx: &App) -> Value {
        let studio = self.studio.read(cx);
        let status = runtime().recording();
        let mut v = output_json(None);
        v["active"] = studio.is_recording().into();
        v["paused"] = status.paused.into();
        v["path"] = status.path.as_ref().map(|p| p.to_string_lossy().into_owned()).into();
        v["bytes"] = status.bytes.into();
        v["folder"] = studio.recording_folder().to_string_lossy().into_owned().into();
        v
    }

    fn streaming_json(&self, cx: &App) -> Value {
        let w = self.studio.read(cx).workspace();
        let destinations: Vec<Value> = w
            .outputs
            .destinations
            .iter()
            .map(|d| {
                let mut s = output_json(Some(OutputKey::Stream(d.id)));
                s["name"] = d.name.clone().into();
                s["enabled"] = d.enabled.into();
                s
            })
            .collect();
        json!({"active": runtime().any_stream_running(), "destinations": destinations})
    }

    fn stats_json(&mut self, cx: &App) -> Value {
        let studio = self.studio.read(cx);
        let s = &studio.stats;
        let (cpu_s, rss) = process_usage();
        let now = Instant::now();
        let cpu = self.remote.cpu.replace((now, cpu_s)).map(|(t, c)| {
            let wall = now.duration_since(t).as_secs_f64();
            if wall > 0.05 { (cpu_s - c) / wall * 100.0 } else { 0.0 }
        });
        let rec = runtime().recording();
        let free = lightair_platform::files::space(&studio.recording_folder()).map(|(f, _)| f / (1 << 20));
        json!({
            "fps": s.active_fps,
            "frame_time_ms": s.average_frame_time_ms,
            "total_frames": s.total_frames,
            "lagged_frames": s.lagged_frames,
            "lagged_last_second": studio.recent_lagged,
            "output_skipped_frames": rec.dropped_frames,
            "output_total_frames": rec.total_frames,
            "cpu_percent": cpu,
            "cpu_seconds": cpu_s,
            "memory_mb": rss as f64 / (1 << 20) as f64,
            "disk_free_mb": free,
        })
    }

    /// `object.add`: a new source and an object showing it (or an object for an existing source).
    fn remote_add(&mut self, who: &str, p: &Value, cx: &mut Context<Self>) -> Reply {
        let scene = self.scene_param(p, cx)?;
        let w = self.studio.read(cx).workspace().clone();
        let mut cmds = vec![];
        let (source_id, name, size, track) = if let Some(key) = text(p, "source") {
            let s = source_by(&w, key)?;
            (s.id, text(p, "name").unwrap_or(&s.name).to_string(), self.size_of(s), None)
        } else {
            let kind = text(p, "kind").ok_or_else(|| ApiError::invalid("`kind` is needed (or `source`)"))?;
            let given = p.get("settings").filter(|s| s.is_object()).cloned().unwrap_or(json!({}));
            if matches!(kind, "microphone" | "mic" | "coreaudio_input_capture" | "desktop_audio" | "sck_audio_capture") {
                let mut q = json!({"kind": if kind.contains("desk") || kind.starts_with("sck") { "desktop" } else { "microphone" }});
                if let Some(n) = text(p, "name") {
                    q["name"] = n.into();
                }
                if let Some(d) = given.get("device") {
                    q["device"] = d.clone();
                }
                return self.remote_audio_add(who, &q, cx);
            }
            let made = self.new_source(kind, given, cx)?;
            let name = text(p, "name").map(str::to_owned).unwrap_or(made.name);
            let source = Source::new(&name, made.kind, made.settings);
            let id = source.id;
            let track = made.sound.then(|| Track::new(&name, id));
            cmds.push(Command::AddSource { source });
            (id, name, made.size, track)
        };
        let (w0, h0) = size;
        let layouts: Vec<(Id, Layout)> = w
            .canvases
            .iter()
            .map(|c| {
                let fit = (c.width as f32 / w0).min(c.height as f32 / h0).min(1.0);
                let (lw, lh) = (num(p, "width").unwrap_or(w0 * fit), num(p, "height").unwrap_or(h0 * fit));
                let x = num(p, "x").unwrap_or((c.width as f32 - lw) / 2.0);
                let y = num(p, "y").unwrap_or((c.height as f32 - lh) / 2.0);
                (c.id, Layout::boxed(x, y, lw.max(1.0), lh.max(1.0)))
            })
            .collect();
        let object = Object::new(&name, source_id, layouts);
        let object_id = object.id;
        cmds.push(Command::AddObject { scene: scene.id, object, index: None });
        let track_id = track.as_ref().map(|t| t.id);
        cmds.extend(track.map(|track| Command::AddTrack { track, index: None }));
        self.remote_exec(who, &format!("Add {name}"), cmds, cx)?;
        Ok(json!({"scene": scene.id, "object": object_id, "source": source_id, "track": track_id, "name": name}))
    }

    /// `audio.add`: a sound source with its mixer track (no object: sound has no picture).
    fn remote_audio_add(&mut self, who: &str, p: &Value, cx: &mut Context<Self>) -> Reply {
        let (kind, name, settings) = match text(p, "kind").unwrap_or("microphone") {
            "microphone" | "mic" => (lightair_engine::devices::DEVICE_KIND, "Mic", json!({"device": text(p, "device").unwrap_or("")})),
            "desktop" | "desktop_audio" => (lightair_model::kinds::DESKTOP_AUDIO, "Desktop audio", json!({})),
            "app" | "app_audio" => (lightair_model::kinds::APP_AUDIO, "App audio", json!({"app": text(p, "app").unwrap_or("")})),
            k => return Err(ApiError::invalid(format!("no sound kind {k} (microphone, desktop, app)"))),
        };
        let name = text(p, "name").unwrap_or(name);
        let source = Source::new(name, kind, settings);
        let track = Track::new(name, source.id);
        let ids = json!({"source": source.id, "track": track.id});
        self.remote_exec(who, &format!("Add {name} track"), vec![Command::AddSource { source }, Command::AddTrack { track, index: None }], cx)?;
        Ok(ids)
    }

    /// A source's picture size, as far as Lightair knows it (its settings, or a guess).
    fn size_of(&self, s: &Source) -> (f32, f32) {
        let g = |k: &str| s.settings.get(k).and_then(Value::as_f64).map(|v| v as f32);
        match (g("width"), g("height")) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w, h),
            _ => (1920.0, 1080.0),
        }
    }

    /// What a kind's new source is: Lightair's kind, settings with its defaults, size, sound.
    fn new_source(&self, kind: &str, mut s: Value, cx: &App) -> Result<NewSource, ApiError> {
        let size_in = |s: &Value, w: f32, h: f32| {
            let g = |k: &str| s.get(k).and_then(Value::as_f64).map(|v| v as f32);
            (g("width").unwrap_or(w), g("height").unwrap_or(h))
        };
        let defaults = |s: &mut Value, d: Value| {
            if let (Some(s), Value::Object(d)) = (s.as_object_mut(), d) {
                for (k, v) in d {
                    s.entry(k).or_insert(v);
                }
            }
        };
        Ok(match kind {
            "text" | "text_ft2_source_v2" | "text_ft2_source" => {
                defaults(&mut s, json!({
                    "text": "Text",
                    "font": {"face": crate::ui::TEXT_FACE, "size": 128, "flags": 1},
                    "color1": 0xFFFFFFFFu32, "color2": 0xFFFFFFFFu32,
                }));
                let n = s["text"].as_str().unwrap_or("Text").chars().count().clamp(4, 40) as f32;
                let size = s["font"]["size"].as_f64().unwrap_or(128.0) as f32;
                let name = s["text"].as_str().unwrap_or("Text").chars().take(24).collect();
                NewSource { kind: "text_ft2_source_v2".into(), settings: s, size: (n * size * 0.6, size * 1.4), sound: false, name }
            }
            "image" | "image_source" => {
                let file = s.get("file").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("an image needs settings.file (a path)"))?.to_string();
                let size = image::image_dimensions(&file).map(|(w, h)| (w as f32, h as f32)).unwrap_or((1280.0, 720.0));
                let name = std::path::Path::new(&file).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or("Image".into());
                NewSource { kind: "image_source".into(), settings: s, size, sound: false, name }
            }
            "video" | "media" | "ffmpeg_source" => {
                let file = s.get("local_file").or(s.get("file")).and_then(Value::as_str).map(str::to_owned);
                let file = file.ok_or_else(|| ApiError::invalid("a video needs settings.local_file (a path)"))?;
                s["local_file"] = file.clone().into();
                if let Some(o) = s.as_object_mut() {
                    o.remove("file");
                }
                defaults(&mut s, json!({"looping": true, "is_local_file": true}));
                let name = std::path::Path::new(&file).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or("Video".into());
                NewSource { kind: "ffmpeg_source".into(), settings: s, size: size_in(&json!({}), 1920.0, 1080.0), sound: true, name }
            }
            "color" | "colour" | "color_source_v3" | "color_source" => {
                let w = self.studio.read(cx).workspace().primary_canvas().clone();
                defaults(&mut s, json!({"color": 0xFF4A3BFFu32, "width": w.width, "height": w.height}));
                let size = size_in(&s, w.width as f32, w.height as f32);
                NewSource { kind: "color_source_v3".into(), settings: s, size, sound: false, name: "Colour".into() }
            }
            "browser" | "web" | "browser_source" | lightair_engine::web::KIND => {
                let url = s.get("url").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("a browser needs settings.url"))?;
                s["url"] = super::object_props::web_address(url).into();
                defaults(&mut s, json!({"width": 1920, "height": 1080, "fps": 30}));
                if let Some(css) = s.get("css").cloned().filter(Value::is_string) {
                    s["css"] = css;
                }
                let host = s["url"].as_str().unwrap_or("").split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("Browser").to_string();
                let size = size_in(&s, 1920.0, 1080.0);
                NewSource { kind: lightair_engine::web::KIND.into(), settings: s, size, sound: true, name: host }
            }
            "display" | "window" | "app" | "screen_capture" | "display_capture" | "window_capture" | lightair_model::kinds::SCREEN => {
                use lightair_model::kinds::{APP, DISPLAY, WINDOW};
                let mode = match kind {
                    "window" | "window_capture" => WINDOW,
                    "app" => APP,
                    _ => s.get("mode").and_then(Value::as_i64).unwrap_or(DISPLAY),
                };
                let key = super::screen_pick::key_of(mode);
                let value = match s.get(key).cloned() {
                    Some(v) => v,
                    None if mode == DISPLAY => lightair_engine::displays::main_display_uuid().map(Value::from).unwrap_or(Value::Null),
                    None => return Err(ApiError::invalid(format!("say which {key}: settings.{key} from list_capture_targets"))),
                };
                let mut settings = super::screen_pick::settings(mode, &value);
                if let Some(c) = s.get("cursor") {
                    settings["cursor"] = c.clone();
                }
                let name = match mode {
                    DISPLAY => "Display",
                    WINDOW => "Window",
                    _ => "App",
                };
                NewSource { kind: lightair_model::kinds::SCREEN.into(), settings, size: (1920.0, 1080.0), sound: false, name: name.into() }
            }
            "camera" | "macos-avcapture" | "av_capture_input" | "av_capture_input_v2" | lightair_model::kinds::CAMERA => {
                let device = match s.get("device").and_then(Value::as_str) {
                    Some(d) => d.to_string(),
                    None => {
                        let cams = self.studio.read(cx).engine().call(|_| lightair_engine::kinds::cameras());
                        cams.first().map(|(_, id)| id.clone()).ok_or_else(|| ApiError::state("no camera found"))?
                    }
                };
                let mut settings = self.prefs.camera.settings(&device);
                if let Some(h) = s.get("height") {
                    settings["height"] = h.clone();
                }
                NewSource { kind: lightair_model::kinds::CAMERA.into(), settings, size: (1280.0, 720.0), sound: false, name: "Camera".into() }
            }
            "timer" | lightair_engine::timers::TIMER_KIND => {
                defaults(&mut s, json!({
                    "mode": "countdown", "seconds": 300, "autostart": true,
                    "font": {"face": crate::ui::TEXT_FACE, "size": 160, "flags": 1},
                    "color1": 0xFFFFFFFFu32, "color2": 0xFFFFFFFFu32,
                }));
                NewSource { kind: lightair_engine::timers::TIMER_KIND.into(), settings: s, size: (560.0, 200.0), sound: false, name: "Countdown".into() }
            }
            other => {
                let size = size_in(&s, 1280.0, 720.0);
                NewSource { kind: other.into(), settings: s, size, sound: false, name: other.into() }
            }
        })
    }

    /// Answers `screenshot` once the preview has drawn the scene asked for.
    async fn screenshot(this: WeakEntity<Self>, p: Value, cx: &mut AsyncWindowContext) -> Reply {
        let width = p.get("width").and_then(Value::as_u64).unwrap_or(960).clamp(16, 7680) as usize;
        // the preview, showing the scene asked for; frames counted from when it changed
        let start = this
            .update(cx, |view, cx| -> Result<u64, ApiError> {
                let studio = view.studio.read(cx);
                let w = studio.workspace().clone();
                let scene = match text(&p, "scene") {
                    Some(k) => Some(scene_by(&w, k)?.id).filter(|id| Some(*id) != w.program),
                    None => None,
                };
                let canvas = w.primary_canvas().id;
                let engine = studio.engine().clone();
                let shot = view.remote.shot.get_or_insert_with(|| Shot { preview: Preview::attach(), scene: None, used: Instant::now() });
                shot.used = Instant::now();
                if shot.scene != scene {
                    let source = scene.and_then(|s| engine.call(move |e| e.reconciler.scene_source_ref(s, canvas).map(|p| p as usize)));
                    shot.preview.set_source(source.map(|p| p as *mut lightair_engine::sys::obs_source_t));
                    shot.scene = scene;
                    return Ok(shot.preview.frames() + 3);
                }
                // a new preview's first frames can be from before the last change
                Ok(shot.preview.frames().max(6))
            })
            .map_err(|_| ApiError::failed("Lightair's window is closed"))??;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = this
                .update(cx, |view, _| {
                    let shot = view.remote.shot.as_ref()?;
                    (shot.preview.frames() >= start).then(|| shot.preview.latest_frame()).flatten()
                })
                .ok()
                .flatten();
            if let Some(frame) = frame {
                let (w, h, rgba) = lightair_preview::frame_rgba(&frame, width).ok_or_else(|| ApiError::failed("no picture"))?;
                let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or_else(|| ApiError::failed("bad picture"))?;
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(img)
                    .to_rgb8()
                    .write_to(&mut png, image::ImageFormat::Png)
                    .map_err(|e| ApiError::failed(e.to_string()))?;
                let data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
                // let go of it in a while: it costs a render
                let weak = this.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(Duration::from_secs(30)).await;
                    let _ = weak.update(cx, |view, _| {
                        if view.remote.shot.as_ref().is_some_and(|s| s.used.elapsed() >= Duration::from_secs(29)) {
                            view.remote.shot = None;
                        }
                    });
                })
                .detach();
                return Ok(json!({"mime": "image/png", "data": data, "width": w, "height": h}));
            }
            if Instant::now() > deadline {
                return Err(ApiError::failed("the picture didn't come in time"));
            }
            cx.background_executor().timer(Duration::from_millis(30)).await;
        }
    }
}

struct NewSource {
    kind: String,
    settings: Value,
    size: (f32, f32),
    /// It has sound of its own (gets a mixer track).
    sound: bool,
    name: String,
}

/// `over` merged into `base`, key by key (objects merged in turn).
fn merged(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, v) in o {
                let next = match out.get(k) {
                    Some(old) if old.is_object() && v.is_object() => merged(old, v),
                    _ => v.clone(),
                };
                out.insert(k.clone(), next);
            }
            Value::Object(out)
        }
        _ => over.clone(),
    }
}

/// This process's CPU time (seconds) and resident memory (bytes: its working set).
#[cfg(windows)]
fn process_usage() -> (f64, u64) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let me = GetCurrentProcess();
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // (100-nanosecond units)
        let secs = |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64 / 1e7;
        let cpu = if GetProcessTimes(me, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
            secs(kernel) + secs(user)
        } else {
            0.0
        };
        let mut mem: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let resident = if K32GetProcessMemoryInfo(me, &mut mem, mem.cb) != 0 { mem.WorkingSetSize as u64 } else { 0 };
        (cpu, resident)
    }
}

/// This process's CPU time (seconds) and resident memory (bytes).
#[cfg(not(any(unix, windows)))]
fn process_usage() -> (f64, u64) {
    (0.0, 0)
}

/// This process's CPU time (seconds) and resident memory (bytes).
#[cfg(unix)]
fn process_usage() -> (f64, u64) {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
    let cpu = t(ru.ru_utime) + t(ru.ru_stime);
    (cpu, resident_bytes())
}

#[cfg(target_os = "macos")]
fn resident_bytes() -> u64 {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    let n = unsafe {
        libc::proc_pidinfo(std::process::id() as i32, libc::PROC_PIDTASKINFO, 0, &mut info as *mut _ as *mut libc::c_void, size)
    };
    if n == size { info.pti_resident_size } else { 0 }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn resident_bytes() -> u64 {
    0
}

/// `capture.targets`: what a display, window, app, camera or microphone capture can show.
fn capture_targets(p: &Value, engine: &Arc<lightair_engine::Engine>) -> Reply {
    use lightair_model::kinds::{APP, DISPLAY, WINDOW};
    let kind = p.get("kind").and_then(Value::as_str).unwrap_or("display");
    let items: Vec<Value> = match kind {
        "display" | "window" | "app" => {
            let mode = match kind {
                "display" => DISPLAY,
                "window" => WINDOW,
                _ => APP,
            };
            let key = super::screen_pick::key_of(mode);
            // the system's list fills in a moment after the first ask
            let mut options = lightair_engine::displays::screen_options(mode, key);
            for _ in 0..4 {
                if !options.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
                options = lightair_engine::displays::screen_options(mode, key);
            }
            let hide = |v: &Value| {
                lightair_platform::apps::hide_window(&v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
            };
            super::source::choices(key, &options, |v| mode != DISPLAY && hide(v))
                .into_iter()
                .map(|c| json!({"title": c.title, "detail": c.detail, "settings": {key: c.value}}))
                .collect()
        }
        "camera" => engine
            .call(|_| lightair_engine::kinds::cameras())
            .into_iter()
            .map(|(name, id)| json!({"title": name, "settings": {"device": id}}))
            .collect(),
        "microphone" | "mic" => lightair_engine::devices::input_devices()
            .into_iter()
            .map(|d| json!({"title": d.name, "detail": if d.default { "default" } else { "" }, "settings": {"device": d.name}}))
            .collect(),
        k => return Err(ApiError::invalid(format!("no capture kind {k} (display, window, app, camera, microphone)"))),
    };
    Ok(json!({"kind": kind, "targets": items}))
}
