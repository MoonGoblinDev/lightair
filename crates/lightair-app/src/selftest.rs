//! Headless self-tests run inside the app bundle (so they use Lightair's own macOS privacy
//! permissions). `LIGHTAIR_SELFTEST=camera` opens the first camera with Lightair Effect Stack, waits for
//! frames, prints a report to stdout and exits without opening a window.
//! `LIGHTAIR_SELFTEST=green-key` checks Background removal on macOS's own effect end to end:
//! Lightair's Backgrounds picture set to green, the camera keyed over magenta, and how much of
//! the frame is keyed out, green or neither (numbers only: the frames are never saved).

use std::time::Duration;

use lightair_engine::Engine;
use lightair_engine::obs::{Data, ObsScene, ObsSource};

pub fn run(engine: &Engine, what: &str) {
    match what {
        "camera" => camera(engine),
        "green-key" => green_key(engine),
        "virtual-camera" => virtual_camera(engine),
        "web" => web(engine),
        "agent-page" => agent_page(),
        "script" => script(),
        "cookies" => cookies(),
        "cookie-set" => cookie_set(),
        "twitch-login" => twitch_login(),
        "twitch-me" => twitch_me(),
        "displays" => displays(engine),
        // puts Lightair's Backgrounds back: LIGHTAIR_BG_PICTURE and LIGHTAIR_BG_ON (0/1)
        "restore-backgrounds" => {
            use crate::system_effects as sys;
            if let Ok(p) = std::env::var("LIGHTAIR_BG_PICTURE") {
                sys::set_background_picture(std::path::Path::new(&p));
            }
            if let Ok(on) = std::env::var("LIGHTAIR_BG_ON") {
                sys::set_backgrounds_on(on == "1");
            }
            println!("SELFTEST backgrounds now: {:?} {:?}", sys::backgrounds_on(), sys::background_picture());
        }
        other => println!("SELFTEST unknown: {other}"),
    }
}

fn camera(engine: &Engine) {
    let report = engine.call(|s| {
        let devices = lightair_engine::kinds::cameras();
        let Some((name, uid)) = devices.into_iter().find(|(n, _)| !n.contains("Virtual") && !n.contains("VCam")) else {
            return "no camera device".to_string();
        };
        let Some((kind, settings)) = lightair_engine::kinds::resolve(lightair_core::kinds::CAMERA, &serde_json::json!({ "device": uid }))
        else {
            return "no camera source on this platform".into();
        };
        let Some(cam) = ObsSource::create(kind, "Self-test camera", Some(&Data::from_json(&settings))) else {
            return "could not create the camera source".into();
        };
        let mut look = Data::new();
        look.set_int("smooth", 60);
        if let Some(fx) = ObsSource::create_private(crate::camera_studio::KIND, "Self-test Lightair Effect Stack", Some(&look)) {
            unsafe { lightair_engine::sys::obs_source_filter_add(cam.as_ptr(), fx.as_ptr()) };
            s.sources.insert("fx".into(), fx);
        }
        let scene = ObsScene::create("Self-test").unwrap();
        scene.add(&cam);
        s.sources.insert("cam".into(), cam);
        s.scenes.insert("selftest".into(), scene);
        s.set_program("selftest");
        name
    });
    println!("SELFTEST camera device: {report}");
    let mut ok = false;
    for i in 1..=10 {
        std::thread::sleep(Duration::from_millis(500));
        let (w, h) = engine.call(|s| s.sources.get("cam").map_or((0, 0), |c| (c.width(), c.height())));
        if w > 0 {
            println!("SELFTEST camera frames: {w}x{h} after {} ms", i * 500);
            ok = true;
            break;
        }
    }
    if !ok {
        println!("SELFTEST camera frames: none after 5 s (camera permission not granted?)");
    }
    let stats = engine.call(|s| s.stats());
    println!("SELFTEST stats: {stats:?}");
    engine.call(|s| {
        if let (Some(cam), Some(fx)) = (s.sources.get("cam"), s.sources.get("fx")) {
            unsafe { lightair_engine::sys::obs_source_filter_remove(cam.as_ptr(), fx.as_ptr()) };
        }
        s.sources.remove("fx");
    });
}

fn green_key(engine: &Engine) {
    use crate::system_effects as sys;
    let was_on = sys::backgrounds_on();
    println!("SELFTEST backgrounds on for Lightair: {was_on:?}");
    // LIGHTAIR_SET_LATE=1: turned on while the camera runs (as a scene change does)
    let late = std::env::var("LIGHTAIR_SET_LATE").is_ok_and(|v| v == "1");
    if was_on == Some(false) && !late {
        println!("SELFTEST turn backgrounds on: {} -> {:?}", sys::set_backgrounds_on(true), sys::backgrounds_on());
    }
    let before = sys::background_picture();
    println!("SELFTEST picture before: {before:?}");
    // LIGHTAIR_KEY=blue: a blue screen
    let blue = std::env::var("LIGHTAIR_KEY").is_ok_and(|k| k == "blue");
    if !late {
        println!("SELFTEST set {}: {}", if blue { "blue" } else { "green" }, sys::set_background_colour(if blue { sys::BLUE } else { sys::GREEN }));
    }
    println!("SELFTEST picture now: {:?}", sys::background_picture());
    let report = engine.call(move |s| {
        let devices = lightair_engine::kinds::cameras();
        let Some((name, uid)) = devices.into_iter().find(|(n, _)| !n.contains("Virtual") && !n.contains("VCam")) else {
            return "no camera device".to_string();
        };
        let Some((kind, settings)) = lightair_engine::kinds::resolve(lightair_core::kinds::CAMERA, &serde_json::json!({ "device": uid }))
        else {
            return "no camera source on this platform".into();
        };
        let Some(cam) = ObsSource::create(kind, "Self-test camera", Some(&Data::from_json(&settings))) else {
            return "could not create the camera source".into();
        };
        let mut look = Data::new();
        look.set_int("bg_mode", 2);
        look.set_int("bg_model", lightair_core::catalog::BG_MODEL_SYSTEM);
        look.set_int("bg_feather", 30);
        look.set_int("bg_key", i64::from(blue));
        if let Some(fx) = ObsSource::create_private(crate::camera_studio::KIND, "Self-test green key", Some(&look)) {
            unsafe { lightair_engine::sys::obs_source_filter_add(cam.as_ptr(), fx.as_ptr()) };
            s.sources.insert("fx".into(), fx);
        }
        let magenta = Data::from_json(&serde_json::json!({"color": 0xFFFF00FFu32, "width": 1920, "height": 1080}));
        let back = ObsSource::create("color_source_v3", "Self-test magenta", Some(&magenta)).unwrap();
        let scene = ObsScene::create("Self-test").unwrap();
        scene.add(&back);
        scene.add(&cam);
        s.sources.insert("back".into(), back);
        s.sources.insert("cam".into(), cam);
        s.scenes.insert("selftest".into(), scene);
        s.set_program("selftest");
        format!("{name}\u{1f}{uid}")
    });
    let (report, uid) = report.split_once('\u{1f}').map_or((report.clone(), String::new()), |(a, b)| (a.to_string(), b.to_string()));
    println!("SELFTEST camera device: {report}");
    let preview = engine.call(|_| lightair_preview::Preview::attach());
    // the camera starting, and macOS's effect warming up
    if late {
        std::thread::sleep(Duration::from_secs(3));
        println!("SELFTEST late: colour {} on {}", sys::set_background_colour(if blue { sys::BLUE } else { sys::GREEN }), sys::set_backgrounds_on(true));
    }
    std::thread::sleep(Duration::from_secs(6));
    let (w, h) = engine.call(|s| s.sources.get("cam").map_or((0, 0), |c| (c.width(), c.height())));
    println!("SELFTEST camera frames: {w}x{h}");
    println!("SELFTEST camera backgrounds (active, format supports): {:?}", sys::camera_backgrounds(&uid));
    if let Some(frame) = preview.latest_frame() {
        // the camera fills the middle of the canvas (it's placed at the top-left at its own size):
        // sample where it is
        let (cw, ch) = (w.min(1920) as f32 / 1920.0, h.min(1080) as f32 / 1080.0);
        let (mut keyed, mut green, mut other) = (0, 0, 0);
        for i in 0..48 * 27 {
            let (fx, fy) = ((i % 48) as f32 / 48.0 + 0.01, (i / 48) as f32 / 27.0 + 0.01);
            let Some((_, cb, cr)) = lightair_preview::sample_ycbcr(&frame, fx * cw, fy * ch) else { continue };
            if cb > 170 && cr > 170 {
                keyed += 1;
            } else if (cb < 100 && cr < 100) || (blue && cb > 170 && cr < 110) {
                green += 1;
            } else {
                other += 1;
            }
        }
        // the top corners (most likely background): their mean colour
        let mut sum = (0u32, 0u32, 0u32, 0u32);
        for i in 0..10 * 6 {
            for fx in [(i % 10) as f32 / 10.0 * 0.15, 0.85 + (i % 10) as f32 / 10.0 * 0.15] {
                let fy = (i / 10) as f32 / 6.0 * 0.2;
                if let Some((y, cb, cr)) = lightair_preview::sample_ycbcr(&frame, fx * cw, fy * ch) {
                    sum = (sum.0 + y as u32, sum.1 + cb as u32, sum.2 + cr as u32, sum.3 + 1);
                }
            }
        }
        let k = sum.3.max(1);
        println!("SELFTEST top corners mean YCbCr: ({}, {}, {})", sum.0 / k, sum.1 / k, sum.2 / k);
        let n = (keyed + green + other).max(1) as f32;
        println!(
            "SELFTEST frame: {:.0}% keyed out (magenta shows), {:.1}% key colour left, {:.0}% neither (the person)",
            keyed as f32 * 100.0 / n,
            green as f32 * 100.0 / n,
            other as f32 * 100.0 / n
        );
    } else {
        println!("SELFTEST frame: none");
    }
    drop(preview);
    engine.call(|s| {
        if let (Some(cam), Some(fx)) = (s.sources.get("cam"), s.sources.get("fx")) {
            unsafe { lightair_engine::sys::obs_source_filter_remove(cam.as_ptr(), fx.as_ptr()) };
        }
        s.sources.remove("fx");
    });
    // the picture as it was (the app sets it again while a camera keys the green)
    if let Some(b) = before {
        sys::set_background_picture(std::path::Path::new(&b));
    }
    println!("SELFTEST picture restored: {:?}", sys::background_picture());
    if was_on == Some(false) {
        sys::set_backgrounds_on(false);
        println!("SELFTEST backgrounds back off: {:?}", sys::backgrounds_on());
    }
}

/// `LIGHTAIR_SELFTEST=virtual-camera`: the virtual camera started on a magenta canvas, then
/// the OBS Virtual Camera opened as a camera here: what comes back should be that magenta.
fn virtual_camera(engine: &Engine) {
    let started = engine.call(|s| {
        let magenta = Data::from_json(&serde_json::json!({"color": 0xFFFF00FFu32, "width": 1920, "height": 1080}));
        let back = ObsSource::create("color_source_v3", "Self-test magenta", Some(&magenta)).unwrap();
        let scene = ObsScene::create("Self-test").unwrap();
        scene.add(&back);
        s.sources.insert("back".into(), back);
        s.scenes.insert("selftest".into(), scene);
        s.set_program("selftest");
        s.outputs.start_virtual_camera(None, None)
    });
    println!("SELFTEST start: {started:?}");
    std::thread::sleep(Duration::from_secs(1));
    println!("SELFTEST state: {:?}", lightair_engine::outputs::runtime().get(lightair_engine::outputs::OutputKey::VirtualCamera).state);
    // Windows: the camera opened by another program (ffmpeg, through DirectShow, as Zoom or
    // Teams would), which reads the centre's colour
    if cfg!(windows) {
        let name = lightair_engine::vcam::camera().map_or("none", |c| c.name());
        println!("SELFTEST camera: {name}");
        let t = std::time::Instant::now();
        let out = std::process::Command::new("ffmpeg")
            // (Windows lists cameras added with MFCreateVirtualCamera with a suffix of its own)
            .args(["-hide_banner", "-loglevel", "error", "-f", "dshow", "-i", &format!("video={name} (Windows Virtual Camera)")])
            .args(["-rtbufsize", "64M", "-frames:v", "20", "-vf", "scale=16:9", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output();
        match out {
            Ok(o) if o.stdout.len() >= 16 * 9 * 3 => {
                // the last of the frames read
                let last = &o.stdout[o.stdout.len() - 16 * 9 * 3..];
                println!("SELFTEST frames read: {}", o.stdout.len() / (16 * 9 * 3));
                let at = |x: usize, y: usize| &last[(y * 16 + x) * 3..][..3];
                println!("SELFTEST colour back (RGB, magenta is 255/0/255): centre {:?} corner {:?} after {:?}", at(8, 4), at(1, 1), t.elapsed());
            }
            Ok(o) => println!("SELFTEST colour back: nothing ({})", String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => println!("SELFTEST colour back: no ffmpeg ({e})"),
        }
        engine.call(|s| s.outputs.stop_virtual_camera());
        std::thread::sleep(Duration::from_millis(500));
        println!("SELFTEST stopped: {:?}", lightair_engine::outputs::runtime().get(lightair_engine::outputs::OutputKey::VirtualCamera).state);
        return;
    }
    // the virtual camera's device: Lightair's own when installed, else OBS's (by the UUID its
    // plugin declares)
    let ours = "2DF109AC-DE07-42DF-A0D7-098033FFE986".to_string();
    let uuid = if lightair_engine::kinds::cameras().iter().any(|(_, u)| u.eq_ignore_ascii_case(&ours)) {
        ours
    } else {
        std::process::Command::new("defaults")
            .args(["read", "/Applications/OBS.app/Contents/PlugIns/mac-virtualcam.plugin/Contents/Info", "OBSCameraDeviceUUID"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let found = engine.call(move |s| {
        let devices = lightair_engine::kinds::cameras();
        let Some((name, uid)) = devices.into_iter().find(|(_, u)| u.eq_ignore_ascii_case(&uuid)) else {
            return format!("no camera with uid {uuid}");
        };
        let (kind, settings) = lightair_engine::kinds::resolve(lightair_core::kinds::CAMERA, &serde_json::json!({ "device": uid })).unwrap();
        let cam = ObsSource::create(kind, "Self-test virtual camera in", Some(&Data::from_json(&settings))).unwrap();
        // read back on a canvas of its own (on the program it would loop into itself)
        let canvas = lightair_engine::obs::ObsCanvas::create("Self-test readback", 640, 360, 30, 1).unwrap();
        let scene = canvas.create_scene("Self-test in").unwrap();
        scene.add(&cam);
        unsafe { lightair_engine::sys::obs_canvas_set_channel(canvas.as_ptr(), 0, scene.source_ptr()) };
        s.sources.insert("cam".into(), cam);
        s.scenes.insert("selftest-in".into(), scene);
        let ptr = canvas.as_ptr() as usize;
        std::mem::forget(canvas);
        format!("{name}\u{1f}{ptr}")
    });
    let (found, canvas) = found.split_once('\u{1f}').map_or((found.clone(), 0usize), |(a, b)| (a.to_string(), b.parse().unwrap_or(0)));
    println!("SELFTEST reading back from: {found}");
    std::thread::sleep(Duration::from_secs(4));
    let (w, h) = engine.call(|s| s.sources.get("cam").map_or((0, 0), |c| (c.width(), c.height())));
    println!("SELFTEST frames back: {w}x{h}");
    if canvas != 0 {
        let preview = engine.call(move |_| lightair_preview::Preview::attach_canvas(canvas as *mut lightair_engine::sys::obs_canvas_t));
        std::thread::sleep(Duration::from_secs(1));
        match preview.latest_frame() {
            Some(f) => {
                let at = |x: f32, y: f32| lightair_preview::sample_ycbcr(&f, x, y);
                println!("SELFTEST colour back (YCbCr, magenta is about 73/227/244): centre {:?} corner {:?}", at(0.5, 0.5), at(0.1, 0.1));
            }
            None => println!("SELFTEST colour back: no frame"),
        }
        // one scene, not the one on air: a green scene while magenta stays on air
        engine.call(|s| {
            s.outputs.stop_virtual_camera();
            let green = Data::from_json(&serde_json::json!({"color": 0xFF00FF00u32, "width": 1920, "height": 1080}));
            let src = ObsSource::create("color_source_v3", "Self-test green", Some(&green)).unwrap();
            let scene = ObsScene::create("Self-test green scene").unwrap();
            scene.add(&src);
            let pick = lightair_engine::outputs::VirtualCameraScene { source: scene.source_ptr(), width: 1920, height: 1080, fps_num: 30, fps_den: 1 };
            s.sources.insert("green".into(), src);
            s.scenes.insert("green".into(), scene);
            println!("SELFTEST start on a scene: {:?}", s.outputs.start_virtual_camera(None, Some(pick)));
        });
        std::thread::sleep(Duration::from_secs(2));
        match preview.latest_frame() {
            Some(f) => {
                let at = |x: f32, y: f32| lightair_preview::sample_ycbcr(&f, x, y);
                println!("SELFTEST scene back (YCbCr, green is about 182/30/12): centre {:?}", at(0.5, 0.5));
            }
            None => println!("SELFTEST scene back: no frame"),
        }
        drop(preview);
    }
    engine.call(|s| {
        s.outputs.stop_virtual_camera();
        s.sources.remove("cam");
    });
    std::thread::sleep(Duration::from_millis(500));
    println!("SELFTEST stopped: {:?}", lightair_engine::outputs::runtime().get(lightair_engine::outputs::OutputKey::VirtualCamera).state);
}

/// The loudest sample the self-test page has sent libobs (linear amplitude bits).
static WEB_PEAK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn web_audio(_: *mut std::ffi::c_void, _: *mut lightair_engine::sys::obs_source_t, audio: *const lightair_engine::sys::audio_data, _: bool) {
    let Some(audio) = (unsafe { audio.as_ref() }) else { return };
    let plane = audio.data[0] as *const f32;
    if plane.is_null() {
        return;
    }
    let samples = unsafe { std::slice::from_raw_parts(plane, audio.frames as usize) };
    let loudest = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    WEB_PEAK.fetch_max(loudest.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// `LIGHTAIR_SELFTEST=twitch-login`: signs in to Twitch's chat server with the profile's
/// Twitch sign-in and joins LIGHTAIR_SELFTEST_CHANNEL, sending nothing.
/// `LIGHTAIR_SELFTEST=twitch-me`: who Twitch's API says is signed in (what moderation acts as);
/// reads only.
fn twitch_me() {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(lightair_live::moderate::twitch_me());
    });
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
        if let Ok(r) = rx.try_recv() {
            println!("SELFTEST twitch-me: {}", if r.is_ok() { "the API took the sign-in" } else { "refused" });
            println!("SELFTEST twitch-me detail: {:?}", r.map(|(_, login)| login));
            return;
        }
    }
    println!("SELFTEST twitch-me: no answer");
}

fn twitch_login() {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    let channel = std::env::var("LIGHTAIR_SELFTEST_CHANNEL").unwrap_or_default();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(lightair_live::send::check_twitch(&channel));
    });
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
        if let Ok(r) = rx.try_recv() {
            println!("SELFTEST twitch-login: {r:?}");
            return;
        }
    }
    println!("SELFTEST twitch-login: no answer");
}

/// `LIGHTAIR_SELFTEST=cookie-set`: what bringing a sign-in over from a browser does last: hands
/// Lightair's browser a cookie, read back as sending reads sign-ins.
fn cookie_set() {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    let _page = lightair_web::Page::open_agent("https://example.com/", String::new(), std::sync::Arc::new(|_| {}));
    let (tx, rx) = std::sync::mpsc::channel();
    let start = std::time::Instant::now();
    let mut sent = false;
    while start.elapsed() < Duration::from_secs(30) {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
        if !sent && start.elapsed() > Duration::from_secs(2) {
            sent = true;
            let tx = tx.clone();
            // an hour from now, in µs since 1601
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
            let spec = lightair_web::CookieSpec {
                url: "https://example.com/".into(),
                name: "lightair_set".into(),
                value: "brought".into(),
                domain: String::new(),
                path: "/".into(),
                secure: true,
                http_only: true,
                expires: now + 11_644_473_600_000_000 + 3_600_000_000,
                same_site: 2,
            };
            lightair_web::set_cookies(vec![spec], move |n| {
                std::thread::spawn(move || {
                    let _ = tx.send((n, lightair_live::pages::cookie("https://example.com", "lightair_set")));
                });
            });
        }
        if let Ok((n, v)) = rx.try_recv() {
            println!("SELFTEST cookie-set: took {n}, read back {v:?}");
            return;
        }
    }
    println!("SELFTEST cookie-set: timed out");
}

/// `LIGHTAIR_SELFTEST=cookies`: a cookie a page sets, read back as sending reads sign-ins.
fn cookies() {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = parking_lot::Mutex::new(tx);
    let script = format!("{}document.cookie = 'lightair_test=yes; path=/; max-age=60'; lightair('set');", lightair_live::pages::PRELUDE);
    let _page = lightair_web::Page::open_agent("https://example.com/", script, std::sync::Arc::new(move |m| {
        let _ = tx.lock().send(m);
    }));
    let (got_tx, got_rx) = std::sync::mpsc::channel();
    let start = std::time::Instant::now();
    let mut asked = false;
    while start.elapsed() < Duration::from_secs(30) {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
        if !asked && rx.try_recv().is_ok() {
            asked = true;
            let got = got_tx.clone();
            std::thread::spawn(move || {
                let _ = got.send(lightair_live::pages::cookie("https://example.com", "lightair_test"));
            });
        }
        if let Ok(v) = got_rx.try_recv() {
            println!("SELFTEST cookies: {v:?} after {} ms", start.elapsed().as_millis());
            return;
        }
    }
    println!("SELFTEST cookies: no answer");
}

/// `LIGHTAIR_SELFTEST=script`: an automation's JavaScript step, run in its hidden page.
fn script() {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ctx = serde_json::json!({"name": "Mochi", "value": 5});
        let first = lightair_live::pages::run_script("await new Promise(r => setTimeout(r, 50)); return { greeting: 'Hi ' + ctx.name, double: ctx.value * 2 };", &ctx, Duration::from_secs(10));
        let second = lightair_live::pages::run_script("throw new Error('nope')", &ctx, Duration::from_secs(10));
        let _ = tx.send((first, second));
    });
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
        if let Ok((first, second)) = rx.try_recv() {
            println!("SELFTEST script: {first:?} then {second:?} after {} ms", start.elapsed().as_millis());
            return;
        }
    }
    println!("SELFTEST script: no answer");
}

/// `LIGHTAIR_SELFTEST=agent-page`: a hidden page's script sends a message back (the bridge the
/// chat readers that need a browser use); `LIGHTAIR_SELFTEST_URL` loads a page of one's own.
fn agent_page() {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    let url = std::env::var("LIGHTAIR_SELFTEST_URL").unwrap_or_else(|_| "data:text/html,<title>agent</title><p>hello</p>".into());
    let got = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let sink = got.clone();
    // LIGHTAIR_SELFTEST_SCRIPT: a script of one's own (exploring a site's page)
    let script = format!(
        "{}{}",
        lightair_live::pages::PRELUDE,
        std::env::var("LIGHTAIR_SELFTEST_SCRIPT")
            .unwrap_or_else(|_| "lightair('title=' + document.title + ' body=' + (document.body ? document.body.innerText.slice(0, 120) : ''));".into())
    );
    // LIGHTAIR_SELFTEST_EARLY: run as each load starts too
    let early = std::env::var("LIGHTAIR_SELFTEST_EARLY").map(|e| format!("{}{e}", lightair_live::pages::PRELUDE)).unwrap_or_default();
    let page = lightair_web::Page::open_agent_early(&url, early, script, std::sync::Arc::new(move |m| sink.lock().push(m)));
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(40) && got.lock().is_empty() {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
    }
    println!("SELFTEST agent page messages: {:?} after {} ms", got.lock(), start.elapsed().as_millis());
    drop(page);
}

/// `LIGHTAIR_SELFTEST=web`: a web page (magenta, playing a tone) on air, Chromium driven from
/// this thread; the program's colour and the page's sound.
fn web(engine: &Engine) {
    // the application object Chromium works with on macOS (the window loop makes it otherwise)
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let _app: *mut AnyObject = unsafe { msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
    }
    println!("SELFTEST Chromium bundled: {}", lightair_web::available());
    engine.call(|s| {
        let url = "data:text/html,<html><body style='margin:0;background:%23ff00ff'><div style='position:fixed;left:0;top:0;width:300px;height:100%25;background:%23000'></div><script>const c=new AudioContext();const o=c.createOscillator();o.connect(c.destination);o.start();</script></body></html>";
        let settings = Data::from_json(&serde_json::json!({"url": url, "width": 1920, "height": 1080, "fps": 30}));
        let src = ObsSource::create(lightair_engine::web::KIND, "Self-test page", Some(&settings)).unwrap();
        unsafe { lightair_engine::sys::obs_source_add_audio_capture_callback(src.as_ptr(), Some(web_audio), std::ptr::null_mut()) };
        let scene = ObsScene::create("Self-test").unwrap();
        scene.add(&src);
        s.sources.insert("page".into(), src);
        s.scenes.insert("selftest".into(), scene);
        s.set_program("selftest");
    });
    let preview = engine.call(|_| lightair_preview::Preview::attach());
    let start = std::time::Instant::now();
    let mut next = 1;
    while start.elapsed() < Duration::from_secs(10) {
        lightair_web::pump();
        std::thread::sleep(Duration::from_millis(8));
        if start.elapsed() >= Duration::from_secs(next) {
            next += 1;
            // halfway: the box resized (a window-like resize: the page lays out at the new size)
            if next == 8 {
                // custom CSS: the page turns green
                engine.call(|s| {
                    if let Some(page) = s.sources.get("page") {
                        page.update(&Data::from_json(&serde_json::json!({"css": "body { background: %2300ff00 !important; }".replace("%23", "#")})));
                    }
                });
            }
            if next == 6 {
                // then zoomed to 200%: the 300 px strip covers 600 pixels
                engine.call(|s| {
                    if let Some(page) = s.sources.get("page") {
                        page.update(&Data::from_json(&serde_json::json!({"zoom": 200})));
                    }
                });
            }
            if next == 5 {
                engine.call(|s| {
                    if let Some(page) = s.sources.get("page") {
                        page.update(&Data::from_json(&serde_json::json!({"width": 1000, "height": 700})));
                    }
                });
            }
            let (w, h) = engine.call(|s| s.sources.get("page").map_or((0, 0), |c| (c.width(), c.height())));
            let frame = preview.latest_frame();
            let colour = frame.as_ref().and_then(|f| lightair_preview::sample_ycbcr(f, 0.3, 0.3));
            let at_768 = frame.as_ref().and_then(|f| lightair_preview::sample_ycbcr(f, 0.4, 0.3));
            let peak = f32::from_bits(WEB_PEAK.swap(0, std::sync::atomic::Ordering::Relaxed));
            println!("SELFTEST {} s: page {w}x{h}, program YCbCr {colour:?}, at x 768 {at_768:?} (magenta is about 105/212/234: full-range BT.601), sound peak {peak:.3}", next - 1);
        }
    }
}

/// `LIGHTAIR_SELFTEST=displays`: the displays the picker lists, and whether each one's capture
/// renders a live thumbnail (numbers only: its size, frames, mean brightness).
fn displays(engine: &Engine) {
    let found = engine.call(|_| lightair_engine::displays::list(true));
    println!("SELFTEST displays: {}", found.len());
    let mut previews = vec![];
    for d in &found {
        let preview = lightair_preview::Preview::attach();
        preview.set_own_size(true);
        let (w, h) = d.size.unwrap_or((16, 9));
        preview.set_size(Some((480, (480 * h / w.max(1)).max(2))));
        if let Some(c) = &d.capture {
            preview.set_source(Some(unsafe { lightair_engine::sys::obs_source_get_ref(c.as_ptr()) }));
        }
        previews.push(preview);
    }
    std::thread::sleep(Duration::from_secs(3));
    for (d, p) in found.iter().zip(&previews) {
        let frame = p.latest_frame();
        let size = frame.as_ref().map(lightair_preview::frame_size);
        let mean = frame.as_ref().and_then(|f| lightair_preview::frame_rgba(f, 64)).map(|(_, _, px)| {
            px.chunks(4).map(|c| (c[0] as u32 + c[1] as u32 + c[2] as u32) / 3).sum::<u32>() / (px.len() as u32 / 4).max(1)
        });
        println!("SELFTEST {:?} {:?} main={} capture={} frames={} thumbnail={size:?} mean={mean:?}", d.name, d.size, d.main, d.capture.is_some(), p.frames());
    }
    for p in &previews {
        p.set_source(None);
    }
    drop(previews);
    engine.call(move |_| drop(found));
}
