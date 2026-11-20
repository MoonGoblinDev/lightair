//! An avatar drawn by libobs: a VRM model (or a Live2D or Inochi2D one), transparent around it.
//! Needs a model: LIGHTAIR_VRM, LIGHTAIR_LIVE2D, LIGHTAIR_INOCHI2D (not checked in); skipped otherwise. LIGHTAIR_VRM_OUT (a folder) keeps the picture.

use std::path::PathBuf;
use std::time::Duration;

use lightair_engine::{Engine, EngineConfig};
use lightair_model::{Command, Document, Fit, Layout, Object, Scene, Source, Workspace, diff};
use lightair_preview::{Preview, frame_rgba};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

#[test]
fn an_avatar_is_drawn() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(file) = std::env::var_os("LIGHTAIR_VRM").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_VRM not set; skipping");
        return;
    };
    drawn(file, "libobs.png");
}

/// A Live2D model the same way (LIGHTAIR_LIVE2D: a .model3.json, not checked in).
#[test]
fn a_live2d_avatar_is_drawn() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(file) = std::env::var_os("LIGHTAIR_LIVE2D").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_LIVE2D not set; skipping");
        return;
    };
    drawn(file, "libobs_live2d.png");
}

/// An Inochi2D puppet the same way (LIGHTAIR_INOCHI2D: an .inp or .inx, not checked in).
#[test]
fn an_inochi2d_avatar_is_drawn() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(file) = std::env::var_os("LIGHTAIR_INOCHI2D").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_INOCHI2D not set; skipping");
        return;
    };
    drawn(file, "libobs_inochi2d.png");
}

fn drawn(file: PathBuf, picture: &str) {
    let engine =
        Engine::start(EngineConfig { canvas: (1280, 720), output: (1280, 720), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Avatar"));
    let f = doc.workspace().primary_canvas().id;
    // a grey backdrop, so see-through shows
    let back = Source::new("Back", "color_source_v3", json!({"color": 0xFF505050u32, "width": 1280, "height": 720}));
    let avatar =
        Source::new("Avatar", "lightair_avatar", json!({"file": file.to_string_lossy(), "width": 720, "height": 720}));
    let mut scene = Scene::new("Main");
    scene.objects = vec![
        Object::new("Back", back.id, [(f, Layout { fit: Fit::Fill, ..Layout::boxed(0.0, 0.0, 1280.0, 720.0) })]),
        Object::new("Avatar", avatar.id, [(f, Layout { fit: Fit::Fit, ..Layout::boxed(280.0, 0.0, 720.0, 720.0) })]),
    ];
    apply(&engine, &mut doc, Command::AddSource { source: back });
    apply(&engine, &mut doc, Command::AddSource { source: avatar });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    // the model loads in the background
    std::thread::sleep(Duration::from_millis(3000));
    let n = preview.frames();
    while preview.frames() < n + 3 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let (w, h, rgba) = frame_rgba(&preview.latest_frame().unwrap(), 1280).unwrap();
    if let Some(dir) = std::env::var_os("LIGHTAIR_VRM_OUT") {
        image::save_buffer(PathBuf::from(dir).join(picture), &rgba, w as u32, h as u32, image::ColorType::Rgba8)
            .unwrap();
    }
    let px = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    // the corner: the backdrop; the middle: the avatar (not the backdrop's grey)
    let grey = px(20, 20);
    let differs =
        (0..h).step_by(8).flat_map(|y| (w / 2 - 100..w / 2 + 100).step_by(8).map(move |x| (x, y))).filter(|&(x, y)| {
            let p = px(x, y);
            (0..3).any(|k| (p[k] as i32 - grey[k] as i32).abs() > 20)
        });
    let count = differs.count();
    assert!(count > 200, "the avatar shows in the middle ({count} points differ from the backdrop)");
    drop(preview);
    drop(engine);
}

/// The Effect Stack's body tracker on a picture of a posed avatar (LIGHTAIR_BODY_IMAGE): a body,
/// its wrists where the picture has them.
#[test]
fn the_body_tracker_sees_a_body() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let Some(picture) = std::env::var_os("LIGHTAIR_BODY_IMAGE").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_BODY_IMAGE not set; skipping");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !cfg!(target_os = "macos") || !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (640, 360),
        output: (640, 360),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let img = image::open(&picture).unwrap().to_rgba8();
    let (w, h) = img.dimensions();
    // on a background like the renders', BGRA
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in img.pixels() {
        let a = p[3] as u32;
        let mix = |c: u8, bg: u32| ((c as u32 * a + bg * (255 - a)) / 255) as u8;
        bgra.extend_from_slice(&[mix(p[2], 70), mix(p[1], 60), mix(p[0], 60), 255]);
    }
    let body = engine.call(move |_| lightair_engine::avatar::track_body_once(&bgra, w, h)).expect("a body");
    eprintln!(
        "3D: {}; left wrist 3D {:?}, 2D {:?}",
        body.joints3d.is_some(),
        body.joints3d.map(|j| j[13]),
        body.joints2d[13]
    );
    // (LIGHTAIR_BODY_JSON_OUT: the joints, for lightair-avatar's round-trip test)
    if let Some(out) = std::env::var_os("LIGHTAIR_BODY_JSON_OUT") {
        let j3: Vec<serde_json::Value> = body.joints3d.map_or(vec![], |j| {
            j.iter().map(|p| p.map_or(serde_json::Value::Null, |p| json!([p.x, p.y, p.z]))).collect()
        });
        let j2: Vec<serde_json::Value> =
            body.joints2d.iter().map(|p| p.map_or(serde_json::Value::Null, |(p, c)| json!([p.x, p.y, c]))).collect();
        std::fs::write(out, json!({"j3": if j3.is_empty() { serde_json::Value::Null } else { json!(j3) }, "j2": j2, "aspect": w as f64 / h as f64}).to_string()).unwrap();
    }
    let (wrist, conf) = body.joints2d[13].expect("the left wrist in the picture");
    assert!(conf > 0.3 && wrist.x > 0.5, "the person's left wrist is on the picture's right: {wrist:?}");
    drop(engine);
}
