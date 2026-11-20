use std::io::Write;

use lightair_core::{Command, Source, Object, Layout, Workspace, Scene};
use lightair_store::{Package, StoreError, load_snapshot, save};
use serde_json::json;

fn sample() -> (Workspace, Vec<Command>) {
    let workspace = Workspace::new("Show");
    let canvas = workspace.primary_canvas().id;
    let cam = Source::new("Camera", lightair_core::kinds::CAMERA, json!({"device": "abc"}));
    let scene = Scene::new("Main");
    let object = Object::new("Camera", cam.id, [(canvas, Layout::boxed(0.0, 0.0, 1920.0, 1080.0))]);
    let cmds = [Command::AddSource { source: cam }, Command::AddObject { scene: scene.id, object, index: None }];
    let mut cmds_full = vec![Command::AddScene { scene, index: None }];
    cmds_full.splice(0..0, cmds[..1].to_vec());
    cmds_full.push(cmds[1].clone());
    (workspace, cmds_full)
}

#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (mut workspace, cmds) = sample();
    for c in &cmds {
        c.apply(&mut workspace).unwrap();
    }
    save(dir.path(), &workspace).unwrap();
    assert_eq!(load_snapshot(dir.path()).unwrap(), workspace);
    assert!(dir.path().join("scenes").read_dir().unwrap().count() == 1);
}

#[test]
fn journal_replays_after_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let (initial, cmds) = sample();
    let mut expected = initial.clone();
    {
        let mut opened = Package::open(dir.path(), || initial.clone()).unwrap();
        for c in &cmds {
            c.apply(&mut opened.workspace).unwrap();
            c.apply(&mut expected).unwrap();
            opened.package.record(c).unwrap();
        }
        opened.package.flush().unwrap();
        // Simulate a crash mid-write: a torn final line.
        let mut log = std::fs::OpenOptions::new().append(true).open(dir.path().join("journal/current.log")).unwrap();
        log.write_all(b"{\"RenameScene\":{\"scene\"").unwrap();
        // No compaction: the process "dies" here.
    }
    let reopened = Package::open(dir.path(), || unreachable!("package exists")).unwrap();
    assert_eq!(reopened.recovered, 3);
    assert_eq!(reopened.workspace, expected);
    // Recovery compacts, so a second open replays nothing.
    drop(reopened);
    let again = Package::open(dir.path(), || unreachable!()).unwrap();
    assert_eq!(again.recovered, 0);
    assert_eq!(again.workspace, expected);
}

#[test]
fn removed_scenes_lose_their_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut workspace, cmds) = sample();
    for c in &cmds {
        c.apply(&mut workspace).unwrap();
    }
    save(dir.path(), &workspace).unwrap();
    let id = workspace.scenes[0].id;
    Command::RemoveScene { scene: id }.apply(&mut workspace).unwrap();
    save(dir.path(), &workspace).unwrap();
    assert_eq!(dir.path().join("scenes").read_dir().unwrap().count(), 0);
}

#[test]
fn newer_formats_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (workspace, _) = sample();
    save(dir.path(), &workspace).unwrap();
    let path = dir.path().join("workspace.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["formatVersion"] = 99.into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(matches!(load_snapshot(dir.path()), Err(StoreError::TooNew { found: 99 })));
}

#[test]
fn a_format_1_workspace_opens_with_portable_kinds_and_is_saved_upgraded() {
    let dir = tempfile::tempdir().unwrap();
    let mut workspace = Workspace::new("Old");
    let screen = Source::new("Display", "screen_capture", json!({"type": 0, "display_uuid": "U", "show_cursor": false}));
    let mic = Source::new("Mic", "coreaudio_input_capture", json!({"device_id": "default"}));
    for source in [screen, mic] {
        Command::AddSource { source }.apply(&mut workspace).unwrap();
    }
    save(dir.path(), &workspace).unwrap();
    // as a canvas-1 Lightair wrote it
    let manifest = dir.path().join("workspace.json");
    let mut m: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    m["formatVersion"] = 1.into();
    std::fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();

    let opened = Package::open(dir.path(), || Workspace::new("unused")).unwrap();
    let by_name = |p: &Workspace, n: &str| p.sources.values().find(|i| i.name == n).unwrap().as_ref().clone();
    let d = by_name(&opened.workspace, "Display");
    assert_eq!(d.kind, lightair_core::kinds::SCREEN);
    assert_eq!(d.settings, json!({"mode": 0, "display": "U", "cursor": false}));
    assert_eq!(by_name(&opened.workspace, "Mic").settings, json!({"device": "default"}));
    // saved in the new canvas: the files say so too
    let m: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(m["formatVersion"], lightair_store::FORMAT_VERSION);
    assert_eq!(by_name(&load_snapshot(dir.path()).unwrap(), "Mic").kind, lightair_core::kinds::MIC);
}

#[test]
fn a_workspace_saved_as_a_project_opens_and_is_renamed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut workspace, cmds) = sample();
    for c in &cmds {
        c.apply(&mut workspace).unwrap();
    }
    save(dir.path(), &workspace).unwrap();
    // as Lightair saved it before workspaces were called workspaces
    std::fs::rename(dir.path().join("workspace.json"), dir.path().join("project.json")).unwrap();
    assert!(lightair_store::is_workspace(dir.path()));
    assert_eq!(load_snapshot(dir.path()).unwrap(), workspace);
    assert!(dir.path().join("workspace.json").exists() && !dir.path().join("project.json").exists());
}

/// Compaction writes on the package's own thread; commands recorded meanwhile go to the next
/// journal and aren't lost.
#[test]
fn compaction_in_the_background_keeps_what_comes_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let (initial, cmds) = sample();
    let mut expected = initial.clone();
    {
        let mut opened = Package::open(dir.path(), || initial.clone()).unwrap();
        let (first, rest) = cmds.split_at(1);
        for c in first {
            c.apply(&mut expected).unwrap();
            opened.package.record(c).unwrap();
        }
        opened.package.compact(&expected).unwrap();
        // recorded while the snapshot may still be being written
        for c in rest {
            c.apply(&mut expected).unwrap();
            opened.package.record(c).unwrap();
        }
        opened.package.flush().unwrap();
        opened.package.wait();
        assert_eq!(load_snapshot(dir.path()).unwrap().sources.len(), 1, "the snapshot has the first command");
    }
    let reopened = Package::open(dir.path(), || unreachable!()).unwrap();
    assert_eq!(reopened.recovered, 2, "the rest come from the journal");
    assert_eq!(reopened.workspace, expected);
}

/// A crash before a compaction's snapshot was written: the rotated journal is replayed, then
/// the current one.
#[test]
fn a_rotated_journal_not_yet_saved_is_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let (initial, cmds) = sample();
    let mut expected = initial.clone();
    {
        let mut opened = Package::open(dir.path(), || initial.clone()).unwrap();
        for c in &cmds {
            c.apply(&mut expected).unwrap();
            opened.package.record(c).unwrap();
        }
        opened.package.flush().unwrap();
        opened.package.wait();
    }
    // as the writer would have left it, had the app died before the snapshot: the first two
    // commands in a rotated journal, the last in the current one
    let journal = dir.path().join("journal");
    let text = std::fs::read_to_string(journal.join("current.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    std::fs::write(journal.join("1.log"), format!("{}
{}
", lines[0], lines[1])).unwrap();
    std::fs::write(journal.join("current.log"), format!("{}
", lines[2])).unwrap();
    let reopened = Package::open(dir.path(), || unreachable!()).unwrap();
    assert_eq!(reopened.recovered, 3);
    assert_eq!(reopened.workspace, expected);
    drop(reopened);
    assert!(!journal.join("1.log").exists(), "saved, then deleted");
    assert_eq!(Package::open(dir.path(), || unreachable!()).unwrap().recovered, 0);
}

/// A crash after a compaction's snapshot but before its journal was deleted: not replayed
/// again (the manifest says the snapshot has it).
#[test]
fn a_rotated_journal_already_saved_is_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let (initial, cmds) = sample();
    let mut expected = initial.clone();
    {
        let mut opened = Package::open(dir.path(), || initial.clone()).unwrap();
        for c in &cmds {
            c.apply(&mut expected).unwrap();
            opened.package.record(c).unwrap();
        }
        let journal = dir.path().join("journal");
        let text = std::fs::read_to_string(journal.join("current.log")).unwrap();
        opened.package.compact(&expected).unwrap();
        opened.package.wait();
        // the deleted journal back, as if the delete never happened
        std::fs::write(journal.join("1.log"), text).unwrap();
    }
    let reopened = Package::open(dir.path(), || unreachable!()).unwrap();
    assert_eq!(reopened.recovered, 0);
    assert_eq!(reopened.workspace, expected);
}
