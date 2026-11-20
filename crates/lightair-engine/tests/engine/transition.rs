//! The Move transition: a matching object glides from its layout in scene A to its layout in
//! scene B, then the canvas lands on B.

use std::time::Duration;

use lightair_model::{
    Command, Document, Source, Object, Layout, Workspace, Scene, TransitionKind, TransitionSettings, diff,
};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

#[test]
fn move_glides_matching_objects_then_lands() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine =
        Engine::start(EngineConfig { canvas: (1280, 720), output: (1280, 720), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Move"));
    let canvas = doc.workspace().primary_canvas().id;
    let cam = Source::new("Cam", "color_source_v3", json!({"color": 0xFFFFFFFFu32, "width": 400, "height": 400}));
    let cam_id = cam.id;
    let mut a = Scene::new("A");
    a.objects.push(Object::new("Cam", cam_id, [(canvas, Layout::boxed(0.0, 0.0, 200.0, 200.0))]));
    let mut b = Scene::new("B");
    b.objects.push(Object::new("Cam", cam_id, [(canvas, Layout::boxed(1000.0, 400.0, 200.0, 200.0))]));
    let (a_id, b_id) = (a.id, b.id);
    let apply = |doc: &mut Document, cmd: Command| {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    };
    apply(&mut doc, Command::AddSource { source: cam });
    apply(&mut doc, Command::AddScene { scene: a, index: None });
    apply(&mut doc, Command::AddScene { scene: b, index: None });
    apply(
        &mut doc,
        Command::SetTransition {
            settings: TransitionSettings { kind: TransitionKind::Move, duration_ms: 800, ..Default::default() },
        },
    );
    assert_eq!(doc.workspace().program, Some(a_id));
    let on_air = engine.call(move |s| s.reconciler.describe_on_air(canvas));
    assert_eq!(on_air.len(), 1, "{on_air:?}");
    assert!(on_air[0].ends_with(" 100 100"), "A: item centred at (100,100): {on_air:?}");

    apply(&mut doc, Command::SetProgram { scene: Some(b_id) });
    std::thread::sleep(Duration::from_millis(400));
    let mid = engine.call(move |s| s.reconciler.describe_on_air(canvas));
    let xy: Vec<f32> = mid[0].split(' ').skip(1).map(|v| v.parse().unwrap()).collect();
    eprintln!("halfway: {mid:?}");
    assert!(xy[0] > 150.0 && xy[0] < 1050.0, "x between A and B mid-move: {mid:?}");
    assert!(xy[1] > 110.0 && xy[1] < 490.0, "y between A and B mid-move: {mid:?}");

    std::thread::sleep(Duration::from_millis(700));
    let end = engine.call(move |s| {
        s.reconciler.cleanup();
        s.reconciler.describe_on_air(canvas)
    });
    eprintln!("landed: {end:?}");
    assert!(end[0].ends_with(" 1100 500"), "lands on B's layout: {end:?}");
    drop(engine);
}
