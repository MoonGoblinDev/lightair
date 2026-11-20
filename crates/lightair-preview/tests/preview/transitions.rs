//! Slide, Wipe and Dip render what they should on the way, sampled from the program's frames.

use std::time::{Duration, Instant};

use lightair_model::{
    Command, Direction, Document, Id, Source, Object, Layout, Workspace, Scene, TransitionKind, TransitionSettings, diff,
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

/// Samples (left quarter, right quarter, centre) of the newest frame.
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
fn black(p: (u8, u8, u8)) -> bool {
    p.0 < 50 && (110..150).contains(&p.1) && (110..150).contains(&p.2)
}

/// Switches to `to` with `settings` and samples every frame the preview gets until the
/// transition is over. Watching every frame, not one timed sample, holds on a busy machine: a
/// late frame is just one fewer seen.
fn transition(
    engine: &Engine,
    doc: &mut Document,
    preview: &Preview,
    settings: TransitionSettings,
    to: Id,
) -> Vec<[(u8, u8, u8); 3]> {
    apply(engine, doc, Command::SetTransition { settings });
    let start = Instant::now();
    apply(engine, doc, Command::SetProgram { scene: Some(to) });
    // a little past the end, so the next one doesn't start while this one runs
    let end = start + Duration::from_millis(settings.duration_ms as u64 + 300);
    let (mut seen, mut frames) = (Vec::new(), preview.frames());
    while Instant::now() < end {
        if preview.frames() != frames {
            frames = preview.frames();
            seen.push(sample(preview));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    seen
}

#[test]
fn directional_and_dip_transitions() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Transitions"));
    let f = doc.workspace().primary_canvas().id;
    let r = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 320, "height": 180}));
    let b = Source::new("Blue", "color_source_v3", json!({"color": 0xFFFF0000u32, "width": 320, "height": 180}));
    let mut a_scene = Scene::new("A");
    a_scene.objects.push(Object::new("Red", r.id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let mut b_scene = Scene::new("B");
    b_scene.objects.push(Object::new("Blue", b.id, [(f, Layout::boxed(0.0, 0.0, 320.0, 180.0))]));
    let (a, bb) = (a_scene.id, b_scene.id);
    for cmd in [
        Command::AddSource { source: r },
        Command::AddSource { source: b },
        Command::AddScene { scene: a_scene, index: None },
        Command::AddScene { scene: b_scene, index: None },
        Command::SetProgram { scene: Some(a) },
    ] {
        apply(&engine, &mut doc, cmd);
    }
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(300));
    assert!(red(sample(&preview)[2]), "starts on A");

    let slide = TransitionSettings { kind: TransitionKind::Slide, duration_ms: 800, direction: Direction::Left };
    let seen = transition(&engine, &mut doc, &preview, slide, bb);
    assert!(seen.iter().any(|[l, r, _]| red(*l) && blue(*r)), "B pushes in from the right: {seen:?}");
    assert!(blue(sample(&preview)[2]), "lands on B");

    let wipe = TransitionSettings { kind: TransitionKind::Wipe, duration_ms: 800, direction: Direction::Right };
    let seen = transition(&engine, &mut doc, &preview, wipe, a);
    assert!(seen.iter().any(|[l, r, _]| red(*l) && blue(*r)), "A wipes in from the left: {seen:?}");
    assert!(red(sample(&preview)[2]), "lands on A");

    let dip = TransitionSettings { kind: TransitionKind::Dip, duration_ms: 800, ..Default::default() };
    let seen = transition(&engine, &mut doc, &preview, dip, bb);
    assert!(seen.iter().any(|[_, _, c]| black(*c)), "dips through black: {seen:?}");
    assert!(blue(sample(&preview)[2]), "lands on B");

    // Back to Fade: the fade source is restored and still works.
    let fade = TransitionSettings { kind: TransitionKind::Fade, duration_ms: 300, ..Default::default() };
    transition(&engine, &mut doc, &preview, fade, a);
    assert!(red(sample(&preview)[2]), "fade lands on A");

    // A scene's own transition: switching to B dips through black (B's own), switching back
    // to A fades (the default), no black on the way.
    apply(&engine, &mut doc, Command::SetSceneTransition { scene: bb, transition: Some(dip) });
    let watch = |doc: &mut Document, to: Id, ms: u64| {
        apply(&engine, doc, Command::SetProgram { scene: Some(to) });
        let end = Instant::now() + Duration::from_millis(ms + 300);
        let (mut seen, mut frames) = (Vec::new(), preview.frames());
        while Instant::now() < end {
            if preview.frames() != frames {
                frames = preview.frames();
                seen.push(sample(&preview));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        seen
    };
    let seen = watch(&mut doc, bb, 800);
    assert!(seen.iter().any(|[_, _, c]| black(*c)), "to B: its own dip: {seen:?}");
    assert!(blue(sample(&preview)[2]), "lands on B");
    let seen = watch(&mut doc, a, 300);
    assert!(!seen.iter().any(|[_, _, c]| black(*c)), "back to A: the default fade, no black: {seen:?}");
    assert!(red(sample(&preview)[2]), "lands on A");
    drop(preview);
    drop(engine);
}
