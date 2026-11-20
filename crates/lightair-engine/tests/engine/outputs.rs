//! Recording and streaming end to end, verified with ffprobe (and ffmpeg as a local RTMP
//! server). Skips the parts whose tools aren't installed.

use std::io::Write;
use std::path::Path;
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, Track, diff};
use lightair_engine::outputs::{
    ClipsConfig, Container, OutputKey, OutputState, RecordingConfig, StreamConfig, default_encoders, runtime,
};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn have(tool: &str) -> bool {
    Proc::new(tool).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn tone_wav(path: &Path) {
    let rate = 48_000u32;
    let mut data = Vec::new();
    for i in 0..rate * 2 {
        let s = (0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin() * i16::MAX as f32) as i16;
        data.extend_from_slice(&s.to_le_bytes());
        data.extend_from_slice(&s.to_le_bytes());
    }
    let mut f = std::fs::File::create(path).unwrap();
    for chunk in [
        &b"RIFF"[..],
        &(36 + data.len() as u32).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &2u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 4).to_le_bytes(),
        &4u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &(data.len() as u32).to_le_bytes(),
    ] {
        f.write_all(chunk).unwrap();
    }
    f.write_all(&data).unwrap();
}

/// (codec types and names, duration seconds) from ffprobe.
fn probe(path: &Path) -> (Vec<String>, f64) {
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let streams = v["streams"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    format!("{}:{}", s["codec_type"].as_str().unwrap_or(""), s["codec_name"].as_str().unwrap_or(""))
                })
                .collect()
        })
        .unwrap_or_default();
    let duration = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    (streams, duration)
}

/// Waits for an output's state (returning as soon as it's there); the timeout is generous, since
/// encoders start slowly while other tests use the hardware encoder too.
fn wait_for(key: OutputKey, timeout: Duration, pred: impl Fn(&OutputState) -> bool) -> OutputState {
    let start = Instant::now();
    loop {
        let state = runtime().get(key).state;
        if pred(&state) || start.elapsed() > timeout {
            return state;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn record_and_stream() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    if !have("ffprobe") {
        eprintln!("ffprobe not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lightair-outputs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("tone.wav");
    tone_wav(&wav);

    // libobs's replay buffer writes clips through the obs-ffmpeg-mux helper, which it looks for
    // next to the running executable.
    // (Windows: `cargo xtask test` puts it there.)
    #[cfg(unix)]
    {
        let helper = std::env::current_exe().unwrap().with_file_name("obs-ffmpeg-mux");
        if !helper.exists() {
            let _ = std::os::unix::fs::symlink("/Applications/OBS.app/Contents/MacOS/obs-ffmpeg-mux", &helper);
        }
    }
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Outputs"));
    let canvas = doc.workspace().primary_canvas().id;
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 640, "height": 360}));
    let tone = Source::new(
        "Tone",
        "ffmpeg_source",
        json!({"local_file": wav.to_string_lossy(), "looping": true, "is_local_file": true}),
    );
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Colour", colour.id, [(canvas, Layout::boxed(0.0, 0.0, 640.0, 360.0))]));
    let track = Track::new("Tone", tone.id);
    for cmd in [
        Command::AddSource { source: colour },
        Command::AddSource { source: tone },
        Command::AddScene { scene, index: None },
        Command::AddTrack { track, index: None },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }
    let encoders = default_encoders(&engine.capabilities.encoders, 2500, false);
    eprintln!("encoders: {} + {}", encoders.video_encoder, encoders.audio_encoder);

    // ---- recording
    let file = dir.join("take.mp4");
    let canvas = lightair_model::Id::new();
    let cfg = RecordingConfig {
        extra_mixes: vec![],
        canvas,
        path: file.clone(),
        label: None,
        split_minutes: None,
        container: Container::HybridMp4,
        encoders: encoders.clone(),
    };
    engine.call(move |s| s.outputs.start_recording(&cfg, None)).expect("recording starts");
    let state =
        wait_for(OutputKey::Recording(canvas), Duration::from_secs(10), |s| matches!(s, OutputState::Active { .. }));
    assert!(matches!(state, OutputState::Active { .. }), "recording active: {state:?}");
    // A second of it, and until the first write lands (later on a busy machine).
    let recording = Instant::now();
    let deadline = recording + Duration::from_secs(5);
    while (recording.elapsed() < Duration::from_secs(1) || runtime().get(OutputKey::Recording(canvas)).bytes == 0)
        && Instant::now() < deadline
    {
        engine.call(|s| s.outputs.sample());
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(runtime().get(OutputKey::Recording(canvas)).bytes > 0, "bytes written while recording");
    engine.call(|s| s.outputs.stop_recording());
    let state = wait_for(OutputKey::Recording(canvas), Duration::from_secs(10), |s| !s.is_running());
    assert_eq!(state, OutputState::Idle, "recording stopped cleanly");
    let (streams, duration) = probe(&file);
    eprintln!("recording: {streams:?}, {duration:.2} s");
    assert!(streams.iter().any(|s| s == "video:h264"), "{streams:?}");
    assert!(streams.iter().any(|s| s == "audio:aac"), "{streams:?}");
    assert!(duration > 0.8, "duration {duration}");

    // ---- clips: rolling buffer, save the last seconds
    let clips_dir = dir.join("clips");
    let cfg = ClipsConfig { folder: clips_dir.clone(), seconds: 5, encoders: encoders.clone() };
    engine.call(move |s| s.outputs.start_clips(&cfg, None)).expect("replay buffer starts");
    let state = wait_for(OutputKey::Clips, Duration::from_secs(10), |s| matches!(s, OutputState::Active { .. }));
    assert!(matches!(state, OutputState::Active { .. }), "replay buffer active: {state:?}");
    std::thread::sleep(Duration::from_millis(1100));
    assert!(engine.call(|s| s.outputs.save_clip()), "save accepted");
    let start = Instant::now();
    while runtime().get(OutputKey::Clips).saved_clips == 0 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = runtime().get(OutputKey::Clips);
    eprintln!(
        "clips status: saved={} path={:?} files={:?}",
        status.saved_clips,
        status.path,
        std::fs::read_dir(&clips_dir).map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
    );
    let clip = status.path.expect("clip path reported");
    let (streams, duration) = probe(&clip);
    eprintln!("clip {}: {streams:?}, {duration:.2} s", clip.display());
    assert!(streams.iter().any(|s| s == "video:h264"), "{streams:?}");
    assert!(duration > 0.8 && duration < 6.0, "clip length {duration}");
    engine.call(|s| s.outputs.stop_clips());
    wait_for(OutputKey::Clips, Duration::from_secs(10), |s| !s.is_running());

    // ---- streaming to a local RTMP server
    if have("ffmpeg") {
        let received = dir.join("received.flv");
        let mut server = Proc::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-listen",
                "1",
                "-i",
                "rtmp://127.0.0.1:19350/live/test",
                "-c",
                "copy",
                "-f",
                "flv",
            ])
            .arg(&received)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ffmpeg starts");
        std::thread::sleep(Duration::from_millis(700));
        let id = lightair_model::Id::new();
        let cfg = StreamConfig {
            id,
            server: "rtmp://127.0.0.1:19350/live".into(),
            key: "test".into(),
            encoders: default_encoders(&engine.capabilities.encoders, 2500, true),
            reconnect: None,
            vod_mix: None,
        };
        engine.call(move |s| s.outputs.start_stream(&cfg, None)).expect("stream starts");
        let state =
            wait_for(OutputKey::Stream(id), Duration::from_secs(10), |s| matches!(s, OutputState::Active { .. }));
        assert!(matches!(state, OutputState::Active { .. }), "stream active: {state:?}");
        assert!(runtime().any_stream_running());
        std::thread::sleep(Duration::from_millis(1100));
        engine.call(move |s| s.outputs.stop_stream(id));
        let state = wait_for(OutputKey::Stream(id), Duration::from_secs(10), |s| !s.is_running());
        assert_eq!(state, OutputState::Idle);
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = server.kill();
        let (streams, duration) = probe(&received);
        eprintln!("server received: {streams:?}, {duration:.2} s");
        assert!(streams.iter().any(|s| s == "video:h264"), "{streams:?}");
        assert!(streams.iter().any(|s| s == "audio:aac"), "{streams:?}");
        assert!(duration > 0.8, "duration {duration}");
    } else {
        eprintln!("ffmpeg not installed; skipping the streaming half");
    }

    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
