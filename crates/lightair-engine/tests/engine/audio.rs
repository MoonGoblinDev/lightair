//! Audio tracks end to end: media file → track (output channel) → volume meter → level cells.

use std::io::Write;
use std::time::Duration;

use lightair_model::{Command, Document, Source, Workspace, Track, diff};
use lightair_engine::meters::meters;
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

/// Writes a looping-friendly 1 kHz stereo sine at −6 dBFS (amplitude 0.5), 48 kHz, 2 s.
fn tone_wav(path: &std::path::Path) {
    let rate = 48_000u32;
    let frames = rate * 2;
    let mut data = Vec::with_capacity(frames as usize * 4);
    for i in 0..frames {
        let s = (0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin() * i16::MAX as f32) as i16;
        data.extend_from_slice(&s.to_le_bytes());
        data.extend_from_slice(&s.to_le_bytes());
    }
    let mut f = std::fs::File::create(path).unwrap();
    let byte_rate = rate * 4;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data.len() as u32).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    f.write_all(&2u16.to_le_bytes()).unwrap(); // channels
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&byte_rate.to_le_bytes()).unwrap();
    f.write_all(&4u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    f.write_all(&data).unwrap();
}

fn peak_after(track: lightair_model::Id, wait: Duration) -> f32 {
    std::thread::sleep(wait);
    let cell = meters().get(track).expect("meter cell exists");
    let l = cell.levels();
    l.peak[0].max(l.peak[1])
}

#[test]
fn track_levels_follow_the_fader() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let dir = std::env::temp_dir().join(format!("lightair-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("tone.wav");
    tone_wav(&wav);

    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Audio"));
    let source = Source::new(
        "Tone",
        "ffmpeg_source",
        json!({"local_file": wav.to_string_lossy(), "looping": true, "is_local_file": true, "restart_on_activate": false}),
    );
    let track = Track::new("Tone", source.id);
    let track_id = track.id;
    let apply = |doc: &mut Document, cmd: Command| {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    };
    apply(&mut doc, Command::AddSource { source });
    apply(&mut doc, Command::AddTrack { track, index: None });

    let full = peak_after(track_id, Duration::from_millis(1200));
    eprintln!("peak at 0 dB: {full:.1} dBFS");
    assert!((-8.0..=-4.0).contains(&full), "expected about −6 dBFS, got {full}");
    // The stream mix's loudness: a stereo 1 kHz sine at amplitude 0.5 is −6 LUFS.
    let loud = lightair_engine::loudness::stream_loudness().expect("stream meter").momentary;
    eprintln!("stream loudness: {loud:.1} LUFS");
    assert!((loud + 6.0).abs() < 1.0, "expected about −6 LUFS, got {loud}");

    apply(&mut doc, Command::SetTrackVolume { track: track_id, volume_db: -20.0 });
    let quiet = peak_after(track_id, Duration::from_millis(600));
    eprintln!("peak at −20 dB: {quiet:.1} dBFS");
    assert!((full - quiet - 20.0).abs() < 2.0, "fader should lower the level by 20 dB ({full} → {quiet})");
    std::thread::sleep(Duration::from_millis(500));
    let soft = lightair_engine::loudness::stream_loudness().unwrap().momentary;
    eprintln!("stream loudness at −20 dB: {soft:.1} LUFS");
    assert!((soft + 26.0).abs() < 1.5, "expected about −26 LUFS, got {soft}");

    apply(&mut doc, Command::RemoveTrack { track: track_id });
    assert!(meters().get(track_id).is_none(), "meter cell removed with the track");
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
