//! The engine's buffer size changes live: the engine restarts at 128 frames and keeps playing
//! the mixer.

use std::time::Duration;

use lightair_model::{Command, Document, Source, Workspace, Track, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

fn peak_of(ring: &mut rtrb::Consumer<f32>) -> (f32, usize) {
    let n = ring.slots();
    let chunk = ring.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let p = a.iter().chain(b).fold(0f32, |m, v| m.max(v.abs()));
    chunk.commit_all();
    (p, n)
}

#[test]
#[ignore = "depends on this Mac: real-time audio pacing; cargo xtask test --machine"]
fn a_smaller_buffer_restarts_the_engine_and_keeps_playing() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Block"));
    let tone = Source::new("Tone", "lightair_test_tone", json!({"freq": 400.0, "amplitude": 0.3}));
    let mut t = Track::new("Tone", tone.id);
    t.cue = true;
    for cmd in [Command::AddSource { source: tone }, Command::AddTrack { track: t, index: None }] {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    }
    std::thread::sleep(Duration::from_millis(400));

    let workspace = (**doc.workspace()).clone();
    assert!(engine.call(move |s| s.reconciler.set_audio_block(128, &workspace)), "changes while nothing records");
    std::thread::sleep(Duration::from_millis(500));
    let mut cue = engine.call(|s| s.reconciler.audio_test_monitor(false)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let _ = peak_of(&mut cue);
    std::thread::sleep(Duration::from_millis(400));
    let (p, frames) = peak_of(&mut cue);
    eprintln!("after the restart: peak {p:.3}, {frames} samples in 400 ms");
    assert!((p - 0.3).abs() < 0.01, "the mixer plays again ({p})");
    // the monitor keeps a few blocks ahead of the device, no more
    assert!(frames > 0 && frames <= 768 * 2 + 256, "keeps pace ({frames})");
    drop(engine);
}
