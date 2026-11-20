//! Recording pause, markers (MP4 chapters) and split, verified with ffprobe.

use std::path::Path;
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::outputs::{Container, OutputState, RecordingConfig, default_encoders, runtime};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn probe(path: &Path, entries: &str) -> serde_json::Value {
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-show_entries", entries, "-of", "json"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    serde_json::from_slice(&out.stdout).unwrap_or_default()
}

fn duration(path: &Path) -> f64 {
    probe(path, "format=duration")["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0)
}

fn wait(timeout: Duration, pred: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while !pred() {
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    true
}

#[test]
fn pause_marker_split() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let ffprobe = Proc::new("ffprobe").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ffprobe {
        eprintln!("ffprobe not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lightair-rectools-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Rec"));
    let f = doc.workspace().primary_canvas().id;
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 640, "height": 360}));
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Colour", colour.id, [(f, Layout::boxed(0.0, 0.0, 640.0, 360.0))]));
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: colour },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }

    let first = dir.join("take.mp4");
    let cfg = RecordingConfig {
        extra_mixes: vec![],
        canvas: lightair_model::Id::new(),
        label: None,
        split_minutes: None,
        path: first.clone(),
        container: Container::HybridMp4,
        // a keyframe a second, so the split lands soon
        encoders: lightair_engine::outputs::EncoderConfig {
            keyint_sec: 1,
            ..default_encoders(&engine.capabilities.encoders, 2500, false)
        },
    };
    engine.call(move |s| s.outputs.start_recording(&cfg, None)).expect("recording starts");
    assert!(wait(Duration::from_secs(3), || matches!(runtime().recording().state, OutputState::Active { .. })));
    let started = Instant::now();

    std::thread::sleep(Duration::from_millis(800));
    assert!(engine.call(|s| s.outputs.add_marker("Intro")), "marker added");
    assert!(engine.call(|s| s.outputs.pause_recording(true)), "paused");
    let paused = Instant::now();
    assert!(runtime().recording().paused);
    std::thread::sleep(Duration::from_millis(1200));
    assert!(engine.call(|s| s.outputs.pause_recording(false)), "resumed");
    let paused = paused.elapsed().as_secs_f64();
    assert!(!runtime().recording().paused);
    std::thread::sleep(Duration::from_millis(800));

    assert!(engine.call(|s| s.outputs.split_recording()), "split accepted");
    assert!(wait(Duration::from_secs(5), || runtime().recording().files == 2), "file changed");
    let second = runtime().recording().path.expect("new path");
    assert_ne!(second, first);
    std::thread::sleep(Duration::from_millis(800));
    engine.call(|s| s.outputs.stop_recording());
    let recorded = started.elapsed().as_secs_f64() - paused;
    assert!(wait(Duration::from_secs(5), || !runtime().recording().state.is_running()));
    assert_eq!(runtime().recording().markers, 1);

    let (d1, d2) = (duration(&first), duration(&second));
    let chapters = probe(&first, "chapter=start_time:chapter_tags=title");
    eprintln!("first {d1:.2}s, second {} {d2:.2}s, chapters {chapters}", second.display());
    // About 3.8 s of wall time, 1.2 s of it paused: the files hold the rest (the split lands on
    // the next keyframe, so time moves between the two files, not in or out). Counting the
    // paused time would add over a second.
    assert!(d1 > 1.2 && d2 > 0.4, "both files have video: {d1} {d2}");
    let held = d1 + d2;
    assert!(
        held > recorded - 0.6 && held < recorded + paused * 0.6,
        "paused time excluded: {d1} + {d2} for {recorded:.2} s recorded and {paused:.2} s paused"
    );
    let list = chapters["chapters"].as_array().cloned().unwrap_or_default();
    assert!(list.iter().any(|c| c["tags"]["title"] == "Intro"), "chapter present: {chapters}");

    // A marker or split without a running recording is refused.
    assert!(!engine.call(|s| s.outputs.add_marker("Late")));
    assert!(!engine.call(|s| s.outputs.split_recording()));
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
