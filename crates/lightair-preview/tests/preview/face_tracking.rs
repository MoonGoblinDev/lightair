//! The face tracker finds a face and the mesh-drawn makeup reaches it. Needs a picture with a
//! face: set LIGHTAIR_FACE_IMAGE (not checked in); skipped otherwise.

use std::path::PathBuf;
use std::time::Duration;

use lightair_model::{Command, Document, Effect, Fit, Id, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::{Preview, sample_ycbcr};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

/// Luma on a 24×24 grid of the newest frame.
fn grid(preview: &Preview) -> Vec<u8> {
    let n = preview.frames();
    while preview.frames() < n + 5 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let f = preview.latest_frame().unwrap();
    (0..24 * 24)
        .map(|i| sample_ycbcr(&f, (i % 24) as f32 / 24.0 + 0.02, (i / 24) as f32 / 24.0 + 0.02).unwrap().0)
        .collect()
}

#[test]
fn the_tracker_finds_the_face() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(image) = std::env::var_os("LIGHTAIR_FACE_IMAGE").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_FACE_IMAGE not set; skipping");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (1280, 720),
        output: (1280, 720),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("Face"));
    let f = doc.workspace().primary_canvas().id;
    let mut source = Source::new("Face", "image_source", json!({"file": image.to_string_lossy()}));
    let mut studio = Effect::new("Lightair Effect Stack", "lightair_camera_studio", json!({"debug_view": 2}));
    studio.enabled = false;
    let eid = studio.id;
    source.effects.push(studio);
    let iid = source.id;
    let mut scene = Scene::new("Main");
    scene.id = Id::new();
    let sid = scene.id;
    scene.objects =
        vec![Object::new("Face", iid, [(f, Layout { fit: Fit::Fit, ..Layout::boxed(0.0, 0.0, 1280.0, 720.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(800));
    let plain = grid(&preview);
    apply(&engine, &mut doc, Command::SetSourceEffectEnabled { source: iid, effect: eid, enabled: true });
    // Models load on the tracker's threads (the first load also compiles them for this Mac).
    let mut present = false;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        present = engine.call(move |s| s.reconciler.face_present(iid)).unwrap_or(false);
        if present {
            break;
        }
    }
    let changed = plain.iter().zip(&grid(&preview)).filter(|(a, b)| (**a as i32 - **b as i32).abs() > 12).count();
    eprintln!("face present: {present}; {changed} of 576 grid points changed with face regions shown");
    let expect = std::env::var("LIGHTAIR_FACE_EXPECTED").map_or(true, |v| v != "0");
    assert_eq!(present, expect, "the tracker sees a face when there is one");
    let _ = sid;
    drop(preview);
    drop(engine);
}

/// Cb/Cr on a 24×24 grid of the newest frame.
fn chroma(preview: &Preview) -> Vec<(u8, u8)> {
    let n = preview.frames();
    while preview.frames() < n + 5 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let f = preview.latest_frame().unwrap();
    (0..24 * 24)
        .map(|i| {
            let (_, cb, cr) = sample_ycbcr(&f, (i % 24) as f32 / 24.0 + 0.02, (i / 24) as f32 / 24.0 + 0.02).unwrap();
            (cb, cr)
        })
        .collect()
}

#[test]
fn the_background_is_replaced_around_the_person() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(image) = std::env::var_os("LIGHTAIR_FACE_IMAGE").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_FACE_IMAGE not set; skipping");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (1280, 720),
        output: (1280, 720),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("Background"));
    let f = doc.workspace().primary_canvas().id;
    let mut source = Source::new("Person", "image_source", json!({"file": image.to_string_lossy()}));
    // Pure red behind the person (OBS colours are 0xAABBGGRR).
    let studio = Effect::new("Background", "studio.bg_replace", json!({"bg_mode": 3, "bg_color": 0xFF0000FFu32}));
    let eid = studio.id;
    source.effects.push(studio);
    let iid = source.id;
    let mut scene = Scene::new("Main");
    scene.objects =
        vec![Object::new("Person", iid, [(f, Layout { fit: Fit::Fill, ..Layout::boxed(0.0, 0.0, 1280.0, 720.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    let mut person = false;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        person = engine.call(move |s| s.reconciler.person_present(iid)).unwrap_or(false);
        if person {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    let red = |c: &[(u8, u8)]| c.iter().filter(|(cb, cr)| *cr > 200 && *cb < 120).count();
    let replaced = red(&chroma(&preview));
    eprintln!("person present: {person}; {replaced} of 576 grid points red");
    let expect = std::env::var("LIGHTAIR_FACE_EXPECTED").map_or(true, |v| v != "0");
    assert_eq!(person, expect, "the segmenter sees a person when there is one");
    if expect {
        assert!(replaced > 40, "the background turned red");
        assert!(replaced < 540, "the person kept their colours");
    } else {
        assert!(replaced > 540, "no one in the picture: all of it is background");
    }

    // Every model and quality finds the same person.
    if expect {
        for (model, quality, name) in [(0, 0, "Vision fast"), (0, 2, "Vision accurate"), (1, 1, "MediaPipe")] {
            let settings = json!({"bg_mode": 3, "bg_color": 0xFF0000FFu32, "bg_model": model, "bg_quality": quality});
            apply(&engine, &mut doc, Command::SetSourceEffectSettings { source: iid, effect: eid, settings });
            std::thread::sleep(Duration::from_millis(1500));
            let n = red(&chroma(&preview));
            eprintln!("{name}: {n} of 576 grid points red");
            assert!(n > 40 && n < 540, "{name}: background replaced, person kept");
        }
    }

    // Blur keeps the background's own colours: nothing red.
    // (one face effect for another: the same Lightair Effect Stack filter, updated in place)
    let blur = Effect::new("Blur", "studio.bg_blur", json!({"bg_blur": 80}));
    apply(&engine, &mut doc, Command::AddSourceEffect { source: iid, effect: blur, index: None });
    apply(&engine, &mut doc, Command::RemoveSourceEffect { source: iid, effect: eid });
    std::thread::sleep(Duration::from_millis(400));
    assert!(red(&chroma(&preview)) < 5, "blur shows the real background");
    drop(preview);
    drop(engine);
}

/// Background edges hold still on a still picture with camera-like noise (moving grain before
/// the effect), and still follow a sudden move. Measures the person mask itself (shown as grey):
/// how far it moves per frame, and how many frames it takes to catch up with a move.
/// LIGHTAIR_FLICKER_MAX bounds the share of points jumping visibly per frame (default 0.1%).
#[test]
fn background_edges_hold_still() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(image) = std::env::var_os("LIGHTAIR_FACE_IMAGE").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_FACE_IMAGE not set; skipping");
        return;
    };
    if std::env::var("LIGHTAIR_FACE_EXPECTED").is_ok_and(|v| v == "0") {
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (1280, 720),
        output: (1280, 720),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("Flicker"));
    let f = doc.workspace().primary_canvas().id;
    let mut source = Source::new("Person", "image_source", json!({"file": image.to_string_lossy()}));
    // Moving grain first, so the segmenter sees a new, noisy frame every time (like a camera).
    source.effects.push(Effect::new("Grain", "fx.grain", json!({"amount": std::env::var("LIGHTAIR_GRAIN").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.35), "grain_size": 1.0, "colored": true})));
    // A mirror, off for now: switching it on moves the person at once (the motion test).
    let mut mirror = Effect::new("Mirror", "fx.mirror", json!({"axis": 1}));
    mirror.enabled = false;
    let mid = mirror.id;
    source.effects.push(mirror);
    let studio = Effect::new("Background", "studio.bg_replace", json!({"bg_mode": 3, "bg_color": 0xFF0000FFu32}));
    let eid = studio.id;
    source.effects.push(studio);
    let iid = source.id;
    let mut scene = Scene::new("Main");
    scene.objects =
        vec![Object::new("Person", iid, [(f, Layout { fit: Fit::Fill, ..Layout::boxed(0.0, 0.0, 1280.0, 720.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    let max: f64 = std::env::var("LIGHTAIR_FLICKER_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(0.1);

    // The mask itself, as grey: how far it moves from frame to frame.
    const N: usize = 64;
    let mask_at = |preview: &Preview| -> Vec<i32> {
        let n = preview.frames();
        while preview.frames() < n + 1 {
            std::thread::sleep(Duration::from_millis(2));
        }
        let fr = preview.latest_frame().unwrap();
        (0..N * N)
            .map(|i| sample_ycbcr(&fr, (i % N) as f32 / N as f32 + 0.005, (i / N) as f32 / N as f32 + 0.005).unwrap().0 as i32)
            .collect()
    };
    for (model, quality, name) in [(0, 1, "Vision balanced"), (0, 2, "Vision accurate"), (1, 1, "MediaPipe")] {
        let settings = json!({"bg_mode": 3, "bg_model": model, "bg_quality": quality, "bg_show_mask": true});
        apply(&engine, &mut doc, Command::SetSourceEffectSettings { source: iid, effect: eid, settings });
        std::thread::sleep(Duration::from_millis(3000));
        let frames: Vec<Vec<i32>> = (0..60).map(|_| mask_at(&preview)).collect();
        let pairs = frames.windows(2);
        let (mut sum, mut visible) = (0i64, 0usize);
        for w in pairs {
            for (a, b) in w[0].iter().zip(&w[1]) {
                let d = (a - b).abs();
                sum += d as i64;
                visible += (d > 24) as usize;
            }
        }
        let mean = sum as f64 / (59.0 * (N * N) as f64);
        let rate = visible as f64 / (59.0 * (N * N) as f64) * 100.0;
        eprintln!("{name:>16}: mask moves {mean:.3}/255 per point per frame; {rate:.3}% of points jump visibly");
        assert!(rate <= max, "{name}: edges flicker ({rate:.3}% per frame)");
    }

    // Someone moves: the mask follows within a few frames instead of trailing.
    // (Accurate is Vision's slow, large model: it lags by itself, as its hint says.)
    for (model, quality, name, bound) in [(0, 1, "Vision balanced", 6), (0, 2, "Vision accurate", 16)] {
        let settings = json!({"bg_mode": 3, "bg_model": model, "bg_quality": quality, "bg_show_mask": true});
        apply(&engine, &mut doc, Command::SetSourceEffectSettings { source: iid, effect: eid, settings });
        apply(&engine, &mut doc, Command::SetSourceEffectEnabled { source: iid, effect: mid, enabled: false });
        std::thread::sleep(Duration::from_millis(2500));
        apply(&engine, &mut doc, Command::SetSourceEffectEnabled { source: iid, effect: mid, enabled: true });
        let frames: Vec<Vec<i32>> = (0..40).map(|_| mask_at(&preview)).collect();
        let settled = frames.last().unwrap();
        let off = |f: &Vec<i32>| f.iter().zip(settled).filter(|(a, b)| (**a - **b).abs() > 64).count();
        let caught = frames.iter().position(|f| off(f) * 100 < N * N).unwrap_or(frames.len());
        eprintln!("{name:>16}: caught up with a sudden move in {caught} frames");
        assert!(caught <= bound, "{name}: the edge trails a move ({caught} frames)");
    }
    drop(preview);
    drop(engine);
}

/// Edges don't jitter while the person sways (a slow wave plus camera-like grain before the
/// effect): an edge point crossing once as the person moves is right; flipping back and forth
/// is jitter. Counts reversals of each point's mask value over 1.5 s, per edge point.
#[test]
fn background_edges_steady_in_motion() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let Some(image) = std::env::var_os("LIGHTAIR_FACE_IMAGE").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_FACE_IMAGE not set; skipping");
        return;
    };
    if std::env::var("LIGHTAIR_FACE_EXPECTED").is_ok_and(|v| v == "0") {
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !dir.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (1280, 720),
        output: (1280, 720),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    let mut doc = Document::new(Workspace::new("Sway"));
    let f = doc.workspace().primary_canvas().id;
    let mut source = Source::new("Person", "image_source", json!({"file": image.to_string_lossy()}));
    source.effects.push(Effect::new("Sway", "fx.wave", json!({"amount": 0.35, "frequency": 0.1, "speed": 0.15})));
    source.effects.push(Effect::new("Grain", "fx.grain", json!({"amount": std::env::var("LIGHTAIR_GRAIN").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.2), "grain_size": 1.0, "colored": true})));
    let studio = Effect::new("Background", "studio.bg_replace", json!({"bg_mode": 3}));
    let eid = studio.id;
    source.effects.push(studio);
    let iid = source.id;
    let mut scene = Scene::new("Main");
    scene.objects =
        vec![Object::new("Person", iid, [(f, Layout { fit: Fit::Fill, ..Layout::boxed(0.0, 0.0, 1280.0, 720.0) })])];
    apply(&engine, &mut doc, Command::AddSource { source });
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    let max: f64 = std::env::var("LIGHTAIR_JITTER_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(0.7);

    const N: usize = 96;
    let mask_at = |preview: &Preview| -> Vec<i32> {
        let n = preview.frames();
        while preview.frames() < n + 1 {
            std::thread::sleep(Duration::from_millis(2));
        }
        let fr = preview.latest_frame().unwrap();
        (0..N * N)
            .map(|i| sample_ycbcr(&fr, (i % N) as f32 / N as f32 + 0.003, (i / N) as f32 / N as f32 + 0.003).unwrap().0 as i32)
            .collect()
    };
    // Warm up first: right after the models load, Vision and Core ML run slower for a few
    // seconds, so masks come less often and jump further. A live session is always warm.
    std::thread::sleep(Duration::from_millis(8000));
    for (model, quality, name) in [(0, 1, "Vision balanced"), (0, 2, "Vision accurate"), (1, 1, "MediaPipe")] {
        let settings = json!({"bg_mode": 3, "bg_model": model, "bg_quality": quality, "bg_show_mask": true});
        apply(&engine, &mut doc, Command::SetSourceEffectSettings { source: iid, effect: eid, settings });
        std::thread::sleep(Duration::from_millis(3000));
        let frames: Vec<Vec<i32>> = (0..90).map(|_| mask_at(&preview)).collect();
        let (mut reversals, mut edge) = (0usize, 0usize);
        let mut travel = 0i64;
        for i in 0..N * N {
            let series: Vec<i32> = frames.iter().map(|f| f[i]).collect();
            let (lo, hi) = (*series.iter().min().unwrap(), *series.iter().max().unwrap());
            if hi - lo < 48 {
                continue; // not on the edge
            }
            edge += 1;
            let mut last = 0;
            for w in series.windows(2) {
                let d = w[1] - w[0];
                travel += d.abs() as i64;
                if d.abs() < 12 {
                    continue;
                }
                if last != 0 && d.signum() != last {
                    reversals += 1;
                }
                last = d.signum();
            }
        }
        let per_point = reversals as f64 / edge.max(1) as f64;
        let wobble = travel as f64 / edge.max(1) as f64 / 255.0;
        eprintln!("{name:>16}: {edge} edge points, {per_point:.2} reversals each in 1.5 s, travel {wobble:.2} full swings each");
        assert!(per_point <= max, "{name}: edges jitter ({per_point:.2} reversals per edge point)");
    }
    drop(preview);
    drop(engine);
}
