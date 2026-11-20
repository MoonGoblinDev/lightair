//! The live follow-zoom (a scene's `ScreenZoomPointer`, drawn by the pointer filter): zoomed in, the
//! picture is magnified; off, it's as it was. Shown on a picture, not a screen capture (a capture
//! would ask for the screen-recording permission).

use std::path::PathBuf;
use std::time::Duration;

use lightair_model::{Command, Document, Effect, Fit, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

/// Y, Cb, Cr on a 16×16 grid of a frame rendered after `wait`.
fn grid(preview: &Preview, wait: Duration) -> Vec<(u8, u8, u8)> {
    std::thread::sleep(wait);
    let n = preview.frames();
    while preview.frames() < n + 4 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let f = preview.latest_frame().unwrap();
    (0..16 * 16).map(|i| sample_ycbcr(&f, (i % 16) as f32 / 16.0 + 0.03, (i / 16) as f32 / 16.0 + 0.03).unwrap()).collect()
}

/// A picture where every spot differs: red across, green down (a BMP, written here).
fn gradient(dir: &std::path::Path) -> PathBuf {
    let (w, h) = (320u32, 180u32);
    let row = (w * 3).div_ceil(4) * 4;
    let mut b = Vec::new();
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(54 + row * h).to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(-(h as i32)).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    for y in 0..h {
        for x in 0..w {
            // B, G, R
            b.extend_from_slice(&[128, (y * 255 / h) as u8, (x * 255 / w) as u8]);
        }
        b.resize(b.len() + (row - w * 3) as usize, 0);
    }
    let path = dir.join("gradient.bmp");
    std::fs::write(&path, b).unwrap();
    path
}

fn changed(a: &[(u8, u8, u8)], b: &[(u8, u8, u8)]) -> usize {
    let d = |x: u8, y: u8| (x as i32 - y as i32).abs();
    a.iter().zip(b).filter(|(p, q)| d(p.0, q.0) + d(p.1, q.1) + d(p.2, q.2) > 12).count()
}

#[test]
fn the_follow_zoom_magnifies_and_lets_go() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = std::env::temp_dir().join(format!("lightair-zoom-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let picture = gradient(&dir);
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Zoom"));
    let f = doc.workspace().primary_canvas().id;
    let source = Source::new("Picture", "image_source", json!({"file": picture.to_string_lossy()}));
    let iid = source.id;
    let mut scene = Scene::new("Main");
    scene.objects = vec![Object::new("Picture", iid, [(f, Layout { fit: Fit::Fill, ..Layout::boxed(0.0, 0.0, 640.0, 360.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    let plain = grid(&preview, Duration::from_millis(300));

    // the pointer filter as a scene's look puts it, always zoomed (the pointer not drawn)
    let look = |zoom: i64| json!({"draw": false, "clicks": 0, "zoom": zoom, "zoom_scale": 3.0, "zoom_motion": 0.0});
    let fx = Effect::new("Screen look", lightair_engine::pointer::KIND, look(2));
    let fid = fx.id;
    apply(&engine, &mut doc, Command::AddSourceEffect { source: iid, effect: fx, index: None });
    let zoomed = grid(&preview, Duration::from_millis(900));
    let n = changed(&plain, &zoomed);
    eprintln!("zoomed: {n} of 256 changed");
    assert!(n > 180, "magnified: {n} of 256 changed");

    // off: it eases back out to the picture as it was
    apply(&engine, &mut doc, Command::SetSourceEffectSettings { source: iid, effect: fid, settings: look(0) });
    let out = grid(&preview, Duration::from_millis(1500));
    let n = changed(&plain, &out);
    assert!(n < 10, "as it was: {n} of 256 changed");
    drop(preview);
    drop(engine);
    let _ = std::fs::remove_file(&picture);
    let _ = std::fs::remove_dir(&dir);
}
