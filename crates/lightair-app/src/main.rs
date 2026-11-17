//! Lightair application entry point.

// Windows: a window app, no console (development builds keep theirs, for the log)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod activity_panel;
mod assets;
mod automations_panel;
mod browser_import;
mod console;
mod triggers;
mod updater;
mod camera_studio;
mod chat_panel;
mod chat_send;
mod editor;
mod effect_drag;
mod encoding;
mod eyedropper;
mod frame_view;
mod geometry;
mod shortcuts;
mod library;
mod mcp_bridge;
mod mixer;
mod native_preview;
mod now_playing;
#[cfg(target_os = "macos")]
mod plugin_editor;
#[cfg(windows)]
#[path = "plugin_editor_win.rs"]
mod plugin_editor;
mod plugin_scan;
mod outputs_panel;
mod permissions;
mod platforms;
mod preflight;
mod prefs;
mod reframe;
mod rows;
mod safe_areas;
mod secrets;
mod selftest;
mod shell;
mod starter;
mod storage;
mod stream_panel;
mod studio;
#[cfg(target_os = "macos")]
mod system_effects;
#[cfg(windows)]
#[path = "system_effects_win.rs"]
mod system_effects;
mod ui;
mod widgets;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use lightair_engine::{Engine, EngineConfig};
use lightair_preview::Preview;

use crate::shell::StudioWindow;
use crate::studio::Studio;

fn workspaces_dir() -> PathBuf {
    lightair_platform::paths::workspaces_dir()
}

fn main() {
    // Chromium's own processes, when Chromium started this program for one (Windows, where
    // lightair-web-helper.exe isn't beside it)
    #[cfg(windows)]
    if let Some(code) = lightair_web::helper_main() {
        std::process::exit(code);
    }
    // a helper process listing a VST3 bundle's effects (plugin_scan)
    if let Some(code) = plugin_scan::probe_main() {
        std::process::exit(code);
    }
    // an agent's MCP connection (stdout is its, so before logging starts)
    if let Some(code) = mcp_bridge::main_if_asked() {
        std::process::exit(code);
    }
    // Chromium's library before any other thread exists (lightair_web::preload)
    lightair_web::preload();
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("LIGHTAIR_LOG").unwrap_or_else(|_| "info,libobs=warn".into()))
        .init();
    // Chromium's profile (cookies: platforms signed in to) kept with Lightair's data
    lightair_web::set_profile_dir(lightair_platform::paths::data_dir().join("Chromium"));
    // the chat readers that need a browser (Kick, the signed-in platforms) use Lightair's
    lightair_live::pages::set_host(Arc::new(chat_panel::HiddenPages));

    // The canvas size comes from the workspace's primary canvas; read it before starting libobs.
    // the one asked for, the one open last, or the default
    let workspace_dir = std::env::var_os("LIGHTAIR_PROJECT")
        // (absolute: on Windows the engine works from libobs's folder once it starts)
        .map(|p| std::path::absolute(&p).unwrap_or_else(|_| PathBuf::from(p)))
        .or_else(|| prefs::load_from(&prefs::path()).last_workspace.filter(|d| lightair_store::is_workspace(d)))
        .unwrap_or_else(|| workspaces_dir().join("Default.lightair"));
    let canvas = lightair_store::load_snapshot(&workspace_dir)
        .map(|p| (p.primary_canvas().width, p.primary_canvas().height))
        .unwrap_or((1920, 1080));

    // This Mac's preferences: the sample rate is fixed once libobs starts.
    let prefs = prefs::load_from(&prefs::path());
    // the remote API's password, read while libobs starts (the API answers with the window)
    if prefs.remote.enabled && (prefs.remote.require_password || prefs.remote.lan) && std::env::var_os("LIGHTAIR_REMOTE_OFF").is_none() {
        secrets::read_remote_password_early();
    }
    prefs::LOUDNESS_TARGET.store(prefs.loudness_target, std::sync::atomic::Ordering::Relaxed);
    let sample_rate = if prefs.sample_rate == 44_100 { 44_100 } else { 48_000 };
    prefs::RUNNING_SAMPLE_RATE.store(sample_rate, std::sync::atomic::Ordering::Relaxed);
    let engine = Arc::new(
        Engine::start(EngineConfig {
            canvas,
            output: canvas,
            sample_rate,
            // the shipped app loads it from its PlugIns with the rest
            extra_plugin_dirs: camera_studio::plugin_dir().filter(|_| camera_studio::bundled_plugins().is_none()).into_iter().collect(),
            ..Default::default()
        })
        .unwrap_or_else(|e| panic!("could not start the engine: {e}")),
    );
    tracing::info!(version = engine.version, workspace = %workspace_dir.display(), "libobs started");
    if let Some((name, id)) = prefs.headphones_device.clone() {
        engine.call(move |_| lightair_engine::monitoring::set(&name, &id));
    }
    let monitor = prefs.low_latency_headphones.then(|| prefs.headphones_device.clone().map_or(String::new(), |(_, id)| id));
    let program = prefs.headphones_hear_program;
    let block = prefs.audio_block;
    engine.call(move |s| {
        s.reconciler.set_headphones(monitor, program);
        // before the engine starts: nothing to restart
        s.reconciler.set_audio_block(block, &lightair_core::Workspace::new(""));
    });
    if let Ok(what) = std::env::var("LIGHTAIR_SELFTEST") {
        selftest::run(&engine, &what);
        return;
    }
    // iPhones and iPads by USB, among the cameras
    lightair_platform::devices::allow_ios_screens();
    let preview = Preview::attach();

    gpui_kit::application().with_assets(assets::Assets).run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // VST3 plugins are made, edited and torn down on this thread
        // (GPUI's foreground executor: the main thread on both platforms)
        let (jobs, mut queue) = futures_channel::mpsc::unbounded::<Box<dyn FnOnce() + Send>>();
        lightair_engine::plugins::set_main_thread(move |job| {
            let _ = jobs.unbounded_send(job);
        });
        cx.foreground_executor()
            .spawn(async move {
                use futures_util::StreamExt;
                while let Some(job) = queue.next().await {
                    job();
                }
            })
            .detach();
        frame_view::init(cx);
        // the preview under the UI shows through where the window leaves room (native_preview)
        gpui_kit::component::set_transparent_roots(native_preview::on());
        ui::apply_theme(cx);
        shell::bind_keys(cx);
        shell::app_menu::bind(cx);

        let starter: fn() -> lightair_core::Workspace = match std::env::var("LIGHTAIR_STARTER").as_deref() {
            Ok("plain") => starter::plain_workspace,
            _ => starter::starter_workspace,
        };
        let studio = cx.new(|_| Studio::open(engine.clone(), workspace_dir.clone(), starter));

        // Stats at 1 Hz; journal flush and periodic compaction.
        let ticker = studio.downgrade();
        cx.spawn(async move |cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                ticks += 1;
                let alive = ticker.update(cx, |s, cx| {
                    s.refresh_stats();
                    s.refit_imported(cx);
                    s.run_timer_ends(cx);
                    s.refresh_plugins();
                    s.end_replays(cx);
                    s.flush();
                    if ticks.is_multiple_of(60) {
                        s.compact();
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let on_quit = studio.clone();
        cx.on_app_quit(move |cx| {
            on_quit.update(cx, |s, _| s.compact());
            async {}
        })
        .detach();
        cx.on_window_closed(|cx, _| cx.quit()).detach();

        // A copy started by a test or screenshot hook stays in the background: it must never take
        // the keyboard from the app someone is typing in.
        let background = std::env::var_os("LIGHTAIR_OPEN").is_some();
        // its size; a screenshot hook can ask for another ("900x700")
        let (w, h) = std::env::var("LIGHTAIR_WINDOW")
            .ok()
            .and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
            .unwrap_or((1440.0, 900.0));
        let options = WindowOptions {
            focus: !background,
            // the canvas preview is drawn under the UI, showing where the UI leaves room
            window_background: if native_preview::on() { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Opaque },
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(w), px(h)), cx))),
            titlebar: Some(TitlebarOptions {
                title: Some("Lightair".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(15.))),
            }),
            ..Default::default()
        };
        let preview = preview.clone();
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| StudioWindow::new(studio, preview, window, cx)))
            .expect("open window");
        if !background {
            cx.activate(true);
        }
    });
}
