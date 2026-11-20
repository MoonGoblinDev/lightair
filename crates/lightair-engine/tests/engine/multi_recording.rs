//! Recording several canvases at once: each to its own file at its own size, paused and
//! stopped together. Verified with ffprobe.

use std::path::Path;
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Canvas, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::outputs::{Container, OutputKey, RecordingConfig, default_encoders, runtime};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

/// (width, height, seconds) of a file's video.
fn probe(path: &Path) -> (u64, u64, f64) {
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height:format=duration"])
        .args(["-of", "json"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let s = &v["streams"][0];
    let d = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    (s["width"].as_u64().unwrap_or(0), s["height"].as_u64().unwrap_or(0), d)
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
fn two_canvases_record_together() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    if !Proc::new("ffprobe").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success()) {
        eprintln!("ffprobe not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lightair-multirec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    // A landscape (primary) and a vertical canvas, one scene laid out in both.
    let mut doc = Document::new(Workspace::new("Two"));
    let landscape = doc.workspace().primary_canvas().id;
    let vertical = Canvas { width: 360, height: 640, ..Canvas::vertical() };
    let vid = vertical.id;
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 640, "height": 640}));
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new(
        "Colour",
        colour.id,
        [(landscape, Layout::boxed(0.0, 0.0, 640.0, 360.0)), (vid, Layout::boxed(0.0, 0.0, 360.0, 640.0))],
    ));
    let scene_id = scene.id;
    for cmd in [
        Command::AddCanvas { canvas: vertical },
        Command::AddSource { source: colour },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }

    let encoders = default_encoders(&engine.capabilities.encoders, 2500, false);
    let files: Vec<_> = [(landscape, "Landscape"), (vid, "Vertical")]
        .into_iter()
        .map(|(canvas, label)| {
            let path = dir.join(format!("take {label}.mp4"));
            let cfg = RecordingConfig {
                extra_mixes: vec![],
                canvas,
                path: path.clone(),
                label: Some(label.into()),
                split_minutes: None,
                container: Container::HybridMp4,
                encoders: encoders.clone(),
            };
            engine
                .call(move |s| {
                    let video = s.reconciler.canvas_video(canvas);
                    s.outputs.start_recording(&cfg, video)
                })
                .expect("recording starts");
            (canvas, path)
        })
        .collect();
    assert!(wait(Duration::from_secs(5), || files.iter().all(|(c, _)| matches!(
        runtime().get(OutputKey::Recording(*c)).state,
        lightair_engine::outputs::OutputState::Active { .. }
    ))));
    assert!(runtime().any_recording_running());

    std::thread::sleep(Duration::from_millis(700));
    // Paused together.
    assert!(engine.call(|s| s.outputs.pause_recording(true)), "both paused");
    assert!(files.iter().all(|(c, _)| runtime().get(OutputKey::Recording(*c)).paused));
    std::thread::sleep(Duration::from_millis(500));
    assert!(engine.call(|s| s.outputs.pause_recording(false)), "both resumed");
    std::thread::sleep(Duration::from_millis(300));
    // A pause and resume quicker than a frame (a double-click) must not wedge the recording.
    engine.call(|s| {
        s.outputs.pause_recording(true);
        s.outputs.pause_recording(false);
    });
    std::thread::sleep(Duration::from_millis(600));
    // Stopped together.
    engine.call(|s| s.outputs.stop_recording());
    let stopped = wait(Duration::from_secs(5), || !runtime().any_recording_running());
    for (c, _) in &files {
        eprintln!("after stop {c}: {:?}", runtime().get(OutputKey::Recording(*c)).state);
    }
    assert!(stopped, "both stopped");

    let (lw, lh, ld) = probe(&files[0].1);
    let (vw, vh, vd) = probe(&files[1].1);
    eprintln!("landscape {lw}x{lh} {ld:.2}s, vertical {vw}x{vh} {vd:.2}s");
    assert_eq!((lw, lh), (640, 360), "the landscape canvas at its size");
    assert_eq!((vw, vh), (360, 640), "the vertical canvas at its size");
    assert!(ld > 0.8 && vd > 0.8, "both have video");
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
