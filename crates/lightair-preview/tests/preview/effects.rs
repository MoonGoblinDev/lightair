//! Video effects from the Inspector catalog render as described (same settings the app sends).

use std::time::Duration;

use lightair_model::{Command, Document, Effect, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn centre(preview: &Preview) -> (u8, u8, u8) {
    let n = preview.frames();
    while preview.frames() < n + 3 {
        std::thread::sleep(Duration::from_millis(10));
    }
    sample_ycbcr(&preview.latest_frame().unwrap(), 0.5, 0.5).unwrap()
}

#[test]
fn colour_correction_and_green_screen() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Effects"));
    let f = doc.workspace().primary_canvas().id;
    let red = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 320, "height": 180}));
    let green = Source::new("Green", "color_source_v3", json!({"color": 0xFF00FF00u32, "width": 320, "height": 180}));
    let (red_id, green_id) = (red.id, green.id);
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Red", red_id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: red },
        Command::AddSource { source: green },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    let p = centre(&preview);
    assert!(p.2 > 200, "red: {p:?}");

    // Colour correction, saturation −1: grey.
    let cc = Effect::new("Colour correction", "color_filter_v2", json!({"saturation": -1.0}));
    let cc_id = cc.id;
    apply(&engine, &mut doc, Command::AddSourceEffect { source: red_id, effect: cc, index: None });
    let p = centre(&preview);
    eprintln!("desaturated: {p:?}");
    assert!((118..138).contains(&p.1) && (118..138).contains(&p.2), "grey: {p:?}");
    apply(&engine, &mut doc, Command::RemoveSourceEffect { source: red_id, effect: cc_id });
    assert!(centre(&preview).2 > 200, "red again");

    // Green object on top hides red; Green screen removes it.
    let object = Object::new("Green", green_id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]);
    apply(&engine, &mut doc, Command::AddObject { scene: scene_id, object, index: None });
    let p = centre(&preview);
    assert!(p.1 < 100 && p.2 < 100, "green on top: {p:?}");
    let key = Effect::new(
        "Green screen",
        "chroma_key_filter_v2",
        json!({"key_color_type": "green", "similarity": 400, "smoothness": 80, "spill": 100}),
    );
    apply(&engine, &mut doc, Command::AddSourceEffect { source: green_id, effect: key, index: None });
    let p = centre(&preview);
    eprintln!("keyed: {p:?}");
    assert!(p.2 > 200, "keyed out, red shows through: {p:?}");
    drop(preview);
    drop(engine);
}
