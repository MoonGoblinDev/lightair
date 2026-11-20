//! Reconciler idempotence (docs/04-engine-libobs.md, "Reconciler"): applying the edits one by
//! one must leave libobs in exactly the same state as rebuilding from the final document.

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

#[test]
fn incremental_reconcile_matches_full_rebuild() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (1280, 720), output: (1280, 720), ..Default::default() })
        .expect("engine starts");

    let mut doc = Document::new(Workspace::new("Test"));
    let canvas = doc.workspace().primary_canvas().id;
    let red = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 400, "height": 300}));
    let blue = Source::new("Blue", "color_source_v3", json!({"color": 0xFFFF0000u32, "width": 400, "height": 300}));
    let (red_id, blue_id) = (red.id, blue.id);
    let main = Scene::new("Main");
    let brb = Scene::new("BRB");
    let (main_id, brb_id) = (main.id, brb.id);
    let l1 = Object::new("Red", red_id, [(canvas, Layout::boxed(0.0, 0.0, 640.0, 360.0))]);
    let l2 = Object::new("Blue", blue_id, [(canvas, Layout::boxed(320.0, 180.0, 640.0, 360.0))]);
    let l3 = Object::new("Red again", red_id, [(canvas, Layout::boxed(10.0, 10.0, 100.0, 100.0))]);
    let (l1_id, l2_id) = (l1.id, l2.id);

    let edits = vec![
        Command::AddSource { source: red },
        Command::AddSource { source: blue },
        Command::AddScene { scene: main, index: None },
        Command::AddScene { scene: brb, index: None },
        Command::AddObject { scene: main_id, object: l1, index: None },
        Command::AddObject { scene: main_id, object: l2, index: None },
        Command::AddObject { scene: brb_id, object: l3, index: None },
        Command::SetLayout {
            scene: main_id,
            object: l1_id,
            canvas,
            layout: Layout { rotation: 15.0, ..Layout::boxed(50.0, 60.0, 300.0, 200.0) },
        },
        Command::MoveObject { scene: main_id, object: l2_id, to: 0 },
        Command::SetObjectVisible { scene: main_id, object: l1_id, canvas, visible: false },
        Command::SetSourceSettings {
            source: blue_id,
            settings: json!({"color": 0xFF00FF00u32, "width": 800, "height": 600}),
        },
        Command::SetProgram { scene: Some(brb_id) },
        Command::RemoveObject { scene: brb_id, object: doc_object_placeholder() },
    ];

    let mut applied = 0;
    for edit in edits {
        let Ok(change) = doc.apply(&edit) else { continue };
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
        applied += 1;
    }
    assert!(applied >= 12, "edits applied: {applied}");
    // Undo two steps and redo one, through the same path.
    for change in [doc.undo().unwrap(), doc.undo().unwrap(), doc.redo().unwrap()] {
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }

    let incremental = engine.call(|s| s.reconciler.describe());
    let final_workspace = doc.workspace().clone();
    let rebuilt = engine.call(move |s| {
        s.reconciler.clear();
        let empty = final_workspace.empty_like();
        let patches = diff(&empty, &final_workspace);
        s.reconciler.apply(&final_workspace, &patches);
        s.reconciler.describe()
    });
    eprintln!("{}", incremental.join("\n"));
    assert_eq!(incremental, rebuilt);
    assert!(incremental.iter().any(|l| l.contains("(program)")));
    assert_eq!(engine.call(|s| s.reconciler.counts()), (2, 2, 3));

    // ---- add a vertical canvas: every scene gets a second libobs scene in a new canvas
    let vertical = lightair_model::Canvas::vertical();
    let vid = vertical.id;
    let mut commands = vec![Command::AddCanvas { canvas: vertical }];
    for scene in &doc.workspace().scenes {
        for object in &scene.objects {
            commands.push(Command::SetLayout {
                scene: scene.id,
                object: object.id,
                canvas: vid,
                layout: Layout::boxed(0.0, 420.0, 1080.0, 1080.0),
            });
        }
    }
    let change = doc.apply(&Command::Batch { label: "Add vertical".into(), commands }).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    let incremental = engine.call(|s| s.reconciler.describe());
    let final_workspace = doc.workspace().clone();
    let rebuilt = engine.call(move |s| {
        s.reconciler.clear();
        let patches = diff(&final_workspace.empty_like(), &final_workspace);
        s.reconciler.apply(&final_workspace, &patches);
        s.reconciler.describe()
    });
    assert_eq!(incremental, rebuilt);
    assert_eq!(engine.call(|s| s.reconciler.counts()), (2, 4, 6), "scenes and items doubled across two formats");
    assert!(
        incremental.iter().any(|l| l.contains(&vid.to_string()) && l.contains("(program)")),
        "vertical canvas has the program"
    );
    let canvas_ok = engine.call(move |s| {
        let Some(c) = s.reconciler.canvas_ref(vid) else { return false };
        let mut ovi = lightair_engine::sys::obs_video_info::default();
        let ok = unsafe { lightair_engine::sys::obs_canvas_get_video_info(c, &mut ovi) };
        unsafe { lightair_engine::sys::obs_canvas_release(c) };
        ok && ovi.base_width == 1080 && ovi.base_height == 1920
    });
    assert!(canvas_ok, "vertical canvas is 1080x1920");
}

/// A object id that doesn't exist: the last edit must fail validation and change nothing.
fn doc_object_placeholder() -> lightair_model::Id {
    lightair_model::Id::new()
}
