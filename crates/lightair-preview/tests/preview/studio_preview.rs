//! Studio mode's preview: a preview can show a scene that isn't on air, and go back to the
//! program, while the program itself stays put.

use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Frame, Preview, sample_ycbcr};
use serde_json::json;

/// Waits for a frame rendered after this call and samples its centre.
fn centre_after(preview: &Preview) -> (u8, u8, u8) {
    let start = preview.frames();
    let deadline = Instant::now() + Duration::from_secs(3);
    while preview.frames() < start + 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame: Frame = preview.latest_frame().expect("frames arrive");
    sample_ycbcr(&frame, 0.5, 0.5).expect("readable frame")
}

/// Red has Cr well above neutral; blue has Cb well above neutral.
fn is_red((_, cb, cr): (u8, u8, u8)) -> bool {
    cr > 200 && cb < 128
}
fn is_blue((_, cb, cr): (u8, u8, u8)) -> bool {
    cb > 200 && cr < 128
}

#[test]
fn preview_shows_an_off_air_scene() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Studio mode"));
    let f = doc.workspace().primary_canvas().id;
    // libobs colours are ABGR.
    let red = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 320, "height": 180}));
    let blue = Source::new("Blue", "color_source_v3", json!({"color": 0xFFFF0000u32, "width": 320, "height": 180}));
    let mut live = Scene::new("Live");
    live.objects.push(Object::new("Red", red.id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let mut next = Scene::new("Next");
    next.objects.push(Object::new("Blue", blue.id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let (live_id, next_id) = (live.id, next.id);
    for cmd in [
        Command::AddSource { source: red },
        Command::AddSource { source: blue },
        Command::AddScene { scene: live, index: None },
        Command::AddScene { scene: next, index: None },
        Command::SetProgram { scene: Some(live_id) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }

    let program = engine.call(|_| Preview::attach());
    let preview = engine.call(|_| Preview::attach());
    assert!(is_red(centre_after(&program)), "program shows Live");
    assert!(is_red(centre_after(&preview)), "preview starts on the program");

    let next_src = engine.call(move |s| s.reconciler.scene_source_ref(next_id, f).map(|p| p as usize)).unwrap();
    preview.set_source(Some(next_src as *mut _));
    let p = centre_after(&preview);
    assert!(is_blue(p), "preview shows the off-air scene: {p:?}");
    let p = centre_after(&program);
    assert!(is_red(p), "program unchanged: {p:?}");

    preview.set_source(None);
    assert!(is_red(centre_after(&preview)), "preview back on the program");
    drop((program, preview));
    drop(engine);
}
