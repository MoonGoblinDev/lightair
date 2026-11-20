//! As in OBS, a video plays while a scene showing it is on air and stops when it goes off air
//! (no decoding off air), here with a mixer track for its sound too; switching between two
//! scenes that both show it keeps it playing.

use std::time::Duration;

use lightair_model::{Command, Document, Layout, Object, Scene, Source, Track, TransitionKind, TransitionSettings, Workspace, diff};
use lightair_engine::media::MediaState;
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn state(engine: &Engine, id: lightair_model::Id) -> (MediaState, i64) {
    std::thread::sleep(Duration::from_millis(700));
    let st = engine.call(move |s| s.reconciler.media_status(id)).unwrap();
    (st.state, st.time_ms)
}

#[test]
fn a_video_plays_only_on_air() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    // a 3-second 160×90 clip (ffmpeg testsrc), looping
    let video = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/clip.mp4");
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Media on air"));
    let canvas = doc.workspace().primary_canvas().id;
    let src = Source::new("Video", "ffmpeg_source", json!({"local_file": video.to_string_lossy(), "looping": true, "is_local_file": true}));
    let id = src.id;
    let (a, b, c) = (Scene::new("A"), Scene::new("B (no video)"), Scene::new("C (video too)"));
    let (a_id, b_id, c_id) = (a.id, b.id, c.id);
    let object = |n: &str| Object::new(n, id, [(canvas, Layout::boxed(0.0, 0.0, 320.0, 180.0))]);
    for cmd in [
        Command::AddSource { source: src },
        Command::AddScene { scene: b, index: None },
        Command::AddScene { scene: a, index: None },
        Command::AddScene { scene: c, index: None },
        Command::AddObject { scene: a_id, object: object("in A"), index: None },
        Command::AddObject { scene: c_id, object: object("in C"), index: None },
        Command::AddTrack { track: Track::new("Video", id), index: None },
        Command::SetProgram { scene: Some(b_id) },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    // with each kind of switch (Move builds scenes of its own on the way)
    for kind in [TransitionKind::Cut, TransitionKind::Fade, TransitionKind::Move] {
        let settings = TransitionSettings { kind, duration_ms: 300, ..Default::default() };
        apply(&engine, &mut doc, Command::SetTransition { settings });
        let off = state(&engine, id);
        eprintln!("{kind:?}: B on air: {off:?}");
        assert_ne!(off.0, MediaState::Playing, "{kind:?}: off air, not playing");
        apply(&engine, &mut doc, Command::SetProgram { scene: Some(a_id) });
        let on = state(&engine, id);
        eprintln!("{kind:?}: A on air: {on:?}");
        assert_eq!(on.0, MediaState::Playing, "{kind:?}: on air, playing");
        apply(&engine, &mut doc, Command::SetProgram { scene: Some(c_id) });
        let still = state(&engine, id);
        eprintln!("{kind:?}: C on air: {still:?}");
        assert_eq!(still.0, MediaState::Playing, "{kind:?}: still on air in C, playing");
        assert!(still.1 > on.1, "{kind:?}: carried on, not restarted: {on:?} then {still:?}");
        apply(&engine, &mut doc, Command::SetProgram { scene: Some(b_id) });
    }
    let off = state(&engine, id);
    assert_ne!(off.0, MediaState::Playing, "off air again: stopped");
    drop(engine);
}
