//! GPUI draws NV12 only: `nv12` gives a frame as NV12 whatever the renderer (on Metal, frames
//! are BGRA and get converted), with the same colours, once per frame.

use std::time::Duration;

use lightair_model::{Command, Document, Layout, Object, Scene, Source, Workspace, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, frame_is_nv12, nv12, sample_ycbcr};
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

#[test]
fn frames_come_as_nv12_with_the_same_colours() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("NV12"));
    let canvas = doc.workspace().primary_canvas().id;
    // orange on the left half, blue on the right
    let orange = Source::new("Orange", "color_source_v3", json!({"color": 0xFF1E78FFu32, "width": 160, "height": 180}));
    let blue = Source::new("Blue", "color_source_v3", json!({"color": 0xFFFF5010u32, "width": 160, "height": 180}));
    let mut scene = Scene::new("Two");
    scene.objects.push(Object::new("Orange", orange.id, [(canvas, Layout::boxed(0.0, 0.0, 160.0, 180.0))]));
    scene.objects.push(Object::new("Blue", blue.id, [(canvas, Layout::boxed(160.0, 0.0, 160.0, 180.0))]));
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: orange },
        Command::AddSource { source: blue },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    wait_frames(&preview, 3);
    let frame = preview.latest_frame().unwrap();
    let converted = nv12(&frame).expect("an NV12 frame");
    assert!(frame_is_nv12(&converted));
    for x in [0.25, 0.75] {
        let (a, b) = (sample_ycbcr(&frame, x, 0.5).unwrap(), sample_ycbcr(&converted, x, 0.5).unwrap());
        eprintln!("at {x}: frame {a:?}, NV12 {b:?}");
        let near = |p: u8, q: u8| p.abs_diff(q) <= 3;
        assert!(near(a.0, b.0) && near(a.1, b.1) && near(a.2, b.2), "at {x}: {a:?} vs {b:?}");
    }
    // the same frame drawn again (another view, another redraw): converted once
    let again = nv12(&frame).unwrap();
    assert_eq!(objc2_core_foundation::CFRetained::as_ptr(&again), objc2_core_foundation::CFRetained::as_ptr(&converted));
    drop(preview);
    drop(engine);
}
