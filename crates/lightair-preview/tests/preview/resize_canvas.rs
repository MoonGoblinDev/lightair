//! Canvases resize live: the main canvas (a video reset) and another one, keeping their scenes.

use std::time::Duration;

use lightair_model::{Command, Document, Fit, Canvas, Id, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, frame_size, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

/// The newest frame after a few more have rendered.
fn settled(preview: &Preview) -> lightair_preview::Frame {
    let n = preview.frames();
    while preview.frames() < n + 4 {
        std::thread::sleep(Duration::from_millis(10));
    }
    preview.latest_frame().unwrap()
}

fn red(p: (u8, u8, u8)) -> bool {
    p.2 > 200 && p.1 < 128
}

#[test]
fn the_main_canvas_resizes_live() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut workspace = Workspace::new("Resize");
    workspace.canvases[0] = Canvas { width: 320, height: 180, ..workspace.canvases[0].clone() };
    let mut doc = Document::new(workspace);
    let f = doc.workspace().primary_canvas().id;
    let r = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 64, "height": 64}));
    let rid = r.id;
    let mut scene = Scene::new("Main");
    let sid = Id::new();
    scene.id = sid;
    scene.objects =
        vec![Object::new("Red", rid, [(f, Layout { fit: Fit::Stretch, ..Layout::boxed(0.0, 0.0, 320.0, 180.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source: r });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    assert_eq!(frame_size(&settled(&preview)), (320, 180));

    let bigger = Canvas { width: 640, height: 360, ..doc.workspace().primary_canvas().clone() };
    apply(&engine, &mut doc, Command::UpdateCanvas { canvas: bigger });
    std::thread::sleep(Duration::from_millis(200));
    let frame = settled(&preview);
    eprintln!("after resize: {:?}", frame_size(&frame));
    assert_eq!(frame_size(&frame), (640, 360), "the main canvas is bigger now");
    // The red object still covers its old box (top-left quarter), and the scene is still there.
    assert!(red(sample_ycbcr(&frame, 0.2, 0.2).unwrap()), "the scene survives the reset");
    drop(preview);
    drop(engine);
}
