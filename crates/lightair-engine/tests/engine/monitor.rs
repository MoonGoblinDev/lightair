//! Direct monitoring: the cue mix (or the program) straight from the engine, block by block.

use std::time::Duration;

use lightair_model::{Command, Document, Source, Workspace, Track, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn peak_of(ring: &mut rtrb::Consumer<f32>) -> f32 {
    let n = ring.slots();
    let chunk = ring.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let p = a.iter().chain(b).fold(0f32, |m, v| m.max(v.abs()));
    chunk.commit_all();
    p
}

#[test]
#[ignore = "depends on this Mac: real-time audio pacing; cargo xtask test --machine"]
fn the_monitor_hears_the_cued_tracks() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Monitor"));
    let apply = |doc: &mut Document, cmd: Command| {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    };
    let a = Source::new("A", "lightair_test_tone", json!({"freq": 300.0, "amplitude": 0.4}));
    let b = Source::new("B", "lightair_test_tone", json!({"freq": 500.0, "amplitude": 0.2}));
    let mut cued = Track::new("Cued", a.id);
    cued.cue = true;
    let other = Track::new("Other", b.id);
    apply(&mut doc, Command::AddSource { source: a });
    apply(&mut doc, Command::AddSource { source: b });
    apply(&mut doc, Command::AddTrack { track: cued, index: None });
    apply(&mut doc, Command::AddTrack { track: other, index: None });
    std::thread::sleep(Duration::from_millis(400));

    let mut cue = engine.call(|s| s.reconciler.audio_test_monitor(false)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let _ = peak_of(&mut cue); // the monitor fills a few blocks ahead, then keeps pace
    std::thread::sleep(Duration::from_millis(400));
    let p = peak_of(&mut cue);
    eprintln!("cue monitor peak {p:.3}");
    assert!((p - 0.4).abs() < 0.01, "only the cued track ({p})");

    let mut program = engine.call(|s| s.reconciler.audio_test_monitor(true)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let p = peak_of(&mut program);
    eprintln!("program monitor peak {p:.3}");
    assert!(p > 0.5 && p <= 0.61, "both tracks ({p})");
    drop(engine);
}
