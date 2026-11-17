//! Shapes belong to objects: one object of a source can be a circle while another isn't, and the
//! circle is cut from the cropped picture. Skipped when Lightair's module isn't built
//! (`cargo xtask modules`).

use std::path::PathBuf;
use std::time::Duration;

use lightair_core::{Command, Crop, Document, Fit, Id, Source, Object, Layout, Workspace, Scene, Shape, ShapeKind, diff};
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
fn black(p: (u8, u8, u8)) -> bool {
    p.0 < 30 && (118..138).contains(&p.1) && (118..138).contains(&p.2)
}
fn white(p: (u8, u8, u8)) -> bool {
    p.0 > 200 && (118..138).contains(&p.1) && (118..138).contains(&p.2)
}

#[test]
fn shapes_are_per_object_and_follow_the_crop() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; run `cargo xtask modules`");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (320, 180),
        output: (320, 180),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("Shapes"));
    let f = doc.workspace().primary_canvas().id;
    // One 640×360 red source in two objects: left half and right half of the canvas.
    let r = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 640, "height": 360}));
    let rid = r.id;
    let left_box = Layout { fit: Fit::Stretch, ..Layout::boxed(0.0, 0.0, 160.0, 180.0) };
    let right_box = Layout { fit: Fit::Stretch, ..Layout::boxed(160.0, 0.0, 160.0, 180.0) };
    let a = Object::new("A", rid, [(f, left_box)]);
    let b = Object::new("B", rid, [(f, right_box)]);
    let (aid, sid) = (a.id, Id::new());
    let mut scene = Scene::new("Main");
    scene.id = sid;
    scene.objects = vec![a, b];
    apply(&engine, &mut doc, Command::AddSource { source: r });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    assert!(red(at(&preview, 4.0, 4.0)) && red(at(&preview, 164.0, 4.0)), "both start plain");

    // A becomes a circle: its corner goes, B's stays (same source).
    let circle = Layout { shape: Some(Shape::new(ShapeKind::Circle)), ..left_box };
    apply(&engine, &mut doc, Command::SetLayout { scene: sid, object: aid, canvas: f, layout: circle });
    let (a_corner, a_mid, b_corner) = (at(&preview, 4.0, 4.0), at(&preview, 80.0, 90.0), at(&preview, 164.0, 4.0));
    eprintln!("A corner {a_corner:?}, A middle {a_mid:?}, B corner {b_corner:?}");
    assert!(black(a_corner) && red(a_mid), "A is a circle");
    assert!(red(b_corner), "B, the same input, isn't");

    // Crop A to the right half of the source (320×360 visible, fitted into 160×180): the circle
    // (radius 160 source px = 80 canvas px) is centred on what's left, at (80, 90).
    let cropped = Layout { crop: Crop { left: 320, top: 0, right: 0, bottom: 0 }, ..circle };
    apply(&engine, &mut doc, Command::SetLayout { scene: sid, object: aid, canvas: f, layout: cropped });
    let (inside, outside) = (at(&preview, 80.0, 20.0), at(&preview, 80.0, 4.0));
    eprintln!("cropped: (80,20) {inside:?}, (80,4) {outside:?}");
    // A mask on the uncropped source would centre at its middle — the crop's left edge — and
    // leave (80, 20) outside.
    assert!(red(inside), "the circle follows the crop");
    assert!(black(outside), "and is still a circle");

    // A border.
    let mut shape = Shape::new(ShapeKind::Circle);
    shape.border = 16.0;
    let ringed = Layout { shape: Some(shape), ..cropped };
    apply(&engine, &mut doc, Command::SetLayout { scene: sid, object: aid, canvas: f, layout: ringed });
    let ring = at(&preview, 4.0, 90.0);
    eprintln!("border: {ring:?}");
    assert!(white(ring), "a white border inside the circle's edge");

    // No shape: back to a plain object.
    apply(&engine, &mut doc, Command::SetLayout { scene: sid, object: aid, canvas: f, layout: left_box });
    assert!(red(at(&preview, 4.0, 4.0)), "the shape is gone");
    drop(preview);
    drop(engine);
}
