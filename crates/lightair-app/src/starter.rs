//! The workspace a first launch starts with (until onboarding generates one from the user's
//! devices; docs/05-interface-shell.md, "Onboarding").

use lightair_core::{Command, Source, Object, Layout, Workspace, Scene, Track};
use serde_json::json;

pub fn starter_workspace() -> Workspace {
    let mut p = Workspace::new("My Show");
    let canvas = p.primary_canvas().clone();
    let (w, h) = (canvas.width as f32, canvas.height as f32);

    let background = Source::new(
        "Background",
        "color_source_v3",
        json!({"color": 0xFF1B1B1Cu32, "width": canvas.width, "height": canvas.height}),
    );
    let display = Source::new("Main display", lightair_core::kinds::SCREEN, crate::shell::display_settings());
    let icon_path = brand_icon();
    let logo = Source::new("Lightair logo", "image_source", json!({"file": icon_path.to_string_lossy()}));

    let mut main = Scene::new("Main");
    main.objects = vec![
        Object::new("Background", background.id, [(canvas.id, Layout::boxed(0.0, 0.0, w, h))]),
        Object::new("Main display", display.id, [(canvas.id, Layout::boxed(w * 0.05, h * 0.08, w * 0.9, h * 0.84))]),
        Object::new("Logo", logo.id, [(canvas.id, Layout::boxed(w - 200.0, h - 200.0, 160.0, 160.0))]),
    ];
    let mut soon = Scene::new("Starting Soon");
    soon.objects = vec![
        Object::new("Background", background.id, [(canvas.id, Layout::boxed(0.0, 0.0, w, h))]),
        Object::new("Logo", logo.id, [(canvas.id, Layout::boxed(w / 2.0 - 160.0, h / 2.0 - 160.0, 320.0, 320.0))]),
    ];
    let mut brb = Scene::new("Be Right Back");
    brb.objects = vec![
        Object::new("Background", background.id, [(canvas.id, Layout::boxed(0.0, 0.0, w, h))]),
        Object::new("Logo", logo.id, [(canvas.id, Layout::boxed(80.0, h - 240.0, 160.0, 160.0))]),
    ];

    let mic = Source::new(crate::mixer::SYSTEM_DEFAULT, lightair_core::kinds::MIC, json!({"device": ""}));
    let desktop = Source::new("Desktop audio", lightair_core::kinds::DESKTOP_AUDIO, json!({}));
    let mut mic_track = Track::new(crate::mixer::SYSTEM_DEFAULT, mic.id);
    mic_track.effects = crate::mixer::voice_clarity();
    let desktop_track = Track::new("Desktop audio", desktop.id);

    for cmd in [
        Command::AddSource { source: background },
        Command::AddSource { source: display },
        Command::AddSource { source: logo },
        Command::AddScene { scene: soon, index: None },
        Command::AddScene { scene: main, index: None },
        Command::AddScene { scene: brb, index: None },
        Command::AddSource { source: mic },
        Command::AddSource { source: desktop },
        Command::AddTrack { track: mic_track, index: None },
        Command::AddTrack { track: desktop_track, index: None },
    ] {
        cmd.apply(&mut p).expect("starter workspace is valid");
    }
    p
}

/// A starter with no capture sources (colour, text, a timer and silent tracks), so it never
/// triggers a camera, screen or microphone permission prompt. Used for screenshots and demos
/// with `LIGHTAIR_STARTER=plain`.
pub fn plain_workspace() -> Workspace {
    let mut p = Workspace::new("Demo");
    let canvas = p.primary_canvas().clone();
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let bg = Source::new("Background", "color_source_v3", json!({"color": 0xFF2A1A12u32, "width": w, "height": h}));
    let card = Source::new("Card", "color_source_v3", json!({"color": 0xFF3B4AFFu32, "width": 960, "height": 540}));
    let title = Source::new(
        "Title",
        "text_ft2_source_v2",
        json!({"text": "Lightair", "font": {"face": crate::ui::TEXT_FACE, "size": 144, "flags": 1}, "color1": 0xFFFFFFFFu32, "color2": 0xFFFFFFFFu32}),
    );
    let timer = Source::new(
        "Countdown",
        lightair_engine::timers::TIMER_KIND,
        json!({"mode": "countdown", "seconds": 300, "font": {"face": crate::ui::TEXT_FACE, "size": 96, "flags": 0}, "color1": 0xFFFFFFFFu32, "color2": 0xFFFFFFFFu32}),
    );
    let icon = brand_icon();
    let logo = Source::new("Logo", "image_source", json!({"file": icon.to_string_lossy()}));
    let f = canvas.id;
    let mut main = Scene::new("Main");
    main.objects.push(Object::new("Background", bg.id, [(f, Layout::boxed(0.0, 0.0, w, h))]));
    main.objects.push(Object::new("Card", card.id, [(f, Layout::boxed(480.0, 270.0, 960.0, 540.0))]));
    main.objects.push(Object::new("Title", title.id, [(f, Layout::boxed(660.0, 440.0, 600.0, 200.0))]));
    let mut soon = Scene::new("Starting Soon");
    soon.objects.push(Object::new("Background", bg.id, [(f, Layout::boxed(0.0, 0.0, w, h))]));
    soon.objects.push(Object::new("Countdown", timer.id, [(f, Layout::boxed(760.0, 470.0, 400.0, 140.0))]));
    let logo_box = Layout {
        shape: Some(lightair_core::Shape::new(lightair_core::ShapeKind::Circle)),
        crop: lightair_core::Crop { left: 64, top: 64, right: 64, bottom: 64 },
        tilt_y: 20.0,
        ..Layout::boxed(1500.0, 640.0, 360.0, 360.0)
    };
    soon.objects.push(Object::new("Logo", logo.id, [(f, logo_box)]));
    let tone = |name: &str| Source::new(name, "color_source_v3", json!({}));
    let (a, b) = (tone("Mic (demo)"), tone("Music (demo)"));
    let tracks = [Track::new("Mic", a.id), Track::new("Music", b.id)];
    let mut commands = vec![
        Command::AddSource { source: bg },
        Command::AddSource { source: card },
        Command::AddSource { source: title },
        Command::AddSource { source: timer },
        Command::AddSource { source: logo },
        Command::AddSource { source: a },
        Command::AddSource { source: b },
        Command::AddScene { scene: soon, index: None },
        Command::AddScene { scene: main, index: None },
    ];
    commands.extend(tracks.into_iter().map(|track| Command::AddTrack { track, index: None }));
    for c in commands {
        c.apply(&mut p).expect("plain starter is valid");
    }
    p
}

/// The brand icon: the app's own (Contents/Resources), or the repository's, found from the
/// running executable whether it runs from `target/debug` or inside the development app bundle.
fn brand_icon() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let own = exe.parent()?.parent()?.join("Resources/lightair-icon-512.png");
            if own.exists() {
                return Some(own);
            }
            exe.ancestors().map(|a| a.join("branding/final/icon/lightair-icon-512.png")).find(|p| p.exists())
        })
        .unwrap_or_default()
}
