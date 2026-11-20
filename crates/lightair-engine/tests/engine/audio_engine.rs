//! The audio engine in real time against libobs: a tone source → track (engine effects, fader,
//! mute) → bus → Program in libobs mix 1, and a send → a mix in libobs mix 2. libobs itself no
//! longer mixes the source.

use std::sync::Mutex;
use std::time::Duration;

use lightair_model::{AudioMix, Bus, Command, Document, Effect, Source, Workspace, Route, Send, Track, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

/// The loudest sample each libobs mix carried since the last read.
static PEAKS: Mutex<[f32; 2]> = Mutex::new([0.0; 2]);

unsafe extern "C" fn on_mix(param: *mut std::ffi::c_void, _: usize, data: *mut libobs_sys::audio_data) {
    let mix = param as usize;
    let Some(data) = (unsafe { data.as_ref() }) else { return };
    let mut peak = 0f32;
    for c in 0..2 {
        if !data.data[c].is_null() {
            let s = unsafe { std::slice::from_raw_parts(data.data[c] as *const f32, data.frames as usize) };
            peak = s.iter().fold(peak, |m, v| m.max(v.abs()));
        }
    }
    let mut p = PEAKS.lock().unwrap();
    p[mix] = p[mix].max(peak);
}

/// Peaks over `ms`, after a moment for audio already on its way (libobs buffers ~100 ms).
fn peaks_after(ms: u64) -> [f32; 2] {
    std::thread::sleep(Duration::from_millis(300));
    *PEAKS.lock().unwrap() = [0.0; 2];
    std::thread::sleep(Duration::from_millis(ms));
    *PEAKS.lock().unwrap()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[test]
fn the_engine_mixes_tracks_into_libobs() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or("warn".into())).with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    for mix in 0..2usize {
        unsafe { libobs_sys::obs_add_raw_audio_callback(mix, std::ptr::null(), Some(on_mix), mix as *mut _) };
    }
    let mut doc = Document::new(Workspace::new("Engine"));
    let apply = |doc: &mut Document, cmd: Command| {
        let change = doc.apply(&cmd).unwrap();
        let patches = diff(&change.before, &change.after);
        let workspace = change.after.clone();
        engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    };
    let tone = Source::new("Tone", "lightair_test_tone", json!({"freq": 440.0, "amplitude": 0.5}));
    let mut track = Track::new("Tone", tone.id);
    // −6 dB in the engine's own Gain effect
    track.effects = vec![Effect::new("Gain", "audio.gain", json!({"db": -6.0206}))];
    let tid = track.id;
    apply(&mut doc, Command::AddSource { source: tone });
    apply(&mut doc, Command::AddTrack { track, index: None });

    let p = peaks_after(600);
    eprintln!("program {:.1} dBFS", db(p[0]));
    assert!((db(p[0]) - db(0.25)).abs() < 0.5, "0.5 through −6 dB: 0.25, and not mixed twice ({})", p[0]);

    // muted: the stream is silent
    let t = (**doc.workspace().track(tid).unwrap()).clone();
    apply(&mut doc, Command::UpdateTrack { track: Track { muted: true, ..t.clone() } });
    let p = peaks_after(600);
    assert!(p[0] < 1e-3, "muted: silence ({})", p[0]);

    // through a bus at −6 dB, and a pre-fader send to a mix (libobs mix 2)
    let bus = Bus { volume_db: -6.0206, ..Bus::new("Guests") };
    let phones = AudioMix::new("Headphones");
    apply(&mut doc, Command::AddBus { bus: bus.clone(), index: None });
    apply(&mut doc, Command::AddMix { mix: phones.clone() });
    apply(&mut doc, Command::UpdateTrack {
        track: Track {
            muted: false,
            route: Route::Bus(bus.id),
            sends: vec![Send { mix: phones.id, level_db: 0.0, pre_fader: true }],
            ..t
        },
    });
    let p = peaks_after(600);
    eprintln!("program via bus {:.1} dBFS, headphones mix {:.1} dBFS", db(p[0]), db(p[1]));
    assert!((db(p[0]) - db(0.125)).abs() < 0.5, "−6 in the track, −6 on the bus ({})", p[0]);
    assert!((db(p[1]) - db(0.25)).abs() < 0.5, "the send: after the effect, before the faders ({})", p[1]);

    for mix in 0..2usize {
        unsafe { libobs_sys::obs_remove_raw_audio_callback(mix, Some(on_mix), mix as *mut _) };
    }
    drop(engine);
}
