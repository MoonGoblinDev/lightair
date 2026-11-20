//! Multitrack recording: each track to its own WAV, sample-aligned, at the chosen point; plus
//! the program for audio-only recordings.

use std::time::Duration;

use lightair_model::{Command, Document, Effect, Source, Workspace, Track, TrackChannels, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

#[test]
fn tracks_record_to_aligned_files() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = std::env::temp_dir().join(format!("lightair-track_files-{}", std::process::id()));
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Multitrack"));
    let apply = |doc: &mut Document, cmd: Command| {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    };
    let tone = Source::new("Tone", "lightair_test_tone", json!({"freq": 440.0, "amplitude": 0.5}));
    let mut host = Track::new("Host", tone.id);
    host.channels = TrackChannels::Mono(0);
    host.effects = vec![Effect::new("Gain", "audio.gain", json!({"db": -6.0206}))];
    host.volume_db = -20.0; // the fader doesn't touch a pre-fader file
    let guest = Track::new("Guest", tone.id);
    apply(&mut doc, Command::AddSource { source: tone });
    apply(&mut doc, Command::AddTrack { track: host, index: None });
    apply(&mut doc, Command::AddTrack { track: guest, index: None });
    std::thread::sleep(Duration::from_millis(500));

    let workspace = doc.workspace().clone();
    let d = dir.clone();
    let files = engine.call(move |s| s.reconciler.start_multitrack(&workspace, &d, "Show", true)).unwrap();
    assert_eq!(files.len(), 3, "two tracks and the program");
    std::thread::sleep(Duration::from_millis(1000));
    let report = engine.call(|s| s.reconciler.stop_multitrack()).expect("finished");
    eprintln!("{} frames, start {}", report.frames, report.start_ns);
    assert!((40_000..60_000).contains(&report.frames), "about a second ({})", report.frames);
    assert!(report.start_ns > 0);

    let read = |name: &str| {
        let mut r = hound::WavReader::open(dir.join(name)).unwrap();
        let spec = r.spec();
        let samples: Vec<f32> = r.samples::<i32>().map(|s| s.unwrap() as f32 / 8_388_607.0).collect();
        (spec, samples)
    };
    let (hs, host) = read("Show - Host.wav");
    let (gs, guest) = read("Show - Guest.wav");
    let (ps, _) = read("Show - Program.wav");
    assert_eq!((hs.channels, hs.bits_per_sample, hs.sample_rate), (1, 24, 48_000), "a mono track: a mono file");
    assert_eq!((gs.channels, ps.channels), (2, 2));
    assert_eq!(host.len() as u64, report.frames, "every file the same length");
    assert_eq!(guest.len() as u64, report.frames * 2);
    let peak = |x: &[f32]| x.iter().fold(0f32, |m, v| m.max(v.abs()));
    let tail = |x: &[f32]| x[x.len() / 2..].to_vec();
    assert!((peak(&tail(&host)) - 0.25).abs() < 0.01, "after its effect, before its fader: {}", peak(&tail(&host)));
    assert!((peak(&tail(&guest)) - 0.5).abs() < 0.01);
    // the same source in both: sample-aligned files (the host is the guest at −6 dB)
    for k in (host.len() / 2..host.len()).step_by(997) {
        assert!((host[k] - guest[k * 2] * 0.5).abs() < 1e-3, "frame {k}: {} vs {}", host[k], guest[k * 2]);
    }
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
