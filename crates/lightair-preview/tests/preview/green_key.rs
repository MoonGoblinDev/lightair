//! Background removal with macOS's own effect: macOS paints pure green behind the person (with
//! their edge blended into it), and Lightair Effect Stack's green key takes it out. A skin-coloured disc
//! on pure green, its rim blended into the green as macOS blends it, over a magenta backdrop:
//! the green goes (magenta shows), the disc stays, and its rim shows no green.

use std::path::PathBuf;
use std::time::Duration;

use lightair_core::{Command, Document, Effect, Fit, Layout, Object, Scene, Source, Workspace, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

/// The test picture, as a 24-bit BMP: 640×360, the disc's centre at the middle, radius 120, its
/// last 12 px blended into the backdrop (pure green, or pure blue).
fn picture(path: &std::path::Path, backdrop: [f32; 3]) {
    let (w, h) = (640i32, 360i32);
    let row = (w * 3 + 3) / 4 * 4;
    let mut px = vec![0u8; (row * h) as usize];
    let (skin, green) = ([225.0, 172.0, 140.0], backdrop);
    for y in 0..h {
        for x in 0..w {
            let d = (((x - w / 2).pow(2) + (y - h / 2).pow(2)) as f32).sqrt();
            let a = ((120.0 - d) / 12.0).clamp(0.0, 1.0);
            let c: Vec<u8> = (0..3).map(|k| (skin[k] * a + green[k] * (1.0 - a)).round() as u8).collect();
            // BMP rows go bottom-up, pixels as BGR
            let i = ((h - 1 - y) * row + x * 3) as usize;
            px[i..i + 3].copy_from_slice(&[c[2], c[1], c[0]]);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + px.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    out.extend_from_slice(&px);
    std::fs::write(path, out).unwrap();
}

#[test]
fn the_green_macos_paints_is_keyed_out() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let file = std::env::temp_dir().join(format!("lightair-green-key-{}.bmp", std::process::id()));
    picture(&file, [0.0, 255.0, 0.0]);
    let engine = Engine::start(EngineConfig {
        canvas: (640, 360),
        output: (640, 360),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("Green key"));
    let f = doc.workspace().primary_canvas().id;
    let backdrop = Source::new("Magenta", "color_source_v3", json!({"color": 0xFFFF00FFu32, "width": 640, "height": 360}));
    let mut person = Source::new("Person", "image_source", json!({"file": file.to_string_lossy()}));
    let pid = person.id;
    person.effects.push(Effect::new(
        "Background removal",
        "studio.bg_remove",
        json!({"bg_mode": 2, "bg_model": lightair_core::catalog::BG_MODEL_SYSTEM, "bg_feather": 30}),
    ));
    let full = Layout { fit: Fit::Stretch, ..Layout::boxed(0.0, 0.0, 640.0, 360.0) };
    let mut scene = Scene::new("Main");
    scene.objects = vec![Object::new("Magenta", backdrop.id, [(f, full)]), Object::new("Person", person.id, [(f, full)])];
    apply(&engine, &mut doc, Command::AddSource { source: backdrop });
    apply(&engine, &mut doc, Command::AddSource { source: person });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(1500));
    let n = preview.frames();
    while preview.frames() < n + 5 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let frame = preview.latest_frame().unwrap();
    let at = |x: f32, y: f32| sample_ycbcr(&frame, x / 640.0, y / 360.0).unwrap();
    // green: Cb and Cr both low; magenta: both high; skin: Cr above Cb
    let green = |(_, cb, cr): (u8, u8, u8)| cb < 90 && cr < 90;
    let magenta = |(_, cb, cr): (u8, u8, u8)| cb > 170 && cr > 170;
    for (x, y) in [(20.0, 20.0), (620.0, 20.0), (20.0, 340.0), (620.0, 340.0), (100.0, 180.0), (540.0, 180.0)] {
        assert!(magenta(at(x, y)), "the green is gone at ({x}, {y}): {:?}", at(x, y));
    }
    let centre = at(320.0, 180.0);
    assert!(!green(centre) && !magenta(centre) && centre.2 > centre.1, "the person stays: {centre:?}");
    // the rim, where the disc is blended into the green: no green left, at every angle, and
    // half way through the blend, half person and half backdrop (the edge keeps its width)
    let (skin, back) = (centre, at(20.0, 20.0));
    for k in 0..16 {
        let t = k as f32 / 16.0 * std::f32::consts::TAU;
        for r in [110.0, 114.0, 117.0] {
            let p = at(320.0 + r * t.cos(), 180.0 + r * t.sin());
            assert!(!green(p), "green spill at radius {r}, angle {k}/16: {p:?}");
        }
        // the inside of the edge, mostly person: no green cast (Cb and Cr don't sink below skin's)
        let p = at(320.0 + 110.0 * t.cos(), 180.0 + 110.0 * t.sin());
        assert!(p.1 + 6 >= skin.1 && p.2 + 6 >= skin.2, "a green cast inside the edge, angle {k}/16: {p:?} (skin {skin:?})");
    }
    // straight out (where the samples fall on whole pixels): half way through the blend
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let p = at(320.0 + 114.0 * dx, 180.0 + 114.0 * dy);
        let share = (back.1 as f32 - p.1 as f32) / (back.1 as f32 - skin.1 as f32);
        assert!((0.3..=0.7).contains(&share), "half way through the edge, {share:.2} of it is the person: {p:?}");
    }

    // a blue screen (for someone wearing green): the same, keyed on blue
    let blue_file = std::env::temp_dir().join(format!("lightair-blue-key-{}.bmp", std::process::id()));
    picture(&blue_file, [0.0, 0.0, 255.0]);
    let eid = doc.workspace().sources[&pid].effects[0].id;
    apply(&engine, &mut doc, Command::SetSourceSettings { source: pid, settings: json!({"file": blue_file.to_string_lossy()}) });
    let settings = json!({"bg_mode": 2, "bg_model": lightair_core::catalog::BG_MODEL_SYSTEM, "bg_feather": 30, "bg_key": 1});
    apply(&engine, &mut doc, Command::SetSourceEffectSettings { source: pid, effect: eid, settings });
    std::thread::sleep(Duration::from_millis(1500));
    let n = preview.frames();
    while preview.frames() < n + 5 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let frame = preview.latest_frame().unwrap();
    let at = |x: f32, y: f32| sample_ycbcr(&frame, x / 640.0, y / 360.0).unwrap();
    // blue: Cb high, Cr low
    let blue = |(_, cb, cr): (u8, u8, u8)| cb > 170 && cr < 110;
    for (x, y) in [(20.0, 20.0), (620.0, 340.0), (100.0, 180.0)] {
        assert!(magenta(at(x, y)), "the blue is gone at ({x}, {y}): {:?}", at(x, y));
    }
    let centre = at(320.0, 180.0);
    assert!(!blue(centre) && !magenta(centre) && centre.2 > centre.1, "the person stays on blue: {centre:?}");
    for k in 0..16 {
        let t = k as f32 / 16.0 * std::f32::consts::TAU;
        let p = at(320.0 + 114.0 * t.cos(), 180.0 + 114.0 * t.sin());
        assert!(!blue(p), "blue spill, angle {k}/16: {p:?}");
    }
    drop(preview);
    drop(engine);
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(&blue_file);
}
