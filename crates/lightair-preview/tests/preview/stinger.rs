//! The Stinger transition plays its video over the switch. Its own file: libobs is one
//! instance per process, so engine tests can't run side by side in one test binary.

use std::time::Duration;

use lightair_model::{
    Command, Document, Source, Object, Layout, Workspace, Scene, TransitionKind, TransitionSettings, diff,
};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn sample(preview: &Preview) -> [(u8, u8, u8); 3] {
    let f = preview.latest_frame().expect("frames");
    [0.25, 0.75, 0.5].map(|x| sample_ycbcr(&f, x, 0.5).unwrap())
}

fn red(p: (u8, u8, u8)) -> bool {
    p.2 > 200 && p.1 < 128
}
fn blue(p: (u8, u8, u8)) -> bool {
    p.1 > 200 && p.2 < 128
}

#[test]
fn stinger_plays_over_the_switch() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = std::env::temp_dir().join(format!("lightair-stinger-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let video = dir.join("stinger.mp4");
    let made = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=0x00FF00:s=320x180:d=1.2:r=30"])
        .args(["-c:v", "mpeg4", "-q:v", "2"])
        .arg(&video)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("ffmpeg not available; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Stinger"));
    let f = doc.workspace().primary_canvas().id;
    let r = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 320, "height": 180}));
    let b = Source::new("Blue", "color_source_v3", json!({"color": 0xFFFF0000u32, "width": 320, "height": 180}));
    let mut a_scene = Scene::new("A");
    a_scene.objects.push(Object::new("Red", r.id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let mut b_scene = Scene::new("B");
    b_scene.objects.push(Object::new("Blue", b.id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let bb = b_scene.id;
    for cmd in [
        Command::AddSource { source: r },
        Command::AddSource { source: b },
        Command::AddScene { scene: a_scene, index: None },
        Command::AddScene { scene: b_scene, index: None },
        Command::SetStinger {
            stinger: Some(lightair_model::Stinger { path: video.to_string_lossy().into_owned(), cut_ms: 600 }),
        },
        Command::SetTransition {
            settings: TransitionSettings { kind: TransitionKind::Stinger, duration_ms: 1200, ..Default::default() },
        },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(300));
    assert!(red(sample(&preview)[2]), "starts on A");

    apply(&engine, &mut doc, Command::SetProgram { scene: Some(bb) });
    // Watch the program: green (the stinger) must show, then blue.
    let start = std::time::Instant::now();
    let (mut saw_green, mut ends_blue) = (false, false);
    while start.elapsed() < Duration::from_millis(2500) && !(saw_green && ends_blue) {
        let c = sample(&preview)[2];
        let green = c.1 < 100 && c.2 < 100 && c.0 > 100;
        saw_green |= green;
        ends_blue = blue(c);
        std::thread::sleep(Duration::from_millis(30));
    }
    eprintln!("stinger: saw green {saw_green}, ends blue {ends_blue}");
    assert!(saw_green, "the stinger video plays over the switch");
    assert!(ends_blue, "lands on B");
    drop(preview);
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
