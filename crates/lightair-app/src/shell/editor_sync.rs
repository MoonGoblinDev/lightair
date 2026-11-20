//! Lightair Editor's round trip with this desktop (lightair-live's docs/05 §4.2 *Takes…* and §7;
//! lightair-editor's docs §5 and §10): the workspace's takes, recordings, clips, exports and
//! Editor projects listed for a paired phone or tablet, fetched in ranged, resumable pieces,
//! edits sent back with the revision they started from, and exports and projects sent here.
//!
//! Answered on the remote API's thread, never the UI's (files are read and hashed here), from the
//! workspace open now. Transfers are lightair-remote's (`Transfers`, served on the phones' TLS
//! port); this file decides what's in them and what happens to what arrives.
//!
//! - **Revision:** the first 16 hex digits of the SHA-256 of the document as stored (a project's
//!   `edits/<id>.json`, a take's own `edit.json`; `""` when there's none).
//! - **Conflicts:** a document sent with a revision the desktop has moved past isn't merged: the
//!   desktop keeps its own and stores the other as a copy ("*Name* (edited on *device*)"), and
//!   answers `{conflict: true, revision, copy}`.
//! - **Checkouts:** `takes.checkout` marks an item "Being edited on *device*" in
//!   `<workspace>/checkouts.json` until `takes.checkin`, or 7 days without word.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use lightair_model::Id;
use lightair_remote::{ApiError, Call, ErrorKind, FileSpec, Pairing, Reply, Transfers};
use lightair_store::recordings::{self, Index, Item, Kind};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// What the desktop answers for (`session.hello`'s features).
pub(crate) const FEATURES: &[&str] = &["takes", "clips", "edits", "transfer"];

/// A checkout lasts this long without word from the device.
const CHECKOUT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

/// The Editor's round trip: the workspace open now, the transfers, and what's finished.
#[derive(Default)]
pub(crate) struct EditorSync {
    workspace: Mutex<Option<PathBuf>>,
    transfers: OnceLock<Arc<Transfers>>,
    pairing: OnceLock<Arc<Pairing>>,
    /// Uploads that finished, by token: what they became (for `transfer.status`).
    done: Mutex<HashMap<String, Value>>,
    /// Events for phones (`transfer.done`), sent by the app on its next look.
    events: Mutex<Vec<(&'static str, Value)>>,
    watching: OnceLock<()>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn modified_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// A document's revision: the start of its SHA-256, `""` when there's none.
pub(crate) fn revision(path: &Path) -> String {
    std::fs::read(path).map(|b| revision_of(&b)).unwrap_or_default()
}

fn revision_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorKind::Invalid, m)
}

fn need<'a>(p: &'a Value, k: &str) -> Result<&'a str, ApiError> {
    p.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| invalid(format!("`{k}` is needed")))
}

/// Writes a document whole, through a temporary file.
fn write_doc(path: &Path, doc: &Value) -> std::io::Result<String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(doc).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(revision_of(&bytes))
}

fn failed(e: impl std::fmt::Display) -> ApiError {
    ApiError::failed(e.to_string())
}

/// The take folder of a studio take (where its `take.json` is).
fn take_folder(item: &Item) -> Option<PathBuf> {
    item.files.iter().find_map(|f| {
        let dir = f.path.parent()?;
        dir.join("take.json").is_file().then(|| dir.to_path_buf())
    })
}

/// A file's name in a transfer: its folder's name, then its own.
fn in_folder(folder: &Path, file: &Path) -> String {
    let dir = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{dir}/{name}")
}

/// The files directly in a folder (no partial downloads, no hidden ones).
fn files_in(folder: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            !n.starts_with('.') && !n.ends_with(".partial") && !n.ends_with(".tmp")
        })
        .collect();
    out.sort();
    out
}

fn is_video(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "mp4" | "mov" | "m4v" | "mkv"))
}

/// The proxy lightair-render made for a video, when there is one (named by the video's content,
/// so the phone finds it as its own).
fn proxy_of(video: &Path) -> Option<PathBuf> {
    let name = lightair_render::sequence::content_name(video)?;
    let p = lightair_render::sound::cache().join(format!("proxy-{name}.mp4"));
    p.is_file().then_some(p)
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Recording => "recording",
        Kind::StudioTake => "take",
        Kind::Clip => "clip",
        Kind::Multitrack => "multitrack",
        Kind::Export => "export",
    }
}

fn thumbnail(path: Option<&PathBuf>) -> Value {
    let Some(bytes) = path.and_then(|p| std::fs::read(p).ok()) else { return Value::Null };
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
    json!({"mime": mime, "data": base64::engine::general_purpose::STANDARD.encode(bytes)})
}

impl EditorSync {
    /// What it needs from the server and the phones' pairing (once each).
    pub(crate) fn connect(self: &Arc<Self>, transfers: Arc<Transfers>, pairing: Arc<Pairing>) {
        let _ = self.transfers.set(transfers);
        let _ = self.pairing.set(pairing);
        if self.watching.set(()).is_ok() {
            let me = Arc::downgrade(self);
            let _ = std::thread::Builder::new().name("lightair-editor-sync".into()).spawn(move || {
                while let Some(me) = me.upgrade() {
                    me.take_uploads();
                    drop(me);
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
    }

    /// The workspace open now (`None`: unsaved).
    pub(crate) fn set_workspace(&self, dir: Option<PathBuf>) {
        let mut w = self.workspace.lock();
        if *w != dir {
            *w = dir;
        }
    }

    /// Events for phones since the last look.
    pub(crate) fn take_events(&self) -> Vec<(&'static str, Value)> {
        std::mem::take(&mut *self.events.lock())
    }

    fn workspace(&self) -> Result<PathBuf, ApiError> {
        self.workspace.lock().clone().ok_or_else(|| ApiError::state("Save the workspace on the computer first."))
    }

    fn transfers(&self) -> Result<&Arc<Transfers>, ApiError> {
        self.transfers.get().ok_or_else(|| ApiError::state("The remote API is starting."))
    }

    fn device_name(&self, device: &str) -> String {
        self.pairing
            .get()
            .and_then(|p| p.devices().into_iter().find(|d| d.id == device).map(|d| d.name))
            .unwrap_or_else(|| "a phone".into())
    }

    /// Answers a call this file is for (`None`: another's).
    pub(crate) fn call(&self, call: &Call) -> Option<Reply> {
        let family = call.method.split('.').next().unwrap_or("");
        if !matches!(family, "takes" | "clips" | "edits" | "transfer") {
            return None;
        }
        let p = &call.params;
        let device = call.device.as_deref();
        let paired = || {
            device.ok_or_else(|| {
                ApiError::new(ErrorKind::State, "Only a paired phone or tablet can do this (on the phones' port).")
            })
        };
        Some(match call.method.as_str() {
            "takes.list" => self.list(p, &[Kind::StudioTake, Kind::Recording, Kind::Export], "items"),
            "clips.list" => self.list(p, &[Kind::Clip], "clips"),
            "takes.fetch" => paired().and_then(|d| self.fetch_item(d, need(p, "take")?, p)),
            "clips.fetch" => paired().and_then(|d| self.fetch_item(d, need(p, "clip")?, p)),
            "takes.put_edit" => paired().and_then(|d| self.put_take_edit(d, p)),
            "takes.checkout" => paired().and_then(|d| self.checkout(d, p, true)),
            "takes.checkin" => paired().and_then(|d| self.checkout(d, p, false)),
            "edits.list" => self.list_projects(p),
            "edits.fetch" => paired().and_then(|d| self.fetch_project(d, p)),
            "edits.put" => paired().and_then(|d| self.put_project(d, p)),
            "transfer.upload" => paired().and_then(|d| self.upload(d, p)),
            "transfer.status" => self.status(p),
            m => Err(ApiError::new(ErrorKind::Unknown, format!("no method {m}"))),
        })
    }

    // ------------------------------------------------------------------ listing

    fn checkouts(&self, ws: &Path) -> HashMap<String, Value> {
        let path = ws.join("checkouts.json");
        let mut all: HashMap<String, Value> =
            std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let stale = now_ms() - CHECKOUT_FOR.as_millis() as i64;
        all.retain(|_, v| v["seen_ms"].as_i64().unwrap_or(0) > stale);
        all
    }

    fn list(&self, p: &Value, kinds: &[Kind], key: &str) -> Reply {
        let ws = self.workspace()?;
        let since = p.get("since").and_then(Value::as_i64).unwrap_or(0);
        let only: Option<Vec<String>> = match p.get("kind") {
            Some(Value::String(k)) => Some(vec![k.clone()]),
            Some(Value::Array(a)) => Some(a.iter().filter_map(|k| k.as_str().map(str::to_owned)).collect()),
            _ => None,
        };
        let checkouts = self.checkouts(&ws);
        let index = Index::load(&ws);
        let items: Vec<Value> = index
            .newest_first()
            .into_iter()
            .filter(|i| kinds.contains(&i.kind))
            .filter(|i| only.as_ref().is_none_or(|o| o.iter().any(|k| k == kind_name(i.kind))))
            .filter_map(|i| {
                let folder = take_folder(i);
                let edit = folder.as_ref().map(|f| f.join("edit.json"));
                let changed = i.started_ms.max(edit.as_deref().map_or(0, modified_ms));
                if changed < since {
                    return None;
                }
                let videos: Vec<&Path> = i.files.iter().map(|f| f.path.as_path()).filter(|p| is_video(p)).collect();
                let formats: Vec<String> = i.files.iter().filter_map(|f| f.canvas.clone().or(f.track.clone())).collect();
                Some(json!({
                    "id": i.id,
                    "kind": kind_name(i.kind),
                    "name": i.title,
                    "created": i.started_ms,
                    "modified": changed,
                    "duration_ms": i.duration_ms,
                    "formats": formats,
                    "size_bytes": i.bytes(),
                    "revision": edit.as_deref().map(revision).unwrap_or_default(),
                    "has_proxies": videos.iter().any(|v| proxy_of(v).is_some()),
                    "thumbnail": thumbnail(i.thumbnail.as_ref()),
                    "markers": i.markers.iter().map(|m| json!({"at_ms": m.at_ms, "label": m.label})).collect::<Vec<_>>(),
                    "missing": !i.missing().is_empty(),
                    "checked_out": checkouts.get(&i.id.to_string()).map(|c| c["device_name"].clone()),
                }))
            })
            .collect();
        Ok(json!({ key: items, "cursor": now_ms() }))
    }

    fn list_projects(&self, p: &Value) -> Reply {
        let ws = self.workspace()?;
        let since = p.get("since").and_then(Value::as_i64).unwrap_or(0);
        let checkouts = self.checkouts(&ws);
        let dir = lightair_render::sequence::Project::dir(&ws);
        let mut projects: Vec<(i64, Value)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|path| {
                let bytes = std::fs::read(&path).ok()?;
                let project: lightair_render::sequence::Project = serde_json::from_slice(&bytes).ok()?;
                if project.modified_ms < since {
                    return None;
                }
                let (w, h) = project.sequence.size;
                let thumb = recordings::thumbnails_dir(&ws).join(format!("{}.png", project.id));
                Some((
                    project.modified_ms,
                    json!({
                        "id": project.id,
                        "name": project.name,
                        "created": project.created_ms,
                        "modified": project.modified_ms,
                        "revision": revision_of(&bytes),
                        "size": [w, h],
                        "aspect": aspect(w, h),
                        "fps": project.sequence.fps,
                        "duration_ms": project.sequence.duration_ms(),
                        "media": project.sequence.media.len(),
                        "thumbnail": thumbnail(Some(&thumb)),
                        "checked_out": checkouts.get(&project.id.to_string()).map(|c| c["device_name"].clone()),
                    }),
                ))
            })
            .collect();
        projects.sort_by_key(|(m, _)| std::cmp::Reverse(*m));
        Ok(json!({"projects": projects.into_iter().map(|(_, v)| v).collect::<Vec<_>>(), "cursor": now_ms()}))
    }

    // ------------------------------------------------------------------ fetching

    fn item(&self, ws: &Path, key: &str) -> Result<Item, ApiError> {
        let id: Id = serde_json::from_value(json!(key)).map_err(|_| invalid(format!("not an id: {key}")))?;
        Index::load(ws).get(id).cloned().ok_or_else(|| ApiError::not_found(format!("no take {key}")))
    }

    /// A take (its folder's files, folder kept in the names, its proxies), or another item's files.
    fn fetch_item(&self, device: &str, key: &str, p: &Value) -> Reply {
        let ws = self.workspace()?;
        let item = self.item(&ws, key)?;
        let proxies_only = p.get("proxies_only").and_then(Value::as_bool) == Some(true);
        let mut files: Vec<(String, PathBuf)> = vec![];
        if let Some(folder) = take_folder(&item) {
            for f in files_in(&folder) {
                let proxy = is_video(&f).then(|| proxy_of(&f)).flatten();
                if let Some(px) = &proxy {
                    files.push((px.file_name().unwrap_or_default().to_string_lossy().into_owned(), px.clone()));
                }
                if proxies_only && proxy.is_some() {
                    continue;
                }
                files.push((in_folder(&folder, &f), f));
            }
        } else {
            for f in item.files.iter().filter(|f| f.path.is_file()) {
                let name = f.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                if !files.iter().any(|(n, _)| *n == name) {
                    files.push((name, f.path.clone()));
                }
            }
        }
        if let Some(only) = p.get("files").and_then(Value::as_array) {
            let only: Vec<&str> = only.iter().filter_map(Value::as_str).collect();
            files.retain(|(n, _)| only.contains(&n.as_str()));
        }
        if files.is_empty() {
            return Err(ApiError::not_found("Its files aren't on this computer any more."));
        }
        let t = self.transfers()?.offer_download(device, files).map_err(failed)?;
        let revision = take_folder(&item).map(|f| revision(&f.join("edit.json"))).unwrap_or_default();
        Ok(json!({"transfer": t.to_json(), "revision": revision}))
    }

    fn fetch_project(&self, device: &str, p: &Value) -> Reply {
        let ws = self.workspace()?;
        let key = need(p, "project")?;
        let path = lightair_render::sequence::Project::dir(&ws).join(format!("{key}.json"));
        let bytes = std::fs::read(&path).map_err(|_| ApiError::not_found(format!("no project {key}")))?;
        let project: lightair_render::sequence::Project = serde_json::from_slice(&bytes).map_err(failed)?;
        let mut files: Vec<(String, PathBuf)> = vec![("project.json".into(), path.clone())];
        // each medium's files: `media/<n>-<file>`, a take's folder whole
        let mut about: HashMap<String, (usize, PathBuf)> = HashMap::new();
        for (n, m) in project.sequence.media.iter().enumerate() {
            let mut add = |name: String, file: PathBuf, desktop: &Path| {
                if file.is_file() && !files.iter().any(|(x, _)| *x == name) {
                    about.insert(name.clone(), (n, desktop.to_path_buf()));
                    files.push((name, file));
                }
            };
            match &m.studio_take {
                Some(folder) => {
                    let base = format!("media/{n}-{}", folder.file_name().unwrap_or_default().to_string_lossy());
                    for f in files_in(folder) {
                        let name = format!("{base}/{}", f.file_name().unwrap_or_default().to_string_lossy());
                        add(name, f.clone(), folder);
                    }
                }
                None => {
                    let name = format!("media/{n}-{}", m.path.file_name().unwrap_or_default().to_string_lossy());
                    add(name, m.path.clone(), &m.path);
                }
            }
            if let Some(px) = is_video(&m.path).then(|| proxy_of(&m.path)).flatten() {
                let name = px.file_name().unwrap_or_default().to_string_lossy().into_owned();
                add(name, px.clone(), &m.path);
            }
        }
        if let Some(only) = p.get("files").and_then(Value::as_array) {
            let only: Vec<&str> = only.iter().filter_map(Value::as_str).collect();
            files.retain(|(n, _)| only.contains(&n.as_str()));
        }
        let t = self.transfers()?.offer_download(device, files).map_err(failed)?;
        let mut out = t.to_json();
        // which medium each file is, and its path here (for the phone's path map)
        if let Some(entries) = out["manifest"].as_array_mut() {
            for e in entries {
                if let Some((n, desktop)) = e["name"].as_str().and_then(|n| about.get(n)) {
                    e["medium"] = json!(n);
                    e["path"] = json!(desktop);
                }
            }
        }
        Ok(json!({"transfer": out, "revision": revision_of(&bytes)}))
    }

    // ------------------------------------------------------------------ edits back

    fn put_take_edit(&self, device: &str, p: &Value) -> Reply {
        let ws = self.workspace()?;
        let item = self.item(&ws, need(p, "take")?)?;
        let edit = p
            .get("edit")
            .filter(|e| e.is_object())
            .ok_or_else(|| invalid("`edit` (the edit.json document) is needed"))?;
        serde_json::from_value::<lightair_render::edit::Edit>(edit.clone())
            .map_err(|e| invalid(format!("not an edit: {e}")))?;
        let folder = take_folder(&item).ok_or_else(|| ApiError::state("Only a studio take has an edit."))?;
        let path = folder.join("edit.json");
        let base = p.get("base_revision").and_then(Value::as_str).unwrap_or("");
        let current = revision(&path);
        if current == base {
            let revision = write_doc(&path, edit).map_err(failed)?;
            self.seen(&ws, &item.id.to_string(), device);
            return Ok(json!({"revision": revision}));
        }
        // both changed: the desktop keeps its own; the phone's is kept beside it
        let name = format!("edit (edited on {}).json", self.device_name(device).replace(['/', ':', '\\'], "-"));
        let copy = folder.join(&name);
        let copy_revision = write_doc(&copy, edit).map_err(failed)?;
        Ok(json!({"conflict": true, "revision": current, "copy": name, "copy_revision": copy_revision}))
    }

    fn put_project(&self, device: &str, p: &Value) -> Reply {
        let ws = self.workspace()?;
        let doc = p
            .get("project")
            .filter(|e| e.is_object())
            .ok_or_else(|| invalid("`project` (the project document) is needed"))?;
        let project: lightair_render::sequence::Project =
            serde_json::from_value(doc.clone()).map_err(|e| invalid(format!("not a project: {e}")))?;
        let dir = lightair_render::sequence::Project::dir(&ws);
        let path = dir.join(format!("{}.json", project.id));
        let current = revision(&path);
        let base = p.get("base_revision").and_then(Value::as_str);
        // new here (no base) and not here yet, or unchanged here since the phone's base
        if (base.is_none() && current.is_empty()) || base == Some(current.as_str()) {
            let revision = write_doc(&path, doc).map_err(failed)?;
            self.seen(&ws, &project.id.to_string(), device);
            return Ok(json!({"revision": revision, "project": project.id}));
        }
        let mut copy = doc.clone();
        let id = Id::new();
        copy["id"] = json!(id);
        copy["name"] = json!(format!("{} (edited on {})", project.name, self.device_name(device)));
        let copy_revision = write_doc(&dir.join(format!("{id}.json")), &copy).map_err(failed)?;
        Ok(
            json!({"conflict": true, "revision": current, "project": project.id, "copy": id, "copy_revision": copy_revision}),
        )
    }

    fn checkout(&self, device: &str, p: &Value, out: bool) -> Reply {
        let ws = self.workspace()?;
        let key = need(p, "item")?;
        let mut all = self.checkouts(&ws);
        if out {
            let name = p
                .get("device_name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| self.device_name(device));
            let since = all.get(key).and_then(|c| c["since_ms"].as_i64()).unwrap_or_else(now_ms);
            all.insert(
                key.to_owned(),
                json!({"device": device, "device_name": name, "since_ms": since, "seen_ms": now_ms()}),
            );
        } else {
            all.remove(key);
        }
        write_doc(&ws.join("checkouts.json"), &json!(all)).map_err(failed)?;
        Ok(json!({}))
    }

    /// The device still has it checked out: its 7 days start again.
    fn seen(&self, ws: &Path, key: &str, device: &str) {
        let mut all = self.checkouts(ws);
        if let Some(c) = all.get_mut(key)
            && c["device"] == device
        {
            c["seen_ms"] = json!(now_ms());
            let _ = write_doc(&ws.join("checkouts.json"), &json!(all));
        }
    }

    // ------------------------------------------------------------------ sent here

    fn upload(&self, device: &str, p: &Value) -> Reply {
        let ws = self.workspace()?;
        let kind = need(p, "kind")?;
        if !matches!(kind, "export" | "project") {
            return Err(invalid("`kind` is export or project"));
        }
        let files: Vec<FileSpec> = p
            .get("files")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(FileSpec::from_json).collect())
            .unwrap_or_default();
        if files.is_empty() {
            return Err(invalid("`files` ([{name, size, sha256}]) is needed"));
        }
        if kind == "project" && !files.iter().any(|f| f.name == "project.json") {
            return Err(invalid("a project is sent as project.json and its media (media/…)"));
        }
        let dir = ws.join("recordings").join("from phones").join(format!("{}", now_ms()));
        let meta = json!({"kind": kind, "project": p.get("project"), "workspace": ws});
        let t = self.transfers()?.offer_upload(device, files, dir, meta).map_err(failed)?;
        Ok(json!({"transfer": t.to_json()}))
    }

    fn status(&self, p: &Value) -> Reply {
        let token = need(p, "token")?;
        let mut s = self
            .transfers()?
            .status(token)
            .ok_or_else(|| ApiError::not_found("No such transfer (it may have ended)."))?;
        if let Some(d) = self.done.lock().get(token) {
            s["item"] = d.clone();
        }
        Ok(s)
    }

    /// Finished uploads: an export indexed, a project put in `edits/` with its media.
    fn take_uploads(&self) {
        let Some(t) = self.transfers.get() else { return };
        for f in t.take_finished() {
            let ws = f.meta["workspace"].as_str().map(PathBuf::from);
            let kind = f.meta["kind"].as_str().unwrap_or("");
            let result = match (kind, ws) {
                ("export", Some(ws)) => self.take_export(&ws, &f),
                ("project", Some(ws)) => self.take_project(&ws, &f),
                _ => Err("an upload of an unknown kind".to_string()),
            };
            let item = match result {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(e, "editor sync: an upload couldn't be taken in");
                    json!({"error": e})
                }
            };
            self.done.lock().insert(f.transfer.token.clone(), item.clone());
            self.events.lock().push(("transfer.done", json!({"token": f.transfer.token, "kind": kind, "item": item})));
        }
    }

    fn take_export(&self, ws: &Path, f: &lightair_remote::Finished) -> Result<Value, String> {
        let from: Option<Id> = f.meta["project"].as_str().and_then(|p| serde_json::from_value(json!(p)).ok());
        let mut ids = vec![];
        for spec in &f.transfer.manifest {
            let path = f.dir.join(&spec.name);
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Export".into());
            let mut item = Item::new(Kind::Export, title, vec![recordings::File::new(path.clone())], now_ms());
            item.from = from;
            item.duration_ms = recordings::mp4_duration_ms(&path);
            ids.push(item.id);
            Index::add_to(ws, item).map_err(|e| e.to_string())?;
        }
        Ok(json!({"exports": ids}))
    }

    fn take_project(&self, ws: &Path, f: &lightair_remote::Finished) -> Result<Value, String> {
        let mut doc: Value =
            serde_json::from_slice(&std::fs::read(f.dir.join("project.json")).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let project: lightair_render::sequence::Project =
            serde_json::from_value(doc.clone()).map_err(|e| e.to_string())?;
        // its media beside the projects, and the document pointed at them
        let media = lightair_render::sequence::Project::dir(ws).join("media").join(project.id.to_string());
        std::fs::create_dir_all(&media).map_err(|e| e.to_string())?;
        for spec in f.transfer.manifest.iter().filter(|s| s.name != "project.json") {
            let rel = spec.name.strip_prefix("media/").unwrap_or(&spec.name);
            let to = media.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::rename(f.dir.join(&spec.name), &to).map_err(|e| e.to_string())?;
        }
        let point = |v: &mut Value| {
            if let Some(s) = v.as_str().and_then(|s| s.strip_prefix("media/")) {
                *v = json!(media.join(s));
            }
        };
        if let Some(ms) = doc["sequence"]["media"].as_array_mut() {
            for m in ms {
                for key in ["path", "studio_take", "look"] {
                    point(&mut m[key]);
                }
            }
        }
        let dir = lightair_render::sequence::Project::dir(ws);
        let mut id = project.id;
        if dir.join(format!("{id}.json")).exists() {
            // already here: kept as a copy, as a conflict would be
            id = Id::new();
            doc["id"] = json!(id);
            doc["name"] = json!(format!("{} (from {})", project.name, self.device_name(&f.device)));
        }
        let revision = write_doc(&dir.join(format!("{id}.json")), &doc).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&f.dir);
        Ok(json!({"project": id, "revision": revision}))
    }
}

fn aspect(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    format!("{}:{}", w / g, h / g)
}

#[cfg(test)]
mod tests {
    use super::{aspect, revision_of, safe_folder_names};

    #[test]
    fn aspects_and_revisions() {
        assert_eq!(aspect(1080, 1920), "9:16");
        assert_eq!(aspect(1920, 1080), "16:9");
        assert_eq!(revision_of(b"{}").len(), 16);
        assert_ne!(revision_of(b"{}"), revision_of(b"{ }"));
        assert!(safe_folder_names());
    }
}

/// (Names made from folders and files never escape a transfer.)
#[cfg(test)]
fn safe_folder_names() -> bool {
    let n = in_folder(
        Path::new("/w/recordings/Studio take 10.32.lightair-take"),
        Path::new("/w/recordings/Studio take 10.32.lightair-take/screen.mp4"),
    );
    lightair_remote::transfer::safe_name(&n) && n == "Studio take 10.32.lightair-take/screen.mp4"
}

#[cfg(test)]
mod round_trip {
    // (not `super::*`: gpui's `test` attribute would shadow the standard one)
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use lightair_model::Id;
    use lightair_remote::{Call, Finished, Transfer, Transfers};
    use lightair_render::sequence::{Media, MediaKind, Project};
    use lightair_store::recordings::{self, File, Index, Item, Kind};
    use serde_json::{Value, json};

    use super::{EditorSync, revision};

    fn call(sync: &EditorSync, method: &str, params: Value) -> Result<Value, String> {
        let c = Call { method: method.into(), params, client: "test", device: Some("ipad-1".into()) };
        sync.call(&c).expect("one of the sync's methods").map_err(|e| e.message)
    }

    fn medium(path: &Path) -> Media {
        Media {
            id: Id::new(),
            name: "clip".into(),
            kind: MediaKind::Video,
            path: path.to_path_buf(),
            duration_ms: 1000,
            width: 1280,
            height: 720,
            audio: vec![],
            studio_take: None,
            item: None,
            look: None,
            transcript: None,
            tracks: vec![],
            zooms: None,
        }
    }

    /// A workspace with a studio take (its edit), a clip and a project using a video.
    fn workspace() -> (PathBuf, Id, Id, Id) {
        let ws = std::env::temp_dir().join(format!("lightair-editor-sync-{}-{}", std::process::id(), Id::new()));
        let take = ws.join("recordings/Studio take 10.32.lightair-take");
        std::fs::create_dir_all(&take).unwrap();
        std::fs::write(take.join("take.json"), json!({"video": [{"name": "Screen", "file": "screen.mp4"}], "audio": ["mic.wav"], "started_ms": 1000, "duration_ms": 5000}).to_string()).unwrap();
        std::fs::write(take.join("screen.mp4"), vec![1u8; 4096]).unwrap();
        std::fs::write(take.join("mic.wav"), vec![2u8; 512]).unwrap();
        std::fs::write(take.join("edit.json"), serde_json::to_vec(&lightair_render::edit::Edit::new(5000)).unwrap())
            .unwrap();
        let take_item = recordings::studio_take_item(&take).unwrap();
        let clip_path = ws.join("recordings/Clip 1.mp4");
        std::fs::write(&clip_path, vec![3u8; 2048]).unwrap();
        let clip = Item::new(Kind::Clip, "Clip 1", vec![File::new(clip_path.clone())], 2000);
        let (take_id, clip_id) = (take_item.id, clip.id);
        Index::add_to(&ws, take_item).unwrap();
        Index::add_to(&ws, clip).unwrap();
        let mut project = Project::new("Vlog", (1080, 1920), 30);
        project.sequence.media.push(medium(&clip_path));
        project.save(&ws).unwrap();
        (ws, take_id, clip_id, project.id)
    }

    fn sync_for(ws: &Path) -> (Arc<EditorSync>, Arc<Transfers>) {
        let sync = Arc::new(EditorSync::default());
        let transfers = Arc::new(Transfers::default());
        let pairing = lightair_remote::Pairing::new(vec![], |_| false, |_| {});
        sync.connect(transfers.clone(), pairing);
        sync.set_workspace(Some(ws.to_path_buf()));
        (sync, transfers)
    }

    #[test]
    fn takes_and_clips_are_listed_fetched_and_edited() {
        let (ws, take, clip, _) = workspace();
        let (sync, _) = sync_for(&ws);

        let list = call(&sync, "takes.list", json!({})).unwrap();
        let items = list["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "takes, recordings and exports only: {list}");
        let t = &items[0];
        assert_eq!((t["kind"].as_str(), t["name"].as_str()), (Some("take"), Some("Studio take 10.32")));
        let edit = ws.join("recordings/Studio take 10.32.lightair-take/edit.json");
        assert_eq!(t["revision"].as_str().unwrap(), revision(&edit));
        assert!(list["cursor"].as_i64().unwrap() > 0);
        let clips = call(&sync, "clips.list", json!({})).unwrap();
        assert_eq!(clips["clips"][0]["id"], json!(clip));
        // nothing new since the cursor
        let later = call(&sync, "takes.list", json!({"since": list["cursor"]})).unwrap();
        assert!(later["items"].as_array().unwrap().is_empty());

        // fetched: the take folder's files, the folder kept in their names
        let f = call(&sync, "takes.fetch", json!({"take": take})).unwrap();
        let mut names: Vec<&str> =
            f["transfer"]["manifest"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "Studio take 10.32.lightair-take/edit.json",
                "Studio take 10.32.lightair-take/mic.wav",
                "Studio take 10.32.lightair-take/screen.mp4",
                "Studio take 10.32.lightair-take/take.json"
            ]
        );
        assert_eq!(f["transfer"]["token"].as_str().unwrap().len(), 22);

        // an edit back from the revision it started at: taken
        let base = t["revision"].as_str().unwrap().to_owned();
        let mut changed = serde_json::to_value(lightair_render::edit::Edit::new(5000)).unwrap();
        changed["size"] = json!([1080, 1920]);
        let r = call(&sync, "takes.put_edit", json!({"take": take, "edit": changed, "base_revision": base})).unwrap();
        assert_ne!(r["revision"], json!(base));
        assert_eq!(r["revision"].as_str().unwrap(), revision(&edit));
        // another from the old revision: the desktop's kept, the phone's beside it
        let r = call(&sync, "takes.put_edit", json!({"take": take, "edit": changed, "base_revision": base})).unwrap();
        assert_eq!(r["conflict"], true);
        let copy = r["copy"].as_str().unwrap();
        assert!(ws.join("recordings/Studio take 10.32.lightair-take").join(copy).is_file());
        // not an edit at all
        assert!(
            call(&sync, "takes.put_edit", json!({"take": take, "edit": {"nonsense": 1}, "base_revision": ""})).is_err()
        );

        // checked out, and back in
        call(&sync, "takes.checkout", json!({"item": take, "device_name": "Sam's iPad"})).unwrap();
        let list = call(&sync, "takes.list", json!({})).unwrap();
        assert_eq!(list["items"][0]["checked_out"], "Sam's iPad");
        call(&sync, "takes.checkin", json!({"item": take})).unwrap();
        assert!(call(&sync, "takes.list", json!({})).unwrap()["items"][0]["checked_out"].is_null());

        // only for paired devices
        let c = Call { method: "takes.fetch".into(), params: json!({"take": take}), client: "JSON-RPC", device: None };
        assert!(sync.call(&c).unwrap().is_err());
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn projects_go_out_and_come_back() {
        let (ws, _, _, project) = workspace();
        let (sync, transfers) = sync_for(&ws);

        let list = call(&sync, "edits.list", json!({})).unwrap();
        let p = &list["projects"][0];
        assert_eq!((p["name"].as_str(), p["aspect"].as_str()), (Some("Vlog"), Some("9:16")));
        let base = p["revision"].as_str().unwrap().to_owned();

        // the document and its media, each medium's desktop path in the manifest
        let f = call(&sync, "edits.fetch", json!({"project": project})).unwrap();
        let manifest = f["transfer"]["manifest"].as_array().unwrap();
        assert_eq!(manifest[0]["name"], "project.json");
        let m = manifest.iter().find(|m| m["name"] == "media/0-Clip 1.mp4").expect("the clip");
        assert_eq!(
            (m["medium"].as_u64(), m["path"].as_str().map(|p| p.ends_with("recordings/Clip 1.mp4"))),
            (Some(0), Some(true))
        );

        // back with changes: taken; again from the old revision: a copy
        let path = Project::dir(&ws).join(format!("{project}.json"));
        let mut doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        doc["name"] = json!("Vlog, cut");
        let r = call(&sync, "edits.put", json!({"project": doc, "base_revision": base})).unwrap();
        assert_eq!(r["revision"].as_str().unwrap(), revision(&path));
        let r = call(&sync, "edits.put", json!({"project": doc, "base_revision": base})).unwrap();
        assert_eq!(r["conflict"], true);
        let copy: Id = serde_json::from_value(r["copy"].clone()).unwrap();
        let copied = Project::load(&ws, copy).unwrap();
        assert_eq!(copied.name, "Vlog, cut (edited on a phone)");

        // a project made on the phone, sent here: its media beside the projects, paths pointed there
        let mut new = Project::new("From the phone", (1080, 1080), 30);
        new.sequence.media.push(medium(Path::new("media/walk.mp4")));
        let up = ws.join("recordings/from phones/1");
        std::fs::create_dir_all(up.join("media")).unwrap();
        std::fs::write(up.join("project.json"), serde_json::to_vec(&new).unwrap()).unwrap();
        std::fs::write(up.join("media/walk.mp4"), vec![9u8; 100]).unwrap();
        let spec = |n: &str| lightair_remote::FileSpec { name: n.into(), size: 0, sha256: String::new() };
        let finished = Finished {
            transfer: Transfer { token: "t".into(), manifest: vec![spec("project.json"), spec("media/walk.mp4")] },
            device: "ipad-1".into(),
            dir: up.clone(),
            meta: json!({"kind": "project", "workspace": ws}),
        };
        let r = sync.take_project(&ws, &finished).unwrap();
        let id: Id = serde_json::from_value(r["project"].clone()).unwrap();
        let landed = Project::load(&ws, id).unwrap();
        let walk = Project::dir(&ws).join("media").join(new.id.to_string()).join("walk.mp4");
        assert_eq!(landed.sequence.media[0].path, walk);
        assert!(walk.is_file() && !up.exists());
        drop(transfers);
        let _ = std::fs::remove_dir_all(ws);
    }
}
