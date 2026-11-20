//! The engine draws the hierarchy: a picture inside an empty moves, turns and hides with it.

use std::time::Duration;

use lightair_model::{Command, Document, Fit, Id, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

/// Canvas pixel (x, y) of the newest frame (canvas is 320×180).
fn at(preview: &Preview, x: f32, y: f32) -> (u8, u8, u8) {
    let n = preview.frames();
    while preview.frames() < n + 3 {
        std::thread::sleep(Duration::from_millis(10));
    }
    sample_ycbcr(&preview.latest_frame().unwrap(), x / 320.0, y / 180.0).unwrap()
}

fn red(p: (u8, u8, u8)) -> bool {
    p.2 > 200 && p.1 < 128
}

#[test]
fn pictures_follow_their_empty() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Hierarchy"));
    let f = doc.workspace().primary_canvas().id;
    let r = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 40, "height": 40}));
    let rid = r.id;
    // An empty at (60, 60) holding a 40×40 red square centred on it.
    let group = Object::empty("Group", [(f, (60.0, 60.0))]);
    let gid = group.id;
    let square =
        Object::new("Square", rid, [(f, Layout { fit: Fit::Stretch, ..Layout::boxed(-20.0, -20.0, 40.0, 40.0) })])
            .under(gid);
    let sid = Id::new();
    let mut scene = Scene::new("Main");
    scene.id = sid;
    scene.objects = vec![group, square];
    apply(&engine, &mut doc, Command::AddSource { source: r });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    assert!(red(at(&preview, 60.0, 60.0)), "the square sits on the empty's pivot");
    assert!(!red(at(&preview, 200.0, 120.0)));

    // Move the empty: the square goes with it.
    let set = |doc: &mut Document, f_: &dyn Fn(&mut Layout)| {
        let mut layout = doc.workspace().scene(sid).unwrap().object(gid).unwrap().layouts[&f];
        f_(&mut layout);
        Command::SetLayout { scene: sid, object: gid, canvas: f, layout }
    };
    let cmd = set(&mut doc, &|l| {
        l.x = 200.0;
        l.y = 120.0;
    });
    apply(&engine, &mut doc, cmd);
    let (old, new) = (at(&preview, 60.0, 60.0), at(&preview, 200.0, 120.0));
    eprintln!("moved: old {old:?}, new {new:?}");
    assert!(!red(old) && red(new), "the square moved with its empty");

    // Scale the empty: the square grows (40 → 80 px).
    let cmd = set(&mut doc, &|l| {
        l.scale_x = 2.0;
        l.scale_y = 2.0;
    });
    apply(&engine, &mut doc, cmd);
    assert!(red(at(&preview, 200.0 + 35.0, 120.0)), "the square grew with its empty");

    // Hide the empty: the square goes too.
    let cmd = set(&mut doc, &|l| l.visible = false);
    apply(&engine, &mut doc, cmd);
    assert!(!red(at(&preview, 200.0, 120.0)), "hidden with its empty");
    drop(preview);
    drop(engine);
}
