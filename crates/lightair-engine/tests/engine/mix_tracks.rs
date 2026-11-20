//! A recording carries mixes as extra audio tracks: the program first, then each chosen mix.

use std::process::Command as Proc;
use std::time::{Duration, Instant};

use lightair_model::{AudioMix, Command, Document, Source, Object, Layout, Workspace, Scene, Send, Track, diff};
use lightair_engine::outputs::{Container, OutputKey, OutputState, RecordingConfig, default_encoders, runtime};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

#[test]
fn a_recording_carries_the_program_and_a_mix() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    if Proc::new("ffprobe").arg("-version").output().is_err() {
        eprintln!("ffprobe not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lightair-mixtracks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Tracks"));
    let canvas = doc.workspace().primary_canvas().id;
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 320, "height": 180}));
    let tone = Source::new("Tone", "lightair_test_tone", json!({"freq": 440.0, "amplitude": 0.3}));
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Colour", colour.id, [(canvas, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let vod = AudioMix::new("VOD");
    let mut track = Track::new("Tone", tone.id);
    track.sends = vec![Send { mix: vod.id, level_db: 0.0, pre_fader: false }];
    for cmd in [
        Command::AddSource { source: colour },
        Command::AddSource { source: tone },
        Command::AddScene { scene, index: None },
        Command::AddMix { mix: vod },
        Command::AddTrack { track, index: None },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }
    let file = dir.join("take.mkv");
    let canvas = lightair_model::Id::new();
    let cfg = RecordingConfig {
        canvas,
        path: file.clone(),
        label: None,
        split_minutes: None,
        container: Container::Mkv,
        encoders: default_encoders(&engine.capabilities.encoders, 2500, false),
        extra_mixes: vec![1],
    };
    engine.call(move |s| s.outputs.start_recording(&cfg, None)).expect("recording starts");
    let start = Instant::now();
    while !matches!(runtime().get(OutputKey::Recording(canvas)).state, OutputState::Active { .. })
        && start.elapsed() < Duration::from_secs(3)
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(800));
    engine.call(|s| s.outputs.stop_recording());
    let start = Instant::now();
    while runtime().get(OutputKey::Recording(canvas)).state.is_running() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(&file)
        .output()
        .unwrap();
    let streams = String::from_utf8_lossy(&out.stdout).to_string();
    let audio = streams.lines().filter(|l| l.trim() == "audio").count();
    eprintln!("streams: {:?}", streams.lines().collect::<Vec<_>>());
    assert_eq!(audio, 2, "the program and the VOD mix");
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
