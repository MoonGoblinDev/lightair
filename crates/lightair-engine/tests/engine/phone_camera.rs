//! The Phone camera source against real libobs: an MPEG-TS stream sent to its feed's UDP port
//! (as the app's SRT listener hands it on) is decoded, turned upright by the phone's rotation,
//! counted as gone when it stops, and decoded again by a new decoder when a new stream starts.

use std::net::UdpSocket;
use std::path::Path;
use std::time::{Duration, Instant};

use lightair_engine::phone_camera::{self, KIND};
use lightair_engine::{Engine, EngineConfig};
use lightair_model::{Command, Document, Layout, Object, Scene, Source, Workspace, diff};
use serde_json::json;

/// Eight seconds of a 320×180 test picture and a tone, as a phone sends them (H.264, AAC, MPEG-TS,
/// a keyframe a second); `None` without ffmpeg.
fn stream(dir: &Path) -> Option<Vec<u8>> {
    let out = dir.join("phone.ts");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x180:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
        ])
        .args(["-t", "8", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(["-f", "mpegts"])
        .arg(&out)
        .status()
        .ok()?;
    made.success().then(|| std::fs::read(&out).ok()).flatten()
}

/// Sends the stream to the feed in seven-packet datagrams over about `seconds`.
fn send(ts: &[u8], feed: &phone_camera::Feed, seconds: f64) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let chunks: Vec<&[u8]> = ts.chunks(188 * 7).collect();
    let gap = Duration::from_secs_f64(seconds / chunks.len() as f64);
    for c in chunks {
        let _ = socket.send_to(c, feed.addr());
        feed.data(c.len());
        std::thread::sleep(gap);
    }
}

fn wait_for(what: impl Fn() -> bool, within: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < within {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

#[test]
fn phone_camera_decodes_turns_and_recovers() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = std::env::temp_dir().join(format!("lightair-phone-camera-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let Some(ts) = stream(&dir) else {
        eprintln!("no ffmpeg on PATH: skipped");
        return;
    };

    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Phone"));
    let f = doc.workspace().primary_canvas().id;
    let camera = Source::new("Phone camera", KIND, json!({"width": 1280, "height": 720}));
    let id = camera.id;
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Phone camera", id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: camera },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }
    let size = || engine.call(move |s| s.reconciler.source_size(id));
    // before any picture: the size asked of the phone, the placeholder showing
    assert!(wait_for(|| size() == Some((1280, 720)), Duration::from_secs(3)), "placeholder size {:?}", size());

    let feed = phone_camera::feed(&id.to_string());
    let sending = {
        let (ts, feed) = (ts.clone(), feed.clone());
        std::thread::spawn(move || send(&ts, &feed, 8.0))
    };
    // (a decoder probes up to 1.5 s, longer on a busy machine)
    assert!(wait_for(|| size() == Some((320, 180)), Duration::from_secs(7)), "decoded size {:?}", size());
    // the phone held upright: a quarter turn clockwise
    feed.set_rotation(90);
    assert!(wait_for(|| size() == Some((180, 320)), Duration::from_secs(1)), "turned size {:?}", size());
    sending.join().unwrap();
    // the phone went quiet: gone within GONE_AFTER (the placeholder shows, at the last size)
    assert!(wait_for(|| !feed.live(), phone_camera::GONE_AFTER + Duration::from_millis(200)));
    assert_eq!(size(), Some((180, 320)));

    // it comes back with a new stream: a new decoder, pictures again
    feed.set_rotation(0);
    feed.new_stream();
    let sending = {
        let (ts, feed) = (ts.clone(), feed.clone());
        std::thread::spawn(move || send(&ts, &feed, 8.0))
    };
    assert!(
        wait_for(|| feed.live() && size() == Some((320, 180)), Duration::from_secs(7)),
        "after a new stream {:?}",
        size()
    );
    sending.join().unwrap();

    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
