//! Crop view: while a object is being cropped, the cut-away picture fades in around the crop as a
//! ghost, and fades out again before the crop is applied, with the kept part unchanged
//! throughout. Skipped when Lightair's module isn't built (`cargo xtask modules`).

use std::path::PathBuf;
use std::time::Duration;

use lightair_core::{Command, Crop, Document, Fit, Id, Source, Object, Layout, Workspace, Scene, diff};
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

#[test]
fn the_cut_away_part_fades_in_and_out() {
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
    let mut doc = Document::new(Workspace::new("Crop view"));
    let f = doc.workspace().primary_canvas().id;
    // A 640×360 red source cropped to its left half, filling the canvas's left half: uncropped,
    // it would reach across the whole canvas.
    let r = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 640, "height": 360}));
    let rid = r.id;
    let layout = Layout {
        fit: Fit::Stretch,
        crop: Crop { left: 0, top: 0, right: 320, bottom: 0 },
        ..Layout::boxed(0.0, 0.0, 160.0, 180.0)
    };
    let a = Object::new("A", rid, [(f, layout)]);
    let (aid, sid) = (a.id, Id::new());
    let mut scene = Scene::new("Main");
    scene.id = sid;
    scene.objects = vec![a];
    apply(&engine, &mut doc, Command::AddSource { source: r });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    assert!(red(at(&preview, 80.0, 90.0)) && black(at(&preview, 240.0, 90.0)), "cropped to the left half");

    // Crop view: the right half fades in as a ghost; the kept part stays as it was.
    let workspace = doc.workspace().clone();
    engine.call(move |s| s.reconciler.set_crop_preview(Some((sid, f, aid)), &workspace));
    let first = at(&preview, 240.0, 90.0);
    std::thread::sleep(Duration::from_millis(500));
    let (kept, ghost) = (at(&preview, 80.0, 90.0), at(&preview, 240.0, 90.0));
    eprintln!("crop view: first frame {first:?}, kept {kept:?}, ghost {ghost:?}");
    assert!(red(kept), "the kept part is unchanged");
    assert!(!black(ghost) && !red(ghost), "the cut-away part shows faded: {ghost:?}");
    assert!(first.0 < ghost.0 || first.2 < ghost.2, "it fades in rather than popping: {first:?} → {ghost:?}");

    // Cropping further while in crop view moves the window, not the picture.
    let tighter = Layout { crop: Crop { right: 480, ..layout.crop }, width: 80.0, ..layout };
    apply(&engine, &mut doc, Command::SetLayout { scene: sid, object: aid, canvas: f, layout: tighter });
    std::thread::sleep(Duration::from_millis(300));
    let (kept, ghost) = (at(&preview, 40.0, 90.0), at(&preview, 120.0, 90.0));
    eprintln!("tighter: kept {kept:?}, now cut {ghost:?}");
    assert!(red(kept) && !red(ghost) && !black(ghost), "the window follows the crop");

    // Leaving: the ghost fades away, then the crop applies again, with nothing left behind.
    let workspace = doc.workspace().clone();
    engine.call(move |s| s.reconciler.fade_crop_preview(&workspace));
    std::thread::sleep(Duration::from_millis(400));
    let faded = at(&preview, 240.0, 90.0);
    let workspace = doc.workspace().clone();
    engine.call(move |s| s.reconciler.set_crop_preview(None, &workspace));
    let (kept, gone) = (at(&preview, 40.0, 90.0), at(&preview, 240.0, 90.0));
    eprintln!("after: faded {faded:?}, kept {kept:?}, gone {gone:?}");
    assert!(black(faded), "faded out before leaving");
    assert!(red(kept) && black(gone), "back to the crop");
    drop(preview);
    drop(engine);
}
