//! Audio Unit hosting against Apple's own units (always installed): scan, load, process, state.
#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lightair_audio::dsp::Plugin;
use lightair_engine::plugins::{self, AudioUnitPlugin, PluginInfo};

const SR: f64 = 48_000.0;

fn hipass(list: &[PluginInfo]) -> PluginInfo {
    list.iter().find(|p| p.vendor == "Apple" && p.name == "AUHipass").cloned().expect("AUHipass is installed")
}

fn load(info: &PluginInfo) -> Arc<AudioUnitPlugin> {
    let (tx, rx) = std::sync::mpsc::channel();
    plugins::load_audio_unit(info, SR, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(10)).expect("loads in time").expect("loads")
}

/// RMS of the last half of `blocks` blocks of a sine at `hz` through the plugin.
fn level(p: &dyn Plugin, hz: f32, blocks: usize) -> f32 {
    let mut t = 0usize;
    let (mut sum, mut n) = (0.0f64, 0usize);
    for b in 0..blocks {
        let mut l: Vec<f32> =
            (0..256).map(|i| (std::f32::consts::TAU * hz * (t + i) as f32 / SR as f32).sin() * 0.5).collect();
        let mut r = l.clone();
        t += 256;
        assert!(p.process(&mut l, &mut r));
        if b >= blocks / 2 {
            sum += l.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
            n += l.len();
        }
    }
    (sum / n as f64).sqrt() as f32
}

#[test]
fn audio_units_scan_load_process_and_keep_state() {
    let list = plugins::scan_audio_units();
    assert!(list.len() >= 5, "Apple's effects at least: {}", list.len());
    let info = hipass(&list);
    assert_eq!(info.format, "au");

    let p = load(&info);
    // a high pass (default cutoff well above 100 Hz): lows go, highs stay
    let low = level(&*p, 100.0, 40);
    let high = level(&*p, 15_000.0, 40);
    let sine = 0.5 / 2f32.sqrt();
    assert!(low < sine * 0.1, "100 Hz cut: {low}");
    assert!(high > sine * 0.8, "15 kHz kept: {high}");

    // state: read, and given to a fresh instance
    let state = p.state().expect("has state");
    assert!(state.len() > 20);
    let q = load(&info);
    assert!(q.set_state(&state));
    assert!(!q.crashed());
}

#[test]
fn a_workspace_insert_loads_and_reports() {
    let info = hipass(&plugins::scan_audio_units());
    let e = lightair_model::catalog::plugin_effect("au", &info.id, &info.name, &info.vendor);
    assert!(plugins::instance(&e).is_none(), "not loaded yet");
    let t0 = Instant::now();
    while !plugins::changed() {
        assert!(t0.elapsed() < Duration::from_secs(10), "loads");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(plugins::status(e.id), plugins::Status::Ready { .. }), "{:?}", plugins::status(e.id));
    let p = plugins::instance(&e).expect("ready");
    assert!(level(&*p, 100.0, 40) < 0.05);
    assert!(plugins::take_state(e.id).is_some());
    // a missing one fails, reported
    let gone = lightair_model::catalog::plugin_effect("au", "au:61756678:00000000:00000000", "Gone", "Nobody");
    assert!(plugins::instance(&gone).is_none());
    assert!(matches!(plugins::status(gone.id), plugins::Status::Failed(_)));
    plugins::retain(&Default::default());
    assert_eq!(plugins::status(e.id), plugins::Status::None);
}
