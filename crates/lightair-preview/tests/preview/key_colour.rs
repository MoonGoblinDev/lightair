//! Keying with a picked colour: Green screen's custom key colour, and the live bypass the
//! eyedropper uses while picking.

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

/// Green screen with a custom key colour keys that colour; switched off live (as the eyedropper
/// does while picking), the picture shows unkeyed, and on again it keys.
#[test]
fn custom_key_colour_and_live_bypass() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Key"));
    let f = doc.workspace().primary_canvas().id;
    // An orange screen: none of the presets.
    let screen = Source::new("Screen", "color_source_v3", json!({"color": 0xFF1E78FFu32, "width": 320, "height": 180}));
    let sid = screen.id;
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Screen", sid, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: screen },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    let before = centre(&preview);
    assert!(before.0 > 100, "orange shows: {before:?}");
    let key = Effect::new(
        "Green screen",
        "chroma_key_filter_v2",
        json!({"key_color_type": "custom", "key_color": 0xFF1E78FFu32, "similarity": 400, "smoothness": 80, "spill": 100}),
    );
    let kid = key.id;
    apply(&engine, &mut doc, Command::AddSourceEffect { source: sid, effect: key, index: None });
    let keyed = centre(&preview);
    eprintln!("orange {before:?}, keyed {keyed:?}");
    assert!(keyed.0 < 40, "the custom colour is keyed out: {keyed:?}");
    engine.call(move |s| s.reconciler.set_filter_live(sid, kid, false));
    let bypassed = centre(&preview);
    assert!(bypassed.0 > 100, "bypassed live: the screen shows: {bypassed:?}");
    engine.call(move |s| s.reconciler.set_filter_live(sid, kid, true));
    assert!(centre(&preview).0 < 40, "keyed again");
    drop(preview);
    drop(engine);
}
