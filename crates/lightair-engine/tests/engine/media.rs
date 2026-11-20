//! Media transport on a media source: plays, pauses, seeks and restarts.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::media::{MediaAction, MediaState, MediaStatus};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn silence_wav(path: &Path, seconds: u32) {
    let rate = 48_000u32;
    let data = vec![0u8; (rate * seconds * 4) as usize];
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

fn wait(engine: &Engine, id: lightair_model::Id, pred: impl Fn(&MediaStatus) -> bool) -> MediaStatus {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let status = engine.call(move |s| s.reconciler.media_status(id)).expect("media input");
        if pred(&status) || Instant::now() > deadline {
            return status;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn media_transport() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = std::env::temp_dir().join(format!("lightair-media-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("clip.wav");
    silence_wav(&wav, 6);

    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Media"));
    let f = doc.workspace().primary_canvas().id;
    let media =
        Source::new("Clip", "ffmpeg_source", json!({"local_file": wav.to_string_lossy(), "is_local_file": true}));
    let colour = Source::new("Colour", "color_source_v3", json!({"color": 0xFF000000u32}));
    let (media_id, colour_id) = (media.id, colour.id);
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Clip", media_id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: media },
        Command::AddSource { source: colour },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }

    assert_eq!(engine.call(move |s| s.reconciler.media_status(colour_id)), None, "a colour isn't media");

    let s = wait(&engine, media_id, |s| s.state == MediaState::Playing && s.time_ms > 300);
    eprintln!("playing: {s:?}");
    assert_eq!(s.state, MediaState::Playing);
    assert!((5900..=6100).contains(&s.duration_ms), "duration {}", s.duration_ms);

    engine.call(move |s| s.reconciler.media_action(media_id, MediaAction::Pause));
    let paused = wait(&engine, media_id, |s| s.state == MediaState::Paused);
    assert_eq!(paused.state, MediaState::Paused);
    std::thread::sleep(Duration::from_millis(400));
    let still = engine.call(move |s| s.reconciler.media_status(media_id)).unwrap();
    assert!((still.time_ms - paused.time_ms).abs() < 100, "time holds while paused: {paused:?} → {still:?}");

    engine.call(move |s| s.reconciler.media_action(media_id, MediaAction::Seek(4000)));
    engine.call(move |s| s.reconciler.media_action(media_id, MediaAction::Play));
    let s = wait(&engine, media_id, |s| s.state == MediaState::Playing && s.time_ms >= 4000);
    assert!(s.time_ms >= 4000, "seeked: {s:?}");

    engine.call(move |s| s.reconciler.media_action(media_id, MediaAction::Restart));
    let s = wait(&engine, media_id, |s| s.state == MediaState::Playing && s.time_ms < 2000);
    assert!(s.time_ms < 2000, "restarted: {s:?}");

    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
