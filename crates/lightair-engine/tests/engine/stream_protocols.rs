//! SRT, MPEG-TS over UDP and WHIP destinations.

use std::path::Path;
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Id, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::outputs::{OutputKey, OutputState, StreamConfig, StreamProtocol, default_encoders, runtime};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn wait<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn streams(path: &Path) -> Vec<String> {
    Proc::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

#[test]
fn urls_pick_the_protocol() {
    assert_eq!(StreamProtocol::of("rtmps://live.example/app"), Some(StreamProtocol::Rtmp));
    assert_eq!(StreamProtocol::of("SRT://host:9000"), Some(StreamProtocol::MpegTs));
    assert_eq!(StreamProtocol::of("https://whip.example/endpoint"), Some(StreamProtocol::Whip));
    assert_eq!(StreamProtocol::of("ftp://nope"), None);
    assert_eq!(StreamProtocol::label("srt://h:1"), "SRT");
    assert_eq!(StreamProtocol::label("rtmps://h"), "RTMPS");
}

#[test]
fn srt_udp_and_whip() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Protocols"));
    let f = doc.workspace().primary_canvas().id;
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 640, "height": 360}));
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Colour", colour.id, [(f, Layout::boxed(0.0, 0.0, 640.0, 360.0))]));
    apply(&engine, &mut doc, Command::AddSource { source: colour });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let caps = engine.capabilities.encoders.clone();
    let cfg = |server: String, key: &str| StreamConfig {
        id: Id::new(),
        server,
        key: key.into(),
        encoders: default_encoders(&caps, 2500, true),
        reconnect: None,
        vod_mix: None,
    };

    // ---- SRT: a media source in the same engine is the SRT listener; the output calls it.
    let receiver = Source::new(
        "SRT in",
        "ffmpeg_source",
        json!({"is_local_file": false, "input": "srt://127.0.0.1:19381?mode=listener", "input_format": "mpegts"}),
    );
    let rx = receiver.id;
    apply(&engine, &mut doc, Command::AddSource { source: receiver });
    // Media sources open their source while shown.
    engine.call(move |s| s.reconciler.hold_sources(&[rx]));
    std::thread::sleep(Duration::from_millis(1000));
    let srt = cfg("srt://127.0.0.1:19381".into(), "");
    let srt_id = srt.id;
    engine.call(move |s| s.outputs.start_stream(&srt, None)).expect("SRT starts");
    let active = wait(Duration::from_secs(6), || {
        matches!(runtime().get(OutputKey::Stream(srt_id)).state, OutputState::Active { .. }).then_some(())
    });
    assert!(active.is_some(), "SRT output active: {:?}", runtime().get(OutputKey::Stream(srt_id)).state);
    // An SRT caller is only active once the listener accepted it; then data must flow.
    let flowed = wait(Duration::from_secs(5), || {
        engine.call(|s| s.outputs.sample());
        let bytes = runtime().get(OutputKey::Stream(srt_id)).bytes;
        (bytes > 100_000).then_some(bytes)
    });
    let bytes = flowed.unwrap_or_else(|| runtime().get(OutputKey::Stream(srt_id)).bytes);
    eprintln!("SRT sent {bytes} bytes");
    assert!(bytes > 100_000, "the stream flows over SRT: {bytes} bytes");
    engine.call(move |s| s.outputs.stop_stream(srt_id));
    engine.call(|s| s.reconciler.release_sources());

    // ---- MPEG-TS over UDP to ffmpeg (same output as SRT, another transport).
    let have_ffmpeg = Proc::new("ffmpeg").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success());
    if have_ffmpeg {
        let dir = std::env::temp_dir().join(format!("lightair-proto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("udp.ts");
        let mut ff = Proc::new("ffmpeg")
            // a short probe: ffmpeg otherwise studies the stream for 5 s before writing
            .args(["-nostdin", "-loglevel", "error", "-analyzeduration", "1500000", "-i", "udp://127.0.0.1:19382"])
            .args(["-t", "0.8", "-c", "copy"])
            .arg(&file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let udp = cfg("udp://127.0.0.1:19382?pkt_size=1316".into(), "");
        let udp_id = udp.id;
        engine.call(move |s| s.outputs.start_stream(&udp, None)).expect("UDP starts");
        let _ = wait(Duration::from_secs(10), || ff.try_wait().ok().flatten());
        let _ = ff.kill();
        engine.call(move |s| s.outputs.stop_stream(udp_id));
        let got = streams(&file);
        eprintln!("UDP received: {got:?}");
        assert!(got.iter().any(|c| c == "h264") && got.iter().any(|c| c == "aac"), "{got:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- WHIP to an endpoint that isn't there: fails, in plain language, without taking
    // anything down.
    let whip = cfg("http://127.0.0.1:9/whip".into(), "token");
    let whip_id = whip.id;
    let started = engine.call(move |s| s.outputs.start_stream(&whip, None));
    let failed = wait(Duration::from_secs(10), || match runtime().get(OutputKey::Stream(whip_id)).state {
        OutputState::Failed(reason) => Some(reason),
        _ => None,
    })
    .or_else(|| started.err());
    eprintln!("WHIP: {failed:?}");
    assert!(failed.is_some_and(|r| !r.is_empty()), "an unreachable WHIP endpoint reports a failure");

    // A bad scheme is refused up front.
    let bad = cfg("ftp://example".into(), "k");
    assert!(engine.call(move |s| s.outputs.start_stream(&bad, None)).is_err());
    drop(engine);
}
