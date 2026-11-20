//! As in OBS, a source with a picture runs only while it's on air: on a mixer track but in no
//! object on air, it's neither drawn nor running (no decoding, no painting, no sound); in the
//! program scene it runs, and its sound reaches its track.

use std::time::Duration;

use lightair_model::{Command, Document, Scene, Source, Track, Workspace, diff};
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
fn a_track_with_a_picture_runs_only_on_air() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Track picture"));
    let orange = Source::new("Orange", "color_source_v3", json!({"color": 0xFF1E78FFu32, "width": 320, "height": 180}));
    let (sid, track) = (orange.id, Track::new("Orange", orange.id));
    let scene = Scene::new("Empty");
    let scene_id = scene.id;
    for cmd in [
        Command::AddSource { source: orange },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(scene_id) },
        Command::AddTrack { track, index: None },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    let shown = centre(&preview);
    eprintln!("empty scene with an orange track: {shown:?}");
    assert!(shown.0 < 40, "the track's picture isn't drawn: {shown:?}");
    assert!(!engine.call(move |s| s.reconciler.source_active(sid)), "off air: not running");
    // on air: running
    let object = lightair_model::Object::new("Orange", sid, [(doc.workspace().primary_canvas().id, lightair_model::Layout::boxed(0.0, 0.0, 320.0, 180.0))]);
    apply(&engine, &mut doc, Command::AddObject { scene: scene_id, object, index: None });
    let shown = centre(&preview);
    assert!(shown.0 > 40, "on air, it's drawn: {shown:?}");
    assert!(engine.call(move |s| s.reconciler.source_active(sid)), "on air: running");
    drop(preview);
    drop(engine);
}
