//! Every shader effect (`fx.*`, run on the lightair_fx filter) compiles and changes the picture,
//! with the settings the catalog sends: uniform names as keys.

use std::path::PathBuf;
use std::time::Duration;

use lightair_model::{Command, Document, Effect, Fit, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::{Value, json};

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

/// Y, Cb, Cr on a 16×16 grid of a frame rendered after the last change.
fn grid(preview: &Preview) -> Vec<(u8, u8, u8)> {
    let n = preview.frames();
    while preview.frames() < n + 4 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let f = preview.latest_frame().unwrap();
    (0..16 * 16).map(|i| sample_ycbcr(&f, (i % 16) as f32 / 16.0 + 0.03, (i / 16) as f32 / 16.0 + 0.03).unwrap()).collect()
}

fn changed(a: &[(u8, u8, u8)], b: &[(u8, u8, u8)]) -> usize {
    let d = |x: u8, y: u8| (x as i32 - y as i32).abs();
    a.iter().zip(b).filter(|(p, q)| d(p.0, q.0) + d(p.1, q.1) + d(p.2, q.2) > 12).count()
}

#[test]
fn every_shader_effect_renders() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let picture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../branding/final/board.png");
    let engine = Engine::start(EngineConfig {
        canvas: (640, 360),
        output: (640, 360),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("FX"));
    let f = doc.workspace().primary_canvas().id;
    let source = Source::new("Picture", "image_source", json!({"file": picture.to_string_lossy()}));
    let iid = source.id;
    let mut scene = Scene::new("Main");
    scene.objects =
        vec![Object::new("Picture", iid, [(f, Layout { fit: Fit::Fill, ..Layout::boxed(0.0, 0.0, 640.0, 360.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(300));
    let plain = grid(&preview);

    let cases: &[(&str, Value)] = &[
        ("mono", json!({"tint": 1.0})), // the board is mostly neutral already
        ("duotone", json!({})),
        ("posterize", json!({"levels": 3.0})),
        ("invert", json!({})),
        ("vignette", json!({"amount": 1.0, "vignette_size": 0.2})),
        ("grain", json!({"amount": 1.0, "grain_size": 2.0})),
        ("vhs", json!({"amount": 1.0, "bleed": 1.0})),
        ("scanlines", json!({"amount": 1.0, "line_size": 2.0, "curvature": 1.0})),
        ("halftone", json!({"dot_size": 10.0})),
        ("pixelate", json!({"block_size": 40.0})),
        ("edges", json!({})),
        ("sketch", json!({})),
        ("chroma", json!({"amount": 1.0})),
        ("glow", json!({"amount": 1.0, "threshold": 0.3})),
        ("emboss", json!({})),
        ("blur", json!({"amount": 1.0})),
        ("tiltshift", json!({"amount": 1.0, "focus_size": 0.0})),
        ("mirror", json!({"axis": 1})),
        ("kaleido", json!({"segments": 5.0})),
        ("bulge", json!({"amount": 1.0, "radius": 0.6})),
        ("swirl", json!({"amount": 1.0, "radius": 0.6})),
        ("wave", json!({"amount": 1.0, "frequency": 1.0})),
    ];
    let mut failed = Vec::new();
    for (name, settings) in cases {
        let effect = Effect::new(*name, format!("fx.{name}"), settings.clone());
        let eid = effect.id;
        apply(&engine, &mut doc, Command::AddSourceEffect { source: iid, effect, index: None });
        let n = changed(&plain, &grid(&preview));
        eprintln!("{name:>10}: {n:>3} of 256 grid points changed");
        if n < 6 {
            failed.push(*name);
        }
        apply(&engine, &mut doc, Command::RemoveSourceEffect { source: iid, effect: eid });
    }
    assert!(failed.is_empty(), "no visible change from: {failed:?}");
    assert!(changed(&plain, &grid(&preview)) < 3, "removing the effects restores the picture");
    drop(preview);
    drop(engine);
}
