//! Portable source kinds play as the platform's libobs sources, and a source whose kind maps to
//! another libobs type after an edit gets a new source, still in its scenes.
//!
//! Windows only: there a screen source's mode picks the libobs type. (On macOS the one such change,
//! desktop audio to a loopback device, needs the screen recording permission in the test.)
#![cfg(windows)]

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff, kinds};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

#[test]
fn a_screen_source_changing_mode_is_recreated_in_place() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (1280, 720), output: (1280, 720), ..Default::default() })
        .expect("engine starts");
    let mut doc = Document::new(Workspace::new("Kinds"));
    let canvas = doc.workspace().primary_canvas().id;
    let screen = Source::new("Screen", kinds::SCREEN, json!({"mode": kinds::DISPLAY}));
    let id = screen.id;
    let scene = Scene::new("Main");
    let sid = scene.id;
    let object = Object::new("Screen", id, [(canvas, Layout::boxed(0.0, 0.0, 1280.0, 720.0))]);
    let mut apply = |cmd: Command| {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    };
    apply(Command::AddSource { source: screen });
    apply(Command::AddScene { scene, index: None });
    apply(Command::AddObject { scene: sid, object, index: None });
    assert_eq!(engine.call(move |s| s.reconciler.source_type(id)).as_deref(), Some("monitor_capture"));

    apply(Command::SetSourceSettings { source: id, settings: json!({"mode": kinds::WINDOW}) });
    assert_eq!(engine.call(move |s| s.reconciler.source_type(id)).as_deref(), Some("window_capture"));
    let items = engine.call(|s| s.reconciler.describe());
    assert_eq!(items.iter().filter(|l| l.contains(&id.to_string())).count(), 1, "{items:?}");
}
