//! A Studio studio_take records each source to its own file, at its own size, and lets its canvases go
//! once they've stopped. (Colour sources stand in for the screen and camera: the same path.)

use std::path::Path;
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn probe(path: &Path) -> (u64, u64, f64) {
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height:format=duration"])
        .args(["-of", "json"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let s = &v["streams"][0];
    let d = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    (s["width"].as_u64().unwrap_or(0), s["height"].as_u64().unwrap_or(0), d)
}

#[test]
fn each_source_records_at_its_own_size() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    if !Proc::new("ffprobe").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success()) {
        eprintln!("ffprobe not installed; skipping");
        return;
    }
    let engine = Engine::start(EngineConfig { canvas: (1280, 720), output: (1280, 720), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Take"));
    let f = doc.workspace().primary_canvas().id;
    let wide = Source::new("Wide", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 1600, "height": 900}));
    let tall = Source::new("Tall", "color_source_v3", json!({"color": 0xFF22AA44u32, "width": 480, "height": 640}));
    let (wid, tid) = (wide.id, tall.id);
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Wide", wid, [(f, Layout::boxed(0.0, 0.0, 1280.0, 720.0))]));
    scene.objects.push(Object::new("Tall", tid, [(f, Layout::boxed(900.0, 100.0, 240.0, 320.0))]));
    let sid = scene.id;
    for cmd in [
        Command::AddSource { source: wide },
        Command::AddSource { source: tall },
        Command::AddScene { scene, index: None },
        Command::SetProgram { scene: Some(sid) },
    ] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }
    let dir = std::env::temp_dir().join(format!("lightair-take-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let workspace = doc.workspace().clone();
    let (folder, caps) = (dir.clone(), engine.capabilities.encoders.clone());
    let tracks = engine
        .call(move |s| {
            let sources: Vec<_> = workspace
                .sources
                .values()
                .filter_map(|i| s.reconciler.source_type(i.id).map(|_| ((**i).clone(), i.id)))
                .collect();
            // the live sources, as studio_take_sources gives them (colour sources aren't screens or cameras)
            let sources = sources
                .into_iter()
                .filter_map(|(i, id)| s.reconciler.source_source(id).map(|p| (i, p)))
                .collect();
            let studio_take = lightair_engine::studio_take::StudioTake::start(sources, &folder, (30, 1), &mut s.outputs, &caps)?;
            let tracks = studio_take.tracks.clone();
            s.studio_take = Some(studio_take);
            Ok::<_, String>(tracks)
        })
        .expect("take starts");
    assert_eq!(tracks.len(), 2);
    std::thread::sleep(Duration::from_secs(2));
    engine.post(|s| s.outputs.stop_recording());
    let start = Instant::now();
    while !engine.call(|s| s.studio_take.as_ref().is_none_or(|t| t.finished())) {
        assert!(start.elapsed() < Duration::from_secs(10), "the take's files stop");
        std::thread::sleep(Duration::from_millis(50));
    }
    engine.call(|s| s.studio_take = None);
    let mut sizes: Vec<(u64, u64)> = tracks
        .iter()
        .map(|t| {
            let (w, h, secs) = probe(&t.file);
            assert!(secs > 1.0, "{} holds {secs} s", t.file.display());
            (w, h)
        })
        .collect();
    sizes.sort();
    assert_eq!(sizes, vec![(480, 640), (1600, 900)], "each at its own size");
    let _ = std::fs::remove_dir_all(&dir);
}
