//! Countdown and stopwatch objects: start on air, pause, add time, end with an event.

use std::time::{Duration, Instant};

use lightair_model::{Command, Document, Source, Object, Layout, Workspace, Scene, diff};
use lightair_engine::timers::{TIMER_KIND, TimerAction};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn wait_text(engine: &Engine, id: lightair_model::Id, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let t = engine.call(move |s| s.reconciler.timer_text(id)).unwrap_or_default();
        if t == want || Instant::now() > deadline {
            return t;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn countdown_and_stopwatch() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Timers"));
    let f = doc.workspace().primary_canvas().id;
    let countdown = Source::new("Countdown", TIMER_KIND, json!({"mode": "countdown", "seconds": 2, "end_text": "Go!"}));
    let watch = Source::new("Stopwatch", TIMER_KIND, json!({"mode": "stopwatch", "autostart": false}));
    let (cd, sw) = (countdown.id, watch.id);
    let mut on_air = Scene::new("Soon");
    on_air.objects.push(Object::new("Countdown", cd, [(f, Layout::boxed(0.0, 0.0, 320.0, 90.0))]));
    on_air.objects.push(Object::new("Stopwatch", sw, [(f, Layout::boxed(0.0, 90.0, 320.0, 90.0))]));
    let scene = on_air.id;
    // The first scene goes on air by itself; the timers' scene comes second.
    apply(&engine, &mut doc, Command::AddScene { scene: Scene::new("Live"), index: None });
    apply(&engine, &mut doc, Command::AddSource { source: countdown });
    apply(&engine, &mut doc, Command::AddSource { source: watch });
    apply(&engine, &mut doc, Command::AddScene { scene: on_air, index: None });

    // Not on air yet: the countdown waits at its full length.
    assert_eq!(wait_text(&engine, cd, "0:02"), "0:02");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!engine.call(move |s| s.reconciler.timer_status(cd)).unwrap().running, "waits until on air");

    // On air: it starts by itself; the stopwatch (no autostart) doesn't.
    apply(&engine, &mut doc, Command::SetProgram { scene: Some(scene) });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !engine.call(move |s| s.reconciler.timer_status(cd)).unwrap().running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(engine.call(move |s| s.reconciler.timer_status(cd)).unwrap().running);
    assert!(!engine.call(move |s| s.reconciler.timer_status(sw)).unwrap().running);

    // Pause holds it; adding a second extends it.
    engine.call(move |s| s.reconciler.timer_action(cd, TimerAction::Pause));
    let held = engine.call(move |s| s.reconciler.timer_status(cd)).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(engine.call(move |s| s.reconciler.timer_status(cd)).unwrap().seconds, held.seconds, "paused");
    engine.call(move |s| s.reconciler.timer_action(cd, TimerAction::Add(1)));
    assert_eq!(engine.call(move |s| s.reconciler.timer_status(cd)).unwrap().seconds, held.seconds + 1);
    engine.call(move |s| s.reconciler.timer_action(cd, TimerAction::Start));
    engine.call(move |s| s.reconciler.timer_action(sw, TimerAction::Start));

    // It ends: end text, an ended event, stopped.
    assert_eq!(wait_text(&engine, cd, "Go!"), "Go!");
    let status = engine.call(move |s| s.reconciler.timer_status(cd)).unwrap();
    assert!(status.ended && !status.running && status.seconds == 0, "{status:?}");
    assert_eq!(engine.call(|s| s.reconciler.take_ended_timers()), vec![cd]);
    assert!(engine.call(|s| s.reconciler.take_ended_timers()).is_empty(), "reported once");

    // The stopwatch has been counting since it was started.
    // The text is from the last frame, the status is live: they can differ by a second at a
    // boundary, never more.
    let text = engine.call(move |s| s.reconciler.timer_text(sw)).unwrap();
    let s = engine.call(move |s| s.reconciler.timer_status(sw)).unwrap();
    assert!(s.running && s.seconds >= 2, "{s:?}");
    let shown = [format!("0:{:02}", s.seconds), format!("0:{:02}", s.seconds - 1)];
    assert!(shown.contains(&text), "stopwatch shows {text}, status {s:?}");

    // Reset brings the countdown back.
    engine.call(move |s| s.reconciler.timer_action(cd, TimerAction::Reset));
    assert_eq!(wait_text(&engine, cd, "0:02"), "0:02");
    drop(engine);
}
