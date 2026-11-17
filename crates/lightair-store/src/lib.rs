//! Workspace packages on disk (docs/14-data-persistence-settings.md).
//!
//! ```text
//! My Show.lightair/
//! ├─ workspace.json     manifest: canvas version, id, name, canvases, program, scene order
//! ├─ sources.json      media sources
//! ├─ audio.json       mixer tracks and their effects
//! ├─ mixer.json       buses, mixes and the master
//! ├─ outputs.json     recording and streaming settings (no secrets)
//! ├─ scenes/<id>.json one file per scene (diff-friendly)
//! └─ journal/current.log   committed commands since the last compaction (JSON lines)
//! ```
//!
//! Every committed command is appended to the journal (there is no Save button). Compaction
//! writes the JSON files atomically and truncates the journal. Opening a package replays any
//! journal tail, which is how a crash is recovered.

pub mod import_obs;

pub mod input_log;
pub mod recordings;
pub mod studio_take_scene;
pub use import_obs::{EstimatedObject, ImportError, ImportReport, import_obs_collection};

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightair_core::{Command, Canvas, Id, Source, OutputSettings, Workspace, Scene, Track};
use serde::{Deserialize, Serialize};

/// Current on-disk canvas. Bump it and add a migration in [`migrate`] when the schema changes.
/// 2: capture sources use Lightair's portable kinds (lightair_core::kinds).
pub const FORMAT_VERSION: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
    #[error("this workspace was saved by a newer Lightair (format {found}, this version reads up to {FORMAT_VERSION})")]
    TooNew { found: u32 },
    #[error("scene file missing for {0}")]
    MissingScene(Id),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io { path: path.to_path_buf(), source }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    id: Id,
    name: String,
    #[serde(alias = "formats")]
    canvases: Vec<Canvas>,
    program: Option<Id>,
    /// Scene ids in display order; scene contents live in `scenes/<id>.json`.
    scenes: Vec<Id>,
    #[serde(default)]
    transition: lightair_core::TransitionSettings,
    #[serde(default)]
    stinger: Option<lightair_core::Stinger>,
}

/// Writes `bytes` to `path` atomically (temp file + rename), so a crash never leaves a torn file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp).map_err(io(&tmp))?;
        f.write_all(bytes).map_err(io(&tmp))?;
        f.sync_all().map_err(io(&tmp))?;
    }
    fs::rename(&tmp, path).map_err(io(path))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| StoreError::Json { path: path.into(), source })?;
    write_atomic(path, &bytes)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path).map_err(io(path))?;
    serde_json::from_slice(&bytes).map_err(|source| StoreError::Json { path: path.into(), source })
}

/// Writes the whole workspace into `dir` (created if needed). Scene files for removed scenes are
/// deleted.
pub fn save(dir: &Path, workspace: &Workspace) -> Result<(), StoreError> {
    let scenes_dir = dir.join("scenes");
    fs::create_dir_all(&scenes_dir).map_err(io(&scenes_dir))?;
    for scene in &workspace.scenes {
        write_json(&scenes_dir.join(format!("{}.json", scene.id)), &**scene)?;
    }
    let keep: std::collections::HashSet<String> = workspace.scenes.iter().map(|s| format!("{}.json", s.id)).collect();
    for entry in fs::read_dir(&scenes_dir).map_err(io(&scenes_dir))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") && !keep.contains(&name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    let sources: Vec<&Source> = workspace.sources.values().map(|i| &**i).collect();
    write_json(&dir.join("sources.json"), &sources)?;
    // (before the rename, the sources were saved as inputs.json)
    let _ = fs::remove_file(dir.join("inputs.json"));
    let tracks: Vec<&Track> = workspace.tracks.iter().map(|t| &**t).collect();
    write_json(&dir.join("audio.json"), &tracks)?;
    write_json(&dir.join("mixer.json"), &workspace.audio)?;
    write_json(&dir.join("outputs.json"), &workspace.outputs)?;
    // The manifest goes last: it is what makes the new scene files "current".
    write_json(
        &manifest(dir),
        &Manifest {
            format_version: FORMAT_VERSION,
            id: workspace.id,
            name: workspace.name.clone(),
            canvases: workspace.canvases.clone(),
            program: workspace.program,
            scenes: workspace.scenes.iter().map(|s| s.id).collect(),
            transition: workspace.transition,
            stinger: workspace.stinger.clone(),
        },
    )
}

/// A workspace's manifest file (the scenes' order, its canvases…): `workspace.json`. A
/// workspace saved before the rename has `project.json`; it's renamed the first time it's read.
pub fn manifest(dir: &Path) -> std::path::PathBuf {
    let new = dir.join("workspace.json");
    let old = dir.join("project.json");
    if !new.exists()
        && old.exists()
        && let Err(e) = fs::rename(&old, &new)
    {
        tracing::warn!(%e, dir = %dir.display(), "couldn't rename project.json to workspace.json");
        return old;
    }
    new
}

/// Whether `dir` holds a workspace.
pub fn is_workspace(dir: &Path) -> bool {
    dir.join("workspace.json").exists() || dir.join("project.json").exists()
}

/// Reads the workspace files in `dir` (without replaying the journal).
pub fn load_snapshot(dir: &Path) -> Result<Workspace, StoreError> {
    let raw: serde_json::Value = read_json(&manifest(dir))?;
    let version = raw.get("formatVersion").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if version > FORMAT_VERSION {
        return Err(StoreError::TooNew { found: version });
    }
    let manifest: Manifest = serde_json::from_value(migrate(raw, version))
        .map_err(|source| StoreError::Json { path: manifest(dir), source })?;
    let sources_file = if dir.join("sources.json").exists() { dir.join("sources.json") } else { dir.join("inputs.json") };
    let sources: Vec<Source> = read_json(&sources_file)?;
    let audio = dir.join("audio.json");
    let tracks: Vec<Track> = if audio.exists() { read_json(&audio)? } else { Vec::new() };
    let mixer = dir.join("mixer.json");
    let audio_setup: lightair_core::AudioSetup = if mixer.exists() { read_json(&mixer)? } else { Default::default() };
    let outputs_path = dir.join("outputs.json");
    let outputs: OutputSettings =
        if outputs_path.exists() { read_json(&outputs_path)? } else { OutputSettings::default() };
    let mut scenes = Vec::with_capacity(manifest.scenes.len());
    for id in &manifest.scenes {
        let path = dir.join("scenes").join(format!("{id}.json"));
        if !path.exists() {
            return Err(StoreError::MissingScene(*id));
        }
        scenes.push(Arc::new(read_json::<Scene>(&path)?));
    }
    let mut workspace = Workspace {
        id: manifest.id,
        name: manifest.name,
        canvases: manifest.canvases,
        sources: sources.into_iter().map(|i| (i.id, Arc::new(i))).collect::<BTreeMap<_, _>>(),
        scenes,
        program: manifest.program,
        tracks: tracks.into_iter().map(Arc::new).collect(),
        audio: audio_setup,
        outputs,
        transition: manifest.transition,
        stinger: manifest.stinger,
    };
    // canvas 1 stored libobs's macOS ids
    lightair_core::kinds::migrate(&mut workspace);
    Ok(workspace)
}

/// Upgrades an older manifest to the current canvas, one version at a time.
fn migrate(manifest: serde_json::Value, from: u32) -> serde_json::Value {
    let mut value = manifest;
    for version in from..FORMAT_VERSION {
        value = match version {
            0 => {
                // Pre-release packages had no version field; nothing else changed.
                let mut v = value;
                v["formatVersion"] = 1.into();
                v
            }
            // 1 → 2: the sources' kinds (load_snapshot); the manifest is the same
            _ => value,
        };
    }
    value
}

/// An open workspace package: the journal for committed commands, plus compaction.
pub struct Package {
    dir: PathBuf,
    journal: File,
    pending: usize,
}

/// What [`Package::open`] found.
pub struct Opened {
    pub package: Package,
    pub workspace: Workspace,
    /// Commands replayed from the journal (non-zero means the last session didn't close cleanly).
    pub recovered: usize,
}

impl Package {
    /// Opens (or creates, with `initial`) the package at `dir`, replaying any journal tail.
    pub fn open(dir: &Path, initial: impl FnOnce() -> Workspace) -> Result<Opened, StoreError> {
        let mut stale = false;
        let mut workspace = if is_workspace(dir) {
            stale = read_json::<serde_json::Value>(&manifest(dir))?
                .get("formatVersion")
                .and_then(|v| v.as_u64())
                .is_none_or(|v| v < FORMAT_VERSION as u64);
            load_snapshot(dir)?
        } else {
            let p = initial();
            save(dir, &p)?;
            p
        };
        let journal_dir = dir.join("journal");
        fs::create_dir_all(&journal_dir).map_err(io(&journal_dir))?;
        let journal_path = journal_dir.join("current.log");
        let mut recovered = 0;
        if journal_path.exists() {
            let file = File::open(&journal_path).map_err(io(&journal_path))?;
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                // A torn last line (crash mid-write) is ignored.
                let Ok(cmd) = serde_json::from_str::<Command>(&line) else { break };
                if cmd.apply(&mut workspace).is_ok() {
                    recovered += 1;
                }
            }
        }
        // a journal from before portable kinds adds libobs ids
        let migrated = lightair_core::kinds::migrate(&mut workspace);
        let journal = OpenOptions::new().create(true).append(true).open(&journal_path).map_err(io(&journal_path))?;
        let mut package = Package { dir: dir.to_path_buf(), journal, pending: 0 };
        // an older canvas is saved in this one
        if recovered > 0 || migrated || stale {
            tracing::warn!(recovered, "replayed unsaved changes from the journal");
            package.compact(&workspace)?;
        }
        Ok(Opened { package, workspace, recovered })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Appends a committed command. Undo/redo are recorded as their resulting snapshot via
    /// [`Package::compact`] instead.
    pub fn record(&mut self, command: &Command) -> Result<(), StoreError> {
        let path = self.dir.join("journal/current.log");
        let mut line = serde_json::to_vec(command).map_err(|source| StoreError::Json { path: path.clone(), source })?;
        line.push(b'\n');
        self.journal.write_all(&line).map_err(io(&path))?;
        self.pending += 1;
        Ok(())
    }

    /// Makes appended commands durable (fsync). Called on a timer and before outputs start/stop.
    pub fn flush(&mut self) -> Result<(), StoreError> {
        let path = self.dir.join("journal/current.log");
        self.journal.sync_data().map_err(io(&path))
    }

    /// Commands recorded since the last compaction.
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// Writes the workspace files and empties the journal.
    pub fn compact(&mut self, workspace: &Workspace) -> Result<(), StoreError> {
        save(&self.dir, workspace)?;
        let path = self.dir.join("journal/current.log");
        // Emptied through a handle that may write anywhere: Windows opens an append-only file
        // without the right to truncate it. Appends still land at the (new) end.
        let truncate = OpenOptions::new().write(true).truncate(true).open(&path).map_err(io(&path))?;
        truncate.sync_all().map_err(io(&path))?;
        self.pending = 0;
        Ok(())
    }
}
