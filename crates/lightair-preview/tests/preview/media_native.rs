//! Lightair's own media source (macOS, `lightair_media`): a local MP4 plays through it, with
//! the same picture as OBS's media source shows of the same file, its sound, and the media
//! controls (pause, seek, the end).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use lightair_model::{Command, Document, Layout, Object, Scene, Source, Workspace, diff};
use lightair_engine::media::{MediaAction, MediaState};
use lightair_engine::obs::{Data, ObsSource};
use lightair_engine::{Engine, EngineConfig, sys};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn wait_frames(preview: &Preview, n: u64) {
    let start = preview.frames();
    while preview.frames() < start + n {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The loudest sample heard (×1000).
static PEAK: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn heard(_: *mut std::ffi::c_void, _: *mut sys::obs_source_t, audio: *const sys::audio_data, _muted: bool) {
    let a = unsafe { &*audio };
    if a.data[0].is_null() {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts(a.data[0] as *const f32, a.frames as usize) };
    let peak = left.iter().fold(0f32, |m, v| m.max(v.abs()));
    PEAK.fetch_max((peak * 1000.0) as u32, Ordering::Relaxed);
}

#[test]
fn a_local_video_plays_through_lightairs_own_source() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    // two seconds of colour bars (160×90, 30 fps) and a 440 Hz tone
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/bars.mp4");
    let file = file.to_string_lossy().into_owned();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Native media"));
    let canvas = doc.workspace().primary_canvas().id;
    let src = Source::new("Bars", "ffmpeg_source", json!({"local_file": file, "looping": true, "is_local_file": true}));
    let id = src.id;
    let mut scene = Scene::new("Bars");
    scene.objects.push(Object::new("Bars", id, [(canvas, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let scene_id = scene.id;
    for cmd in [Command::AddSource { source: src }, Command::AddScene { scene, index: None }, Command::SetProgram { scene: Some(scene_id) }] {
        apply(&engine, &mut doc, cmd);
    }
    let kind = engine.call(move |s| {
        let p = s.reconciler.source_source(id).unwrap();
        unsafe {
            sys::obs_source_add_audio_capture_callback(p, Some(heard), std::ptr::null_mut());
            std::ffi::CStr::from_ptr(sys::obs_source_get_id(p)).to_string_lossy().into_owned()
        }
    });
    assert_eq!(kind, "lightair_media", "a local MP4 plays through Lightair's own source");

    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(600));
    wait_frames(&preview, 3);
    let size = engine.call(move |s| {
        let p = s.reconciler.source_source(id).unwrap();
        unsafe { (sys::obs_source_get_width(p), sys::obs_source_get_height(p)) }
    });
    assert_eq!(size, (160, 90), "the video's size");
    let ours = preview.latest_frame().unwrap();

    // OBS's media source, the same file, drawn the same way
    let theirs = engine.call(move |_| {
        let settings = Data::from_json(&json!({"local_file": file, "looping": true, "is_local_file": true, "restart_on_activate": false}));
        ObsSource::create_private("ffmpeg_source", "Bars (OBS)", Some(&settings)).unwrap()
    });
    let ptr = theirs.as_ptr() as usize;
    let obs_preview = engine.call(move |_| {
        let p = Preview::attach();
        p.set_own_size(true);
        unsafe { sys::obs_source_get_ref(ptr as *mut sys::obs_source_t) };
        p.set_source(Some(ptr as *mut sys::obs_source_t));
        p
    });
    std::thread::sleep(Duration::from_millis(800));
    wait_frames(&obs_preview, 3);
    let reference = obs_preview.latest_frame().unwrap();
    // the seven bars along the top, and the strip below them
    for (x, y) in [(0.07, 0.3), (0.21, 0.3), (0.36, 0.3), (0.5, 0.3), (0.64, 0.3), (0.79, 0.3), (0.93, 0.3), (0.5, 0.71)] {
        let (a, b) = (sample_ycbcr(&ours, x, y).unwrap(), sample_ycbcr(&reference, x, y).unwrap());
        eprintln!("({x}, {y}): ours {a:?}, OBS's {b:?}");
        let near = |p: u8, q: u8| p.abs_diff(q) <= 6;
        assert!(near(a.0, b.0) && near(a.1, b.1) && near(a.2, b.2), "({x}, {y}): ours {a:?}, OBS's {b:?}");
    }
    drop(obs_preview);
    engine.call(move |_| drop(theirs));

    let peak = PEAK.load(Ordering::Relaxed);
    eprintln!("sound peak: {}", peak as f32 / 1000.0);
    assert!(peak > 50, "the tone is heard: {peak}");

    // pause: the time stands still; play: it moves on (libobs carries media actions out at
    // its next frame)
    let status = |engine: &Engine| {
        std::thread::sleep(Duration::from_millis(100));
        engine.call(move |s| s.reconciler.media_status(id)).unwrap()
    };
    engine.call(move |s| s.reconciler.media_action(id, MediaAction::Pause));
    let a = status(&engine);
    std::thread::sleep(Duration::from_millis(400));
    let b = status(&engine);
    eprintln!("paused: {a:?} then {b:?}");
    assert_eq!(b.state, MediaState::Paused);
    assert_eq!(a.time_ms, b.time_ms, "paused, the time stands still");
    assert!((1900..=2100).contains(&b.duration_ms), "two seconds long: {b:?}");
    engine.call(move |s| s.reconciler.media_action(id, MediaAction::Seek(1500)));
    let c = status(&engine);
    eprintln!("sought to 1.5 s: {c:?}");
    assert!((1400..=1600).contains(&c.time_ms) && c.state == MediaState::Paused, "{c:?}");
    engine.call(move |s| s.reconciler.media_action(id, MediaAction::Play));
    std::thread::sleep(Duration::from_millis(300));
    let d = status(&engine);
    assert_eq!(d.state, MediaState::Playing);
    assert!(d.time_ms != c.time_ms, "playing again: {c:?} then {d:?}");

    // not looping: it ends, and says so
    let settings = json!({"local_file": doc.workspace().sources.get(&id).unwrap().settings["local_file"], "looping": false, "is_local_file": true});
    apply(&engine, &mut doc, Command::SetSourceSettings { source: id, settings });
    engine.call(move |s| s.reconciler.media_action(id, MediaAction::Restart));
    std::thread::sleep(Duration::from_millis(2600));
    let e = status(&engine);
    eprintln!("not looping, after its length: {e:?}");
    assert_eq!(e.state, MediaState::Ended);
    drop(preview);
    drop(engine);
}

/// A file with a rotation (a phone held upright) shows turned, as players show it (OBS 33's
/// media source shows it sideways).
#[test]
fn a_rotated_video_shows_upright() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    // the bars clip, marked as rotated by 90°
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/bars-rotated.mp4");
    let file = file.to_string_lossy().into_owned();
    let engine = Engine::start(EngineConfig { canvas: (180, 320), output: (180, 320), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Rotated media"));
    let canvas = doc.workspace().primary_canvas().id;
    let src = Source::new("Bars", "ffmpeg_source", json!({"local_file": file, "looping": true, "is_local_file": true}));
    let id = src.id;
    let mut scene = Scene::new("Bars");
    scene.objects.push(Object::new("Bars", id, [(canvas, Layout::boxed(0.0, 0.0, 180.0, 320.0))]));
    let scene_id = scene.id;
    for cmd in [Command::AddSource { source: src }, Command::AddScene { scene, index: None }, Command::SetProgram { scene: Some(scene_id) }] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(600));
    wait_frames(&preview, 3);
    let size = engine.call(move |s| {
        let p = s.reconciler.source_source(id).unwrap();
        unsafe { (sys::obs_source_get_width(p), sys::obs_source_get_height(p)) }
    });
    assert_eq!(size, (90, 160), "upright");
    // turned a quarter counter-clockwise, as the file says: the bars run down the left, from
    // blue (the right-most) at the top to grey at the bottom
    let ours = preview.latest_frame().unwrap();
    for (y, luma) in [(0.07, 22), (0.21, 57), (0.5, 112), (0.93, 191)] {
        let shown = sample_ycbcr(&ours, 0.3, y).unwrap();
        eprintln!("(0.3, {y}): {shown:?}");
        assert!(shown.0.abs_diff(luma) <= 6, "(0.3, {y}): {shown:?}, not a bar of luma {luma}");
    }
    drop(preview);
    drop(engine);
}
