//! Encoding options reach the file: codec (HEVC), scaled size and keyframe interval, checked with
//! ffprobe. Skipped where there is no hardware HEVC encoder or no ffprobe.

use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::outputs::{Container, OutputState, RecordingConfig, default_encoders, runtime};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

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
fn hevc_scaled_with_keyframes() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    if !Proc::new("ffprobe").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success()) {
        eprintln!("ffprobe not installed; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let Some(hevc) =
        engine.capabilities.encoders.iter().find(|e| e.contains("videotoolbox") && e.contains("hevc")).cloned()
    else {
        eprintln!("no hardware HEVC encoder; skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("lightair-encoding-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = Document::new(Workspace::new("Enc"));
    let f = doc.workspace().primary_canvas().id;
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 640, "height": 360}));
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Colour", colour.id, [(f, Layout::boxed(0.0, 0.0, 640.0, 360.0))]));
    let sid = scene.id;
    for cmd in [
        Command::AddSource { source: colour },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(sid) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }
    let mut encoders = default_encoders(&engine.capabilities.encoders, 2000, false);
    encoders.video_encoder = hevc;
    encoders.keyint_sec = 1;
    encoders.scaled = Some((320, 180));
    let file = dir.join("hevc.mp4");
    let cfg = RecordingConfig {
        extra_mixes: vec![],
        canvas: f,
        path: file.clone(),
        label: None,
        split_minutes: None,
        container: Container::HybridMp4,
        encoders,
    };
    engine.call(move |s| s.outputs.start_recording(&cfg, None)).expect("recording starts");
    assert!(wait(Duration::from_secs(5), || matches!(runtime().recording().state, OutputState::Active { .. })));
    std::thread::sleep(Duration::from_millis(2200));
    engine.call(|s| s.outputs.stop_recording());
    assert!(wait(Duration::from_secs(5), || !runtime().any_recording_running()));

    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height"])
        .args(["-show_entries", "packet=flags", "-of", "json"])
        .arg(&file)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let s = &v["streams"][0];
    let keyframes = v["packets"]
        .as_array()
        .map_or(0, |p| p.iter().filter(|p| p["flags"].as_str().unwrap_or("").starts_with('K')).count());
    eprintln!("{} {}x{}, {keyframes} keyframes", s["codec_name"], s["width"], s["height"]);
    assert_eq!(s["codec_name"], "hevc");
    assert_eq!((s["width"].as_u64(), s["height"].as_u64()), (Some(320), Some(180)), "scaled");
    assert!(keyframes >= 2, "a keyframe every second over ~2 s: {keyframes}");
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
